use axum::extract::FromRef;

use crate::{database::Database, routes::StreamingSessions};

#[derive(Clone)]
pub struct AppState {
    database: Database,
    streaming_sessions: StreamingSessions,
}

impl AppState {
    pub fn new(database: Database) -> Self {
        let streaming_sessions = StreamingSessions::default();
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
