use std::fmt::{Display, Formatter};

pub use anyhow::{anyhow, bail, Context, Error};

pub type Result<T> = std::result::Result<T, Error>;

pub trait X11ErrorString<T> {
    fn map_xerr(self) -> Result<T>;
}
impl<T> X11ErrorString<T> for std::result::Result<T, x11rb::errors::ReplyError> {
    fn map_xerr(self) -> Result<T> {
        match self {
            Err(x11rb::errors::ReplyError::ConnectionError(err)) => {
                Err(err).context("X11 connection")
            }
            Err(err) => Err(err.into()),
            Ok(o) => Ok(o),
        }
    }
}
impl<T> X11ErrorString<T> for std::result::Result<T, x11rb::errors::ReplyOrIdError> {
    fn map_xerr(self) -> Result<T> {
        match self {
            Err(x11rb::errors::ReplyOrIdError::ConnectionError(err)) => {
                Err(err).context("X11 connection")
            }
            Err(err) => Err(err.into()),
            Ok(o) => Ok(o),
        }
    }
}

impl<T> X11ErrorString<T> for std::result::Result<T, x11rb::x11_utils::X11Error> {
    fn map_xerr(self) -> Result<T> {
        self.map_err(|err| anyhow!("{:?}", err))
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
