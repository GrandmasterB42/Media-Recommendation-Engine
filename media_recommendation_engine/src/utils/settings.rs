use std::{path::Path, sync::Arc, time::SystemTime};

use crate::{
    database::{Database, QueryRowGetConnExt},
    state::{AppResult, Cancellation},
};
use serde::{Deserialize, Serialize};
use tokio::{
    io::AsyncWriteExt,
    sync::{
        watch::{self, Receiver, Sender},
        RwLock,
    },
};
use tracing::{debug, error, info, warn};

use super::{ConvertErr, HandleErr};

pub type ServerConfig = Arc<RwLock<ConfigFile>>;

#[derive(Clone)]
pub struct ServerSettings {
    pub inner: ServerConfig,
    // For now this just indicates whether something has changed, not what (used in the future for web interface setting changes)
    changed: Arc<Sender<()>>,
    abort_wait: Arc<Sender<f64>>,
}

impl ServerSettings {
    const PATH: &'static str = "mreconfig.toml";

    pub async fn new(cancel: Cancellation, db: Database) -> Self {
        let config_file = match tokio::fs::read_to_string(Self::PATH)
            .await
            .log_warn_with_msg("Failed to create config file, trying to create a new one")
        {
            Some(config_file) => config_file,
            None => {
                let default = ConfigFile::default();
                Self::create_config_file(&default).await;
                toml::to_string_pretty(&default)
                    .expect("failed to serialize default config after it should have been created, this should never happen")
            }
        };

        let config: ConfigFile = toml::from_str(&config_file)
            .log_err_with_msg("Failed to parse config file, using the default config instead")
            .unwrap_or_default();

        let change = watch::channel(());

        let send = change.0;
        let recv = change.1;

        let (waiting, r) = watch::channel(0.);
        Box::leak(Box::new(r)); // Leaking the receiver so the channel is only closed when the program ends

        let data = Self {
            inner: Arc::new(RwLock::new(config)),
            changed: Arc::new(send),
            abort_wait: Arc::new(waiting),
        };

        let mut last_admin = data.admin().await;
        data.update_db_to_file_content(&db, &mut last_admin)
            .await
            .log_warn_with_msg("failed to change database in accordance with config file");

        let cloned_settings = data.clone();
        tokio::spawn(async move {
            Self::watch_file(&cloned_settings, cancel, db, recv).await;
        });

        data
    }

    async fn watch_file(&self, cancel: Cancellation, db: Database, mut change: Receiver<()>) {
        let mut last_changed = tokio::fs::metadata(Self::PATH)
            .await
            .unwrap()
            .modified()
            .unwrap_or(SystemTime::now());

        let mut last_admin = self.admin().await;
        let mut last_wait = self.index_wait().await;

        let mut update_file = false;
        loop {
            if !Path::new(Self::PATH).exists() {
                warn!("Config file does not exist, trying to create one.");
                Self::create_config_file(&self.config_cloned().await).await;
            }

            if update_file {
                let mut file = match tokio::fs::File::open(Self::PATH)
                    .await
                    .log_err_with_msg("Failed to open config file, trying to create a new one")
                {
                    Some(file) => file,
                    None => {
                        let config = self.config_cloned().await;
                        Self::create_config_file(&config).await;
                        continue;
                    }
                };

                let server_side = self.config_cloned().await;
                let toml_repr = toml::to_string_pretty(&server_side)
                    .expect("failed to serialize config, this should never happen");

                file.write_all(toml_repr.as_bytes())
                    .await
                    .log_warn_with_msg("Failed to write to config file");
                file.flush()
                    .await
                    .log_warn_with_msg("Failed to flush config file");
            } else {
                let config_file = match tokio::fs::read_to_string(Self::PATH).await {
                    Ok(config_file) => config_file,
                    Err(e) => {
                        error!("Failed to read config file: {e}");
                        let default = ConfigFile::default();
                        Self::create_config_file(&default).await;
                        toml::to_string_pretty(&default)
                            .expect("failed to serialize default config after it should have been created, this should never happen")
                    }
                };

                let config: ConfigFile = toml::from_str(&config_file)
                    .log_err_with_msg(
                        "Failed to parse config file, using the default config instead",
                    )
                    .unwrap_or_default();

                self.config_modifiable()
                    .await
                    .write()
                    .await
                    .clone_from(&config);
            }

            self.update_db_to_file_content(&db, &mut last_admin)
                .await
                .log_warn_with_msg("failed to change database in accordance with config file");

            let current_wait = self.index_wait().await;
            if last_wait != current_wait && current_wait > 0. {
                last_wait = current_wait;
                self.abort_wait
                    .send(current_wait)
                    .expect("This channel should live for the entire porgram");
            } else if current_wait <= 0. {
                // TODO: Make 0 indicate directory watching is enabled instead of wait time
                warn!("Indexing wait time is 0 or less, this is not allowed");
            }

            let (should_stop, u_f, l_c) = tokio::select! {
                _ = change.changed() => {
                    (false, true, last_changed)
                },
                last = Self::resolve_once_modified(last_changed) => {
                    debug!("Registered config file change");
                    (false, false, last)
                },
                _ = cancel.cancelled() => (true, false, last_changed),
            };
            last_changed = l_c;
            update_file = u_f;

            if should_stop {
                break;
            }
        }
    }

