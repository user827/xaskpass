use log::{debug, trace};
use x11rb::connection::Connection as _;
use x11rb::connection::RequestConnection as _;
use x11rb::protocol::present::{self, ConnectionExt as _};
use x11rb::protocol::xproto;

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
    pub dialog: Dialog<'a>,
    eid: XId,
    serial: u32,
    vsync_completed: bool,
    backbuffer_dirty: bool,
    update_pending: bool,
    backbuffer_idle: bool,
}

impl<'a> Backbuffer<'a> {
    pub fn new(conn: &'a Connection, frontbuffer: XId, dialog: Dialog<'a>) -> Result<Self> {
        conn.extension_information(present::X11_EXTENSION_NAME)?
            .ok_or_else(|| Error::Unsupported("x11 present extension required".into()))?;
        // TODO is this correct?
        let (major, minor) = present::X11_XML_VERSION;
        let present_version = conn.present_query_version(major, minor)?.reply().map_xerr(conn)?;
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

        let eid = conn.generate_id().map_xerr(conn)?;
        conn.present_select_input(
            eid,
            frontbuffer,
            present::EventMask::COMPLETE_NOTIFY | present::EventMask::IDLE_NOTIFY,
        )?;

        let mut me = Self {
            conn,
            frontbuffer,
            dialog,
            eid,
            serial: 0,
            vsync_completed: true,
            update_pending: false,
            backbuffer_dirty: true,
            backbuffer_idle: true,
        };
        me.dialog.init(FrameId(me.get_next_serial()));
        Ok(me)
    }

    pub fn update(&mut self) -> Result<bool> {
        if self.backbuffer_idle {
            trace!("update");
            self.backbuffer_dirty = true;
            self.dialog.update(FrameId(self.get_next_serial()))?;
            self.update_pending = false;
            self.present()?;
            Ok(true)
        } else {
            trace!("update: backbuffer not idle");
            self.update_pending = true;
            Ok(false)
        }
    }

    pub fn on_idle_notify(&mut self, ev: &present::IdleNotifyEvent) -> Result<()> {
        if ev.serial == self.serial {
            self.backbuffer_idle = true;
            trace!("idle notify: backbuffer became idle");
            if self.update_pending {
                assert!(self.update()?);
            }
        } else {
            trace!("idle notify: not idle");
        }
        Ok(())
    }

    pub fn on_vsync_completed(&mut self, ev: present::CompleteNotifyEvent) -> Result<()> {
        trace!("vsync completed {:?}", ev);
        if ev.serial == self.serial {
            assert_ne!(ev.mode, present::CompleteMode::SKIP);
            self.vsync_completed = true;
        }
        if self.dialog.on_displayed(FrameId(ev.serial)) {
            self.update()?;
        }
        Ok(())
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
            self.dialog.surface.pixmap,
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
        if let Err(err) = self
            .conn
            .present_select_input(self.eid, self.frontbuffer, 0u32)
        {
            debug!("present select event clear failed: {}", err);
        }
    }
}
