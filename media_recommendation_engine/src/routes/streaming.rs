use std::{collections::HashMap, sync::Arc, time::SystemTime};

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
use tokio::sync::{broadcast, Mutex};
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
    pub sessions: Arc<Mutex<HashMap<u32, Session>>>,
}

impl Default for StreamingSessions {
    fn default() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
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
    state: SessionState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum WSMessage {
    State(SessionState),
    Join(bool), // bool is included to generate actual json, not just "Join"
    Pause(f32),
    Play(f32),
    Seek(f32),
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
    let mut sessions = sessions.sessions.lock().await;
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

    let session = Session {
        _video_id: id,
        stream: serve_file,
        receivers: Vec::new(),
        tx,
        state: SessionState::Playing,
    };

    let mut sessions = sessions.sessions.lock().await;
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

#[derive(Template)]
#[template(path = "../frontend/content/notification.html")]
struct Notification<'a> {
    msg: &'a str,
    script: &'a str,
}

async fn ws_session_callback(socket: WebSocket, id: u32, sessions: StreamingSessions) {
    let user_id = pseudo_random();

    let (mut sender, receiver) = socket.split();

    let (session_receiver, session_sender) = {
        let mut sessions = sessions.sessions.lock().await;
        let session = sessions.get_mut(&id);
        if session.is_none() {
            sender
                .send(Message::Text(
                    Notification {
                        msg: "This session seems to be invalid... Falling back to previous page",
                        script: "/scripts/back.js",
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

    let sessions_ref = sessions.sessions.clone();
    let mut recv_task =
        tokio::spawn(async move { read(receiver, session_sender, id, sessions_ref).await });
    let mut send_task = tokio::spawn(async move { write(sender, session_receiver).await });

    tokio::select! {
        _ = (&mut send_task) => {recv_task.abort()}
        _ = (&mut recv_task) => {send_task.abort()}
    }

    {
        let mut sessions = sessions.sessions.lock().await;
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
    id: u32,
    sessions: Arc<Mutex<HashMap<u32, Session>>>,
) {
    while let Some(msg) = client_receiver.next().await {
        let Ok(msg) = msg else {
            break;
        };

        match msg {
            Message::Text(t) => {
                if let Ok(msg) = serde_json::from_str(&t) {
                    match msg {
                        WSMessage::Pause(_) | WSMessage::Play(_) => {
                            let mut sessions = sessions.lock().await;
                            let session = sessions.get_mut(&id);
                            if session.is_none() {
                                break;
                            }
                            let session = session.unwrap();

                            match msg {
                                WSMessage::Pause(_) => session.state = SessionState::Paused,
                                WSMessage::Play(_) => session.state = SessionState::Playing,
                                _ => unreachable!(),
                            }
                        }
                        WSMessage::Seek(_) => (),
                        WSMessage::Join(_) => {
                            let state = sessions.lock().await[&id].state;
                            session_sender
                                .send(WSMessage::State(state))
                                .log_err_with_msg(
                                    "an error occured while sending a message to the session",
                                );
                        }
                        WSMessage::State(_) => unreachable!(), // This should only be sent from the server
                    }

                    session_sender.send(msg).log_err_with_msg(
                        "an error occured while sending a message to the session",
                    );
                } else {
                    debug!("Recieved malformed json from session websocket");
                }
            }
            // TODO: Consider binary format
            Message::Binary(_) => (),
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Close(_) => break,
        }
    }
}

async fn write(
    mut client_sender: SplitSink<WebSocket, Message>,
    mut session_receiver: broadcast::Receiver<WSMessage>,
) {
    while let Ok(msg) = session_receiver.recv().await {
        let msg = serde_json::to_string(&msg).unwrap();

        let msg = Message::Text(msg);
        let r = client_sender.send(msg).await;
        if r.is_err() {
            debug!("an error occured while sending a message to the client");
        }
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
