use std::fmt::{Display, Formatter};

pub use anyhow::{anyhow, bail, Context, Error};

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
                Err(err).context("X11 connection")
            }
            Err(x11rb::errors::ReplyError::X11Error(err)) => {
                Err(conn.xerr.from(err)).context("X11")
            }
            Ok(o) => Ok(o),
        }
    }
}
impl<T> X11ErrorString<T> for std::result::Result<T, x11rb::errors::ReplyOrIdError> {
    fn map_xerr(self, conn: &Connection) -> Result<T> {
        match self {
            Err(x11rb::errors::ReplyOrIdError::ConnectionError(err)) => {
                Err(err).context("X11 connection")
            }
            Err(x11rb::errors::ReplyOrIdError::X11Error(err)) => {
                Err(conn.xerr.from(err)).context("X11")
            }
            Err(x11rb::errors::ReplyOrIdError::IdsExhausted) => panic!("X11 ids exhausted"),
            Ok(o) => Ok(o),
        }
    }
}

impl<T> X11ErrorString<T> for std::result::Result<T, x11rb::x11_utils::X11Error> {
    fn map_xerr(self, conn: &Connection) -> Result<T> {
        self.map_err(|err| conn.xerr.from(err)).context("X11")
    }
}

#[derive(Debug)]
pub struct Unsupported(pub String);
impl std::error::Error for Unsupported {}

impl Display for Unsupported {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Unsupported: {}", self.0)
    }
}
