use askama::Template;
use askama_axum::IntoResponse;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Redirect,
    routing::{get, post},
    Form, Router,
};
use serde::Deserialize;

use crate::{
    database::{Database, QueryRowGetConnExt},
    state::{AppError, AppResult, AppState},
    utils::{AuthSession, ConvertErr, Credentials, HandleErr},
};

use super::homepage::Index;

pub fn login() -> Router<AppState> {
    Router::new()
        .route("/login", get(login_page))
        .route("/login/submit", post(login_form))
        .route("/logout", get(logout))
        .route("/register", get(register_page))
        .route("/register/submit", post(register_form))
}

#[derive(Template)]
#[template(path = "../frontend/content/login.html")]
struct LoginPage<'a> {
    title: &'a str,
    post_url: &'a str,
    sub_text: Option<&'a str>,
    message: Option<String>,
}

#[derive(Deserialize)]
struct Message {
    message: Option<String>,
}

async fn login_page(
    Query(message): Query<Message>,
    State(db): State<Database>,
) -> AppResult<impl IntoResponse> {
    let login_page = LoginPage {
        title: "Login",
        post_url: "/auth/login/submit",
        sub_text: if is_noonne_registered(db).await? {
            Some(r#"<a href="/auth/register"> Register here! </a>"#)
        } else {
            None
        },
        message: message.message,
    };
    let body = login_page.render()?;

    Ok(Index {
        body,
        all: "".to_owned(),
    })
}

async fn login_form(mut auth: AuthSession, Form(creds): Form<Credentials>) -> impl IntoResponse {
    let user = match auth.authenticate(creds).await {
        Ok(Some(user)) => user,
        Ok(None) => return Redirect::to("/auth/login?message=Wrong Credentials").into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    if auth.login(&user).await.log_warn().is_none() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    Redirect::to("/").into_response()
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
