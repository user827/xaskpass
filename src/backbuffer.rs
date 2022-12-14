use std::ops::Deref;
use std::ptr;

use log::{debug, trace};
use x11rb::connection::Connection as _;
use x11rb::connection::RequestConnection as _;
use x11rb::protocol::present::{self, ConnectionExt as _};
use x11rb::protocol::xproto;
use x11rb::protocol::xproto::PixmapWrapper;
use x11rb::xcb_ffi::XCBConnection;

use crate::dialog::Dialog;
use crate::errors::{Result, Unsupported};
use crate::{Connection, XId};

#[derive(Eq, PartialEq, Debug, Clone, Copy)]
enum State {
    Sync,
    // Window needs redraw
    Exposed,
    // Windows needs redraw from our backbuffer
    Dirty,
}

pub struct Backbuffer<'a> {
    conn: &'a Connection,
    window: xproto::Window,
    eid: Option<XId>,
    serial: u32,
    vsync_completed: bool,
    dirty: State,
    backbuffer_idle: bool,
    surface: XcbSurface<'a>,
    pub(super) cr: cairo::Context,
    pub(super) resize_requested: Option<(u16, u16)>,
    // TODO how to know when the window is not exposed at all?
    pub(super) visible: bool,
}

pub struct Cookie<'a> {
    conn: &'a Connection,
    version: x11rb::cookie::Cookie<'a, XCBConnection, present::QueryVersionReply>,
    caps: Option<x11rb::cookie::Cookie<'a, XCBConnection, present::QueryCapabilitiesReply>>,
    window: xproto::Window,
    surface: XcbSurface<'a>,
    pub(super) cr: cairo::Context,
}

impl<'a> Cookie<'a> {
    pub fn reply(self) -> Result<Backbuffer<'a>> {
        if log::log_enabled!(log::Level::Debug) {
            let version = self.version.reply()?;
            let caps = self.caps.unwrap().reply()?;
            debug!(
                "present version: {}.{}, capabilities: async {}, fence: {}, ust: {}",
                version.major_version,
                version.minor_version,
                caps.capabilities & u32::from(present::Capability::ASYNC) != 0,
                caps.capabilities & u32::from(present::Capability::FENCE) != 0,
                caps.capabilities & u32::from(present::Capability::UST) != 0,
            );
        }

        let me = Backbuffer {
            conn: self.conn,
            window: self.window,
            eid: None,
            serial: 0,
            vsync_completed: true,
            dirty: State::Sync,
            backbuffer_idle: true,
            surface: self.surface,
            cr: self.cr,
            resize_requested: None,
            visible: false,
        };
        Ok(me)
    }
}

impl<'a> Backbuffer<'a> {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(
        conn: &'a Connection,
        window: xproto::Window,
        surface: XcbSurface<'a>,
    ) -> Result<Cookie<'a>> {
        conn.extension_information(present::X11_EXTENSION_NAME)?
            .ok_or_else(|| Unsupported("x11 present extension required".into()))?;
        // TODO is this correct?
        let (major, minor) = present::X11_XML_VERSION;
        let version = conn.present_query_version(major, minor)?;

        let caps = if log::log_enabled!(log::Level::Debug) {
            Some(conn.present_query_capabilities(window)?)
        } else {
            None
        };

        let cr = cairo::Context::new(&surface).expect("cairo context new");

