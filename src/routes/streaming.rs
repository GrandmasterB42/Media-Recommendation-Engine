use std::{collections::HashMap, sync::Arc, time::SystemTime};

use axum::{
    body::Body,
    extract::{
        ws::{Message, WebSocket},
        Path, WebSocketUpgrade,
    },
    http::{Request, StatusCode},
    response::{Html, IntoResponse, Redirect},
    routing::get,
    Extension, Router,
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
    database::{Database, DatabaseResult, QueryRowGetConnExt},
    utils::HandleErr,
};

#[derive(Clone)]
pub struct StreamingSessions {
    pub sessions: Arc<Mutex<HashMap<u32, Session>>>,
}

// TODO: Add current State to the session, so that new clients don't start playing even when stopped
#[derive(Clone)]
pub struct Session {
    _video_id: u64,
    stream: ServeFile,
    receivers: Vec<u32>,
    tx: broadcast::Sender<WSMessage>,
    //visibility: SessionVisibility,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum WSMessage {
    Join,
    Pause(f32),
    Play(f32),
    Seek(f32),
}

// TODO: Actual permissions would be great, not just showing it on the front page

pub fn streaming() -> Router {
    Router::new()
        .route("/content/:id", get(content))
        .route("/video/:id", get(new_session))
        .nest_service("/video/script", ServeFile::new("frontend/video.js"))
        .nest_service("/video/back", ServeFile::new("frontend/back.js"))
        .route("/video/session/:id", get(session))
        .route("/video/session/ws/:id", get(ws_session))
}

async fn content(
    Path(id): Path<u32>,
    Extension(sessions): Extension<StreamingSessions>,
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
    db: Extension<Database>,
    Extension(sessions): Extension<StreamingSessions>,
) -> DatabaseResult<impl IntoResponse> {
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
    };

    let mut sessions = sessions.sessions.lock().await;
    sessions.insert(random, session);

    Ok(Redirect::temporary(&format!("/video/session/{random}")))
}

async fn ws_session(
    ws: WebSocketUpgrade,
    Path(id): Path<u32>,
    Extension(sessions): Extension<StreamingSessions>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| ws_session_callback(socket, id, sessions))
}

async fn ws_session_callback(mut socket: WebSocket, id: u32, sessions: StreamingSessions) {
    let user_id = pseudo_random();
    let (session_receiver, session_sender) = {
        let mut sessions = sessions.sessions.lock().await;
        let session = sessions.get_mut(&id);
        if session.is_none() {
            socket
            .send(Message::Text(
                r#"<div id="notification"> This session seems to be invalid... Falling back to previous page <script src=/video/back> </script></div>"#
                    .to_owned(),
            ))
            .await
            .log_err_with_msg("failed to notify client of invalid session");
            return;
        }
        let session = session.unwrap();

        session.receivers.push(user_id);

        (session.tx.subscribe(), session.tx.clone())
    };

    let (sender, receiver) = socket.split();

    session_sender.send(WSMessage::Join).unwrap();
    let mut recv_task = tokio::spawn(async move { read(receiver, session_sender).await });
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
) {
    while let Some(msg) = client_receiver.next().await {
        let msg = if let Ok(msg) = msg {
            msg
        } else {
            break;
        };

        match msg {
            Message::Text(t) => {
                if let Ok(msg) = serde_json::from_str(&t) {
                    let r = session_sender.send(msg);
                    if r.is_err() {
                        debug!("an error occured while sending a message to the session")
                    }
                } else {
                    debug!("Recieved malformed json from session websocket")
                }
            }
            // TODO: Consider binary format
            Message::Binary(_) => (),
            Message::Ping(_) => continue,
            Message::Pong(_) => continue,
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
            debug!("an error occured while sending a message to the client")
        }
    }
}

async fn session(Path(id): Path<u64>) -> impl IntoResponse {
    Html(format!(
        r##"
<video id="currentvideo" src=/content/{id} controls autoplay width="100%" height=auto hx-history="false" hx-ext="ws" ws-connect="/video/session/ws/{id}"> </video>
<script src="/video/script"></script>
<div id="notification"> </div>"##
    ))
}

fn pseudo_random() -> u32 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos()
}
