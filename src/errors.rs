use std::fmt::{Display, Formatter};

pub use anyhow::Context;

#[macro_export]
macro_rules! bail {
    ($msg:literal $(,)?) => {
        return Err($crate::errors::Error::Generic(anyhow::anyhow!($msg)))
    };
    ($err:expr $(,)?) => {
        return Err($crate::errors::Error::Generic(anyhow::anyhow!($err)))
    };
    ($fmt:expr, $($arg:tt)*) => {
        return Err($crate::errors::Error::Generic(anyhow::anyhow!($fmt, $($arg)*)))
    };
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(thiserror::Error, Debug)]
#[error("Unsupported: {0}")]
pub struct Unsupported(pub String);

#[derive(thiserror::Error, Debug)]
pub enum Error {
    X11(x11rb::x11_utils::X11Error),
    Connection(#[from] x11rb::errors::ConnectionError),
    ReplyOrId(#[from] x11rb::errors::ReplyOrIdError),
    Reply(#[from] x11rb::errors::ReplyError),
    Unsupported(#[from] Unsupported),
    #[error(transparent)]
    Generic(#[from] anyhow::Error),
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Generic(err) => write!(f, "{err:#}"),
            Self::Connection(err) => write!(f, "X11 connection: {err} ({err:?})"),
            Self::Reply(x11rb::errors::ReplyError::ConnectionError(err))
            | Self::ReplyOrId(x11rb::errors::ReplyOrIdError::ConnectionError(err)) => {
                write!(f, "X11 connection: {err} ({err:?})")
            }
            Self::Reply(err) => write!(f, "{err}"),
            Self::ReplyOrId(err) => write!(f, "{err}"),
            Self::X11(err) => write!(f, "{err:?}"),
            Self::Unsupported(err) => write!(f, "{err}"),
        }
    }
}
