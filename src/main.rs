#![feature(pattern)]

use std::{collections::HashMap, sync::Arc};

use axum::{
    response::{Html, Redirect},
    routing::get,
    Extension, Router,
};

use tokio::{net::TcpListener, sync::Mutex};
use tower_http::services::ServeDir;

use tracing::info;

use crate::{
    database::Database,
    indexing::periodic_indexing,
    routes::{streaming, StreamingSessions},
    utils::{htmx, init_tracing, tracing_layer, Ignore},
};

#[macro_use]
mod utils;
mod database;
mod indexing;
mod routes;

#[tokio::main]
async fn main() {
    init_tracing();

    let args = std::env::args().collect::<Vec<_>>();
    if args.get(1).is_some_and(|a| a == "delete_db") {
        std::fs::remove_file("database/database.sqlite").ignore();
        std::fs::remove_file("database/database.sqlite-journal").ignore();
    }

    let db = Database::new().expect("failed to connect to database");

    let sessions = StreamingSessions {
        sessions: Arc::new(Mutex::new(HashMap::new())),
    };

    let app = Router::new()
        .route("/", get(routes::homepage))
        .route(
            "/explore",
            get(|| async { Html("<div> Nothing here yet, come back in some newer version</div>") }),
        )
        .merge(routes::library())
        .nest_service("/styles", ServeDir::new("frontend/styles"))
        .nest_service("/scripts", ServeDir::new("frontend/scripts"))
        // TODO: The Menu bar up top isn't great, settings and logout should probably be in a dropdown to the right and clicking on library again should bring yopu back to the start of the library
        .route("/settings", get(|| async move { "" }))
        .fallback(Redirect::permanent(r#"/?err=404"#))
        .merge(htmx())
        .merge(streaming())
        .merge(tracing_layer())
        // TODO: State instead of Extension?
        .layer(db.clone())
        .layer(Extension(sessions));

    let ip = "0.0.0.0:3000";

    let listener = TcpListener::bind(ip).await.expect("failed to bind to port");

    info!("Starting server on {ip}");

    async fn server(listener: TcpListener, app: Router) {
        axum::serve(listener, app)
            .await
            .expect("failed to start server");
    }
    let server = server(listener, app);

    tokio::spawn(periodic_indexing(db.0));

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
