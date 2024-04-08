use std::{collections::HashSet, ops::Deref};

use axum::{
    async_trait,
    body::Body,
    extract::{OriginalUri, Request},
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
    state::{AppError, AppResult},
};

use super::ConvertErr;

pub type AuthSession = axum_login::AuthSession<Database>;

pub trait AuthExt {
    async fn has_perm(&self, perm: impl Into<Permission>) -> AppResult<bool>;
}

impl AuthExt for AuthSession {
    async fn has_perm(&self, perm: impl Into<Permission>) -> AppResult<bool> {
        if let Some(user) = &self.user {
            self.backend.has_perm(user, perm.into()).await
        } else {
            Err(AppError::Custom(
                "Tried to check permission of a user that isn't logged in".to_string(),
            ))
        }
    }
}

#[derive(Clone)]
pub struct User {
    pub id: i64,
    pub username: String,
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

impl From<rusqlite::Error> for SessionStoreError {
    fn from(err: rusqlite::Error) -> Self {
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
            .query_row_into::<User>(
                "SELECT id, username, password FROM users WHERE username = ?1",
                [creds.username],
            )
            .optional()?;

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
            .query_row_into::<User>(
                "SELECT id, username, password FROM users WHERE id = ?1",
                [id],
            )
            .optional()?;

        Ok(user)
    }
}

#[derive(PartialEq, Eq, Hash)]
pub struct Permission {
    name: String,
}

impl From<&str> for Permission {
    fn from(value: &str) -> Self {
        Self {
            name: value.to_string(),
        }
    }
}

impl From<String> for Permission {
    fn from(value: String) -> Self {
        Self { name: value }
    }
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

        let permissions = conn.prepare(
                "SELECT DISTINCT permissions.name FROM users, permissions, user_permissions WHERE users.id = ?1 AND users.id = user_permissions.userid AND user_permissions.permissionid = permissions.id",
            )?
                .query_map_into([user.id])?
                .collect::<Result<HashSet<_>, _>>()?;
        Ok(permissions)
    }

    async fn get_group_permissions(
        &self,
        user: &User,
    ) -> Result<HashSet<Self::Permission>, Self::Error> {
        let conn = self.get()?;
        let user = user.clone();

        let permissions = conn.prepare(
                "SELECT DISTINCT permissions.name FROM users, groups, permissions, user_groups, group_permissions WHERE users.id = ?1 AND users.id = user_groups.userid AND user_groups.groupid = groups.id AND groups.id = group_permissions.groupid AND group_permissions.permissionid = permissions.id"
            )?
                .query_map_into([user.id])?
                .collect::<Result<HashSet<_>, _>>()?;
        Ok(permissions)
    }
}

fn save_with_conn(
    conn: &rusqlite::Connection,
    record_id: &str,
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
                conn.query_row_get::<bool>(
                    "SELECT exists(SELECT 1 FROM session_store WHERE id = ?1)",
                    [record.id.to_string()],
                )
                .convert_err::<SessionStoreError>()?
            } {
                record.id = Id::default();
            }

            let record_data = rmp_serde::to_vec(&record).convert_err::<SessionStoreError>()?;
            save_with_conn(
                &conn,
                &record.id.to_string(),
                &record_data,
                record.expiry_date.unix_timestamp(),
            )
            .convert_err::<SessionStoreError>()?;

            record.id
        };

        Ok(())
    }

    async fn save(&self, record: &Record) -> session_store::Result<()> {
        let conn = self.get().convert_err::<SessionStoreError>()?;

        let record = record.clone();
        let record_data = rmp_serde::to_vec(&record).convert_err::<SessionStoreError>()?;
        save_with_conn(
            &conn,
            &record.id.to_string(),
            &record_data,
            record.expiry_date.unix_timestamp(),
        )
        .convert_err::<SessionStoreError>()?;

        Ok(())
    }

    async fn load(&self, session_id: &Id) -> session_store::Result<Option<Record>> {
        let conn = self.get().convert_err::<SessionStoreError>()?;

        let session_id = *session_id;
        let data = conn
            .query_row_get::<Vec<u8>>(
                "SELECT data FROM session_store WHERE id = ?1 and expiry_date > ?2",
                params![
                    session_id.to_string(),
                    OffsetDateTime::now_utc().unix_timestamp()
                ],
            )
            .optional()
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
        conn.execute(
            "DELETE FROM session_store WHERE id = ?1",
            [session_id.to_string()],
        )
        .convert_err::<SessionStoreError>()?;

        Ok(())
    }
}

#[async_trait]
impl ExpiredDeletion for Database {
    async fn delete_expired(&self) -> session_store::Result<()> {
        let conn = self.get().convert_err::<SessionStoreError>()?;

        conn.execute(
            "DELETE FROM session_store WHERE expiry_date < ?1",
            [OffsetDateTime::now_utc().unix_timestamp()],
        )
        .convert_err::<SessionStoreError>()?;

        Ok(())
    }
}

pub async fn login_required(
    auth: AuthSession,
    hm: HeaderMap,
    OriginalUri(uri): OriginalUri,
    request: Request,
    next: Next,
) -> Response<Body> {
    if auth.user.is_some() {
        return next.run(request).await.into_response();
    }
    let htmx_enabled = hm.get("HX-Request").is_some();

    if htmx_enabled {
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
        let redirect = format!("/auth/login?next=/{path}");
        (StatusCode::UNAUTHORIZED, [("HX-Redirect", redirect)]).into_response()
    } else {
        let redirect = format!("/auth/login?next={uri}");
        (StatusCode::SEE_OTHER, [("Location", redirect)]).into_response()
    }
}
