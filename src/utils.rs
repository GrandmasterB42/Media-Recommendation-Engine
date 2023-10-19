use axum::{http, response::IntoResponse};
use tracing_subscriber::{prelude::__tracing_subscriber_SubscriberExt, util::SubscriberInitExt};

/// Makes the given path be relative to the crate
macro_rules! relative {
    ($path:expr) => {
        if cfg!(windows) {
            concat!(env!("CARGO_MANIFEST_DIR"), "\\", $path)
        } else {
            concat!(env!("CARGO_MANIFEST_DIR"), "/", $path)
        }
    };
}

/// A CSS response.
///
/// Will automatically set `Content-Type: text/css; charset=utf-8`.
#[derive(Clone, Copy, Debug)]
pub struct Css<T>(pub T);

impl<T> IntoResponse for Css<T>
where
    T: IntoResponse,
{
    fn into_response(self) -> axum::response::Response {
        (
            [(http::header::CONTENT_TYPE, "text/css; charset=utf-8")],
            self.0,
        )
            .into_response()
    }
}

#[derive(Clone, Debug)]
pub struct HXRedirect(pub String);

impl IntoResponse for HXRedirect {
    fn into_response(self) -> axum::response::Response {
        ([("HX-Redirect", self.0)], ()).into_response()
    }
}

#[derive(Clone, Debug)]
pub struct HXLocation(pub String);

impl IntoResponse for HXLocation {
    fn into_response(self) -> axum::response::Response {
        ([("HX-Location", self.0)], ()).into_response()
    }
}

pub fn init_logging() {
    let filter = tracing_subscriber::filter::Targets::new()
        .with_target("tower_http::trace::on_response", tracing::Level::DEBUG)
        .with_target("tower_http::trace::on_request", tracing::Level::DEBUG)
        .with_target("tower_http::trace::make_span", tracing::Level::DEBUG)
        .with_default(tracing::Level::INFO);

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(filter)
        .init();
}
