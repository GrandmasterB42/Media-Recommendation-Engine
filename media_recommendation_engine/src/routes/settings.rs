use askama_axum::IntoResponse;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, patch, post},
    Form, Router,
};

use rusqlite::params;
use serde::Deserialize;

use crate::{
    database::{Database, QueryRowGetConnExt, QueryRowIntoStmtExt},
    state::{AppError, AppResult, AppState, IndexingTrigger, Shutdown},
    utils::{
        frontend_redirect,
        templates::{
            AsDisplay, Creation, CreationInput, LocationEntry, Setting, Settings, SwapIn, UserEntry,
        },
        AuthExt, AuthSession, HXTarget, HandleErr, ServerSettings,
    },
};

pub fn settings() -> Router<AppState> {
    Router::new()
        .route("/", get(settings_page))
        .route("/shutdown", post(shutdown))
        .route("/restart", post(restart))
        .route("/username", patch(username))
        .route("/password", patch(password))
        .route("/user", post(add_user))
        .route("/user/:id", delete(remove_user))
        .route("/location", post(add_location))
        .route("/location/:id", delete(remove_location))
}

async fn settings_page(
    auth: AuthSession,
    State(db): State<Database>,
) -> AppResult<impl IntoResponse> {
    let admin_settings = if auth.has_perm("owner").await? {
        Some(vec![location_addition(&db)?, user_creation(&db)?])
    } else {
        None
    };

    let name = auth.user.unwrap().username; // This route has logged in as a wrapper

    Ok(Settings {
        admin_settings,
        account_settings: vec![],
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
) -> AppResult<impl IntoResponse> {
    let Some(user) = auth.user else {
        return Err(AppError::Status(StatusCode::UNAUTHORIZED));
    };

    let conn = db.get()?;

    let new_name = &new_name.name;

    let user_exists = conn.query_row_get::<bool>(
        "SELECT exists(SELECT 1 FROM users WHERE username = ?1)",
        [new_name],
    )?;

    if user_exists {
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            SwapIn {
                swap_id: "user_error",
                swap_method: None,
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
        )?;
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
) -> AppResult<impl IntoResponse> {
    let Some(user) = auth.user else {
        return Err(AppError::Status(StatusCode::UNAUTHORIZED));
    };

    let conn = db.get()?;

    let new_password = new_password.password.clone();

    if settings.admin().username == user.username {
        settings.update_admin_password(&new_password)
    } else {
        let new_pw =
            tokio::task::spawn_blocking(move || password_auth::generate_hash(new_password))
                .await
                .map_err(|_| AppError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
        conn.execute(
            "UPDATE users SET password = ?1 WHERE username = ?2",
            [new_pw, user.username],
        )?;
    }

    Ok(StatusCode::OK)
}

#[derive(Deserialize)]
struct NewUser {
    username: String,
    password: String,
}

async fn add_user(
    auth: AuthSession,
    State(db): State<Database>,
    Form(new_user): Form<NewUser>,
) -> AppResult<impl IntoResponse> {
    if !auth.has_perm("owner").await.unwrap_or_default() {
        return Err(AppError::Status(StatusCode::UNAUTHORIZED));
    }

    let conn = db.get()?;

    let user_exists = conn.query_row_get::<bool>(
        "SELECT exists(SELECT 1 FROM users WHERE username = ?1)",
        [&new_user.username],
    )?;

    if user_exists {
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            SwapIn {
                swap_id: "user_error",
                swap_method: None,
                content: "That Username is not available!",
            },
        )
            .into_response());
    }

    let password = tokio::task::spawn_blocking(|| password_auth::generate_hash(new_user.password))
        .await
        .log_err_with_msg("Failed to generate password hash")
        .unwrap_or_default();

    let id = conn.query_row_get::<u64>(
        "INSERT INTO users (username, password) VALUES (?1, ?2) RETURNING id",
        params![new_user.username, password],
    )?;

    Ok(SwapIn {
        swap_id: "user_list",
        swap_method: Some("beforeend"),
        content: UserEntry {
            user_id: id,
            name: new_user.username,
            can_delete: true,
        },
    }
    .into_response())
}

async fn remove_user(
    auth: AuthSession,
    State(db): State<Database>,
    Path(user_id): Path<u64>,
) -> AppResult<impl IntoResponse> {
    if !auth.has_perm("owner").await? {
        return Err(AppError::Custom(
            "User doesn't have the permissions to delete a user".to_owned(),
        ));
    }
    let conn = db.get()?;

    let owner_perm_id =
        conn.query_row_get::<u64>("SELECT id FROM permissions WHERE name = ?1", ["owner"])?;
    let is_admin = conn.query_row_get::<bool>(
        "SELECT exists(SELECT 1 FROM user_permissions WHERE userid = ?1 AND permissionid = ?2)",
        params![user_id, owner_perm_id],
    )?;

    if is_admin {
        return Err(AppError::Custom("This user can't be deleted".to_owned()));
    }

    conn.execute("DELETE FROM users WHERE id = ?1", [user_id])?;
    conn.execute("DELETE FROM user_permissions WHERE userid = ?1", [user_id])?;
    conn.execute("DELETE FROM user_groups WHERE userid = ?1", [user_id])?;

    Ok(())
}

fn user_creation(db: &Database) -> AppResult<Setting> {
    let conn = db.get()?;

    let owner_perm_id =
        conn.query_row_get::<u64>("SELECT id FROM permissions WHERE name = ?1", ["owner"])?;

    let users = conn.prepare("SELECT id, username FROM users")?
        .query_map_into::<(u64, String)>([])?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|(id, name)| {
            let is_admin = conn.query_row_get::<bool>(
                "SELECT exists(SELECT 1 FROM user_permissions WHERE userid = ?1 AND permissionid = ?2)",
                params![id, owner_perm_id],
                ).unwrap_or_default();

            UserEntry { user_id: id, name, can_delete: !is_admin }.to_box()
        })
        .collect::<Vec<_>>();

    Ok(Setting::CreationMenu {
        creation: Creation {
            title: "Users",
            list_id: "user_list",
            error_id: "user_error",
            post_addr: "/settings/user",
            entries: users,
            inputs: vec![
                CreationInput {
                    typ: "text",
                    name: "username",
                    placeholder: "Username",
                },
                CreationInput {
                    typ: "password",
                    name: "password",
                    placeholder: "Password",
                },
            ],
        },
    })
}

fn location_addition(db: &Database) -> AppResult<Setting> {
    let conn = db.get()?;

    let locations = conn
        .prepare("SELECT id, path FROM storage_locations")?
        .query_map_into::<(u64, String)>([])?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|(id, path)| {
            LocationEntry {
                location_id: id,
                path,
            }
            .to_box()
        })
        .collect::<Vec<_>>();

    Ok(Setting::CreationMenu {
        creation: Creation {
            title: "Storage Locations",
            list_id: "location_list",
            error_id: "location_error",
            post_addr: "/settings/location",
            entries: locations,
            inputs: vec![CreationInput {
                typ: "text",
                name: "path",
                placeholder: "Path",
            }],
        },
    })
}

