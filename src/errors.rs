use std::ffi::CStr;
use std::fmt::{self, Display, Formatter};
use std::os::raw::c_char;

use log::debug;
use x11rb::xcb_ffi::XCBConnection;

use crate::Connection;

pub mod ffi {
    use std::ffi::c_void;
    use std::os::raw::{c_char, c_int};

    #[repr(C)]
    #[allow(non_camel_case_types)]
    pub struct xcb_errors_context_t {
        private: [u8; 0],
    }

    #[allow(non_camel_case_types)]
    pub type xcb_connection_t = c_void;

    extern "C" {
        pub fn xcb_errors_context_new(
            conn: *mut xcb_connection_t,
            ctx: *mut *mut xcb_errors_context_t,
        ) -> c_int;

        pub fn xcb_errors_get_name_for_major_code(
            ctx: *mut xcb_errors_context_t,
            major_code: u8,
        ) -> *const c_char;

        pub fn xcb_errors_get_name_for_error(
            ctx: *mut xcb_errors_context_t,
            error_code: u8,
            extension: *mut *const c_char,
        ) -> *const c_char;

        pub fn xcb_errors_get_name_for_minor_code(
            ctx: *mut xcb_errors_context_t,
            major_code: u8,
            minor_code: u16,
        ) -> *const c_char;

        pub fn xcb_errors_context_free(ctx: *mut xcb_errors_context_t);
    }
}

#[derive(Debug)]
pub struct Builder {
    ctx: *mut ffi::xcb_errors_context_t,
}

#[derive(Debug)]
pub struct XError {
    major: &'static str,
    minor: &'static str,
    extension: &'static str,
    error: &'static str,
    major_code: u8,
    minor_code: u16,
    error_code: u8,
}

impl Display for XError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "request: {}{}{} (major {}, minor {}), error: {}{}{} ({})",
            self.major,
            if self.minor == "" { "" } else { "-" },
            self.minor,
            self.major_code,
            self.minor_code,
            self.extension,
            if self.extension == "" { "" } else { "-" },
            self.error,
            self.error_code
        )
    }
}
impl std::error::Error for XError {}

impl Builder {
    pub fn new(conn: &XCBConnection) -> Self {
        let mut ctx: *mut ffi::xcb_errors_context_t = std::ptr::null_mut();
        if unsafe { ffi::xcb_errors_context_new(conn.get_raw_xcb_connection(), &mut ctx as _) } < 0
        {
            panic!("xcb error context creation failed");
        };
        Self { ctx }
    }

    pub fn from(&self, err: x11rb::x11_utils::X11Error) -> XError {
        let (error_code, major_code, minor_code) = (err.error_code, err.major_opcode, err.minor_opcode);
        let major = unsafe { ffi::xcb_errors_get_name_for_major_code(self.ctx, major_code) };
        let major = unsafe { CStr::from_ptr(major) }.to_str().unwrap();

        let minor =
            unsafe { ffi::xcb_errors_get_name_for_minor_code(self.ctx, major_code, minor_code) };
        let minor = if minor.is_null() {
            ""
        } else {
            unsafe { CStr::from_ptr(minor) }.to_str().unwrap()
        };
        let mut extension: *const c_char = std::ptr::null();
        let label = unsafe {
            ffi::xcb_errors_get_name_for_error(self.ctx, error_code, &mut extension as _)
        };
        let extension = if extension.is_null() {
            ""
        } else {
            unsafe { CStr::from_ptr(extension) }.to_str().unwrap()
        };
        let label = unsafe { CStr::from_ptr(label) }.to_str().unwrap();
        let major_code = major_code;
        let minor_code = minor_code;
        let error_code = error_code;
        XError {
            major,
            minor,
            extension,
            error: label,
            major_code,
            minor_code,
            error_code,
        }
    }
}

impl Drop for Builder {
    fn drop(&mut self) {
        debug!("dropping Builder");
        unsafe { ffi::xcb_errors_context_free(self.ctx) };
    }
}

pub trait X11ErrorString<E> {
    fn xerr_from(&self, err: E) -> Error;
}
impl X11ErrorString<x11rb::errors::ReplyError> for Connection {
    fn xerr_from(&self, err: x11rb::errors::ReplyError) -> Error {
        match err {
            x11rb::errors::ReplyError::ConnectionError(err) => Error::ConnectionError(err),
            x11rb::errors::ReplyError::X11Error(err) => Error::X11Error(self.xerr.from(err))
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
