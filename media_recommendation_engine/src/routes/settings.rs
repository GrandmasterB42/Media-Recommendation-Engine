use askama_axum::IntoResponse;
use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, patch, post},
    Form, Router,
};
use serde::Deserialize;

use crate::{
    database::{Database, QueryRowGetConnExt},
    state::{AppResult, AppState, Shutdown},
    utils::{
        frontend_redirect,
        templates::{Setting, Settings, SwapIn},
        AuthExt, AuthSession, HXTarget, ServerSettings,
    },
};

pub fn settings() -> Router<AppState> {
    Router::new()
        .route("/", get(settings_page))
        .route("/shutdown", post(shutdown))
        .route("/restart", post(restart))
        .route("/username", patch(username))
        .route("/password", patch(password))
}

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

    let name = auth.user.unwrap().username; // This route has logged in as a wrapper

    Ok(Settings {
        admin_settings,
        account_settings: vec![text_setting, button_setting],
        redirect_back: frontend_redirect("/", HXTarget::All),
        name,
    })
}

// Turning these two function below into one with a const generic didn't seem to work properly. But this does, so I don't care
async fn shutdown(
    auth: AuthSession,
    State(shutdown): State<Shutdown>,
) -> AppResult<impl IntoResponse> {
    if auth.has_perm("owner").await? {
        shutdown.shutdown();
        Ok(StatusCode::ACCEPTED)
    } else {
        Ok(StatusCode::UNAUTHORIZED)
    }
}

async fn restart(
    auth: AuthSession,
    State(shutdown): State<Shutdown>,
) -> AppResult<impl IntoResponse> {
    if auth.has_perm("owner").await? {
        shutdown.restart();
        Ok(StatusCode::ACCEPTED)
    } else {
        Ok(StatusCode::UNAUTHORIZED)
    }
}

#[derive(Deserialize)]
struct ChangeUsername {
    name: String,
}

async fn username(
    auth: AuthSession,
    State(db): State<Database>,
    State(settings): State<ServerSettings>,
    new_name: Form<ChangeUsername>,
) -> Result<impl IntoResponse, impl IntoResponse> {
    let Some(user) = auth.user else {
        return Err(StatusCode::UNAUTHORIZED.into_response());
    };

    let conn = db
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;

    let new_name = &new_name.name;

    let user_exists = conn
        .query_row_get::<bool>(
            "SELECT exists(SELECT 1 FROM users WHERE username = ?1)",
            [new_name],
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;

    if user_exists {
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            SwapIn {
                swap_id: "error",
                content: "That Username is not available!",
            },
        )
            .into_response());
    }

    if settings.admin().username == user.username {
        settings.update_admin_username(new_name);
    } else {
        conn.execute(
            "UPDATE users SET username = ?1 WHERE username = ?2",
            [new_name, &user.username],
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;
    }

    Ok(new_name.clone().into_response())
}

#[derive(Deserialize)]
struct ChangePassword {
    password: String,
}

async fn password(
    auth: AuthSession,
    State(db): State<Database>,
    State(settings): State<ServerSettings>,
    new_password: Form<ChangePassword>,
) -> Result<impl IntoResponse, impl IntoResponse> {
    let Some(user) = auth.user else {
        return Err(StatusCode::UNAUTHORIZED.into_response());
    };

    let conn = db
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;

    let new_password = new_password.password.clone();

    if settings.admin().username == user.username {
        settings.update_admin_password(&new_password)
    } else {
        let new_pw =
            tokio::task::spawn_blocking(move || password_auth::generate_hash(new_password))
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;
        conn.execute(
            "UPDATE users SET password = ?1 WHERE username = ?2",
            [new_pw, user.username],
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;
    }

    Ok(StatusCode::OK.into_response())
}