    async fn create_config_file(config: &ConfigFile) {
        let config = toml::to_string_pretty(config)
            .expect("failed to serialize config, this should never happen");

        while tokio::fs::write(Self::PATH, &config)
            .await
            .log_err_with_msg("Failed to write config file, trying again in half a minute.")
            .is_none()
        {
            tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
        }
    }

    async fn resolve_once_modified(last_changed: SystemTime) -> SystemTime {
        tokio::task::spawn(async move {
            loop {
                let file_last_changed = tokio::fs::metadata(Self::PATH)
                    .await
                    .log_err_with_msg("Failed to get metadata for config file")
                    .unwrap()
                    .modified()
                    .unwrap_or(SystemTime::now());

                if file_last_changed > last_changed {
                    break file_last_changed;
                }
                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            }
        })
        .await
        .expect("failed to resolve once modified, this should never happen")
    }

    pub async fn wait_configured_time(&self) {
        let mut recv = self.abort_wait.subscribe();
        tokio::select! {
            _ = recv.changed() => {
                info!("changed indexing waiting time to {} seconds and started indexing again", *recv.borrow());
            },
            _ = tokio::time::sleep(tokio::time::Duration::from_secs_f64(self.index_wait().await)) => {},
        }
    }

    async fn update_db_to_file_content(
        &self,
        db: &Database,
        last_admin: &mut AdminCredentials,
    ) -> AppResult<()> {
        let conn = db.get()?;

        let users_is_empty = conn
            .call(|conn| {
                Ok(conn
                    .query_row_get("SELECT COUNT(*) FROM users", [])
                    .map(|count: i64| count == 0)
                    .unwrap_or(false))
            })
            .await?;
        let admin = self.admin().await;
        let (username, pw) = (admin.username.clone(), admin.password.clone());
        let password = tokio::task::spawn_blocking(|| password_auth::generate_hash(pw))
            .await
            .expect("generating the password shouldn't fail");

        let (last_username, pw) = (last_admin.username.clone(), last_admin.password.clone());
        let last_password = tokio::task::spawn_blocking(|| password_auth::generate_hash(pw))
            .await
            .expect("generating the password shouldn't fail");

        if (&username, &password) == (&last_username, &last_password) && !users_is_empty {
            return Ok(());
        } else {
            *last_admin = admin;
        }

        // TODO: Once more permission are there, make this remove any user with these permissions, not last_admin and insert this new one. The file is the source of truth
        if !users_is_empty {
            conn.call(|conn| {
                conn.execute("DELETE FROM users WHERE username = ?1", [last_username])
                    .convert_err()
            })
            .await
            .log_err_with_msg("Failed to remove last admin, there might be multiple users now");
        }

        conn.call(|conn| {
            conn.execute(
                "INSERT INTO users (username, password) VALUES (?1, ?2)",
                [username, password],
            )
            .convert_err()
        })
        .await?;

        Ok(())
    }

    #[inline(always)]
    /// When this is used, there has to be a send into the change channel to make the system repsond
    async fn config_modifiable(&self) -> ServerConfig {
        self.inner.clone()
    }

    #[inline(always)]
    pub async fn config_cloned(&self) -> ConfigFile {
        self.inner.read().await.clone()
    }

    #[inline(always)]
    pub async fn port(&self) -> u16 {
        self.config_modifiable().await.read().await.port
    }

    #[inline(always)]
    pub async fn index_wait(&self) -> f64 {
        self.config_modifiable().await.read().await.index_wait
    }

    #[inline(always)]
    pub async fn admin(&self) -> AdminCredentials {
        self.config_modifiable().await.read().await.admin.clone()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigFile {
    port: u16,
    index_wait: f64,
    admin: AdminCredentials,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminCredentials {
    username: String,
    password: String,
}

impl Default for ConfigFile {
    fn default() -> Self {
        Self {
            port: 3000,
            index_wait: 300.,
            admin: Default::default(),
        }
    }
}

impl Default for AdminCredentials {
    fn default() -> Self {
        Self {
            // The username is static for now, file changes don't affect it
            username: "admin".to_owned(),
            password: "admin".to_owned(),
        }
    }
}
