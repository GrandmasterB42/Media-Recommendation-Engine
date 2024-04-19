#![feature(pattern)]

extern crate ffmpeg_next as ffmpeg;

use axum::{middleware, response::Redirect, routing::get, Router};

use axum_login::{
    tower_sessions::{session_store::ExpiredDeletion, Expiry, SessionManagerLayer},
    AuthManagerLayerBuilder,
};
use futures_util::FutureExt;
use state::Shutdown;
use time::Duration;
use tokio::{net::TcpListener, signal};

use tower_sessions::cookie::Key;
use tracing::{info, warn};

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

    let args = std::env::args().collect::<Vec<_>>();
    if args.get(1).is_some_and(|a| a == "delete_db") {
        std::fs::remove_file("database/database.sqlite")
            .log_warn_with_msg("failed to delete database");
        std::fs::remove_file("database/database.sqlite-journal")
            .log_warn_with_msg("failed to delete journal");
        std::fs::remove_file("database/database.sqlite-wal")
            .log_warn_with_msg("failed to delete wal");
        std::fs::remove_file("database/database.sqlite-shm")
            .log_warn_with_msg("failed to delete shm");
    } else if args.len() > 1 {
        let args = &args[1..];
        warn!("provided invalid arguments: \"{args:?}\"");
    }
    drop(args);

    loop {
        let should_restart = server().await;
        if !should_restart {
            break;
        }
        info!("Restarting...");
    }

    info!("Suceessfully shut down");
}

async fn server() -> bool {
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

    let (state, shutdown, settings, restart) = AppState::new(db.clone()).await;

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

    let port = settings.port();
    let ip = format!("0.0.0.0:{port}");

    let listener = TcpListener::bind(&ip)
        .await
        .expect("failed to bind to port");

    info!("Starting server on {ip}");

    tokio::spawn(periodic_indexing(db, settings, shutdown.clone()));

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
