use std::{
    collections::HashMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
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

// TODO: This entire module needs refactoring, notification passing is kind of messy and not that robust, i need to come up with a better solution
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
    notification_sender: mpsc::Sender<(Notification, NotificationType)>,
    state: SessionState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum WSMessage {
    State(SessionState),
    Join(bool), // bool is included to generate actual json, not just "Join"
    Pause(f32),
    Play(f32),
    Seek(f32),
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
    origin_: u32,
    time_: f32,
}

#[derive(Clone, Copy, PartialEq)]
enum NotificationType {
    Join,
    Leave,
    Skip,
    Pause,
    Play,
}

async fn ws_session_callback(socket: WebSocket, id: u32, sessions: StreamingSessions) {
    let user_id = pseudo_random();

    let (mut sender, receiver) = socket.split();

    let (session_receiver, session_sender) = {
        let mut sessions = sessions.lock().await;
        let session = sessions.get_mut(&id);
        if session.is_none() {
            sender
                .send(Message::Text(
                    Notification {
                        msg: "This session seems to be invalid... Falling back to previous page"
                            .to_owned(),
                        script: "/scripts/back.js".to_owned(),
                        origin_: user_id,
                        time_: 0.,
                    }
                    .render()
                    .unwrap(),
                ))
                .await
                .log_err_with_msg("failed to notify client of invalid session");
            return;
        }
        let session = session.unwrap();

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
        read(
            receiver,
            session_sender,
            notification_sender.clone(),
            id,
            user_id,
            sessions_ref,
        )
        .await
    });
    let mut send_task = tokio::spawn(async move { write(sender, session_receiver).await });

    tokio::select! {
        _ = (&mut send_task) => {recv_task.abort()}
        _ = (&mut recv_task) => {send_task.abort()}
    }

    leave_sender
        .send((
            Notification {
                msg: format!("{user_id} left the session"),
                script: String::new(),
                origin_: user_id,
                time_: 0.,
            },
            NotificationType::Leave,
        ))
        .await
        .log_err_with_msg("failed to send notification");
    {
        let mut sessions = sessions.lock().await;
        let session = sessions.get_mut(&id).unwrap();
        session.receivers.retain(|&x| x != user_id);

        if session.receivers.is_empty() {
            sessions.remove(&id);
        }
    }
}

async fn read(
    mut client_receiver: SplitStream<WebSocket>,
    session_sender: broadcast::Sender<WSMessage>,
    notification_sender: mpsc::Sender<(Notification, NotificationType)>,
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
    notification_sender: &mpsc::Sender<(Notification, NotificationType)>,
    text: String,
    session_id: u32,
    client_id: u32,
    sessions: &StreamingSessions,
) -> Result<(), ()> {
    let Ok(msg) = serde_json::from_str(&text) else {
        debug!("Recieved malformed json from session websocket");
        return Err(());
    };

    match msg {
        WSMessage::Pause(_) | WSMessage::Play(_) => {
            let mut sessions = sessions.lock().await;
            let session = sessions.get_mut(&session_id);
            if session.is_none() {
                return Err(());
            }
            let session = session.unwrap();

            send_notification(&session.state, notification_sender, &msg, client_id).await;
            match msg {
                WSMessage::Pause(_) => session.state = SessionState::Paused,
                WSMessage::Play(_) => session.state = SessionState::Playing,
                _ => unreachable!(),
            }
        }
        WSMessage::Seek(_) => {
            let sessions = sessions.lock().await;
            let session = sessions.get(&session_id);
            if session.is_none() {
                return Err(());
            }
            let session = session.unwrap();
            send_notification(&session.state, notification_sender, &msg, client_id).await
        }
        WSMessage::Join(_) => {
            let sessions = sessions.lock().await;
            let session = sessions.get(&session_id);
            if session.is_none() {
                return Err(());
            }
            let session = session.unwrap();
            session_sender
                .send(WSMessage::State(session.state))
                .log_err_with_msg("an error occured while sending a message to the session");
            send_notification(&session.state, notification_sender, &msg, client_id).await;
        }
        WSMessage::State(_) | WSMessage::Notification { .. } => unreachable!(), // This should only be sent from the server
    }

    session_sender
        .send(msg)
        .log_err_with_msg("an error occured while sending a message to the session");

    Ok(())
}

