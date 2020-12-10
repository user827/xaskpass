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

pub type Result<T> = std::result::Result<T, Error>;

pub trait X11ErrorString<T> {
    fn map_xerr(self, conn: &Connection) -> Result<T>;
}
impl<T> X11ErrorString<T> for std::result::Result<T, x11rb::errors::ReplyError> {
    fn map_xerr(self, conn: &Connection) -> Result<T> {
        match self {
            Err(x11rb::errors::ReplyError::ConnectionError(err)) => {
                Err(Error::ConnectionError(err))
            }
            Err(x11rb::errors::ReplyError::X11Error(err)) => {
                Err(Error::X11Error(conn.xerr.from(err)))
            }
            Ok(o) => Ok(o),
        }
    }
}
impl<T> X11ErrorString<T> for std::result::Result<T, x11rb::errors::ReplyOrIdError> {
    fn map_xerr(self, conn: &Connection) -> Result<T> {
        match self {
            Err(x11rb::errors::ReplyOrIdError::ConnectionError(err)) => {
                Err(Error::ConnectionError(err))
            }
            Err(x11rb::errors::ReplyOrIdError::X11Error(err)) => {
                Err(Error::X11Error(conn.xerr.from(err)))
            }
            Err(x11rb::errors::ReplyOrIdError::IdsExhausted) => panic!("X11 ids exhausted"),
            Ok(o) => Ok(o),
        }
    }
}

impl<T> X11ErrorString<T> for std::result::Result<T, x11rb::x11_utils::X11Error> {
    fn map_xerr(self, conn: &Connection) -> Result<T> {
        self.map_err(|err| Error::X11Error(conn.xerr.from(err)))
    }
}

#[derive(Debug)]
pub enum Error {
    Unsupported(String),
    ConnectError(x11rb::errors::ConnectError),
    ConnectionError(x11rb::errors::ConnectionError),
    Error(anyhow::Error),
    X11Error(XError),
    BufferFullError(crate::secret::BufferFull),
}
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
            Error::BufferFullError(err) => {
                write!(f, "Passphrase length limit exceeded: {}", err.limit)
            }
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
impl From<crate::secret::BufferFull> for Error {
    fn from(val: crate::secret::BufferFull) -> Self {
        Error::BufferFullError(val)
    }
}
