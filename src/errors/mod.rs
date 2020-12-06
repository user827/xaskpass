use std::fmt::{Display, Formatter};

use crate::Connection;

#[cfg(xcb_errors)]
mod xcb_errors;
#[cfg(xcb_errors)]
pub use xcb_errors::*;

#[cfg(not(xcb_errors))]
mod fallback;
#[cfg(not(xcb_errors))]
pub use fallback::*;

pub trait X11ErrorString<E> {
    fn xerr_from(&self, err: E) -> Error;
}
impl X11ErrorString<x11rb::errors::ReplyError> for Connection {
    fn xerr_from(&self, err: x11rb::errors::ReplyError) -> Error {
        match err {
            x11rb::errors::ReplyError::ConnectionError(err) => Error::ConnectionError(err),
            x11rb::errors::ReplyError::X11Error(err) => Error::X11Error(self.xerr.from(err)),
        }
    }
}
impl X11ErrorString<x11rb::errors::ReplyOrIdError> for Connection {
    fn xerr_from(&self, err: x11rb::errors::ReplyOrIdError) -> Error {
        match err {
            x11rb::errors::ReplyOrIdError::ConnectionError(err) => Error::ConnectionError(err),
            x11rb::errors::ReplyOrIdError::X11Error(err) => Error::X11Error(self.xerr.from(err)),
            x11rb::errors::ReplyOrIdError::IdsExhausted => panic!("X11 ids exhausted"),
        }
    }
}

impl X11ErrorString<x11rb::x11_utils::X11Error> for Connection {
    fn xerr_from(&self, err: x11rb::x11_utils::X11Error) -> Error {
        Error::X11Error(self.xerr.from(err))
    }
}

#[derive(Debug)]
pub enum Error {
    Unsupported(String),
    ConnectError(x11rb::errors::ConnectError),
    ConnectionError(x11rb::errors::ConnectionError),
    Error(anyhow::Error),
    X11Error(XError),
}
pub type Result<T> = std::result::Result<T, Error>;
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::ConnectError(err) => err.source(),
            Error::ConnectionError(err) => err.source(),
            _ => None,
        }
    }
}
impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::ConnectError(err) => write!(f, "Error creating X11 connection: {}", err),
            Error::ConnectionError(err) => write!(f, "X11 connection error: {}", err),
            Error::X11Error(err) => write!(f, "X11 error: {}", err),
            Error::Error(err) => write!(f, "{:#}", err),
            Error::Unsupported(err) => write!(f, "Unsupported: {}", err),
            //_ => panic!("should convert these errors"),
        }
    }
}
impl From<x11rb::errors::ConnectionError> for Error {
    fn from(val: x11rb::errors::ConnectionError) -> Self {
        Error::ConnectionError(val)
    }
}
impl From<x11rb::errors::ConnectError> for Error {
    fn from(val: x11rb::errors::ConnectError) -> Self {
        Error::ConnectError(val)
    }
}
impl From<anyhow::Error> for Error {
    fn from(val: anyhow::Error) -> Self {
        Error::Error(val)
    }
}