        Ok(Cookie {
            conn,
            version,
            caps,
            window,
            surface,
            cr,
        })
    }

    pub fn set_exposed(&mut self) {
        trace!("set_exposed");
        self.visible = true;
        if self.vsync_completed && self.dirty == State::Sync {
            self.dirty = State::Exposed;
        }
    }

    pub fn init(&mut self, window: xproto::Window, dialog: &mut Dialog) -> Result<()> {
        trace!("init");
        self.eid = Some(self.conn.generate_id()?);
        self.conn.present_select_input(
            self.eid.unwrap(),
            window,
            present::EventMask::COMPLETE_NOTIFY | present::EventMask::IDLE_NOTIFY,
        )?;

        self.window = window;

        let (w, h) = dialog.window_size(&self.cr);
        self.surface.setup_pixmap(w, h)?;
        dialog.cairo_context_changed(&self.cr);
        dialog.init(&self.cr);
        dialog.set_painted();
        self.dirty = State::Dirty;
        Ok(())
    }

    pub fn commit(&mut self, dialog: &mut Dialog) -> Result<()> {
        trace!("commit");
        if !self.visible {
            debug!("not visible");
            return Ok(());
        }
        if dialog.dirty() || self.resize_requested.is_some() {
            self.repaint(dialog)?;
        }
        if self.dirty != State::Sync {
            self.present(dialog)?;
        }
        Ok(())
    }

    fn repaint(&mut self, dialog: &mut Dialog) -> Result<()> {
        trace!("repaint");
        if self.backbuffer_idle {
            self.dirty = State::Dirty;
            if let Some((width, height)) = self.resize_requested {
                trace!("resize requested");
                let surface_cleared = self.surface.resize(width, height)?;
                dialog.resize(&self.cr, width, height, surface_cleared);
                self.resize_requested = None;
            } else {
                dialog.repaint(&self.cr);
            }
            self.surface.flush();
            dialog.set_painted();
        } else {
            trace!("repaint: backbuffer not idle");
        }
        Ok(())
    }

    pub fn on_idle_notify(&mut self, ev: &present::IdleNotifyEvent) {
        trace!("on_idle_notify: {:?}", ev);
        if ev.serial == self.serial {
            self.backbuffer_idle = true;
            trace!("idle notify: backbuffer became idle");
        } else {
            trace!("idle notify: not idle");
        }
    }

    pub fn on_vsync_completed(&mut self, ev: present::CompleteNotifyEvent) {
        trace!("on_vsync_completed: {:?}", ev);
        if ev.serial == self.serial {
            if ev.mode == present::CompleteMode::SKIP {
                debug!("present completemode skip: {:?}", ev);
            }
            self.vsync_completed = true;
        } else {
            panic!("on_vsync_completed: ev.serial != self.serial");
        }
    }

    fn present(&mut self, dialog: &mut Dialog) -> Result<()> {
        trace!("present");
        if !self.vsync_completed {
            trace!(
                "a frame (serial {}) already pending for present",
                self.serial
            );
            return Ok(());
        }
        self.serial = self.get_next_serial();
        self.conn.present_pixmap(
            self.window,
            self.surface.pixmap(),
            self.serial,
            0,                            // valid
            0,                            // update
            0,                            // x_off
            0,                            // y_off
            0,                            // target_crtc
            0,                            // wait_fence
            0,                            // idle_fence
            present::Option::NONE.into(), // options
            0,                            // target_msc
            0,   // divisor, if 0, the presentation occus after the current field
            0,   // remainder
            &[], // notifiers
        )?;
        self.backbuffer_idle = false;
        self.dirty = State::Sync;
        self.vsync_completed = false;

        self.conn.flush()?;
        dialog.set_next_frame();
        Ok(())
    }

    fn get_next_serial(&self) -> u32 {
        self.serial.wrapping_add(1)
    }
}

impl<'a> Drop for Backbuffer<'a> {
    fn drop(&mut self) {
        debug!("dropping backbuffer");
        if let Some(eid) = self.eid {
            if let Err(err) = self.conn.present_select_input(eid, self.window, 0_u32) {
                debug!("present select event clear failed: {}", err);
            }
        }
    }
}

#[derive(Debug)]
pub struct XcbSurface<'a> {
    conn: &'a crate::Connection,
    pixmap: PixmapWrapper<'a, Connection>,
    surface: cairo::XCBSurface,
    width: u16,
    height: u16,
    drawable: xproto::Drawable,
    depth: u8,
}

impl<'a> XcbSurface<'a> {
    pub fn pixmap(&self) -> xproto::Pixmap {
        self.pixmap.pixmap()
    }

