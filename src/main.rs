use std::fs::read_to_string;

use axum::{
    extract::Path,
    http::StatusCode,
    response::{Html, Redirect},
    routing::get,
    Extension, Router,
};

use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};

use tracing::info;
use utils::{init_logging, HXRedirect};

use crate::{database::Database, utils::HXLocation};

#[macro_use]
mod utils;
mod database;

async fn db_test(Extension(database): Extension<Database>) -> Result<(), StatusCode> {
    database
        .connection()
        .map_err(|_e| StatusCode::INTERNAL_SERVER_ERROR)?
        .execute(
            "CREATE TABLE test (id Integer PRIMARY KEY AUTOINCREMENT)",
            (),
        )
        .map_err(|_e| StatusCode::IM_A_TEAPOT)?;
    Ok(())
}

#[tokio::main]
async fn main() {
    init_logging();
    let app = Router::new()
        .route("/test", get(db_test))
        .route("/", get(|| async { Redirect::permanent("/browse") }))
        .route(
            "/explore",
            get(|| async { Html("<div> Nothing here yet, come back in some newer version</div>") }),
        )
        .route(
            "/library",
            get(|| async { Html("<div> Working on this right now!</div>") }),
        )
        .nest_service("/styles", ServeDir::new("frontend/styles"))
        .nest_service("/browse", ServeFile::new("frontend/content/index.html"))
        .nest_service(
            "/settings",
            ServeFile::new("frontend/content/settings.html"),
        )
        .merge(htmx())
        .fallback_service(ServeFile::new("frontend/content/err404.html"))
        // TODO: State instead of Extension?
        .layer(Database::new().expect("failed to connect to database"))
        .layer(TraceLayer::new_for_http());

    let ip = "0.0.0.0:3000";
    info!("Starting server on {}", ip);
    let server = axum::Server::bind(&ip.parse().unwrap()).serve(app.into_make_service());

    /*
    wanted to use .with_graceful_shutdown(),
    but when using:

    async {
            tokio::signal::ctrl_c()
                .await
                .expect("failed to start listening for ctrl+c")
    }

    inside, it doesn't shut down close to immediately,
    but waits until all connections are closed ot something like that?

    This at least gets rid of the error message
    */

    tokio::select! {
        _ = server => {},
        _ = tokio::signal::ctrl_c() => {},
    }
    info!("Suceessfully shut down");
}

fn htmx() -> Router {
    // TODO: LICENSE for Htmx
    // Doesn't need to be a ServeFile because it rarely changes
    let htmx =
        read_to_string(relative!("frontend/htmx.js")).expect("failed to read htmx into memory");

    Router::new()
        .route("/htmx", get(|| async { htmx }))
        .route(
            "/redirect/:re",
            get(|Path(re): Path<String>| async move { HXRedirect(format!("/{re}")) }),
        )
        .route(
            "/location/:loc",
            get(|Path(loc): Path<String>| async move { HXLocation(format!("/{loc}")) }),
        )
}
