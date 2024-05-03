use std::time::Duration;

use axum::{http::Request, response::Response, Router};
use tower_http::trace::TraceLayer;
use tracing::{debug, debug_span, field, Level, Span};
use tracing_subscriber::{
    filter::LevelFilter,
    fmt::{self, time::OffsetTime},
    layer::SubscriberExt,
    util::SubscriberInitExt,
    Layer,
};

use crate::{state::AppState, Logging};

pub fn init_tracing(logging: Logging) {
    let (levelfilter, level) = match logging {
        Logging::None => (LevelFilter::OFF, Level::ERROR),
        Logging::Info => (LevelFilter::INFO, Level::INFO),
        Logging::Debug => (LevelFilter::DEBUG, Level::DEBUG),
        Logging::All => (LevelFilter::DEBUG, Level::DEBUG),
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

pub trait TraceLayerExt {
    fn tracing_layer(self, logging: Logging) -> Self;
}

impl TraceLayerExt for Router<AppState> {
    fn tracing_layer(self, logging: Logging) -> Self {
        match logging {
            Logging::None | Logging::Debug | Logging::Info => return self,
            Logging::All => (),
        }

        self.layer(
            TraceLayer::new_for_http()
                .make_span_with(|_request: &Request<_>| {
                    debug_span!("request", method = field::Empty, uri = field::Empty)
                })
                .on_request(|req: &Request<_>, span: &Span| {
                    span.record("method", req.method().to_string());
                    span.record("uri", req.uri().to_string());
                    debug!("Received Request");
                })
                .on_response(|res: &Response<_>, latency: Duration, _span: &Span| {
                    let status = res.status();
                    debug!("Took {latency:?} to respond with status '{status}'");
                }),
        )
    }
}
