use std::{collections::HashMap, pin::Pin, sync::Arc, time::SystemTime};

use askama::Template;
use axum::{
    body::Body,
    extract::{ws::WebSocket, Request},
    response::IntoResponse,
};
use futures_util::Future;
use serde::{Deserialize, Serialize};
use tokio::sync::{watch, Mutex, Notify};
use tower::Service;
use tower_http::services::ServeFile;
use tracing::error;

use crate::{
    database::{Database, QueryRowGetConnExt},
    state::{AppResult, Shutdown},
    utils::{
        auth::User,
        frontend_redirect, pseudo_random,
        templates::{GridElement, RecommendationPopup},
        ConvertErr, HXTarget, HandleErr,
    },
};

use super::communication::{SessionChannel, UserSessionID};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum SessionState {
    Playing,
    Paused,
}

pub type Sessions = Arc<Mutex<HashMap<u32, Arc<Session>>>>;

#[derive(Clone)]
pub struct StreamingSessions {
    sessions: Sessions,
    rendered_sessions: (Arc<watch::Sender<String>>, watch::Receiver<String>),
    should_rerender: Arc<Notify>,
}

impl StreamingSessions {
    pub fn new(shutdown: Shutdown) -> Self {
        let sessions = Arc::new(Mutex::new(HashMap::new()));

        let (sender, receiver) = watch::channel(String::new());
        let sender = Arc::new(sender);

        let notify = Arc::new(Notify::new());

        tokio::task::spawn(Self::rerender_task(
            notify.clone(),
            sender.clone(),
            sessions.clone(),
            shutdown,
        ));

        Self {
            sessions,
            rendered_sessions: (sender, receiver),
            should_rerender: notify,
        }
    }

    pub async fn get_sessions(sessions: &Sessions) -> impl Iterator<Item = (u32, Arc<Session>)> {
        let iter = sessions
            .lock()
            .await
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect::<Vec<_>>();
        iter.into_iter()
    }

    pub async fn get(&self, id: &u32) -> Option<Arc<Session>> {
        self.sessions.lock().await.get(id).cloned()
    }

    pub async fn insert(&mut self, id: u32, session: Session) {
        if self
            .sessions
            .lock()
            .await
            .insert(id, Arc::new(session))
            .is_some()
        {
            error!("A duplicate session was inserted!");
        };
        self.should_rerender.notify_one();
    }

    pub async fn remove(&mut self, id: &u32) {
        self.sessions.lock().await.remove(id);
        self.should_rerender.notify_one();
    }

    async fn rerender_task(
        rerender: Arc<Notify>,
        send: Arc<watch::Sender<String>>,
        sessions: Sessions,
        shutdown: Shutdown,
    ) {
        loop {
            tokio::select! {
                _ = rerender.notified() => {}
                _ = shutdown.cancelled() => {return;}
            }
            let rendered = Self::render_sessions(&sessions)
                .await
                .log_err_with_msg("Failed to render sessions")
                .unwrap_or_default();
            send.send(rendered)
                .log_err_with_msg("Failed to send renderes Session itno channel");
        }
    }

    async fn render_sessions(sessions: &Sessions) -> AppResult<String> {
        Self::get_sessions(sessions)
            .await
            .map(|(id, _session)| GridElement {
                title: format!("Session {id}"),
                redirect_entire: frontend_redirect(&format!("/video/session/{id}"), HXTarget::All),
                redirect_img: String::new(),
                redirect_title: String::new(),
            })
            .map(|el| el.render().convert_err())
            .collect()
    }

    pub fn render_receiver(&self) -> watch::Receiver<String> {
        self.rendered_sessions.0.subscribe()
    }

    pub async fn new_session(
        &mut self,
        video_id: u64,
        db: &Database,
        shutdown: Shutdown,
    ) -> AppResult<u32> {
        let random = loop {
            let random = pseudo_random();
            if self.get(&random).await.is_none() {
                break random;
            }
        };

        let session = Session::new(db, shutdown, video_id)?;
        self.insert(random, session).await;

        Ok(random)
    }
}

pub struct Session {
    video_id: Mutex<u64>,
    file_path: Mutex<String>,
    stream: Mutex<ServeFile>,
    receivers: Mutex<Vec<(User, UserSessionID)>>,
    channel: SessionChannel,
    state: Mutex<SessionState>,
    time_estimate: Mutex<TimeKeeper>,
    next_recommended: Mutex<RecommendationPopupState>,
    db: Database,
    pub shutdown: Shutdown,
}

impl Session {
    pub fn new(db: &Database, shutdown: Shutdown, video_id: u64) -> AppResult<Self> {
        let file_path: String = db
            .get()?
            .query_row_get("SELECT path FROM data_files WHERE id=?1", [video_id])?;

        let stream = ServeFile::new(&file_path);

        let media_context = ffmpeg::format::input(&file_path)?;
        let total_time = media_context.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE);

        let time_estimate = Mutex::new(TimeKeeper::new(total_time));

        let next_recommended = Mutex::new(RecommendationPopupState::new(db, video_id));

