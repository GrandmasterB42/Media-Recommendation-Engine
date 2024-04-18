use askama_axum::IntoResponse;
use axum::{routing::get, Router};

use crate::state::AppState;

use super::relative;

// This is used for replacing stuff on the index page
#[derive(Clone, Copy)]
pub enum HXTarget {
    All,
    Content,
}

impl HXTarget {
    pub const fn as_str(self) -> &'static str {
        match self {
            HXTarget::All => "all",
            HXTarget::Content => "content",
        }
    }

    pub const fn as_target(self) -> &'static str {
        match self {
            HXTarget::All => "#all",
            HXTarget::Content => "#content",
        }
    }
}

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
    // This guarantees that these files exists and also probably has less overhead than something like ServeDir
    let htmx = std::fs::read_to_string(relative!("../frontend/scripts/htmx.js"))
        .expect("failed to read htmx into memory");

    let htmx_ws = std::fs::read_to_string(relative!("../frontend/scripts/ws.js"))
        .expect("failed to read the htmx websocket extension into memory");

    let htmx_sse = std::fs::read_to_string(relative!("../frontend/scripts/sse.js"))
        .expect("failed to read the htmx server sent events extensions into memory");

    const JSHEADER: [(&str, &str); 1] = [("content-type", "application/javascript; charset=UTF-8")];

    Router::new()
        .route("/htmx", get(|| async { (JSHEADER, htmx).into_response() }))
        .route(
            "/htmx_ws",
            get(|| async { (JSHEADER, htmx_ws).into_response() }),
        )
        .route(
            "/htmx_sse",
            get(|| async { (JSHEADER, htmx_sse).into_response() }),
        )
}
