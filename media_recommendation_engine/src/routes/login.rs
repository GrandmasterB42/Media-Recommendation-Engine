use std::{fmt, str::FromStr};

use askama::Template;
use askama_axum::IntoResponse;
use axum::{
    extract::Query,
    http::StatusCode,
    response::Redirect,
    routing::{get, post},
    Form, Router,
};
use serde::{de, Deserialize, Deserializer};

use crate::{
    state::{AppResult, AppState},
    utils::{
        templates::{Index, LoginPage},
        AuthSession, Credentials, HandleErr,
    },
};

pub fn login() -> Router<AppState> {
    Router::new()
        .route("/login", get(login_page))
        .route("/login/submit", post(login_form))
        .route("/logout", post(logout))
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
struct Params {
    #[serde(default, deserialize_with = "empty_string_as_none")]
    message: Option<String>,
    #[serde(default, deserialize_with = "empty_string_as_none")]
    next: Option<String>,
}

async fn login_page(Query(params): Query<Params>) -> AppResult<impl IntoResponse> {
    let message = params.message;
    let next = params.next;

    let post_url = &match next {
        Some(next) => format!("/auth/login/submit?next={next}"),
        None => "/auth/login/submit".to_owned(),
    };

    let login_page = LoginPage {
        title: "Login",
        post_url,
        sub_text: None,
        message,
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
