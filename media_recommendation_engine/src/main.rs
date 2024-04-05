#![feature(pattern)]
#![feature(never_type)]

extern crate ffmpeg_next as ffmpeg;

use axum::{middleware, response::Redirect, routing::get, Router};

use axum_login::{
    tower_sessions::{session_store::ExpiredDeletion, Expiry, SessionManagerLayer},
    AuthManagerLayerBuilder,
};
use state::Cancellation;
use time::Duration;
use tokio::{net::TcpListener, signal, task::JoinHandle};

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
        warn!("provided invalid arguments: \"{args:?}\"")
    }

    let db = Database::new()
        .await
        .expect("failed to connect to database");

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

    let (state, cancel, settings) = AppState::new(db.clone()).await;

    let app = Router::new()
        .merge(tracing_layer())
        .route("/", get(routes::homepage))
        .merge(routes::library())
        .route("/explore", get(routes::explore))
        .route("/settings", get(|| async move { "" }))
        .nest("/video", routes::streaming())
        .layer(middleware::from_fn(login_required))
        .merge(htmx())
        .merge(dynamic_content())
        .nest("/auth", routes::login())
        .route("/error", get(routes::error))
        .fallback(Redirect::permanent("/error?err=404"))
        .with_state(state)
        .layer(auth);

    let port = settings.port().await;
    let ip = format!("0.0.0.0:{port}");

    let listener = TcpListener::bind(&ip)
        .await
        .expect("failed to bind to port");

    info!("Starting server on {ip}");

    let indexing = tokio::spawn(periodic_indexing(db, settings));

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown(indexing, cancel))
        .await
        .expect("failed to start server");

    info!("Suceessfully shut down");
}

async fn shutdown(indexing: JoinHandle<!>, cancel: Cancellation) {
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

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("Starting to shut down...");

    indexing.abort();
    cancel.cancel();
}