async fn notifier(
    mut receiver: mpsc::Receiver<(Notification, NotificationType)>,
    session_sender: broadcast::Sender<WSMessage>,
) {
    // Limit notifications that actually get sent to clients
    let mut last_notification: Option<(Notification, NotificationType)> = None;
    let mut last_sent = UNIX_EPOCH;

    async fn lazy_or(
        notification: &mut Option<(Notification, NotificationType)>,
        new: &mut mpsc::Receiver<(Notification, NotificationType)>,
    ) -> Option<(Notification, NotificationType)> {
        if notification.is_some() {
            let out = notification.clone();
            *notification = None;
            return out;
        }
        new.recv().await
    }

    while let Some((mut notification, typ)) = lazy_or(&mut last_notification, &mut receiver).await {
        match typ {
            NotificationType::Skip => {
                if !last_sent.elapsed().is_ok_and(|dur| dur.as_millis() > 2000) {
                    // It's been less than two second since the last notification
                    tokio::select! {
                        // it's more than a half a second after the last notification => send
                        _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                            last_notification = None;
                        },
                        // In less than half a second, a new notification has been sent => don't send and see if it needs to be overwritten further
                        _ = receiver.recv() => {
                            last_notification = Some((notification, typ));
                            continue;
                        },
                    }
                } else {
                    // It's been a long enough time since the last notification
                    last_notification = None;
                }
            }
            // skipping while playing causes pause and then play to get sent, so wait if there is a play again, if yes, overwrite
            NotificationType::Pause | NotificationType::Play => {
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_millis(150)) => {},
                    new_notification = receiver.recv() => {
                        if let Some((new_notification, NotificationType::Play)) = new_notification {
                            notification = Notification {
                                msg: seek_text(new_notification.origin_, new_notification.time_),
                                script: String::new(),
                                origin_: new_notification.origin_,
                                time_: new_notification.time_,
                            };
                        }
                    },
                }
                last_notification = None;
            }
            NotificationType::Leave | NotificationType::Join => (),
        }

        let msg = notification
            .render()
            .log_err_with_msg("failed to render notification")
            .unwrap_or_default();

        session_sender
            .send(WSMessage::Notification { msg })
            .log_err_with_msg("failed to send notification to session");
        last_sent = SystemTime::now();
    }
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
    state: &SessionState,
    notification_sender: &mpsc::Sender<(Notification, NotificationType)>,
    msg: &WSMessage,
    client_id: u32,
) {
    // Send messages with regard to state, but not any previous messages
    let (msg, typ, pos) = match msg {
        WSMessage::Seek(pos) => {
            let msg = seek_text(client_id, *pos);
            (msg, NotificationType::Skip, pos)
        }
        WSMessage::Join(_) => {
            let msg = format!("{client_id} joined the session");
            (msg, NotificationType::Join, &0.)
        }
        WSMessage::Pause(pos) => {
            let msg = format!("{client_id} paused the video");
            (msg, NotificationType::Pause, pos)
        }
        WSMessage::Play(pos) => match state {
            SessionState::Paused => {
                let msg = format!("{client_id} resumed the video");
                (msg, NotificationType::Play, pos)
            }
            SessionState::Playing => {
                let msg = seek_text(client_id, *pos);
                (msg, NotificationType::Skip, &0.)
            }
        },
        _ => unreachable!(),
    };

    notification_sender
        .send((
            Notification {
                msg,
                script: String::new(),
                origin_: client_id,
                time_: *pos,
            },
            typ,
        ))
        .await
        .log_err_with_msg("failed to send notification");
}

async fn write(
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
