#![feature(pattern)]

extern crate ffmpeg_next as ffmpeg;

use axum::{response::Redirect, routing::get, Router};

use axum_login::{
    login_required,
    tower_sessions::{session_store::ExpiredDeletion, Expiry, SessionManagerLayer},
    AuthManagerLayerBuilder,
};
use time::Duration;
use tokio::net::TcpListener;

use tower_sessions::cookie::Key;
use tracing::{info, warn};

use crate::{
    database::Database,
    indexing::periodic_indexing,
    routes::dynamic_content,
    state::AppState,
    utils::{htmx, init_tracing, tracing_layer, HandleErr},
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

    let app = Router::new()
        .merge(tracing_layer())
        .route("/", get(routes::homepage))
        .merge(routes::library())
        .route("/explore", get(routes::explore))
        .route("/settings", get(|| async move { "" }))
        .nest("/video", routes::streaming())
        .layer(login_required!(Database, login_url = "/auth/login"))
        .merge(htmx())
        .merge(dynamic_content())
        .nest("/auth", routes::login())
        .route("/error", get(routes::error))
        .fallback(Redirect::permanent("/error?err=404"))
        .with_state(AppState::new(db.clone()))
        .layer(auth);

    let ip = "0.0.0.0:3000";

    let listener = TcpListener::bind(ip).await.expect("failed to bind to port");

    info!("Starting server on {ip}");

    async fn server(listener: TcpListener, app: Router) {
        axum::serve(listener, app)
            .await
            .expect("failed to start server");
    }
    let server = server(listener, app);

    tokio::spawn(periodic_indexing(db));

    /*
    (last tested in axum 0.6)
    TODO: shutting down
    wanted to use .with_graceful_shutdown(),
    but when using:

    async {
            tokio::signal::ctrl_c()
                .await
                .expect("failed to start listening for ctrl+c")
    }

    inside, it doesn't shut down close to immediately,
    but waits until all connections are closed or something like that?

    This at least gets rid of the error message
    */

    tokio::select! {
        _ = server => {},
        _ = tokio::signal::ctrl_c() => {},
    }
    info!("Suceessfully shut down");
}
