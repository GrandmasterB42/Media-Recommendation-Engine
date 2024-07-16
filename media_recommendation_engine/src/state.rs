use std::{
    error::Error,
    fmt::Display,
    ops::Deref,
    sync::{Arc, Mutex},
};

use axum::{
    extract::FromRef,
    http::{self, StatusCode},
    response::IntoResponse,
};
use tokio::sync::{oneshot, Notify};
use tokio_util::sync::CancellationToken;

use crate::{
    database::Database,
    utils::{streaming::StreamingSessions, ServerSettings},
};

#[derive(Clone)]
pub struct AppState {
    database: Database,
    streaming_sessions: StreamingSessions,
    pub shutdown: Shutdown,
    pub serversettings: ServerSettings,
    pub indexing_trigger: IndexingTrigger,
}

impl AppState {
    pub async fn new(database: Database, port: Option<u16>) -> (Self, oneshot::Receiver<bool>) {
        let (shutdown, restart_receiver) = Shutdown::new();
        let streaming_sessions = StreamingSessions::new(shutdown.clone());
        let serversettings = ServerSettings::new(shutdown.clone(), database.clone(), port).await;
        let indexing_trigger = IndexingTrigger(Arc::new(Notify::new()));
        (
            Self {
                database,
                streaming_sessions,
                shutdown,
                serversettings,
                indexing_trigger,
            },
            restart_receiver,
        )
    }
}

impl FromRef<AppState> for Database {
    fn from_ref(state: &AppState) -> Database {
        state.database.clone()
    }
}

impl FromRef<AppState> for StreamingSessions {
    fn from_ref(state: &AppState) -> StreamingSessions {
        state.streaming_sessions.clone()
    }
}

impl FromRef<AppState> for Shutdown {
    fn from_ref(state: &AppState) -> Self {
        state.shutdown.clone()
    }
}

impl FromRef<AppState> for ServerSettings {
    fn from_ref(state: &AppState) -> ServerSettings {
        state.serversettings.clone()
    }
}

impl FromRef<AppState> for IndexingTrigger {
    fn from_ref(state: &AppState) -> IndexingTrigger {
        state.indexing_trigger.clone()
    }
}

#[derive(Clone)]
pub struct IndexingTrigger(Arc<Notify>);

impl Deref for IndexingTrigger {
    type Target = Notify;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Clone)]
pub struct Shutdown {
    cancellation: CancellationToken,
    restart_sender: Arc<Mutex<Option<oneshot::Sender<bool>>>>,
}

impl Shutdown {
    fn new() -> (Self, oneshot::Receiver<bool>) {
        let (restart_sender, restart_receiver) = oneshot::channel();
        let cancellation = CancellationToken::new();
        let shutdown = Self {
            cancellation,
            restart_sender: Arc::new(Mutex::new(Some(restart_sender))),
        };
        (shutdown, restart_receiver)
    }

    /// This function can panic if either it, or restart have been called in this applications lifecycle
    pub fn shutdown(&self) {
        self.restart_sender
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .send(false)
            .unwrap();
        self.cancellation.cancel();
    }

    /// This function can panic if either it, or shutdown have been called in this applications lifecycle
    pub fn restart(&self) {
        self.restart_sender
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .send(true)
            .unwrap();
        self.cancellation.cancel();
    }

    pub async fn cancelled(&self) {
        self.cancellation.cancelled().await;
    }
}

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug)]
pub enum AppError {
    Database(rusqlite::Error),
    Pool(r2d2::Error),
    Templating(askama::Error),
    #[allow(non_camel_case_types)]
    ffmpeg(ffmpeg::Error),
    Status(StatusCode),
    Anyhow(anyhow::Error),
}

impl Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        match self {
            AppError::Database(e) => write!(f, "Database Error: {e}"),
            AppError::Pool(e) => write!(f, "Pool Error: {e}"),
            AppError::Templating(e) => write!(f, "Templating Error: {e}"),
            AppError::ffmpeg(e) => write!(f, "ffmpeg Error: {e}"),
            AppError::Status(e) => write!(f, "{e}"),
            AppError::Anyhow(e) => write!(f, "{e}"),
        }
    }
}

impl Error for AppError {}

impl From<r2d2::Error> for AppError {
    fn from(e: r2d2::Error) -> Self {
        AppError::Pool(e)
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        AppError::Database(e)
    }
}

impl From<askama::Error> for AppError {
    fn from(e: askama::Error) -> Self {
        AppError::Templating(e)
    }
}

impl From<ffmpeg::Error> for AppError {
    fn from(e: ffmpeg::Error) -> Self {
        AppError::ffmpeg(e)
    }
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        AppError::Anyhow(e)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        #[cfg(not(debug_assertions))]
        return (http::StatusCode::INTERNAL_SERVER_ERROR).into_response();
        #[cfg(debug_assertions)]
        return (
            http::StatusCode::INTERNAL_SERVER_ERROR,
            crate::utils::templates::DebugError {
                err: &format!("{self:?}"),
            },
        )
            .into_response();
    }
}