        Ok(Self {
            video_id: Mutex::new(video_id),
            file_path: Mutex::new(file_path),
            stream: Mutex::new(stream),
            receivers: Mutex::new(Vec::new()),
            channel: SessionChannel::new(shutdown.clone()),
            state: Mutex::new(SessionState::Playing),
            time_estimate,
            next_recommended,
            db: db.clone(),
            shutdown,
        })
    }

    pub async fn reuse(&self, video_id: u64) -> AppResult<()> {
        let file_path: String = self
            .db
            .get()?
            .query_row_get("SELECT path FROM data_files WHERE id=?1", [video_id])?;

        if *self.file_path.lock().await == file_path {
            return Ok(());
        }

        *self.video_id.lock().await = video_id;
        self.file_path.lock().await.clone_from(&file_path);

        let media_context = ffmpeg::format::input(&file_path)?;
        let total_time = media_context.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE);

        *self.time_estimate.lock().await = TimeKeeper::new(total_time);
        *self.next_recommended.lock().await = RecommendationPopupState::new(&self.db, video_id);

        let serve_file = ServeFile::new(&file_path);
        self.replace_stream(serve_file, &file_path).await;

        Ok(())
    }
    pub async fn stream(&self, req: Request<Body>) -> impl IntoResponse {
        self.stream.lock().await.call(req).await
    }

    async fn replace_stream(&self, stream: ServeFile, path: &str) {
        *self.stream.lock().await = stream;
        path.clone_into(&mut (self.file_path.lock().await.to_string()));
    }

    pub async fn add_receiver(&self, user: &User, id: UserSessionID) {
        self.receivers.lock().await.push((user.clone(), id));
    }

    pub async fn remove_receiver(&self, id: UserSessionID) {
        self.receivers
            .lock()
            .await
            .retain(|(_, entry)| *entry != id);
    }

    pub async fn receiver_count(&self) -> usize {
        self.receivers.lock().await.len()
    }

    pub async fn get_state(&self) -> SessionState {
        *self.state.lock().await
    }

    pub async fn set_state(&self, state: SessionState) {
        *self.state.lock().await = state;
    }

    pub async fn update_timekeeper(&self, time: f64, state: SessionState) {
        let mut timekeeper = self.time_estimate.lock().await;
        timekeeper.update(time, state);
    }

    pub async fn get_current_video_time(&self) -> f32 {
        self.time_estimate.lock().await.current_estimate() as f32
    }

    pub async fn get_popup(&self) -> AppResult<String> {
        self.next_recommended.lock().await.get_popup().await
    }

    pub async fn when_to_recommend(&self) -> f32 {
        let timekeeper = self.time_estimate.lock().await;
        timekeeper.total_time as f32 * 0.95
    }

    /// Returns when the user disonnects, the returned bool indicates whether the session is now empty
    pub async fn handle_user(session: Arc<Self>, user: User, socket: WebSocket) -> bool {
        let user_id = loop {
            let new_id = pseudo_random();
            if session
                .receivers
                .lock()
                .await
                .iter()
                .filter(|entry| entry.1 == new_id)
                .collect::<Vec<_>>()
                .is_empty()
            {
                break new_id;
            }
        };

        session.add_receiver(&user, user_id).await;

        session
            .channel
            .handle_communications(session.clone(), socket, &user, user_id)
            .await;

        session.remove_receiver(user_id).await;

        if session.receiver_count().await == 0 {
            return true;
        }

        false
    }
}

struct TimeKeeper {
    last_known_time: f64,
    total_time: f64,
    currently_playing: bool,
    last_update: SystemTime,
}

impl TimeKeeper {
    fn new(total_time: f64) -> Self {
        Self {
            last_known_time: 0.0,
            total_time,
            currently_playing: true,
            last_update: SystemTime::now(),
        }
    }

    fn update(&mut self, time: f64, state: SessionState) {
        self.currently_playing = match state {
            SessionState::Paused => false,
            SessionState::Playing => true,
        };
        self.last_known_time = time;
        self.last_update = SystemTime::now();
    }

    fn current_estimate(&self) -> f64 {
        if self.currently_playing {
            SystemTime::now()
                .duration_since(self.last_update)
                .log_warn_with_msg("Failed to estimate current video progress of session")
                .unwrap_or_default()
                .as_secs_f64()
        } else {
            self.last_known_time
        }
    }
}

type PopupFuture = Pin<Box<dyn Future<Output = AppResult<RecommendationPopup>> + Send + Sync>>;

enum Store<A, B> {
    Future(A),
    Result(B),
}
struct RecommendationPopupState {
    inner: Store<PopupFuture, String>,
}

impl RecommendationPopupState {
    fn new(db: &Database, video_id: u64) -> Self {
        let db = db.clone();
        Self {
            inner: Store::Future(Box::pin(RecommendationPopup::new(db, video_id))),
        }
    }

    // I think this currently does all the work in this one await call, but it is supposed to be computed in the background, works for now, hold the joinhandle instead?
    async fn get_popup(&mut self) -> AppResult<String> {
        match self.inner {
            Store::Future(ref mut f) => {
                let popup = f.await?;
                let result = popup
                    .render()
                    .log_err_with_msg("failed to render")
                    .unwrap_or_default();
                self.inner = Store::Result(result.clone());
                Ok(result)
            }
            Store::Result(ref r) => Ok(r.clone()),
        }
    }
}
