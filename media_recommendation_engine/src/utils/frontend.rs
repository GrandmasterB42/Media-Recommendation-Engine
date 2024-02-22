use askama_axum::IntoResponse;
use axum::{routing::get, Router};

use crate::{routes::HXTarget, state::AppState};

use super::relative;

pub fn frontend_redirect(route: &str, target: HXTarget) -> String {
    frontend_redirect_explicit(
        route,
        target,
        Some(&format!(r#"/?{target}={route}"#, target = target.as_str())),
    )
}

pub fn frontend_redirect_explicit(route: &str, target: HXTarget, push_url: Option<&str>) -> String {
    match push_url {
        Some(push_url) => format!(
            r#"hx-get="{route}" hx-target={target} hx-push-url="{push_url}""#,
            route = route,
            target = target.as_target(),
            push_url = push_url
        ),
        None => format!(
            r#"hx-get="{route}" hx-target={target}"#,
            route = route,
            target = target.as_target()
        ),
    }
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
