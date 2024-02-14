use std::{error::Error, fmt::Display};

use axum::{extract::FromRef, http, response::IntoResponse};

use crate::{database::Database, routes::StreamingSessions};

#[derive(Clone)]
pub struct AppState {
    database: Database,
    streaming_sessions: StreamingSessions,
}

impl AppState {
    pub fn new(database: Database) -> Self {
        let streaming_sessions = StreamingSessions::new();
        Self {
            database,
            streaming_sessions,
        }
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

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug)]
pub enum AppError {
    Database(rusqlite::Error),
    DatabaseAsync(tokio_rusqlite::Error),
    Pool(r2d2::Error),
    Templating(askama::Error),
    #[allow(non_camel_case_types)]
    ffmpeg(ffmpeg::Error),
    Custom(String),
}

impl Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        match self {
            AppError::Database(e) => write!(f, "Database Error: {}", e),
            AppError::DatabaseAsync(e) => write!(f, "Database Error: {}", e),
            AppError::Pool(e) => write!(f, "Pool Error: {}", e),
            AppError::Templating(e) => write!(f, "Templating Error: {}", e),
            AppError::ffmpeg(e) => write!(f, "ffmpeg Error: {}", e),
            AppError::Custom(e) => write!(f, "Custom Error: {}", e),
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

impl From<tokio_rusqlite::Error> for AppError {
    fn from(e: tokio_rusqlite::Error) -> Self {
        AppError::DatabaseAsync(e)
    }
}

impl From<askama::Error> for AppError {
    fn from(e: askama::Error) -> Self {
        AppError::Templating(e)
    }
}

impl From<String> for AppError {
    fn from(e: String) -> Self {
        AppError::Custom(e)
    }
}

impl From<&str> for AppError {
    fn from(e: &str) -> Self {
        AppError::Custom(e.to_string())
    }
}

impl From<ffmpeg::Error> for AppError {
    fn from(e: ffmpeg::Error) -> Self {
        AppError::ffmpeg(e)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        #[cfg(not(debug_assertions))]
        return (http::StatusCode::INTERNAL_SERVER_ERROR).into_response();
        #[cfg(debug_assertions)]
        return (
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Error: {self:?}"),
        )
            .into_response();
    }
}
