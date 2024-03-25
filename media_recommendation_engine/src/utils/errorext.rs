use tracing::log::{error, warn};

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

pub trait ConvertErr<T, F> {
    fn convert_err<E: From<F>>(self) -> Result<T, E>;
}

impl<T, F> ConvertErr<T, F> for Result<T, F> {
    #[inline]
    fn convert_err<E: From<F>>(self) -> Result<T, E> {
        self.map_err(Into::into)
    }
}
