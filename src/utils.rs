use std::{
    fs::read_to_string,
    str::{pattern::Pattern, FromStr},
};

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

pub trait Ignore {
    fn ignore(self);
}

impl<T: Sized> Ignore for T {
    fn ignore(self) {}
}

pub trait ParseUntil<D, O, E> {
    /// Parses this type into another type, only using the part before the first occurence of the delimiter
    fn parse_until(&self, delimiter: D) -> Result<O, E>;
}

impl<'a, 'b, D, O, E> ParseUntil<D, O, E> for &'b str
where
    D: Pattern<'a>,
    O: FromStr<Err = E>,
    'b: 'a,
{
    fn parse_until(&self, delimiter: D) -> Result<O, E> {
        self.split_once(delimiter).unwrap_or((self, "")).0.parse()
    }
}

pub trait ParseBetween<D1, D2, O, E> {
    /// Parses this type into another type, only using the part between the first occurence of the first delimiter and the first occurence of the second delimiter after that
    fn parse_between(&self, delimiter1: D1, delimiter2: D2) -> Result<O, E>;
}

impl<'a, 'b, D1, D2, O, E> ParseBetween<D1, D2, O, E> for &'b str
where
    D1: Pattern<'a>,
    D2: Pattern<'a>,
    O: FromStr<Err = E>,
    'b: 'a,
{
    fn parse_between(&self, delimiter1: D1, delimiter2: D2) -> Result<O, E> {
        self.split_once(delimiter1)
            .unwrap_or((self, ""))
            .1
            .parse_until(delimiter2)
    }
}
