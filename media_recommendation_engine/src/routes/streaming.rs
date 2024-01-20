use std::{
    collections::HashMap,
    mem,
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
use tokio::sync::{broadcast, mpsc, Mutex, MutexGuard};
use tower::Service;
use tower_http::services::ServeFile;
use tracing::debug;

use crate::{
    database::{Database, QueryRowGetConnExt},
    state::{AppResult, AppState},
    utils::HandleErr,
};

#[derive(Clone)]
pub struct StreamingSessions {
    sessions: Arc<Mutex<HashMap<u32, Arc<Session>>>>,
}

impl StreamingSessions {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn get_sessions(&self) -> impl Iterator<Item = (u32, Arc<Session>)> {
        let iter = self
            .sessions
            .lock()
            .await
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect::<Vec<_>>();
        iter.into_iter()
    }

    async fn get(&self, id: &u32) -> Option<Arc<Session>> {
        self.sessions.lock().await.get(id).cloned()
    }

    async fn insert(&mut self, id: u32, session: Session) {
        self.sessions.lock().await.insert(id, Arc::new(session));
    }

    async fn remove(&mut self, id: &u32) {
        self.sessions.lock().await.remove(id);
    }
}

pub struct Session {
    _video_id: u64,
    stream: Mutex<ServeFile>,
    receivers: Mutex<Vec<u32>>,
    websocket_sender: broadcast::Sender<WSMessage>,
    notification_sender: mpsc::Sender<Notification>,
    state: Mutex<SessionState>,
}

impl Session {
    fn new(
        _video_id: u64,
        stream: ServeFile,
        notification_sender: mpsc::Sender<Notification>,
        websocket_sender: broadcast::Sender<WSMessage>,
    ) -> Self {
        Self {
            _video_id,
            stream: Mutex::new(stream),
            receivers: Mutex::new(Vec::new()),
            websocket_sender,
            notification_sender,
            state: Mutex::new(SessionState::Playing),
        }
    }

    async fn stream(&self) -> MutexGuard<ServeFile> {
        self.stream.lock().await
    }

    async fn add_receiver(&self, id: u32) {
        self.receivers.lock().await.push(id);
    }

    async fn remove_receiver(&self, id: &u32) {
        self.receivers.lock().await.retain(|x| x != id);
    }

    async fn receiver_count(&self) -> usize {
        self.receivers.lock().await.len()
    }

    async fn get_state(&self) -> SessionState {
        *self.state.lock().await
    }

    async fn set_state(&self, state: SessionState) {
        *self.state.lock().await = state;
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum SessionState {
    Playing,
    Paused,
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
    let Some(session) = sessions.get(&id).await else {
        return Err((StatusCode::FORBIDDEN).into_response());
    };
    let mut stream = session.stream().await;
    Ok(stream.call(request).await)
}

async fn new_session(
    Path(id): Path<u64>,
    State(mut sessions): State<StreamingSessions>,
    State(db): State<Database>,
) -> AppResult<impl IntoResponse> {
    let random = pseudo_random();

    let conn = db.get()?;
    let path: String = conn.query_row_get("SELECT path FROM data_files WHERE id=?1", [id])?;
    let serve_file = ServeFile::new(path);

    let (websocket_sender, _) = broadcast::channel(32);
    let (notification_sender, notification_receiver) = mpsc::channel(32);
    tokio::spawn(notifier(notification_receiver, websocket_sender.clone()));

    let session = Session::new(id, serve_file, notification_sender, websocket_sender);
    sessions.insert(random, session).await;

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

async fn ws_session_callback(socket: WebSocket, id: u32, mut sessions: StreamingSessions) {
    let user_id = pseudo_random();

    let (mut sender, receiver) = socket.split();

    let session = sessions.get(&id).await;
    let (session_receiver, session_sender) = {
        let Some(ref session) = session else {
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

        session.add_receiver(user_id).await;

        sender
            .send(Message::Text(
                serde_json::to_string(&WSMessage::State(session.get_state().await)).unwrap(),
            ))
            .await
            .log_err_with_msg("failed to notify client of current state");

        (
            session.websocket_sender.subscribe(),
            session.websocket_sender.clone(),
        )
    };
    let session = session.unwrap();

    let notification_sender = session.notification_sender.clone();
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

    session.remove_receiver(&user_id).await;

    if session.receiver_count().await == 0 {
        sessions.remove(&id).await;
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

    let Some(session) = sessions.get(&session_id).await else {
        return Err(());
    };

    match msg {
        WSMessage::Pause(_) | WSMessage::Play(_) => {
            match msg {
                WSMessage::Pause(_) => session.set_state(SessionState::Paused).await,
                WSMessage::Play(_) => session.set_state(SessionState::Playing).await,
                _ => unreachable!(),
            }
            send_notification(notification_sender, &msg, client_id).await;
        }
        WSMessage::Seek(_) => {
            send_notification(notification_sender, &msg, client_id).await;
        }
        WSMessage::Join(_) => {
            session_sender
                .send(WSMessage::State(session.get_state().await))
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
    queue: Option<Notification>,
    last_sent: SystemTime,
}

impl NotificationQueue {
    fn new() -> Self {
        Self {
            queue: None,
            last_sent: UNIX_EPOCH,
        }
    }

    fn push(&mut self, notification: Notification) {
        self.queue = Some(notification);
    }

    fn get_and_reset(&mut self, delay: Duration) -> Option<Notification> {
        if self.last_sent.elapsed().is_ok_and(|dur| dur >= delay) {
            self.last_sent = SystemTime::now();
            return mem::take(&mut self.queue);
        }
        None
    }

    fn get_maximum_delay(&self, other: &NotificationQueue) -> Duration {
        let self_delay = {
            if self.queue.is_none() {
                Duration::from_secs(0)
            } else {
                self.last_sent.elapsed().unwrap_or(NOTIFICATION_DELAY)
            }
        };

        let other_delay = {
            if other.queue.is_none() {
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
