use std::fmt::{self, Display, Formatter};

use x11rb::xcb_ffi::XCBConnection;

#[derive(Debug)]
pub struct Builder {}

#[derive(Debug)]
pub struct XError {
    err: x11rb::x11_utils::X11Error,
}

impl Display for XError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "kind: {:?}: (major {}, minor {}, bad_value: {}, sequence: {})",
            self.err.error_kind,
            self.err.major_opcode,
            self.err.minor_opcode,
            self.err.bad_value,
            self.err.sequence,
        )
    }
}
impl std::error::Error for XError {}

impl Builder {
    pub fn new(_conn: &XCBConnection) -> Self {
        Self {}
    }

    pub fn from(&self, err: x11rb::x11_utils::X11Error) -> XError {
        XError { err }
    }
}