    pub fn new(
        conn: &'a XCBConnection,
        drawable: xproto::Drawable,
        depth: u8,
        visual_type: &xproto::Visualtype,
        width: u16,
        height: u16,
    ) -> Result<Self> {
        let pixmap = PixmapWrapper::create_pixmap(conn, depth, drawable, width, height)?;
        let surface = Self::create(conn, pixmap.pixmap(), visual_type, width, height);

        Ok(Self {
            conn,
            pixmap,
            surface,
            width,
            height,
            drawable,
            depth,
        })
    }

    pub fn create(
        conn: &XCBConnection,
        drawable: xproto::Drawable,
        visual_type: &xproto::Visualtype,
        width: u16,
        height: u16,
    ) -> cairo::XCBSurface {
        let cairo_conn =
            unsafe { cairo::XCBConnection::from_raw_none(conn.get_raw_xcb_connection().cast()) };
        let mut xcb_visualtype: xcb_visualtype_t = (*visual_type).into();
        let cairo_visual = unsafe {
            cairo::XCBVisualType::from_raw_none(
                ptr::addr_of_mut!(xcb_visualtype).cast(),
            )
        };
        let cairo_drawable = cairo::XCBDrawable(drawable);
        trace!("creating cairo::XCBSurface {}, {}", width, height);
        cairo::XCBSurface::create(
            &cairo_conn,
            &cairo_drawable,
            &cairo_visual,
            width.into(),
            height.into(),
        )
        .unwrap()
    }

    pub fn resize(&mut self, width: u16, height: u16) -> Result<bool> {
        if width <= self.width && height <= self.height {
            return Ok(false);
        }
        let mut new_width = self.width;
        let mut new_height = self.height;
        debug!("resizing");
        if width > new_width {
            new_width *= 2;
            if width > new_width {
                new_width = width;
            }
        }
        if height > new_height {
            new_height *= 2;
            if height > new_height {
                new_height = height;
            }
        }

        self.setup_pixmap(new_width, new_height)?;
        Ok(true)
    }

    pub fn setup_pixmap(&mut self, new_width: u16, new_height: u16) -> Result<()> {
        let pixmap = PixmapWrapper::create_pixmap(
            self.conn,
            self.depth,
            self.drawable,
            new_width,
            new_height,
        )?;

        let cairo_pixmap = cairo::XCBDrawable(pixmap.pixmap());
        self.surface
            .set_drawable(&cairo_pixmap, new_width.into(), new_height.into())
            .unwrap();
        self.pixmap = pixmap;

        self.width = new_width;
        self.height = new_height;
        Ok(())
    }
}

impl<'a> Drop for XcbSurface<'a> {
    fn drop(&mut self) {
        debug!("dropping xcb surface");
        self.surface.finish();
    }
}

impl<'a> Deref for XcbSurface<'a> {
    type Target = cairo::XCBSurface;
    fn deref(&self) -> &Self::Target {
        &self.surface
    }
}

impl<'a> AsRef<cairo::Surface> for XcbSurface<'a> {
    fn as_ref(&self) -> &cairo::Surface {
        &self.surface
    }
}

#[allow(non_camel_case_types)]
pub type xcb_visualid_t = u32;

#[derive(Debug, Clone, Copy)]
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct xcb_visualtype_t {
    pub visual_id: xcb_visualid_t,
    pub class: u8,
    pub bits_per_rgb_value: u8,
    pub colormap_entries: u16,
    pub red_mask: u32,
    pub green_mask: u32,
    pub blue_mask: u32,
    pub pad0: [u8; 4],
}

impl From<xproto::Visualtype> for xcb_visualtype_t {
    fn from(value: xproto::Visualtype) -> Self {
        Self {
            visual_id: value.visual_id,
            class: value.class.into(),
            bits_per_rgb_value: value.bits_per_rgb_value,
            colormap_entries: value.colormap_entries,
            red_mask: value.red_mask,
            green_mask: value.green_mask,
            blue_mask: value.blue_mask,
            pad0: [0; 4],
        }
    }
}

// NOTES
// documentation for current versions of the present protocol:
// https://gitlab.freedesktop.org/xorg/proto/xorgproto/-/blob/master/presentproto.txt

// If 'divisor' is zero, then the presentation will occur after the current field:
// https://keithp.com/blogs/Present/
