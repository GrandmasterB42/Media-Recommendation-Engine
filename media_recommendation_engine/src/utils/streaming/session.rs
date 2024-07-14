use std::{
    collections::HashMap,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, SystemTime},
};

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

use super::communication::{SessionChannel, UserSessionID, WSSend};

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
        content_id: u64,
        db: &Database,
        shutdown: Shutdown,
    ) -> AppResult<u32> {
        let random = loop {
            let random = pseudo_random();
            if self.get(&random).await.is_none() {
                break random;
            }
        };

        let session = Session::new(db, shutdown, content_id)?;
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
    time_estimate: Arc<TimeKeeper>,
    next_recommended: Arc<Mutex<RecommendationPopupState>>,
    db: Database,
}

impl Session {
    pub fn new(db: &Database, shutdown: Shutdown, content_id: u64) -> AppResult<Self> {
        let file_path: String = db.get()?.query_row_get(
            "SELECT data_file.path FROM content, data_file
                WHERE content.data_id = data_file.id
                AND content.id = ?1
                AND part = 0",
            [content_id],
        )?;

        let stream = ServeFile::new(&file_path);

        let media_context = ffmpeg::format::input(&file_path)?;
        let total_time = media_context.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE);

        let channel = SessionChannel::new(shutdown.clone());

        let time_estimate = Arc::new(TimeKeeper::new(total_time));

        let next_recommended = Arc::new(Mutex::new(RecommendationPopupState::new(db, content_id)));

        Self::send_recommendations(
            time_estimate.clone(),
            channel.clone(),
            next_recommended.clone(),
            shutdown,
        );

        let session = Self {
            video_id: Mutex::new(content_id),
            file_path: Mutex::new(file_path),
            stream: Mutex::new(stream),
            receivers: Mutex::new(Vec::new()),
            channel,
            state: Mutex::new(SessionState::Playing),
            time_estimate,
            next_recommended,
            db: db.clone(),
        };

        Ok(session)
    }

    pub async fn reuse(&self, content_id: u64) -> AppResult<()> {
        let file_path: String = self.db.get()?.query_row_get(
            "SELECT data_file.path FROM data_file, content 
                    WHERE content.id = ?1
                    AND content.data_id = data_file.id",
            [content_id],
        )?;

        if *self.file_path.lock().await == file_path {
            return Ok(());
        }

        *self.video_id.lock().await = content_id;
        self.file_path.lock().await.clone_from(&file_path);

        let media_context = ffmpeg::format::input(&file_path)?;
        let total_time = media_context.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE);

        self.time_estimate.reset(total_time).await;
        *self.next_recommended.lock().await = RecommendationPopupState::new(&self.db, content_id);

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
        self.time_estimate.update(time, state).await;
    }

    pub async fn get_current_video_time(&self) -> f64 {
        self.time_estimate.current_estimate().await
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

    fn send_recommendations(
        timekeeper: Arc<TimeKeeper>,
        channel: SessionChannel,
        popup: Arc<Mutex<RecommendationPopupState>>,
        shutdown: Shutdown,
    ) {
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = TimeKeeper::recommend_now(timekeeper.clone(), shutdown.clone()) => {},
                    _ = shutdown.cancelled() => break,
                }

                let Some(popup) = popup
                    .lock()
                    .await
                    .get_popup()
                    .await
                    .log_warn_with_msg("Rendering a recommendation popup failed with error: ")
                else {
                    continue;
                };

                let msg = WSSend::Notification {
                    msg: popup,
                    origin: u32::MAX, // Probably unlikely, doesn't matter for now
                };

                let Ok(_) = channel.to_websocket.send(msg) else {
                    break;
                };

                channel.has_switched.notified().await;
            }
        });
    }
}

struct TimeKeeper {
    last_known_time: Mutex<f64>,
    total_time: Mutex<f64>,
    currently_playing: AtomicBool,
    last_update: Mutex<SystemTime>,
    was_updated: Notify,
}

impl TimeKeeper {
    fn new(total_time: f64) -> Self {
        Self {
            last_known_time: 0.0.into(),
            total_time: total_time.into(),
            currently_playing: true.into(),
            last_update: SystemTime::now().into(),
            was_updated: Notify::new(),
        }
    }

    async fn reset(&self, total_time: f64) {
        *self.last_known_time.lock().await = 0.;
        *self.total_time.lock().await = total_time;
        self.currently_playing.store(true, Ordering::Relaxed);
        *self.last_update.lock().await = SystemTime::now();
        self.was_updated.notify_one();
    }

    async fn update(&self, time: f64, state: SessionState) {
        self.currently_playing.store(
            match state {
                SessionState::Paused => false,
                SessionState::Playing => true,
            },
            Ordering::Relaxed,
        );
        *self.last_known_time.lock().await = time;
        *self.last_update.lock().await = SystemTime::now();
        self.was_updated.notify_one();
    }

    pub async fn when_to_recommend(&self) -> f64 {
        *self.total_time.lock().await * 0.95
    }

    async fn current_estimate(&self) -> f64 {
        if self.currently_playing.load(Ordering::Relaxed) {
            *self.last_known_time.lock().await
                + SystemTime::now()
                    .duration_since(*self.last_update.lock().await)
                    .log_warn_with_msg("Failed to estimate current video progress of session")
                    .unwrap_or_default()
                    .as_secs_f64()
        } else {
            *self.last_known_time.lock().await
        }
    }

    async fn recommend_now(timekeeper: Arc<Self>, shutdown: Shutdown) -> AppResult<()> {
        const MAX_SLEEP: u64 = 68_719_450_000; // A Little under the maximum sleep time in the tokio docs
        loop {
            let duration = if timekeeper.currently_playing.load(Ordering::Relaxed) {
                let sleep_time =
                    timekeeper.when_to_recommend().await - timekeeper.current_estimate().await;
                let sleep_time = sleep_time.clamp(0., MAX_SLEEP as f64);
                Duration::from_secs_f64(sleep_time)
            } else {
                Duration::from_millis(MAX_SLEEP)
            };

            tokio::select! {
                _ = shutdown.cancelled() => return Ok(()),
                _ = tokio::time::sleep(duration) => break,
                _ = timekeeper.was_updated.notified() => {}
            }
        }
        Ok(())
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
    fn new(db: &Database, content_id: u64) -> Self {
        let db = db.clone();
        Self {
            inner: Store::Future(Box::pin(RecommendationPopup::new(db, content_id))),
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
