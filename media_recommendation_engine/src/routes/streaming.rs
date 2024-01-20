use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use askama::Template;
use axum::{
    body::Body,
    extract::{
        ws::{Message, WebSocket},
        Path, State, WebSocketUpgrade,
    },
    http::{Request, StatusCode},
    response::{IntoResponse, Redirect},
    routing::get,
    Router,
};

use futures_util::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, Mutex};
use tower::Service;
use tower_http::services::ServeFile;
use tracing::debug;

use crate::{
    database::{Database, QueryRowGetConnExt},
    state::{AppResult, AppState},
    utils::HandleErr,
};

// TODO: Improve this datastructure
#[derive(Clone)]
pub struct StreamingSessions(Arc<Mutex<HashMap<u32, Session>>>);

impl Default for StreamingSessions {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(HashMap::new())))
    }
}

impl std::ops::Deref for StreamingSessions {
    type Target = Arc<Mutex<HashMap<u32, Session>>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
enum SessionState {
    Playing,
    Paused,
}

#[derive(Clone)]
pub struct Session {
    _video_id: u64,
    stream: ServeFile,
    receivers: Vec<u32>,
    tx: broadcast::Sender<WSMessage>,
    notification_sender: mpsc::Sender<Notification>,
    state: SessionState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum WSMessage {
    State(SessionState),
    Join(bool), // bool is included to generate actual json, not just "Join"
    Pause(f32),
    Play(f32),
    Seek(f32),
    Update((f32, SessionState)),
    Notification { msg: String },
}

// TODO: Actual permissions would be great, not just showing it on the front page

pub fn streaming() -> Router<AppState> {
    Router::new()
        .route("/content/:id", get(content))
        .route("/:id", get(new_session))
        .route("/session/:id", get(session))
        .route("/session/ws/:id", get(ws_session))
}

async fn content(
    Path(id): Path<u32>,
    State(sessions): State<StreamingSessions>,
    request: Request<Body>,
) -> Result<impl IntoResponse, impl IntoResponse> {
    let mut sessions = sessions.lock().await;
    let session = sessions.get_mut(&id);
    if session.is_none() {
        return Err((StatusCode::FORBIDDEN).into_response());
    }
    let session = session.unwrap();
    Ok(session.stream.call(request).await)
}

async fn new_session(
    Path(id): Path<u64>,
    State(sessions): State<StreamingSessions>,
    State(db): State<Database>,
) -> AppResult<impl IntoResponse> {
    let random = pseudo_random();

    let conn = db.get()?;
    let path: String = conn.query_row_get("SELECT path FROM data_files WHERE id=?1", [id])?;
    let serve_file = ServeFile::new(path);

    let (tx, _) = broadcast::channel(32);

    let (notification_sender, notification_receiver) = mpsc::channel(32);

    tokio::spawn(notifier(notification_receiver, tx.clone()));

    let session = Session {
        _video_id: id,
        stream: serve_file,
        receivers: Vec::new(),
        tx,
        state: SessionState::Playing,
        notification_sender,
    };

    let mut sessions = sessions.lock().await;
    sessions.insert(random, session);

    Ok(Redirect::temporary(&format!("/video/session/{random}")))
}

async fn ws_session(
    ws: WebSocketUpgrade,
    Path(id): Path<u32>,
    State(sessions): State<StreamingSessions>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| ws_session_callback(socket, id, sessions))
}

#[derive(Template, Clone)]
#[template(path = "../frontend/content/notification.html")]
struct Notification {
    msg: String,
    script: String,
    typ: SimplifiedType,
}

#[derive(Clone, PartialEq)]
enum SimplifiedType {
    StateToggle,
    Seek,
    None,
}

async fn ws_session_callback(socket: WebSocket, id: u32, sessions: StreamingSessions) {
    let user_id = pseudo_random();

    let (mut sender, receiver) = socket.split();

    let (session_receiver, session_sender) = {
        let mut sessions = sessions.lock().await;
        let session = sessions.get_mut(&id);
        let Some(session) = session else {
            sender
                .send(Message::Text(
                    Notification {
                        msg: "This session seems to be invalid... Falling back to previous page"
                            .to_owned(),
                        script: "/scripts/back.js".to_owned(),
                        typ: SimplifiedType::None,
                    }
                    .render()
                    .unwrap(),
                ))
                .await
                .log_err_with_msg("failed to notify client of invalid session");
            return;
        };

        session.receivers.push(user_id);

        sender
            .send(Message::Text(
                serde_json::to_string(&WSMessage::State(session.state)).unwrap(),
            ))
            .await
            .log_err_with_msg("failed to notify client of current state");

        (session.tx.subscribe(), session.tx.clone())
    };

    let notification_sender = sessions.lock().await[&id].notification_sender.clone();
    let leave_sender = notification_sender.clone();
    let sessions_ref = sessions.clone();

    let mut recv_task = tokio::spawn(async move {
        receive_client_messages(
            receiver,
            session_sender,
            notification_sender.clone(),
            id,
            user_id,
            sessions_ref,
        )
        .await
    });
    let mut send_task =
        tokio::spawn(async move { send_session_to_clients(sender, session_receiver).await });

    tokio::select! {
        _ = (&mut send_task) => {recv_task.abort()}
        _ = (&mut recv_task) => {send_task.abort()}
    }

    {
        let mut sessions = sessions.lock().await;
        let session = sessions.get_mut(&id).unwrap();
        session.receivers.retain(|&x| x != user_id);

        if session.receivers.is_empty() {
            sessions.remove(&id);
        } else {
            leave_sender
                .send(Notification {
                    msg: format!("{user_id} left the session"),
                    script: String::new(),
                    typ: SimplifiedType::None,
                })
                .await
                .log_err_with_msg("failed to send notification");
        }
    }
}

async fn receive_client_messages(
    mut client_receiver: SplitStream<WebSocket>,
    session_sender: broadcast::Sender<WSMessage>,
    notification_sender: mpsc::Sender<Notification>,
    session_id: u32,
    client_id: u32,
    sessions: StreamingSessions,
) {
    while let Some(msg) = client_receiver.next().await {
        let Ok(msg) = msg else {
            break;
        };

        match msg {
            Message::Text(text) => {
                handle_client_message(
                    &session_sender,
                    &notification_sender,
                    text,
                    session_id,
                    client_id,
                    &sessions,
                )
                .await
                .log_err();
            }
            // TODO: Consider binary format
            Message::Binary(_) => (),
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Close(_) => break,
        }
    }
}

async fn handle_client_message(
    session_sender: &broadcast::Sender<WSMessage>,
    notification_sender: &mpsc::Sender<Notification>,
    text: String,
    session_id: u32,
    client_id: u32,
    sessions: &StreamingSessions,
) -> Result<(), ()> {
    let Ok(msg) = serde_json::from_str(&text) else {
        debug!("Received malformed json from session websocket: {text}");
        return Err(());
    };

    match msg {
        WSMessage::Pause(_) | WSMessage::Play(_) => {
            let mut sessions = sessions.lock().await;
            let Some(session) = sessions.get_mut(&session_id) else {
                return Err(());
            };
            match msg {
                WSMessage::Pause(_) => session.state = SessionState::Paused,
                WSMessage::Play(_) => session.state = SessionState::Playing,
                _ => unreachable!(),
            }
            send_notification(notification_sender, &msg, client_id).await;
        }
        WSMessage::Seek(_) => {
            send_notification(notification_sender, &msg, client_id).await;
        }
        WSMessage::Join(_) => {
            let sessions = sessions.lock().await;
            let Some(session) = sessions.get(&session_id) else {
                return Err(());
            };
            session_sender
                .send(WSMessage::State(session.state))
                .log_err_with_msg("an error occured while sending a message to the session");
            send_notification(notification_sender, &msg, client_id).await;
        }
        WSMessage::Update { .. } => (),
        WSMessage::State(_) | WSMessage::Notification { .. } => unreachable!(), // This should only be sent from the server
    }

    session_sender
        .send(msg)
        .log_err_with_msg("an error occured while sending a message to the session");

    Ok(())
}

const NOTIFICATION_DELAY: Duration = Duration::from_millis(1000);

struct NotificationQueue {
    queue: Vec<Notification>,
    last_sent: SystemTime,
}

impl NotificationQueue {
    fn new() -> Self {
        Self {
            queue: Vec::new(),
            last_sent: UNIX_EPOCH,
        }
    }

