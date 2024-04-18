use std::{
    collections::HashMap,
    mem,
    pin::Pin,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
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

use ffmpeg::format;
use futures_util::{
    stream::{SplitSink, SplitStream},
    Future, SinkExt, StreamExt,
};

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, watch, Mutex, MutexGuard, Notify};
use tower::Service;
use tower_http::services::ServeFile;
use tracing::{debug, error};

use crate::{
    database::{Database, QueryRowGetConnExt},
    state::{AppResult, AppState, Cancellation},
    utils::{
        frontend_redirect, pseudo_random,
        templates::{GridElement, RecommendationPopup, Video},
        AuthSession, ConvertErr, HXTarget, HandleErr,
    },
};

type Sessions = Arc<Mutex<HashMap<u32, Arc<Session>>>>;

#[derive(Clone)]
pub struct StreamingSessions {
    sessions: Sessions,
    rendered_sessions: (Arc<watch::Sender<String>>, watch::Receiver<String>),
    should_rerender: Arc<Notify>,
}

impl StreamingSessions {
    pub fn new(cancel: Cancellation) -> Self {
        let sessions = Arc::new(Mutex::new(HashMap::new()));

        let (sender, receiver) = watch::channel(String::new());
        let sender = Arc::new(sender);

        let notify = Arc::new(Notify::new());

        tokio::task::spawn(Self::rerender_task(
            notify.clone(),
            sender.clone(),
            sessions.clone(),
            cancel,
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

    async fn get(&self, id: &u32) -> Option<Arc<Session>> {
        self.sessions.lock().await.get(id).cloned()
    }

    async fn insert(&mut self, id: u32, session: Session) {
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

    async fn remove(&mut self, id: &u32) {
        self.sessions.lock().await.remove(id);
        self.should_rerender.notify_one();
    }

    async fn rerender_task(
        rerender: Arc<Notify>,
        send: Arc<watch::Sender<String>>,
        sessions: Sessions,
        cancel: Cancellation,
    ) {
        loop {
            tokio::select! {
                _ = rerender.notified() => {}
                _ = cancel.cancelled() => {return;}
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
}

// TODO: This datastructure can be refactored, for example move video_id and file_path into a Stream struct, which is behimd a single mutex, also store what kind it was, so that can be used for the recommendation
pub struct Session {
    video_id: Mutex<u64>,
    file_path: Mutex<String>,
    stream: Mutex<ServeFile>,
    receivers: Mutex<Vec<u32>>,
    websocket_sender: broadcast::Sender<WSMessage>,
    notification_sender: mpsc::Sender<Notification>,
    state: Mutex<SessionState>,
    time_estimate: Mutex<TimeKeeper>,
    next_recommended: Mutex<RecommendationPopupState>,
}

impl Session {
    fn new(
        db: &Database,
        video_id: u64,
        notification_sender: mpsc::Sender<Notification>,
        websocket_sender: broadcast::Sender<WSMessage>,
    ) -> AppResult<Self> {
        let file_path: String = db
            .get()?
            .query_row_get("SELECT path FROM data_files WHERE id=?1", [video_id])?;

        let stream = ServeFile::new(&file_path);

        let media_context = format::input(&file_path)?;
        let total_time = media_context.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE);

        let time_estimate = Mutex::new(TimeKeeper::new(total_time));

        let next_recommended = Mutex::new(RecommendationPopupState::new(db, video_id));

        Ok(Self {
            video_id: Mutex::new(video_id),
            file_path: Mutex::new(file_path),
            stream: Mutex::new(stream),
            receivers: Mutex::new(Vec::new()),
            websocket_sender,
            notification_sender,
            state: Mutex::new(SessionState::Playing),
            time_estimate,
            next_recommended,
        })
    }

    async fn reuse(&self, db: &Database, video_id: u64) -> AppResult<()> {
        let file_path: String = db
            .get()?
            .query_row_get("SELECT path FROM data_files WHERE id=?1", [video_id])?;

        if *self.file_path.lock().await == file_path {
            return Ok(());
        }

        *self.video_id.lock().await = video_id;
        self.file_path.lock().await.clone_from(&file_path);

        let media_context = format::input(&file_path)?;
        let total_time = media_context.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE);

        *self.time_estimate.lock().await = TimeKeeper::new(total_time);
        *self.next_recommended.lock().await = RecommendationPopupState::new(db, video_id);

        let serve_file = ServeFile::new(&file_path);
        self.replace_stream(serve_file, &file_path).await;

        Ok(())
    }

    async fn stream(&self) -> MutexGuard<ServeFile> {
        self.stream.lock().await
    }

    async fn replace_stream(&self, stream: ServeFile, path: &str) {
        *self.stream.lock().await = stream;
        path.clone_into(&mut (self.file_path.lock().await.to_string()));
    }

    async fn add_receiver(&self, id: u32) {
        self.receivers.lock().await.push(id);
    }

    async fn remove_receiver(&self, id: &u32) {
        self.receivers.lock().await.retain(|x| x != id);
    }

    async fn receiver_count(&self) -> usize {
        self.receivers.lock().await.len()
    }

    async fn get_state(&self) -> SessionState {
        *self.state.lock().await
    }

    async fn set_state(&self, state: SessionState) {
        *self.state.lock().await = state;
    }

    async fn update_timekeeper(&self, time: f64, state: SessionState) {
        let mut timekeeper = self.time_estimate.lock().await;
        timekeeper.update(time, state);
    }

    async fn get_popup(&self) -> AppResult<String> {
        self.next_recommended.lock().await.get_popup().await
    }

    async fn when_to_recommend(&self) -> f32 {
        let timekeeper = self.time_estimate.lock().await;
        timekeeper.total_time as f32 * 0.95
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
enum WSMessage {
    /// These are only received from the client
    Update {
        message_type: WSMessageType,
        timestamp: u64,
        video_time: f32,
        state: SessionState,
    },
    SendNext,
    SwitchTo {
        id: u64,
    },
    /// These are only sent from the server
    Notification {
        msg: String,
        origin: u32,
    },
    RequestNext {
        at_greater_than: f32,
    },
    Reload,
    /// This is a special one time message from the client to make other instances send their current state
    Join,
}

impl WSMessage {
    fn new_state(state: SessionState) -> Self {
        Self::Update {
            message_type: WSMessageType::State,
            timestamp: 0,
            video_time: 0.0,
            state,
        }
    }

    fn new_notification(msg: &impl Template, origin: u32) -> Self {
        // TODO: general template render function that doesn't error, but just logs the error
        let msg = msg
            .render()
            .log_err_with_msg("failed to render notification")
            .unwrap_or_default();
        Self::Notification { msg, origin }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum WSMessageType {
    Play,
    Pause,
    Seek,
    State,
    Update,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum SessionState {
    Playing,
    Paused,
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
    State(cancel): State<Cancellation>,
    request: Request<Body>,
) -> Result<impl IntoResponse, impl IntoResponse> {
    let Some(session) = sessions.get(&id).await else {
        return Err((StatusCode::FORBIDDEN).into_response());
    };
    let mut stream = session.stream().await;

    tokio::select! {
        resp = stream.call(request) => Ok(resp.into_response()),
        _  = cancel.cancelled() => Err(StatusCode::REQUEST_TIMEOUT.into_response())
    }
}

async fn new_session(
    Path(id): Path<u64>,
    State(mut sessions): State<StreamingSessions>,
    State(db): State<Database>,
    State(cancel): State<Cancellation>,
) -> AppResult<impl IntoResponse> {
    let random = loop {
        let random = pseudo_random();
        if sessions.get(&random).await.is_none() {
            break random;
        }
    };

    let (websocket_sender, _) = broadcast::channel(32);
    let (notification_sender, notification_receiver) = mpsc::channel(32);
    tokio::spawn(notifier(
        notification_receiver,
        websocket_sender.clone(),
        cancel,
    ));

    let session = Session::new(&db, id, notification_sender, websocket_sender)?;
    sessions.insert(random, session).await;

    Ok(Redirect::temporary(&format!(
        "/?all=/video/session/{random}"
    )))
}

async fn ws_session(
    ws: WebSocketUpgrade,
    Path(id): Path<u32>,
    State(sessions): State<StreamingSessions>,
    State(db): State<Database>,
    State(cancel): State<Cancellation>,
    auth: AuthSession,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| ws_session_callback(socket, id, sessions, db, cancel, auth))
}

// TODO: Decouple notification from the template and move it
#[derive(Template)]
#[template(path = "../frontend/content/notification.html")]
struct Notification {
    msg: String,
    script: String,
    typ: SimplifiedType,
    origin: u32,
}

#[derive(Clone, PartialEq)]
enum SimplifiedType {
    StateToggle,
    Seek,
    None,
}

async fn ws_session_callback(
    socket: WebSocket,
    id: u32,
    mut sessions: StreamingSessions,
    db: Database,
    cancel: Cancellation,
    auth: AuthSession,
) {
    let user = auth.user.unwrap();
    let user_id = pseudo_random();
    let username = user.username;

    let (mut sender, receiver) = socket.split();

    let session = sessions.get(&id).await;
    let (session_receiver, session_sender) = {
        let Some(ref session) = session else {
            sender
                .send(Message::Text(
                    Notification {
                        msg: "This session seems to be invalid... Falling back to previous page"
                            .to_owned(),
                        script: "/scripts/back.js".to_owned(),
                        typ: SimplifiedType::None,
                        origin: 0,
                    }
                    .render()
                    .log_err_with_msg(
                        "failed to render fallback notification, this should never happen",
                    )
                    .unwrap_or_default(),
                ))
                .await
                .log_err_with_msg("failed to notify client of invalid session");
            return;
        };

        session.add_receiver(user_id).await;

        sender
            .send(Message::Text(
                serde_json::to_string(&WSMessage::new_state(session.get_state().await)).unwrap(),
            ))
            .await
            .log_err_with_msg("failed to notify client of current state");

        (
            session.websocket_sender.subscribe(),
            session.websocket_sender.clone(),
        )
    };
    let session = session.unwrap();

    let notification_sender = session.notification_sender.clone();
    let leave_sender = notification_sender.clone();
    let sessions_ref = sessions.clone();

    let mut recv_task = tokio::spawn(async move {
        receive_client_messages(
            &db,
            receiver,
            session_sender,
            notification_sender,
            id,
            (user_id, &username),
            sessions_ref,
        )
        .await
    });

    let mut send_task =
        tokio::spawn(
            async move { send_session_to_clients(sender, session_receiver, user_id).await },
        );

    tokio::select! {
        _ = cancel.cancelled() => {send_task.abort(); recv_task.abort()}
        _ = (&mut send_task) => {recv_task.abort()}
        _ = (&mut recv_task) => {send_task.abort()}
    }

    session.remove_receiver(&user_id).await;

    if session.receiver_count().await == 0 {
        sessions.remove(&id).await;
    } else {
        leave_sender
            .send(Notification {
                msg: format!("{user_id} left the session"),
                script: String::new(),
                typ: SimplifiedType::None,
                origin: 0,
            })
            .await
            .log_err_with_msg("failed to send notification");
    }
}

async fn receive_client_messages(
    db: &Database,
    mut client_receiver: SplitStream<WebSocket>,
    session_sender: broadcast::Sender<WSMessage>,
    notification_sender: mpsc::Sender<Notification>,
    session_id: u32,
    user: (u32, &str),
    sessions: StreamingSessions,
) -> AppResult<()> {
    while let Some(msg) = client_receiver.next().await {
        let Ok(msg) = msg else {
            break;
        };

        match msg {
            Message::Text(text) => {
                handle_client_message(
                    db,
                    &session_sender,
                    &notification_sender,
                    text,
                    session_id,
                    user,
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
    Ok(())
}

async fn handle_client_message(
    // TODO: The send_notification function feels kinda redundant
    db: &Database,
    session_sender: &broadcast::Sender<WSMessage>,
    notification_sender: &mpsc::Sender<Notification>,
    text: String,
    session_id: u32,
    user: (u32, &str),
    sessions: &StreamingSessions,
) -> AppResult<()> {
    let Ok(msg) = serde_json::from_str(&text) else {
        debug!("Received malformed json from session websocket: {text}");
        return Err("exited because of malformed json".into());
    };

    let Some(session) = sessions.get(&session_id).await else {
        return Err("Session no longer exists".into());
    };

    match msg {
        WSMessage::Update {
            ref message_type,
            ref video_time,
            state,
            ..
        } => 'update_block: {
            session.update_timekeeper(*video_time as f64, state).await;
            match message_type {
                WSMessageType::Pause => session.set_state(SessionState::Paused).await,
                WSMessageType::Play => session.set_state(SessionState::Playing).await,
                WSMessageType::Seek => (),
                WSMessageType::Update => break 'update_block,
                WSMessageType::State => unreachable!(), // Only the server should send this
            }
            send_notification(notification_sender, &msg, user).await;
        }
        WSMessage::Join => {
            session_sender
                .send(WSMessage::new_state(session.get_state().await))
                .log_err_with_msg("an error occured while sending a message to the session");

            let at_greater_than = session.when_to_recommend().await;
            session_sender
                .send(WSMessage::RequestNext { at_greater_than })
                .log_err_with_msg("failed to send message to session");

            send_notification(notification_sender, &msg, user).await;
        }
        WSMessage::SendNext => {
            let popup = session.get_popup().await?;

            let msg = WSMessage::Notification {
                msg: popup,
                origin: 0,
            };
            session_sender
                .send(msg)
                .log_err_with_msg("an error occured while sending a message to the session");
            return Ok(());
        }
        WSMessage::SwitchTo { id } => {
            session.reuse(db, id).await.log_err();

            session_sender
                .send(WSMessage::Reload)
                .log_err_with_msg("failed to send to session");

            let at_greater_than = session.when_to_recommend().await;
            session_sender
                .send(WSMessage::RequestNext { at_greater_than })
                .log_err_with_msg("failed to send message to session");
            return Ok(());
        }
        WSMessage::Notification { .. } | WSMessage::RequestNext { .. } | WSMessage::Reload => {
            unreachable!()
        } // Only the server should send this
    }

    session_sender
        .send(msg)
        .log_err_with_msg("an error occured while sending a message to the session");

    Ok(())
}

const NOTIFICATION_DELAY: Duration = Duration::from_millis(1000);

struct NotificationQueue {
    queue: Option<Notification>,
    last_sent: SystemTime,
}

impl NotificationQueue {
    fn new() -> Self {
        Self {
            queue: None,
            last_sent: UNIX_EPOCH,
        }
    }

    fn push(&mut self, notification: Notification) {
        self.queue = Some(notification);
    }

    fn get_and_reset(&mut self, delay: Duration) -> Option<Notification> {
        if self.last_sent.elapsed().is_ok_and(|dur| dur >= delay) {
            self.last_sent = SystemTime::now();
            return mem::take(&mut self.queue);
        }
        None
    }

    fn get_maximum_delay(&self, other: &NotificationQueue) -> Duration {
        let self_delay = {
            if self.queue.is_none() {
                Duration::from_secs(0)
            } else {
                self.last_sent.elapsed().unwrap_or(NOTIFICATION_DELAY)
            }
        };

        let other_delay = {
            if other.queue.is_none() {
                Duration::from_secs(0)
            } else {
                other.last_sent.elapsed().unwrap_or(NOTIFICATION_DELAY)
            }
        };

        self_delay.max(other_delay)
    }
}

async fn notifier(
    mut receiver: mpsc::Receiver<Notification>,
    session_sender: broadcast::Sender<WSMessage>,
    cancel: Cancellation,
) {
    let mut seek_queue = NotificationQueue::new();
    let mut toggle_queue = NotificationQueue::new();

    let mut notification: Option<Notification> = None;
    let mut wait_duration = NOTIFICATION_DELAY;
    while {
        tokio::select! {
            _ = tokio::time::sleep(wait_duration) => true,
            noti = receiver.recv() => {
                notification = noti;
                true
            },
            _ = cancel.cancelled() => false,
        }
    } {
        if let Some(new_notification) = notification {
            match new_notification.typ {
                SimplifiedType::Seek => seek_queue.push(new_notification),
                SimplifiedType::StateToggle => toggle_queue.push(new_notification),
                SimplifiedType::None => {
                    send_to_session(&session_sender, &new_notification);
                    notification = None;
                    continue;
                }
            }
            notification = None;
        }

        let delay = seek_queue.get_maximum_delay(&toggle_queue);
        if delay < NOTIFICATION_DELAY {
            wait_duration = NOTIFICATION_DELAY - delay;
        }

        let seek = seek_queue.get_and_reset(NOTIFICATION_DELAY);
        let toggle = toggle_queue.get_and_reset(NOTIFICATION_DELAY);

        for notification in &[seek, toggle] {
            let Some(notification) = notification else {
                continue;
            };
            send_to_session(&session_sender, notification);
        }
    }
}

fn send_to_session(sender: &broadcast::Sender<WSMessage>, notification: &Notification) {
    sender
        .send(WSMessage::new_notification(
            notification,
            notification.origin,
        ))
        .log_err_with_msg("failed to send notification to session");
}

fn seek_text(username: &str, pos: f32) -> String {
    let pos = pos / 60.0;
    let mut hours = 0;
    let mut minutes = pos.trunc() as u32;
    if minutes > 60 {
        hours = minutes / 60;
        minutes %= 60;
    }
    let seconds = (pos.fract() * 60.0) as u8;
    if hours == 0 {
        format!("{username} skipped to {minutes}:{seconds:0>2}")
    } else {
        format!("{username} skipped to {hours}:{minutes:0>2}:{seconds:0>2}")
    }
}

async fn send_notification(
    notification_sender: &mpsc::Sender<Notification>,
    msg: &WSMessage,
    (client_id, username): (u32, &str),
) {
    let (msg, typ) = match msg {
        WSMessage::Join => (
            format!("{username} joined the session"),
            SimplifiedType::None,
        ),
        WSMessage::Update {
            message_type,
            video_time,
            ..
        } => match message_type {
            WSMessageType::Pause => (
                format!("{username} paused the video"),
                SimplifiedType::StateToggle,
            ),
            WSMessageType::Play => (
                format!("{username} resumed the video"),
                SimplifiedType::StateToggle,
            ),
            WSMessageType::Seek => (seek_text(username, *video_time), SimplifiedType::Seek),
            _ => unreachable!(),
        },
        _ => unreachable!(),
    };

    notification_sender
        .send(Notification {
            msg,
            script: String::new(),
            typ,
            origin: client_id,
        })
        .await
        .log_err_with_msg("failed to send notification");
}

async fn send_session_to_clients(
    mut client_sender: SplitSink<WebSocket, Message>,
    mut session_receiver: broadcast::Receiver<WSMessage>,
    client_id: u32,
) {
    while let Ok(msg) = session_receiver.recv().await {
        let msg = match msg {
            WSMessage::Notification { msg, origin, .. } => {
                if origin == client_id {
                    continue;
                }
                msg
            }
            _ => serde_json::to_string(&msg).unwrap(),
        };

        client_sender
            .send(Message::Text(msg))
            .await
            .log_err_with_msg("an error occured while sending a message to the client");
    }
}

async fn session(Path(id): Path<u64>) -> impl IntoResponse {
    Video { id }
}
