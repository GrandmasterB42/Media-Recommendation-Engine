#![feature(pattern)]

use axum::{
    extract::State,
    http::{HeaderName, HeaderValue},
    response::{Html, Redirect},
    routing::get,
    Router,
};

use macros::template;
use tokio::net::TcpListener;
use tower::ServiceBuilder;
use tower_http::{services::ServeDir, set_header::SetResponseHeaderLayer};

use tracing::info;

use crate::{
    database::Database,
    indexing::periodic_indexing,
    state::AppState,
    templating::TemplatingEngine,
    utils::{htmx, init_tracing, tracing_layer, Ignore},
};

#[macro_use]
mod utils;
mod database;
mod indexing;
mod routes;
mod state;
mod templating;

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
        .merge(tracing_layer())
        .merge(htmx())
        .merge(dynamic_content())
        .fallback(Redirect::permanent("/?err=404"))
        .route("/", get(routes::homepage))
        .merge(routes::library())
        .route(
            "/explore",
            get(|templating: State<TemplatingEngine>| async move {
                template!(
                    settings,
                    templating,
                    "../frontend/content/settings.html",
                    _T
                );
                Html(settings.render())
            }),
        )
        // TODO: The Menu bar up top isn't great, settings and logout should probably be in a dropdown to the right and clicking on library again should bring yopu back to the start of the library
        .route("/settings", get(|| async move { "" }))
        .nest("/video", routes::streaming())
        .with_state(AppState::new(db.clone()));

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

fn dynamic_content() -> Router<AppState> {
    let styles = ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("text/css; charset=UTF-8"),
        ))
        .service(ServeDir::new("frontend/styles"));

    let scripts = ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("application/javascript; charset=UTF-8"),
        ))
        .service(ServeDir::new("frontend/scripts"));

    Router::new()
        .nest_service("/styles", styles)
        .nest_service("/scripts", scripts)
}
