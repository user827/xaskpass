use std::ops::Deref;

use log::{debug, trace};
use x11rb::connection::Connection as _;
use x11rb::connection::RequestConnection as _;
use x11rb::protocol::present::{self, ConnectionExt as _};
use x11rb::protocol::xproto;
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::xcb_ffi::XCBConnection;

use crate::dialog::Dialog;
use crate::errors::{Error, Result, X11ErrorString as _};
use crate::{Connection, XId};

// Hide u32 because CompletionNotify events might not come in in order or the serial might have
// wrapped.
#[derive(Eq, PartialEq, Debug, Clone, Copy)]
pub struct FrameId(u32);

pub struct Backbuffer<'a> {
    conn: &'a Connection,
    frontbuffer: xproto::Drawable,
    eid: Option<XId>,
    serial: u32,
    vsync_completed: bool,
    backbuffer_dirty: bool,
    update_pending: bool,
    backbuffer_idle: bool,
    surface: XcbSurface<'a>,
    pub(super) cr: cairo::Context,
    pub(super) resize_requested: Option<(u16, u16)>,
}

impl<'a> Backbuffer<'a> {
    pub fn new(conn: &'a Connection, frontbuffer: XId, surface: XcbSurface<'a>) -> Result<Self> {
        conn.extension_information(present::X11_EXTENSION_NAME)?
            .ok_or_else(|| Error::Unsupported("x11 present extension required".into()))?;
        // TODO is this correct?
        let (major, minor) = present::X11_XML_VERSION;
        let present_version = conn
            .present_query_version(major, minor)?
            .reply()
            .map_xerr(conn)?;
        let caps = conn
            .present_query_capabilities(frontbuffer)?
            .reply()
            .map_xerr(conn)?;

        debug!(
            "present version: {}.{}, capabilities: async {}, fence: {}",
            present_version.major_version,
            present_version.minor_version,
            caps.capabilities & u32::from(present::Capability::ASYNC) != 0,
            caps.capabilities & u32::from(present::Capability::FENCE) != 0,
        );

        let cr = cairo::Context::new(&surface);

        let me = Self {
            conn,
            frontbuffer,
            eid: None,
            serial: 0,
            vsync_completed: true,
            update_pending: false,
            backbuffer_dirty: true,
            backbuffer_idle: true,
            surface,
            cr,
            resize_requested: None,
        };
        Ok(me)
    }

    pub fn init(&mut self, frontbuffer: xproto::Window, dialog: &mut Dialog) -> Result<()> {
        self.eid = Some(self.conn.generate_id().map_xerr(self.conn)?);
        self.conn.present_select_input(
            self.eid.unwrap(),
            frontbuffer,
            present::EventMask::COMPLETE_NOTIFY | present::EventMask::IDLE_NOTIFY,
        )?;

        self.frontbuffer = frontbuffer;

        let (w, h) = dialog.window_size(&self.cr);
        self.surface.setup_pixmap(frontbuffer, w, h)?;
        dialog.init(&self.cr, FrameId(self.get_next_serial()));
        Ok(())
    }

    pub fn update(&mut self, dialog: &mut Dialog) -> Result<bool> {
        if self.backbuffer_idle {
            trace!("update");
            self.backbuffer_dirty = true;
            if let Some((width, height)) = self.resize_requested {
                trace!("resize requested");
                let surface_cleared = self.surface.resize(width, height)?;
                dialog.resize(
                    &self.cr,
                    width,
                    height,
                    FrameId(self.get_next_serial()),
                    surface_cleared,
                );
                self.resize_requested = None;
            } else {
                dialog.update(&self.cr, FrameId(self.get_next_serial()));
            }
            self.surface.flush();
            self.update_pending = false;
            self.present()?;
            Ok(true)
        } else {
            trace!("update: backbuffer not idle");
            self.update_pending = true;
            Ok(false)
        }
    }