    fn push(&mut self, notification: Notification) {
        self.queue.push(notification);
    }

    fn get_and_reset(&mut self, delay: Duration) -> Option<Notification> {
        if self.last_sent.elapsed().is_ok_and(|dur| dur >= delay) {
            self.last_sent = SystemTime::now();
            let out = self.queue.pop();
            self.queue.clear();
            return out;
        }
        None
    }

    fn get_maximum_delay(&self, other: &NotificationQueue) -> Duration {
        let self_delay = {
            if self.queue.is_empty() {
                Duration::from_secs(0)
            } else {
                self.last_sent.elapsed().unwrap_or(NOTIFICATION_DELAY)
            }
        };

        let other_delay = {
            if other.queue.is_empty() {
                Duration::from_secs(0)
            } else {
                other.last_sent.elapsed().unwrap_or(NOTIFICATION_DELAY)
            }
        };

        self_delay.max(other_delay)
    }
}

async fn notifier(
    mut receiver: mpsc::Receiver<Notification>,
    session_sender: broadcast::Sender<WSMessage>,
) {
    let mut seek_queue = NotificationQueue::new();
    let mut toggle_queue = NotificationQueue::new();

    let mut notification: Option<Notification> = None;
    let mut wait_duration = NOTIFICATION_DELAY;
    while {
        tokio::select! {
            _ = tokio::time::sleep(wait_duration) => true,
            noti = receiver.recv() => {
                notification = noti;
                true
            },
        }
    } {
        if let Some(new_notification) = notification.clone() {
            match new_notification.typ {
                SimplifiedType::Seek => seek_queue.push(new_notification),
                SimplifiedType::StateToggle => toggle_queue.push(new_notification),
                SimplifiedType::None => {
                    send_to_session(&session_sender, &new_notification);
                    notification = None;
                    continue;
                }
            }
            notification = None;
        }

        let delay = seek_queue.get_maximum_delay(&toggle_queue);
        if delay < NOTIFICATION_DELAY {
            wait_duration = NOTIFICATION_DELAY - delay;
        }

        let seek = seek_queue.get_and_reset(NOTIFICATION_DELAY);
        let toggle = toggle_queue.get_and_reset(NOTIFICATION_DELAY);

        for notification in [seek, toggle].iter() {
            let Some(notification) = notification else {
                continue;
            };
            send_to_session(&session_sender, notification);
        }
    }
}

fn send_to_session(sender: &broadcast::Sender<WSMessage>, notification: &Notification) {
    let msg = notification
        .render()
        .log_err_with_msg("failed to render notification")
        .unwrap_or_default();

    sender
        .send(WSMessage::Notification { msg })
        .log_err_with_msg("failed to send notification to session");
}

fn seek_text(client_id: u32, pos: f32) -> String {
    let pos = pos / 60.0;
    let mut hours = 0;
    let mut minutes = pos.trunc() as u32;
    if minutes > 60 {
        hours = minutes / 60;
        minutes %= 60;
    }
    let seconds = (pos.fract() * 60.0) as u8;
    if hours == 0 {
        format!("{client_id} skipped to {minutes}:{seconds:0>2}")
    } else {
        format!("{client_id} skipped to {hours}:{minutes:0>2}:{seconds:0>2}")
    }
}

async fn send_notification(
    notification_sender: &mpsc::Sender<Notification>,
    msg: &WSMessage,
    client_id: u32,
) {
    let (msg, typ) = match msg {
        WSMessage::Seek(pos) => (seek_text(client_id, *pos), SimplifiedType::Seek),
        WSMessage::Join(_) => (
            format!("{client_id} joined the session"),
            SimplifiedType::None,
        ),
        WSMessage::Pause(_) => (
            format!("{client_id} paused the video"),
            SimplifiedType::StateToggle,
        ),
        WSMessage::Play(_) => (
            format!("{client_id} resumed the video"),
            SimplifiedType::StateToggle,
        ),
        _ => unreachable!(),
    };

    notification_sender
        .send(Notification {
            msg,
            script: String::new(),
            typ,
        })
        .await
        .log_err_with_msg("failed to send notification");
}

async fn send_session_to_clients(
    mut client_sender: SplitSink<WebSocket, Message>,
    mut session_receiver: broadcast::Receiver<WSMessage>,
) {
    while let Ok(msg) = session_receiver.recv().await {
        let msg = match msg {
            WSMessage::Notification { msg, .. } => msg,
            _ => serde_json::to_string(&msg).unwrap(),
        };

        client_sender
            .send(Message::Text(msg))
            .await
            .log_err_with_msg("an error occured while sending a message to the client");
    }
}

#[derive(Template)]
#[template(path = "../frontend/content/video.html")]
struct Video {
    id: u64,
}

async fn session(Path(id): Path<u64>) -> impl IntoResponse {
    Video { id }
}

fn pseudo_random() -> u32 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos()
}
