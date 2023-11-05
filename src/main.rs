use std::time::Duration;

use axum::{
    extract::MatchedPath,
    http::{Request, Response},
    response::{Html, Redirect},
    routing::get,
    Router,
};

use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};

use tracing::{debug, debug_span, field, info, Span};

use crate::{
    database::Database,
    indexing::periodic_indexing,
    utils::{htmx, init_tracing},
};

#[macro_use]
mod utils;
mod database;
mod indexing;
mod library;

#[tokio::main]
async fn main() {
    init_tracing();

    let db = Database::new().expect("failed to connect to database");

    let app = Router::new()
        .route("/", get(|| async { Redirect::permanent("/browse") }))
        .route(
            "/explore",
            get(|| async { Html("<div> Nothing here yet, come back in some newer version</div>") }),
        )
        .merge(library::library())
        .nest_service("/styles", ServeDir::new("frontend/styles"))
        .nest_service("/browse", ServeFile::new("frontend/content/index.html"))
        .nest_service(
            "/settings",
            ServeFile::new("frontend/content/settings.html"),
        )
        .merge(htmx())
        .fallback_service(ServeFile::new("frontend/content/err404.html"))
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
