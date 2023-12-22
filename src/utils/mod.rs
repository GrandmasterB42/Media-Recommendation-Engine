mod errorext;
use axum::{http, response::IntoResponse, routing::get, Router};
pub use errorext::{HandleErr, Ignore};

mod parsing;
pub use parsing::{ParseBetween, ParseUntil};

mod tracing;
pub use tracing::init_tracing;

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

pub fn htmx() -> Router {
    // TODO: LICENSE for Htmx?
    // Doesn't need to be a ServeFile because it rarely changes
    let htmx = std::fs::read_to_string(relative!("frontend/htmx.js"))
        .expect("failed to read htmx into memory");

    Router::new().route("/htmx", get(|| async { htmx }))
}
