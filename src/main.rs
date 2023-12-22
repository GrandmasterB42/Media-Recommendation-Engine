#![feature(pattern)]
use std::time::Duration;

use axum::{
    extract::MatchedPath,
    http::{Request, Response},
    response::{Html, Redirect},
    routing::get,
    Router,
};

use tower_http::{services::ServeDir, trace::TraceLayer};

use tracing::{debug, debug_span, field, info, Span};

use crate::{
    database::Database,
    indexing::periodic_indexing,
    utils::{htmx, init_tracing, Ignore},
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

    let app = Router::new()
        .route("/", get(routes::homepage))
        .route(
            "/explore",
            get(|| async { Html("<div> Nothing here yet, come back in some newer version</div>") }),
        )
        .merge(routes::library())
        .nest_service("/styles", ServeDir::new("frontend/styles"))
        // TODO: The Menu bar up top isn't great, settings and logout should probably be in a dropdown to the right and clicking on library again should bring yopu back to the start of the library
        .route("/settings", get(|| async move { "" }))
        .merge(htmx())
        .fallback(Redirect::permanent(r#"/?err=404"#))
        // TODO: State instead of Extension?
        .layer(db.clone())
        // TODO: How do I move this out of here?
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|_request: &Request<_>| {
                    debug_span!("request", method = field::Empty, uri = field::Empty)
                })
                .on_request(|req: &Request<_>, span: &Span| {
                    let method = req.method();
                    let uri = req
                        .extensions()
                        .get::<MatchedPath>()
                        .map(MatchedPath::as_str);
                    span.record("method", method.to_string());
                    span.record("uri", uri);
                    debug!("Received Request");
                })
                .on_response(|res: &Response<_>, latency: Duration, _span: &Span| {
                    let status = res.status();
                    debug!("Took {latency:?} to respond with status '{status}'");
                }),
            // TODO: Add other meaningful options here once necessary
        );

    let ip = "0.0.0.0:3000";
    info!("Starting server on {}", ip);
    let server = axum::Server::bind(&ip.parse().unwrap()).serve(app.into_make_service());

    tokio::spawn(periodic_indexing(db.0));

    /*
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
