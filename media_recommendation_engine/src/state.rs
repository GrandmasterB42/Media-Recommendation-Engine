use axum::extract::FromRef;

use crate::{database::Database, routes::StreamingSessions, templating::TemplatingEngine};

#[derive(Clone)]
pub struct AppState {
    database: Database,
    streaming_sessions: StreamingSessions,
    templating_engine: TemplatingEngine,
}

impl AppState {
    pub fn new(database: Database) -> Self {
        let streaming_sessions = StreamingSessions::default();
        let templating_engine = TemplatingEngine::new();
        Self {
            database,
            streaming_sessions,
            templating_engine,
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

impl FromRef<AppState> for TemplatingEngine {
    fn from_ref(state: &AppState) -> TemplatingEngine {
        state.templating_engine.clone()
    }
}
