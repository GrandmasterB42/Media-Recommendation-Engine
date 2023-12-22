use tracing::Level;
use tracing_subscriber::{
    filter::LevelFilter,
    fmt::{self, time::OffsetTime},
    layer::SubscriberExt,
    util::SubscriberInitExt,
    Layer,
};

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
