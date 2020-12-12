use std::ffi::CStr;
use std::fmt::{self, Display, Formatter};
use std::os::raw::c_char;

use log::debug;
use x11rb::xcb_ffi::XCBConnection;

mod ffi;

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
        if unsafe { ffi::xcb_errors_context_new(conn.get_raw_xcb_connection() as *mut _, &mut ctx as _) } < 0
        {
            panic!("xcb error context creation failed");
        };
        Self { ctx }
    }

    pub fn from(&self, err: x11rb::x11_utils::X11Error) -> XError {
        let (error_code, major_code, minor_code) =
            (err.error_code, err.major_opcode, err.minor_opcode);
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
