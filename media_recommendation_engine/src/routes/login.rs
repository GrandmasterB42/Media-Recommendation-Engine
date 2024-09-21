use askama::Template;
use askama_axum::IntoResponse;
use axum::{
    extract::Query,
    http::StatusCode,
    routing::{get, post},
    Form, Router,
};
use serde::Deserialize;

use crate::{
    state::{AppResult, AppState},
    utils::{
        empty_string_as_none,
        templates::{Index, LoginPage, SwapIn},
        AuthSession, Credentials, HandleErr,
    },
};

pub fn login() -> Router<AppState> {
    Router::new()
        .route("/login", get(login_page))
        .route("/login/submit", post(login_form))
        .route("/logout", post(logout))
}

#[derive(Deserialize)]
struct Next {
    #[serde(default, deserialize_with = "empty_string_as_none")]
    next: Option<String>,
}

#[derive(Deserialize)]
struct Params {
    #[serde(default, deserialize_with = "empty_string_as_none")]
    next: Option<String>,
}

async fn login_page(Query(params): Query<Params>) -> AppResult<impl IntoResponse> {
    let next = params.next;

    let post_url = &match next {
        Some(next) => format!("/auth/login/submit?next={next}"),
        None => "/auth/login/submit".to_owned(),
    };

    let login_page = LoginPage {
        title: "Login",
        post_url,
        sub_text: None,
    };
    let body = login_page.render()?;

    Ok(Index {
        body,
        all: String::new(),
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
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                SwapIn {
                    swap_id: "error",
                    swap_method: None,
                    content: "Wrong Credentials!",
                },
            )
                .into_response()
        }
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    if auth.login(&user).await.log_warn().is_none() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let redirect = next.next.unwrap_or("/".to_owned());

    (StatusCode::OK, [("HX-Redirect", redirect)]).into_response()
}

async fn logout(mut auth: AuthSession) -> impl IntoResponse {
    match auth.logout().await {
        Ok(_) => ([("HX-Redirect", "/auth/login")], "").into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
