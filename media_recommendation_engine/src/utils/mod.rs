use std::time::SystemTime;

mod errorext;
pub use errorext::{ConvertErr, HandleErr, Ignore};

mod parsing;
pub use parsing::{ParseBetween, ParseUntil};

mod tracing;
pub use tracing::{init_tracing, TraceLayerExt};

mod frontend;
pub use frontend::{frontend_redirect, frontend_redirect_explicit, htmx, HXTarget};

mod auth;
pub use auth::{login_required, AuthExt, AuthSession, Credentials};

pub mod templates;

mod settings;
pub use settings::ServerSettings;

pub mod streaming;

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

macro_rules! bail {
    ($err:expr) => {
        return Err(crate::state::AppError::Anyhow(anyhow::anyhow!($err)))
    };
    ($fmt:expr, $($arg:tt)*) => {
        return Err(AppError::Anyhow(anyhow!(format!($fmt, $($arg)*))))
    };
}
pub(crate) use bail;

pub fn pseudo_random() -> u32 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos()
}

pub fn pseudo_random_range(min: u32, max: u32) -> u32 {
    min + (pseudo_random() % (max - min))
}
