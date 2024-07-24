use anyhow::Context;
use askama::Template;
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, State, WebSocketUpgrade,
    },
    http::StatusCode,
    response::{IntoResponse, Redirect},
    routing::get,
    Router,
};

use crate::{
    database::Database,
    state::{AppError, AppResult, AppState, Shutdown},
    utils::{
        streaming::{MediaRequest, Session, StreamingSessions},
        templates::{Notification, Video},
        AuthSession, HandleErr,
    },
};

pub fn streaming() -> Router<AppState> {
    Router::new()
        .route("/content/:id", get(content_playlist))
        .route("/:id", get(new_session))
        .route("/session/:id", get(session))
        .route("/session/ws/:id", get(ws_session))
}

async fn content_playlist(
    Path(content_token): Path<String>,
    State(sessions): State<StreamingSessions>,
    State(shutdown): State<Shutdown>,
) -> AppResult<impl IntoResponse> {
    let seperated = content_token
        .split_once('.')
        .unwrap_or((&content_token, ""));

    let session_id = seperated
        .0
        .parse()
        .with_context(|| "failed to parse session id from content token")?;

    let segment_id: Option<usize> = seperated
        .1
        .split_once('.')
        .unwrap_or((seperated.1, ""))
        .0
        .parse()
        .ok();

    let media_request = if let Some(segment_id) = segment_id {
        MediaRequest::Segment(segment_id)
    } else {
        MediaRequest::PlayList
    };

    let Some(session) = sessions.get(&session_id).await else {
        return Err(AppError::Status(StatusCode::FORBIDDEN));
    };

    tokio::select! {
        resp = session.stream(media_request) => Ok(resp),
        _  = shutdown.cancelled() => Err(AppError::Status(StatusCode::REQUEST_TIMEOUT))
    }
}

async fn new_session(
    Path(id): Path<u64>,
    State(mut sessions): State<StreamingSessions>,
    State(db): State<Database>,
    State(shutdown): State<Shutdown>,
) -> AppResult<impl IntoResponse> {
    let session_id = sessions.new_session(id, &db, shutdown).await?;

    Ok(Redirect::temporary(&format!(
        "/?all=/video/session/{session_id}"
    )))
}

async fn session(Path(id): Path<u64>) -> impl IntoResponse {
    Video { id }
}

async fn ws_session(
    ws: WebSocketUpgrade,
    Path(id): Path<u32>,
    State(sessions): State<StreamingSessions>,
    auth: AuthSession,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| ws_session_callback(socket, id, sessions, auth))
}

async fn ws_session_callback(
    mut socket: WebSocket,
    id: u32,
    mut sessions: StreamingSessions,
    auth: AuthSession,
) {
    let Some(user) = auth.user else {
        return;
    };

    let Some(session) = sessions.get(&id).await else {
        socket
            .send(Message::Text(
                Notification {
                    msg: "This session seems to be invalid... Falling back to previous page"
                        .to_owned(),
                    script: "/scripts/back.js",
                }
                .render()
                .unwrap(),
            ))
            .await
            .log_err_with_msg("failed to notify client of invalid session");
        return;
    };

    let is_empty = Session::handle_user(session, user, socket).await;

    if is_empty {
        sessions.remove(&id).await;
    }
}
