use std::time::SystemTime;

mod errorext;
pub use errorext::{ConvertErr, HandleErr, Ignore};

mod parsing;
pub use parsing::{ParseBetween, ParseUntil};

mod tracing;
pub use tracing::{init_tracing, tracing_layer};

mod frontend;
pub use frontend::{frontend_redirect, frontend_redirect_explicit, htmx, HXTarget};

pub mod auth;
pub use auth::{AuthSession, Credentials};

macro_rules! relative {
    ($path:expr) => {
        if cfg!(windows) {
            concat!(env!("CARGO_MANIFEST_DIR"), "\\", $path)
        } else {
            concat!(env!("CARGO_MANIFEST_DIR"), "/", $path)
        }
    };
}
pub(crate) use relative;

pub fn pseudo_random() -> u32 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos()
}

pub fn pseudo_random_range(min: u32, max: u32) -> u32 {
    min + (pseudo_random() % (max - min))
}
