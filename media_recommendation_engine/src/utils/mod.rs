mod errorext;
use axum::{response::IntoResponse, routing::get, Router};
pub use errorext::{HandleErr, Ignore};

mod parsing;
pub use parsing::{ParseBetween, ParseUntil};

mod tracing;
pub use tracing::{init_tracing, tracing_layer};

use crate::{routes::HXTarget, state::AppState};

macro_rules! relative {
    ($path:expr) => {
        if cfg!(windows) {
            concat!(env!("CARGO_MANIFEST_DIR"), "\\", $path)
        } else {
            concat!(env!("CARGO_MANIFEST_DIR"), "/", $path)
        }
    };
}

pub fn frontend_redirect(route: &str, target: HXTarget) -> String {
    frontend_redirect_explicit(
        route,
        &target,
        &format!(r#"/?{target}={route}"#, target = target.as_str()),
    )
}

pub fn frontend_redirect_explicit(route: &str, target: &HXTarget, push_url: &str) -> String {
    format!(
        r#"hx-get="{route}" hx-target={target} hx-push-url="{push_url}""#,
        target = target.as_target()
    )
}

pub fn htmx() -> Router<AppState> {
    // TODO: LICENSE for Htmx?
    // Doesn't need to be a ServeFile because it rarely changes
    let htmx = std::fs::read_to_string(relative!("../frontend/scripts/htmx.js"))
        .expect("failed to read htmx into memory");

    let htmx_ws = std::fs::read_to_string(relative!("../frontend/scripts/ws.js"))
        .expect("failed to read ws.js into memory");

    Router::new()
        .route(
            "/htmx",
            get(|| async {
                (
                    [("content-type", "application/javascript; charset=UTF-8")],
                    htmx,
                )
                    .into_response()
            }),
        )
        .route(
            "/htmx_ws",
            get(|| async {
                (
                    [("content-type", "application/javascript; charset=UTF-8")],
                    htmx_ws,
                )
                    .into_response()
            }),
        )
}
