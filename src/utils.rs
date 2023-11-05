use std::fs::read_to_string;

use axum::{extract::Path, http, response::IntoResponse, routing::get, Router};

use tracing::{error, warn, Level};
use tracing_subscriber::{
    filter::LevelFilter,
    fmt::{self, time::OffsetTime},
    layer::SubscriberExt,
    util::SubscriberInitExt,
    Layer,
};

// TODO: make utils a module, there is too much different stuff in here

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

pub fn htmx() -> Router {
    // TODO: LICENSE for Htmx?
    // Doesn't need to be a ServeFile because it rarely changes
    let htmx =
        read_to_string(relative!("frontend/htmx.js")).expect("failed to read htmx into memory");

    // TODO: Document difference between redirect and location so you don't have to look into the docs
    Router::new()
        .route("/htmx", get(|| async { htmx }))
        .route(
            "/redirect/*re",
            get(|Path(re): Path<String>| async move { HXRedirect(format!("/{re}")) }),
        )
        .route(
            "/location/*loc",
            get(|Path(loc): Path<String>| async move { HXLocation(format!("/{loc}")) }),
        )
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

pub fn init_tracing() {
    let (levelfilter, level) = {
        #[cfg(debug_assertions)]
        {
            (LevelFilter::DEBUG, Level::DEBUG)
        }
        #[cfg(not(debug_assertions))]
        {
            (LevelFilter::INFO, Level::INFO)
        }
    };

    let filter = tracing_subscriber::filter::Targets::new()
        .with_target("media_recommendation_engine", level);

    let format = time::format_description::parse(
        "[year]-[month padding:zero]-[day padding:zero] [hour]:[minute]:[second]",
    )
    .unwrap();
    let offset = time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC);

    let custom_layer = fmt::layer()
        .with_target(false)
        .with_timer(OffsetTime::new(offset, format))
        .with_filter(levelfilter)
        .with_filter(filter);

    tracing_subscriber::registry()
        .with(
            // TODO: Look into own formatter -> I want pretty colors and noone can stop me
            custom_layer,
        )
        .init();
}

pub trait HandleErr
where
    Self: Sized,
{
    type OkValue;

    fn ignore(self) {}

    fn log_err(self) -> Option<Self::OkValue>;

    fn log_err_with_msg(self, msg: &str) -> Option<Self::OkValue>;

    fn log_warn(self) -> Option<Self::OkValue>;

    fn log_warn_with_msg(self, msg: &str) -> Option<Self::OkValue>;
}

impl<T, E> HandleErr for Result<T, E>
where
    E: std::fmt::Debug,
{
    type OkValue = T;

    fn log_err(self) -> Option<Self::OkValue> {
        match self {
            Ok(v) => Some(v),
            Err(e) => {
                error!("{e:?}");
                None
            }
        }
    }

    fn log_err_with_msg(self, msg: &str) -> Option<Self::OkValue> {
        match self {
            Ok(v) => Some(v),
            Err(e) => {
                error!("{msg}: {e:?}");
                None
            }
        }
    }

    fn log_warn(self) -> Option<Self::OkValue> {
        match self {
            Ok(v) => Some(v),
            Err(e) => {
                warn!("{e:?}");
                None
            }
        }
    }

    fn log_warn_with_msg(self, msg: &str) -> Option<Self::OkValue> {
        match self {
            Ok(v) => Some(v),
            Err(e) => {
                warn!("{msg}: {e:?}");
                None
            }
        }
    }
}
