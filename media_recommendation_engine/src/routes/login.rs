use std::{fmt, str::FromStr};

use askama::Template;
use askama_axum::IntoResponse;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Redirect,
    routing::{get, post},
    Form, Router,
};
use serde::{de, Deserialize, Deserializer};

use crate::{
    database::{Database, QueryRowGetConnExt},
    state::{AppError, AppResult, AppState},
    utils::{
        templates::{Index, LoginPage},
        AuthSession, ConvertErr, Credentials, HandleErr,
    },
};

pub fn login() -> Router<AppState> {
    Router::new()
        .route("/login", get(login_page))
        .route("/login/submit", post(login_form))
        .route("/logout", get(logout))
        .route("/register", get(register_page))
        .route("/register/submit", post(register_form))
}

fn empty_string_as_none<'de, D, T>(de: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr,
    T::Err: fmt::Display,
{
    let opt = Option::<String>::deserialize(de)?;
    match opt.as_deref() {
        None | Some("") => Ok(None),
        Some(s) => FromStr::from_str(s).map_err(de::Error::custom).map(Some),
    }
}

#[derive(Deserialize)]
struct Next {
    #[serde(default, deserialize_with = "empty_string_as_none")]
    next: Option<String>,
}

#[derive(Deserialize)]
struct Message {
    #[serde(default, deserialize_with = "empty_string_as_none")]
    message: Option<String>,
}

#[derive(Deserialize)]
struct Params {
    #[serde(default, deserialize_with = "empty_string_as_none")]
    message: Option<String>,
    #[serde(default, deserialize_with = "empty_string_as_none")]
    next: Option<String>,
}

async fn login_page(
    Query(params): Query<Params>,
    State(db): State<Database>,
) -> AppResult<impl IntoResponse> {
    let message = params.message;
    let next = params.next;

    let post_url = &match next {
        Some(next) => format!("/auth/login/submit?next={next}"),
        None => "/auth/login/submit".to_owned(),
    };

    let sub_text = if is_noonne_registered(db).await? {
        Some(r#"<a href="/auth/register"> Register here! </a>"#)
    } else {
        None
    };

    let login_page = LoginPage {
        title: "Login",
        post_url,
        sub_text,
        message: message.map(|m| m.to_owned()),
    };
    let body = login_page.render()?;

    Ok(Index {
        body,
        all: "".to_owned(),
    })
}

async fn login_form(
    mut auth: AuthSession,
    Query(next): Query<Next>,
    Form(creds): Form<Credentials>,
) -> impl IntoResponse {
    let user = match auth.authenticate(creds).await {
        Ok(Some(user)) => user,
        Ok(None) => {
            let redirect = match next.next {
                Some(next) => format!("/auth/login?message=Wrong Credentials&next={next}"),
                None => "/auth/login?message=Wrong Credentials".to_owned(),
            };
            return Redirect::to(&redirect).into_response();
        }
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    if auth.login(&user).await.log_warn().is_none() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let redirect = next.next.unwrap_or("/".to_owned());

    Redirect::to(&redirect).into_response()
}

async fn logout(mut auth: AuthSession) -> impl IntoResponse {
    match auth.logout().await {
        Ok(_) => ([("HX-Redirect", "/auth/login")], "").into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn register_page(Query(message): Query<Message>) -> AppResult<impl IntoResponse> {
    let register_page = LoginPage {
        title: "Register",
        post_url: "/auth/register/submit",
        sub_text: None,
        message: message.message,
    };
    let body = register_page.render()?;

    Ok(Index {
        body,
        all: "".to_owned(),
    })
}

async fn register_form(
    State(db): State<Database>,
    Form(creds): Form<Credentials>,
) -> AppResult<impl IntoResponse> {
    let conn = db.get()?;

    if is_noonne_registered(db).await? {
        let password = tokio::task::spawn_blocking(|| password_auth::generate_hash(creds.password))
            .await
            .map_err(|e| AppError::Custom(e.to_string()))?;

        conn.call(|conn| {
            conn.execute(
                "INSERT INTO users (username, password) VALUES (?, ?)",
                [creds.username, password],
            )
            .convert_err()
        })
        .await?;
    } else {
        return Ok(
            Redirect::to("/auth/register?message=Multiple Users currently not permitted")
                .into_response(),
        );
    }

    Ok(Redirect::to("/auth/login").into_response())
}

async fn is_noonne_registered(db: Database) -> AppResult<bool> {
    let conn = db.get()?;
    conn.call(|conn| {
        Ok(conn
            .query_row_get("SELECT COUNT(*) FROM users", [])
            .map(|count: i64| count == 0)
            .unwrap_or(false))
    })
    .await
    .convert_err()
}
