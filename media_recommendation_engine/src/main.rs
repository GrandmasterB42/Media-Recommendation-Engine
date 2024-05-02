#![feature(pattern)]

extern crate ffmpeg_next as ffmpeg;

use std::collections::HashSet;

use axum::{middleware, response::Redirect, routing::get, Router};

use axum_login::{
    tower_sessions::{session_store::ExpiredDeletion, Expiry, SessionManagerLayer},
    AuthManagerLayerBuilder,
};
use clap::{Parser, ValueEnum};
use futures_util::FutureExt;
use state::{AppError, AppResult, Shutdown};
use time::Duration;
use tokio::{net::TcpListener, signal};

use tower_sessions::cookie::Key;
use tracing::{error, info};

use crate::{
    database::Database,
    indexing::periodic_indexing,
    routes::dynamic_content,
    state::AppState,
    utils::{htmx, init_tracing, login_required, tracing_layer, HandleErr},
};

#[macro_use]
mod utils;
mod database;
mod indexing;
mod recommendation;
mod routes;
mod state;

#[tokio::main]
async fn main() {
    init_tracing();
    ffmpeg::init().expect("failed to initialize ffmpeg");

    let mut args = Args::parse();

    if let Err(err) = handle_data_delete(args.delete_data).await {
        error!("{err}");
        return;
    }

    loop {
        let should_restart = server(std::mem::take(&mut args.port)).await;
        if !should_restart {
            break;
        }
        info!("Restarting...");
    }

    info!("Suceessfully shut down");
}

async fn server(port: Option<u16>) -> bool {
    let db = Database::new().expect("failed to connect to database");

    let session_store = db.clone();

    tokio::task::spawn(
        session_store
            .clone()
            .continuously_delete_expired(tokio::time::Duration::from_secs(60)),
    );

    let session_layer = SessionManagerLayer::new(session_store.clone())
        .with_secure(false)
        .with_expiry(Expiry::OnInactivity(Duration::days(1)))
        .with_signed(Key::generate());

    let auth = AuthManagerLayerBuilder::new(session_store, session_layer).build();

    let (state, shutdown, settings, indexing_trigger, restart) =
        AppState::new(db.clone(), port).await;

    let app = Router::new()
        .merge(tracing_layer())
        .route("/", get(routes::homepage))
        .merge(routes::library())
        .route("/explore", get(routes::explore))
        .nest("/settings", routes::settings())
        .nest("/video", routes::streaming())
        .layer(middleware::from_fn(login_required))
        .merge(htmx())
        .merge(dynamic_content())
        .nest("/auth", routes::login())
        .route("/error", get(routes::error))
        .fallback(Redirect::permanent("/error?err=404"))
        .with_state(state)
        .layer(auth);

    if let Some(port) = port {
        settings.set_port(port);
    }

    let port = settings.port();
    let ip = format!("0.0.0.0:{port}");

    let listener = TcpListener::bind(&ip)
        .await
        .expect("failed to bind to port");

    info!("Starting server on {ip}");

    tokio::spawn(periodic_indexing(
        db,
        settings,
        indexing_trigger,
        shutdown.clone(),
    ));

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(shutdown))
        .await
        .expect("failed to start server");

    restart.now_or_never().unwrap_or(Ok(false)).unwrap_or(false)
}

async fn shutdown_signal(shutdown: Shutdown) {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    let should_cancel = tokio::select! {
        _ = shutdown.cancelled() => false,
        _ = ctrl_c => true,
        _ = terminate => true,
    };

    info!("Starting to shut down...");

    if should_cancel {
        shutdown.shutdown();
    }
}

async fn handle_data_delete(delete_data: Option<Vec<DeleteKind>>) -> AppResult<()> {
    let Some(delete_data) = delete_data else {
        return Ok(());
    };
    let conn = rusqlite::Connection::open("database/database.sqlite")?;

    let delete_data = delete_data.into_iter().collect::<HashSet<DeleteKind>>();

    let delete_sql = delete_data.iter().filter_map(|&kind| match kind {
        DeleteKind::All => {
            std::fs::remove_file("database/database.sqlite")
                .log_warn_with_msg("failed to delete database");
            std::fs::remove_file("database/database.sqlite-journal")
                .log_warn_with_msg("failed to delete .sqlite-journal file");
            std::fs::remove_file("database/database.sqlite-wal")
                .log_warn_with_msg("failed to delete .sqlite-wal file");
            std::fs::remove_file("database/database.sqlite-shm")
                .log_warn_with_msg("failed to delete .sqlite-shm file");
            None
        }
        DeleteKind::Indexing => Some("indexing.sql"),
        DeleteKind::StorageLocations => Some("storage_locations.sql"),
        DeleteKind::Users => Some("users.sql"),
        DeleteKind::Sessions => Some("sessions.sql"),
    });

    for sql in delete_sql {
        let sql_file = tokio::fs::read_to_string(format!("database/sql/deletion/{sql}"))
            .await
            .map_err(|_| AppError::Custom(format!("Failed to open \"{sql}\"")))?;
        conn.execute_batch(&sql_file)?;
    }

    info!("Successfully deleted requesteddata");

    Ok(())
}

#[derive(Parser, Debug)]
#[command(name = "Media Recommendation Engine")]
#[command(version = "0.0.1")]
#[command(about = "Media Recommendation Engine", long_about = None, )]
struct Args {
    /// Set the port on first startup. Defaults to 3000
    #[arg(short, long)]
    port: Option<u16>,
    /// Delete the specified data on startup
    #[arg(
        value_enum,
        short,
        long,
        value_delimiter = ' ',
        num_args = 1..,
    )]
    delete_data: Option<Vec<DeleteKind>>,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, ValueEnum)]
enum DeleteKind {
    /// Deletes all data stored in the database
    All,
    /// Deletes data created from indexing files
    Indexing,
    /// Deletes known storage locations
    StorageLocations,
    /// Deletes all user data (not permissions and groups)
    Users,
    /// Deletes login and viewing sessions
    Sessions,
}
