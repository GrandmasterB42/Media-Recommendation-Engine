use std::{collections::HashSet, ops::Deref};

use axum::{
    async_trait,
    body::Body,
    extract::Request,
    http::{HeaderMap, Response, StatusCode},
    middleware::Next,
    response::IntoResponse,
};
use axum_login::{
    tower_sessions::{
        session::{Id, Record},
        session_store, ExpiredDeletion, SessionStore,
    },
    AuthUser, AuthnBackend, AuthzBackend, UserId,
};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use time::OffsetDateTime;

use crate::{
    database::{Database, QueryRowGetConnExt, QueryRowIntoConnExt, QueryRowIntoStmtExt},
    state::AppError,
};

use super::ConvertErr;

pub type AuthSession = axum_login::AuthSession<Database>;

#[derive(Clone)]
pub struct User {
    id: i64,
    username: String,
    password: String,
}

impl std::fmt::Debug for User {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("User")
            .field("id", &self.id)
            .field("username", &self.username)
            .field("password", &"[redacted]")
            .finish()
    }
}

impl TryFrom<&rusqlite::Row<'_>> for User {
    type Error = rusqlite::Error;

    fn try_from(row: &rusqlite::Row<'_>) -> Result<Self, Self::Error> {
        Ok(Self {
            id: row.get(0)?,
            username: row.get(1)?,
            password: row.get(2)?,
        })
    }
}

impl AuthUser for User {
    type Id = i64;

    fn id(&self) -> Self::Id {
        self.id
    }

    fn session_auth_hash(&self) -> &[u8] {
        self.password.as_bytes()
    }
}

#[derive(Deserialize)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

struct SessionStoreError(session_store::Error);

impl Deref for SessionStoreError {
    type Target = session_store::Error;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<session_store::Error> for SessionStoreError {
    fn from(err: session_store::Error) -> Self {
        Self(err)
    }
}

impl From<SessionStoreError> for session_store::Error {
    fn from(err: SessionStoreError) -> Self {
        err.0
    }
}

impl From<r2d2::Error> for SessionStoreError {
    fn from(err: r2d2::Error) -> Self {
        session_store::Error::Backend(err.to_string()).into()
    }
}

impl From<tokio_rusqlite::Error> for SessionStoreError {
    fn from(err: tokio_rusqlite::Error) -> Self {
        session_store::Error::Backend(err.to_string()).into()
    }
}

impl From<rmp_serde::encode::Error> for SessionStoreError {
    fn from(err: rmp_serde::encode::Error) -> Self {
        session_store::Error::Encode(err.to_string()).into()
    }
}

impl From<rmp_serde::decode::Error> for SessionStoreError {
    fn from(err: rmp_serde::decode::Error) -> Self {
        session_store::Error::Decode(err.to_string()).into()
    }
}

#[async_trait]
impl AuthnBackend for Database {
    type User = User;
    type Credentials = Credentials;
    type Error = AppError;

    async fn authenticate(
        &self,
        creds: Self::Credentials,
    ) -> Result<Option<Self::User>, Self::Error> {
        let conn = self.get()?;

        let user = conn
            .call(move |conn| {
                conn.query_row_into::<User>(
                    "SELECT id, username, password FROM users WHERE username = ?1",
                    [creds.username],
                )
                .optional()
                .convert_err()
            })
            .await?;

        tokio::task::spawn_blocking(|| {
            Ok(user.filter(|user| {
                password_auth::verify_password(creds.password, &user.password).is_ok()
            }))
        })
        .await
        .map_err(|e| AppError::Custom(e.to_string()))?
    }

    async fn get_user(&self, id: &UserId<Self>) -> Result<Option<Self::User>, Self::Error> {
        let conn = self.get()?;

        let id = *id;
        let user = conn
            .call(move |conn| {
                conn.query_row_into::<User>(
                    "SELECT id, username, password FROM users WHERE id = ?1",
                    [id],
                )
                .optional()
                .convert_err()
            })
            .await?;

        Ok(user)
    }
}

#[derive(PartialEq, Eq, Hash)]
pub struct Permission {
    name: String,
}

impl TryFrom<&rusqlite::Row<'_>> for Permission {
    type Error = rusqlite::Error;

    fn try_from(row: &rusqlite::Row<'_>) -> Result<Self, Self::Error> {
        Ok(Self { name: row.get(0)? })
    }
}

#[async_trait]
impl AuthzBackend for Database {
    type Permission = Permission;

    async fn get_user_permissions(
        &self,
        user: &User,
    ) -> Result<HashSet<Self::Permission>, Self::Error> {
        let conn = self.get()?;
        let user = user.clone();
        conn.call(move |conn: &mut rusqlite::Connection| {
            let permissions = conn.prepare(
                "SELECT DISTINCT permissions.name FROM users, permission, user_permissions WHERE users.id = ?1 AND users.id = user_permissions.userid AND user_permissions.permissionid = permissions.id",
            )?
                .query_map_into([user.id])?
                .collect::<Result<HashSet<_>, _>>()?;
            Ok(permissions)
        })
        .await
        .convert_err()
    }