    pub fn on_idle_notify(&mut self, ev: &present::IdleNotifyEvent) -> bool {
        if ev.serial == self.serial {
            self.backbuffer_idle = true;
            trace!("idle notify: backbuffer became idle");
            if self.update_pending {
                return true;
            }
        } else {
            trace!("idle notify: not idle");
        }
        false
    }

    pub fn on_vsync_completed(&mut self, ev: present::CompleteNotifyEvent) -> FrameId {
        if ev.serial == self.serial {
            assert_ne!(ev.mode, present::CompleteMode::SKIP);
            self.vsync_completed = true;
        }
        FrameId(ev.serial)
    }

    pub fn present(&mut self) -> Result<()> {
        if !self.vsync_completed && !self.backbuffer_dirty {
            trace!("redraw for the current frame already pending");
            return Ok(());
        }
        trace!("present");
        self.serial = self.get_next_serial();
        self.conn.present_pixmap(
            self.frontbuffer,
            self.surface.pixmap,
            self.serial,
            0,                            // valid
            0,                            // update
            0,                            // x_off
            0,                            // y_off
            0,                            // target_crtc
            0,                            // wait_fence
            0,                            // idle_fence
            present::Option::COPY.into(), // options
            0,                            // target_msc
            0,                            // divisor
            0,                            // remainder
            &[],                          // notifiers
        )?;
        self.backbuffer_idle = false;
        self.backbuffer_dirty = false;
        self.vsync_completed = false;
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
            if let Err(err) = self.conn.present_select_input(eid, self.frontbuffer, 0u32) {
                debug!("present select event clear failed: {}", err);
            }
        }
    }
}

#[derive(Debug)]
pub struct XcbSurface<'a> {
    conn: &'a crate::Connection,
    pub pixmap: xproto::Pixmap,
    surface: cairo::XCBSurface,
    width: u16,
    height: u16,
    drawable: xproto::Drawable,
    depth: u8,
}

impl<'a> XcbSurface<'a> {
    pub fn new(
        conn: &'a crate::Connection,
        drawable: xproto::Drawable,
        depth: u8,
        visual_type: &xproto::Visualtype,
        width: u16,
        height: u16,
    ) -> Result<Self> {
        let pixmap = conn.generate_id().map_xerr(conn)?;
        conn.create_pixmap(depth, pixmap, drawable, width, height)?;

        let surface = Self::create(conn, pixmap, visual_type, width, height);

        Ok(Self {
            conn,
            surface,
            pixmap,
            drawable,
            height,
            width,
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
            unsafe { cairo::XCBConnection::from_raw_none(conn.get_raw_xcb_connection() as _) };
        let mut xcb_visualtype: xcb_visualtype_t = (*visual_type).into();
        let cairo_visual =
            unsafe { cairo::XCBVisualType::from_raw_none(&mut xcb_visualtype as *mut _ as _) };
        let cairo_drawable = cairo::XCBDrawable(drawable);
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

        self.setup_pixmap(self.drawable, new_width, new_height)?;
        Ok(true)
    }

    pub fn setup_pixmap(
        &mut self,
        drawable: xproto::Drawable,
        new_width: u16,
        new_height: u16,
    ) -> Result<()> {
        let pixmap = self.conn.generate_id().map_xerr(self.conn)?;
        self.conn
            .create_pixmap(self.depth, pixmap, drawable, new_width, new_height)?;

        let cairo_pixmap = cairo::XCBDrawable(pixmap);
        self.surface
            .set_drawable(&cairo_pixmap, new_width.into(), new_height.into())
            .unwrap();
        self.conn.free_pixmap(self.pixmap)?;
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
        if let Err(err) = self.conn.free_pixmap(self.pixmap) {
            debug!("free pixmap failed: {}", err);
        }
    }
}

impl<'a> Deref for XcbSurface<'a> {
    type Target = cairo::XCBSurface;
    fn deref(&self) -> &Self::Target {
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
