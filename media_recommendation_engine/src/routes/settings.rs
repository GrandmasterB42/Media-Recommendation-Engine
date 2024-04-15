use std::time::Duration;

use askama_axum::IntoResponse;
use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Router,
};

use crate::{
    state::{AppResult, AppState, Cancellation},
    utils::{
        frontend_redirect,
        templates::{Setting, Settings},
        AuthExt, AuthSession, HXTarget,
    },
};

pub fn settings() -> Router<AppState> {
    Router::new()
        .route("/", get(settings_page))
        .route("/shutdown", post(shutdown))
        .route("/restart", post(restart))
}

async fn shutdown(
    auth: AuthSession,
    State(cancel): State<Cancellation>,
) -> AppResult<impl IntoResponse> {
    if auth.has_perm("owner").await? {
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel.cancel();
        });
        Ok(StatusCode::ACCEPTED)
    } else {
        Ok(StatusCode::UNAUTHORIZED)
    }
}

async fn restart() {}

async fn settings_page(auth: AuthSession) -> AppResult<impl IntoResponse> {
    let text_setting = Setting::TextSetting {
        prompt: "This is a text prompt",
        action: "This is done when ",
    };
    let button_setting = Setting::Button {
        label: "This is a button label",
        class: "cool styling for the button",
        action: "a button action",
    };

    let mut admin_settings = None;
    if auth.has_perm("owner").await? {
        admin_settings = Some(vec![]);
    }

    Ok(Settings {
        admin_settings,
        account_settings: vec![text_setting, button_setting],
        redirect_back: frontend_redirect("/", HXTarget::All),
    })
}
