use std::{path::Path, sync::Arc, time::SystemTime};

use crate::{
    database::{Database, QueryRowGetConnExt, QueryRowGetStmtExt},
    state::{AppResult, Shutdown},
};

use serde::{Deserialize, Serialize};
use tokio::{
    io::AsyncWriteExt,
    sync::watch::{self, Receiver, Sender},
};
use tracing::{debug, error, info, warn};

use super::HandleErr;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigFile {
    port: u16,
    index_wait: f64,
    admin: AdminCredentials,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdminCredentials {
    pub username: String,
    password: String,
}

impl Default for ConfigFile {
    fn default() -> Self {
        Self {
            port: 3000,
            index_wait: 300.,
            admin: AdminCredentials::default(),
        }
    }
}

impl Default for AdminCredentials {
    fn default() -> Self {
        Self {
            username: "admin".to_owned(),
            password: "admin".to_owned(),
        }
    }
}

#[derive(Clone)]
pub struct ServerSettings {
    port: (Arc<Sender<u16>>, Receiver<u16>),
    index_wait: (Arc<Sender<f64>>, Receiver<f64>),
    admin: (Arc<Sender<AdminCredentials>>, Receiver<AdminCredentials>),
}

impl ServerSettings {
    const PATH: &'static str = "mreconfig.toml";

    pub async fn new(shutdown: Shutdown, db: Database) -> Self {
        let config = if let Some(config_file) = tokio::fs::read_to_string(Self::PATH)
            .await
            .log_warn_with_msg("Failed to create config file, trying to create a new one")
        {
            toml::from_str(&config_file)
                .log_err_with_msg("Failed to parse config file, using the default config instead")
                .unwrap_or_default()
        } else {
            let default = ConfigFile::default();
            Self::write_config_file(&default).await;
            default
        };

        let (port, port_recv) = watch::channel(config.port);
        let (index_wait, index_wait_recv) = watch::channel(config.index_wait);
        let (admin, admin_recv) = watch::channel(config.admin.clone());

        let data = Self {
            port: (Arc::new(port), port_recv),
            index_wait: (Arc::new(index_wait), index_wait_recv),
            admin: (Arc::new(admin), admin_recv),
        };

        {
            let mut last_admin = data.admin();
            data.update_db_to_file_content(&db, &mut last_admin)
                .await
                .log_warn_with_msg("failed to change database in accordance with config file");

            let mut copy = data.clone();
            tokio::spawn(async move {
                copy.watch_file(shutdown, db).await;
            });
        }

        data
    }

    fn create_config(&self) -> ConfigFile {
        let port = self.port();
        let index_wait = self.index_wait();
        let admin = self.admin();
        ConfigFile {
            port,
            index_wait,
            admin,
        }
    }

    async fn watch_file(&mut self, shutdown: Shutdown, db: Database) {
        let mut last_changed = tokio::fs::metadata(Self::PATH)
            .await
            .unwrap()
            .modified()
            .unwrap_or(SystemTime::now());

        let mut last_admin = self.admin();

        let mut update_file = false;
        let mut file_is_update_origin = false;
        loop {
            if !Path::new(Self::PATH).exists() {
                warn!("Config file does not exist, trying to create one.");
                self.write_config_from_self().await;
            }

            if file_is_update_origin {
                file_is_update_origin = false;
                update_file = false;
            }

            if update_file {
                let Some(mut file) = tokio::fs::File::options()
                    .read(true)
                    .write(true)
                    .open(Self::PATH)
                    .await
                    .log_err_with_msg("Failed to open config file, trying to create a new one")
                else {
                    self.write_config_from_self().await;
                    continue;
                };

                let server_side = self.create_config();
                let toml_repr = toml::to_string_pretty(&server_side)
                    .expect("failed to serialize config, this should never happen");

                file.write_all(toml_repr.as_bytes())
                    .await
                    .log_warn_with_msg("Failed to write to config file");
                file.flush()
                    .await
                    .log_warn_with_msg("Failed to flush config file");
            } else {
                let config = match tokio::fs::read_to_string(Self::PATH).await {
                    Ok(config_file) => toml::from_str(&config_file)
                        .log_err_with_msg(
                            "Failed to parse config file, using the default config instead",
                        )
                        .unwrap_or_default(),
                    Err(e) => {
                        error!("Failed to read config file: {e}");
                        self.write_config_from_self().await;
                        continue;
                    }
                };

                self.set_all(config);
                file_is_update_origin = true;
            }

            self.update_db_to_file_content(&db, &mut last_admin)
                .await
                .log_warn_with_msg("failed to change database in accordance with config file");

            let (u_f, l_c) = tokio::select! {
                _ = self.any_changed() => {
                    file_is_update_origin = false;
                    (true, last_changed)
                },
                last = Self::resolve_once_modified(last_changed) => {
                    debug!("Registered config file change");
                    (false, last)
                },
                _ = shutdown.cancelled() => return,
            };
            last_changed = l_c;
            update_file = u_f;
        }
    }

    async fn any_changed(&mut self) {
        tokio::select! {
            _ = self.port.1.changed() => {},
            _ = self.index_wait.1.changed() => {},
            _ = self.admin.1.changed() => {},
        }
    }

    async fn write_config_file(config: &ConfigFile) {
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

    async fn write_config_from_self(&self) {
        let config = self.create_config();
        Self::write_config_file(&config).await;
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
        let mut recv = self.index_wait.0.subscribe();
        tokio::select! {
            _ = recv.changed() => {
                info!("changed indexing waiting time to {} seconds and started indexing again", *recv.borrow());
            },
            _ = tokio::time::sleep(tokio::time::Duration::from_secs_f64(self.index_wait())) => {},
        }
    }

    async fn update_db_to_file_content(
        &self,
        db: &Database,
        last_admin: &mut AdminCredentials,
    ) -> AppResult<()> {
        let conn = db.get()?;

        let users_is_empty = conn
            .query_row_get("SELECT COUNT(*) FROM users", [])
            .map(|count: i64| count == 0)
            .unwrap_or(false);

        let admin = self.admin();
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
        }
        *last_admin = admin;

        let owner_permission_id =
            conn.query_row_get::<u32>("SELECT id FROM permissions WHERE name = 'owner'", [])?;

        let mut stmt =
            conn.prepare("SELECT userid FROM user_permissions WHERE permissionid = ?1")?;
        let user_ids_with_perm = stmt
            .query_map_get::<u32>([owner_permission_id])?
            .map(|v| v.unwrap())
            .collect::<Vec<_>>();

        for user_id in user_ids_with_perm {
            conn.execute("DELETE FROM user_permissions WHERE userid = ?1", [user_id])?;
            conn.execute("DELETE FROM users WHERE id = ?1", [user_id])?;
        }

        let user_id = conn.query_row_get::<u32>(
            "INSERT INTO users (username, password) VALUES (?1, ?2) RETURNING id",
            [username, password],
        )?;

        conn.execute(
            "INSERT INTO user_permissions (userid, permissionid) VALUES (?1, ?2)",
            [user_id, owner_permission_id],
        )?;

        Ok(())
    }

    pub fn port(&self) -> u16 {
        *self.port.1.borrow()
    }

    pub fn set_port(&self, port: u16) {
        self.port.0.send_if_modified(|current| {
            let is_different = *current != port;
            if is_different {
                warn!("The port to spawn the server on was modified, this will only take effect after a restart of the server.");
                *current = port;
            }
            is_different
        });
    }

    pub fn index_wait(&self) -> f64 {
        *self.index_wait.1.borrow()
    }

    pub fn set_index_wait(&self, wait: f64) {
        self.index_wait.0.send_if_modified(|current| {
            let is_different = (*current - wait).abs() > f64::EPSILON;
            if is_different {
                *current = wait;
            }
            is_different
        });
    }

    pub fn admin(&self) -> AdminCredentials {
        self.admin.1.borrow().clone()
    }

    pub fn set_admin(&self, admin: AdminCredentials) {
        self.admin.0.send_if_modified(|current| {
            let is_different = *current != admin;
            if is_different {
                *current = admin;
            }
            is_different
        });
    }

    pub fn update_admin_username(&self, username: &str) {
        let pw = self.admin().password;
        self.set_admin(AdminCredentials {
            username: username.to_owned(),
            password: pw,
        })
    }

    pub fn update_admin_password(&self, password: &str) {
        let username = self.admin().username;
        self.set_admin(AdminCredentials {
            username,
            password: password.to_owned(),
        })
    }

    pub fn set_all(&self, config: ConfigFile) {
        let (port, wait, admin) = (config.port, config.index_wait, config.admin);
        self.set_port(port);
        self.set_index_wait(wait);
        self.set_admin(admin);
    }
}
