use std::{fmt, str::FromStr, time::SystemTime};

mod errorext;
pub use errorext::{ConvertErr, HandleErr, Ignore};

mod parsing;
pub use parsing::{ParseBetween, ParseUntil};

mod tracing;
use serde::{de, Deserialize, Deserializer};
pub use tracing::{init_tracing, TraceLayerExt};

mod frontend;
pub use frontend::{frontend_redirect, frontend_redirect_explicit, htmx, HXTarget};

mod auth;
pub use auth::{login_required, AuthExt, AuthSession, Credentials};

pub mod templates;

mod settings;
pub use settings::ServerSettings;

pub mod streaming;

mod watchstream;
pub use watchstream::WatchStream;

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

macro_rules! status {
    ($status:expr) => {
        return Err(AppError::Status($status))
    };
}
pub(crate) use status;

pub fn pseudo_random() -> u32 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos()
}

pub fn pseudo_random_range(min: u32, max: u32) -> u32 {
    min + (pseudo_random() % (max - min))
}

pub fn empty_string_as_none<'de, D, T>(de: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr,
    T::Err: fmt::Display,
{
    let opt = Option::<String>::deserialize(de)?;
    match opt.as_deref() {
        None | Some("") => Ok(None),
        Some(s) => FromStr::from_str(s).map_err(de::Error::custom).map(Some),
    }
}