    async fn get_group_permissions(
        &self,
        user: &User,
    ) -> Result<HashSet<Self::Permission>, Self::Error> {
        let conn = self.get()?;
        let user = user.clone();
        conn.call(move |conn: &mut rusqlite::Connection| {
            let permissions = conn.prepare(
                "SELECT DISTINCT permissions.name FROM users, groups, permission, user_groups, group_permissions WHERE users.id = ?1 AND users.id = user_groups.userid AND user_groups.groupid = groups.id AND groups.id = group_permissions.groupid AND group_permissions.permissionid = permissions.id"
            )?
                .query_map_into([user.id])?
                .collect::<Result<HashSet<_>, _>>()?;
            Ok(permissions)
        })
        .await
        .convert_err()
    }
}

fn save_with_conn(
    conn: &rusqlite::Connection,
    record_id: String,
    record_data: &[u8],
    record_expiry_date: i64,
) -> rusqlite::Result<usize> {
    conn.execute(
        "
        insert into session_store
            (id, data, expiry_date) values (?1, ?2, ?3)
        on conflict(id) do update set
            data = excluded.data,
            expiry_date = excluded.expiry_date
        ",
        params![record_id, record_data, record_expiry_date],
    )
}

#[async_trait]
impl SessionStore for Database {
    async fn create(&self, record: &mut Record) -> session_store::Result<()> {
        let conn = self.get().convert_err::<SessionStoreError>()?;

        record.id = {
            let mut record = record.clone();

            while {
                conn.call(move |conn| {
                    conn.query_row_get::<bool>(
                        "SELECT exists(SELECT 1 FROM session_store WHERE id = ?1)",
                        [record.id.to_string()],
                    )
                    .convert_err()
                })
                .await
                .convert_err::<SessionStoreError>()?
            } {
                record.id = Id::default();
            }

            let record_data = rmp_serde::to_vec(&record).convert_err::<SessionStoreError>()?;
            conn.call(move |conn| {
                Ok(save_with_conn(
                    conn,
                    record.id.to_string(),
                    &record_data,
                    record.expiry_date.unix_timestamp(),
                )?)
            })
            .await
            .convert_err::<SessionStoreError>()?;

            record.id
        };

        Ok(())
    }

    async fn save(&self, record: &Record) -> session_store::Result<()> {
        let conn = self.get().convert_err::<SessionStoreError>()?;

        let record = record.clone();
        let record_data = rmp_serde::to_vec(&record).convert_err::<SessionStoreError>()?;
        conn.call(move |conn| {
            Ok(save_with_conn(
                conn,
                record.id.to_string(),
                &record_data,
                record.expiry_date.unix_timestamp(),
            )?)
        })
        .await
        .convert_err::<SessionStoreError>()?;

        Ok(())
    }

    async fn load(&self, session_id: &Id) -> session_store::Result<Option<Record>> {
        let conn = self.get().convert_err::<SessionStoreError>()?;

        let session_id = *session_id;
        let data = conn
            .call(move |conn| {
                conn.query_row_get::<Vec<u8>>(
                    "SELECT data FROM session_store WHERE id = ?1 and expiry_date > ?2",
                    params![
                        session_id.to_string(),
                        OffsetDateTime::now_utc().unix_timestamp()
                    ],
                )
                .optional()
                .convert_err()
            })
            .await
            .convert_err::<SessionStoreError>()?;

        match data {
            Some(data) => rmp_serde::from_slice::<Record>(&data)
                .map(Some)
                .convert_err::<SessionStoreError>()
                .map_err(|e| e.0),
            None => Ok(None),
        }
    }

    async fn delete(&self, session_id: &Id) -> session_store::Result<()> {
        let conn = self.get().convert_err::<SessionStoreError>()?;

        let session_id = *session_id;
        conn.call(move |conn| {
            conn.execute(
                "DELETE FROM session_store WHERE id = ?1",
                [session_id.to_string()],
            )
            .convert_err()
        })
        .await
        .convert_err::<SessionStoreError>()?;

        Ok(())
    }
}

#[async_trait]
impl ExpiredDeletion for Database {
    async fn delete_expired(&self) -> session_store::Result<()> {
        let conn = self.get().convert_err::<SessionStoreError>()?;

        conn.call(move |conn| {
            conn.execute(
                "DELETE FROM session_store WHERE expiry_date < ?1",
                [OffsetDateTime::now_utc().unix_timestamp()],
            )
            .convert_err()
        })
        .await
        .convert_err::<SessionStoreError>()?;

        Ok(())
    }
}

pub async fn login_required(
    auth: AuthSession,
    hm: HeaderMap,
    request: Request,
    next: Next,
) -> Response<Body> {
    let current = hm.get("HX-Current-Url");
    let complete = current
        .and_then(|current| current.to_str().ok())
        .unwrap_or_default();
    let path = complete
        .split_once("//")
        .unwrap_or(("", ""))
        .1
        .split_once('/')
        .unwrap_or(("", ""))
        .1;

    println!("/{path}");

    match auth.user {
        Some(_) => next.run(request).await.into_response(),
        None => (
            StatusCode::UNAUTHORIZED,
            [("HX-Redirect", format!("/auth/login?next=/{path}"))],
        )
            .into_response(),
    }
}
