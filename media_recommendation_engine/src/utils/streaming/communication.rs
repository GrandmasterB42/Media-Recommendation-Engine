use std::{
    mem,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use askama::Template;
use axum::extract::ws::{Message, WebSocket};
use futures_util::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, Notify};
use tracing::debug;

use crate::{
    state::{AppResult, Shutdown},
    utils::{auth::User, templates::Notification as NotificationTemplate, HandleErr},
};

use super::{session::SessionState, Session};

pub type UserSessionID = u32;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum WSSend {
    Notification {
        msg: String,
        origin: UserSessionID,
    },
    Update {
        message_type: WSMessageType,
        timestamp: u64,
        video_time: f32,
        state: SessionState,
    },
    Reload,
    Join,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WSReceive {
    Update {
        message_type: WSMessageType,
        timestamp: u64,
        video_time: f32,
        state: SessionState,
    },
    SwitchTo {
        id: u64,
    },
    Join,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WSMessageType {
    Play,
    Pause,
    Seek,
    State,
    Update,
}

#[derive(Clone, PartialEq)]
enum SimplifiedType {
    StateToggle,
    Seek,
    None,
}

struct Notification {
    notification: NotificationTemplate<'static>,
    origin: UserSessionID,
    typ: SimplifiedType,
}

#[derive(Clone)]
pub struct SessionChannel {
    pub to_websocket: broadcast::Sender<WSSend>,
    to_notification_limiter: mpsc::Sender<Notification>,
    pub has_switched: Arc<Notify>,
    shutdown: Shutdown,
}

impl SessionChannel {
    pub fn new(shutdown: Shutdown) -> Self {
        let (websocket_sender, _) = broadcast::channel(32);
        let (notification_sender, notification_receiver) = mpsc::channel(32);

        let channel = Self {
            to_websocket: websocket_sender,
            to_notification_limiter: notification_sender,
            has_switched: Notify::new().into(),
            shutdown,
        };

        let cloned = channel.clone();
        tokio::spawn(cloned.notifier(notification_receiver));

        channel
    }

    pub fn send(&self, msg: WSSend) {
        self.to_websocket
            .send(msg)
            .log_err_with_msg("Failed to send message to websocket broadcast");
    }

    fn send_notification(&self, notification: &Notification) {
        let origin = notification.origin;
        let msg = notification
            .notification
            .render()
            .log_err_with_msg("Failed to render notification template, this should not happen")
            .unwrap_or_default();
        self.send(WSSend::Notification { msg, origin });
    }

    async fn send_text_notification(&self, msg: String, origin: UserSessionID) {
        self.to_notification_limiter
            .send(Notification {
                notification: NotificationTemplate { msg, script: "" },
                origin,
                typ: SimplifiedType::None,
            })
            .await
            .log_err_with_msg("Failed to send text notification to session");
    }

    async fn send_throttled_notification(
        &self,
        msg: String,
        origin: UserSessionID,
        typ: SimplifiedType,
    ) {
        self.to_notification_limiter
            .send(Notification {
                notification: NotificationTemplate { msg, script: "" },
                origin,
                typ,
            })
            .await
            .log_err_with_msg("failed to send notification to session");
    }

    async fn notifier(self, mut receiver: mpsc::Receiver<Notification>) {
        let mut seek_queue = NotificationQueue::new();
        let mut toggle_queue = NotificationQueue::new();

        let mut notification = None;
        let mut wait_duration = NOTIFICATION_DELAY;

        while {
            tokio::select! {
                _ = tokio::time::sleep(wait_duration) => true,
                noti = receiver.recv() => {
                    notification = noti;
                    true
                },
                _ = self.shutdown.cancelled() => false,
            }
        } {
            if let Some(new_notification) = notification {
                match new_notification.typ {
                    SimplifiedType::Seek => seek_queue.push(new_notification),
                    SimplifiedType::StateToggle => toggle_queue.push(new_notification),
                    SimplifiedType::None => {
                        self.send_notification(&new_notification);
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
                self.send_notification(notification);
            }
        }
    }

    pub async fn handle_communications(
        &self,
        session: Arc<Session>,
        socket: WebSocket,
        user: &User,
        user_id: UserSessionID,
    ) {
        let (mut sender, receiver) = socket.split();

        sender
            .send(Message::Text(
                serde_json::to_string(&WSReceive::Update {
                    message_type: WSMessageType::Update,
                    timestamp: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .log_err_with_msg("Failed to get current systemtime")
                        .unwrap_or_default()
                        .as_secs(),
                    video_time: session.get_current_video_time().await as f32,
                    state: session.get_state().await,
                })
                .unwrap(),
            ))
            .await
            .log_err_with_msg("failed to notify client of current state");

        let (new_user, new_session) = (user.clone(), session.clone());
        let channel = self.clone();
        let mut recv_task: tokio::task::JoinHandle<Result<(), crate::state::AppError>> =
            tokio::spawn(async move {
                channel
                    .receive_client_messages(receiver, new_user, user_id, new_session)
                    .await
            });

        let channel = self.clone();
        let mut send_task = tokio::spawn(async move {
            channel.send_session_to_clients(sender, user_id).await;
        });

        tokio::select! {
            _ = self.shutdown.cancelled() => {send_task.abort(); recv_task.abort()}
            _ = (&mut send_task) => {recv_task.abort()}
            _ = (&mut recv_task) => {send_task.abort()}
        }

        if session.receiver_count().await != 1 {
            self.send_text_notification(format!("{} left the session", user.username), user_id)
                .await;
        }
    }

    async fn send_session_to_clients(
        self,
        mut client_sender: SplitSink<WebSocket, Message>,
        user_id: UserSessionID,
    ) {
        let mut receiver = self.to_websocket.subscribe();
        while let Ok(msg) = receiver.recv().await {
            let msg = match msg {
                WSSend::Notification { msg, origin } => {
                    if origin == user_id {
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

    async fn receive_client_messages(
        self,
        mut client_receiver: SplitStream<WebSocket>,
        user: User,
        user_id: UserSessionID,
        session: Arc<Session>,
    ) -> AppResult<()> {
        while let Some(msg) = client_receiver.next().await {
            let Ok(msg) = msg else {
                break;
            };

            match msg {
                Message::Text(text) => {
                    self.handle_client_message(text, &user, user_id, &session)
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
        &self,
        message: String,
        user: &User,
        user_id: UserSessionID,
        session: &Session,
    ) -> AppResult<()> {
        let Ok(msg) = serde_json::from_str(&message) else {
            debug!("Received malformed json from session websocket: {message}");
            return Err("exited because of malformed json".into());
        };

        match msg {
            WSReceive::Update {
                message_type,
                timestamp,
                video_time,
                state,
            } => {
                session.update_timekeeper(video_time as f64, state).await;
                let username = &user.username;
                match message_type {
                    WSMessageType::Pause => {
                        session.set_state(SessionState::Paused).await;
                        self.send_throttled_notification(
                            format!("{username} paused the video"),
                            user_id,
                            SimplifiedType::StateToggle,
                        )
                        .await;
                    }
                    WSMessageType::Play => {
                        session.set_state(SessionState::Playing).await;
                        self.send_throttled_notification(
                            format!("{username} resumed the video"),
                            user_id,
                            SimplifiedType::StateToggle,
                        )
                        .await;
                    }
                    WSMessageType::Seek => {
                        self.send_throttled_notification(
                            Self::seek_text(username, video_time),
                            user_id,
                            SimplifiedType::Seek,
                        )
                        .await;
                    }
                    WSMessageType::Update => (),
                    WSMessageType::State => unreachable!(), // Only the server should send this
                }

                self.send(WSSend::Update {
                    message_type,
                    timestamp,
                    video_time,
                    state,
                });
            }
            WSReceive::Join => {
                self.send(WSSend::Update {
                    message_type: WSMessageType::State,
                    timestamp: 0,
                    video_time: 0.,
                    state: session.get_state().await,
                });

                let username = &user.username;
                self.send_text_notification(format!("{username} joined the session"), user_id)
                    .await;
                self.send(WSSend::Join);
            }
            WSReceive::SwitchTo { id } => {
                session.reuse(id).await.log_err();

                self.has_switched.notify_one();

                self.send(WSSend::Reload);
            }
        }

        Ok(())
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
}

const NOTIFICATION_DELAY: Duration = Duration::from_millis(1000);

struct NotificationQueue<T> {
    queue: Option<T>,
    last_sent: SystemTime,
}

impl<T> NotificationQueue<T> {
    fn new() -> Self {
        Self {
            queue: None,
            last_sent: std::time::UNIX_EPOCH,
        }
    }

    fn push(&mut self, notification: T) {
        self.queue = Some(notification);
    }

    fn get_and_reset(&mut self, delay: Duration) -> Option<T> {
        if self.last_sent.elapsed().is_ok_and(|dur| dur >= delay) {
            self.last_sent = SystemTime::now();
            return mem::take(&mut self.queue);
        }
        None
    }

    fn get_maximum_delay(&self, other: &NotificationQueue<T>) -> Duration {
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