#[derive(Deserialize)]
struct AddLocation {
    path: String,
}

async fn add_location(
    auth: AuthSession,
    State(db): State<Database>,
    State(trigger): State<IndexingTrigger>,
    Form(location): Form<AddLocation>,
) -> AppResult<impl IntoResponse> {
    if !auth.has_perm("owner").await.unwrap_or_default() {
        return Err(AppError::Status(StatusCode::UNAUTHORIZED));
    }

    let conn = db.get()?;

    if !std::path::Path::new(&location.path).exists() {
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            SwapIn {
                swap_id: "location_error",
                swap_method: None,
                content: format!("Could not access the location: \"{}\"", location.path),
            },
        )
            .into_response());
    }

    let id = conn.query_row_get::<u64>(
        "INSERT INTO storage_locations (path) VALUES (?1) RETURNING id",
        params![location.path],
    )?;

    trigger.notify_one();

    Ok(SwapIn {
        swap_id: "location_list",
        swap_method: Some("beforeend"),
        content: LocationEntry {
            location_id: id,
            path: location.path,
        },
    }
    .into_response())
}

async fn remove_location(
    auth: AuthSession,
    State(db): State<Database>,
    State(trigger): State<IndexingTrigger>,
    Path(id): Path<u64>,
) -> AppResult<impl IntoResponse> {
    if !auth.has_perm("owner").await? {
        return Err(AppError::Status(StatusCode::UNAUTHORIZED));
    }

    let deletion_amount = db
        .get()?
        .execute("DELETE FROM storage_locations WHERE id = ?1", [id])?;

    if deletion_amount == 0 {
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            SwapIn {
                swap_id: "location_error",
                swap_method: None,
                content: "Failed to delete requested storage location".to_owned(),
            },
        )
            .into_response());
    }

    trigger.notify_one();

    Ok(().into_response())
}
