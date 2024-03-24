mod explore;
mod homepage;
mod library;
mod login;
mod streaming;

pub use explore::explore;
pub use homepage::{homepage, HXTarget};
pub use library::library;
pub use login::login;
pub use streaming::{streaming, StreamingSessions};

use crate::state::AppState;
use axum::{
    http::{HeaderName, HeaderValue},
    Router,
};
use tower::ServiceBuilder;
use tower_http::{services::ServeDir, set_header::SetResponseHeaderLayer};

pub fn dynamic_content() -> Router<AppState> {
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

    let icons = ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("image/svg+xml; charset=UTF-8"),
        ))
        .service(ServeDir::new("frontend/icons"));

    Router::new()
        .nest_service("/styles", styles)
        .nest_service("/scripts", scripts)
        .nest_service("/icons", icons)
}
