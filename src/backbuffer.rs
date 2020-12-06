use log::{debug, trace};
use x11rb::connection::Connection as _;
use x11rb::connection::RequestConnection as _;
use x11rb::protocol::present::{self, ConnectionExt as _};
use x11rb::protocol::xproto;

use crate::dialog::Dialog;
use crate::errors::{Error, Result, X11ErrorString as _};
use crate::{Connection, XId};

pub struct Backbuffer<'a> {
    conn: &'a Connection,
    frontbuffer: xproto::Drawable,
    pub dialog: Dialog<'a>,
    eid: XId,
    serial: u32,
    last_completed_serial: u32,
    last_updated: u32,
    last_presented: u32,
    update_pending: bool,
    backbuffer_idle_event_received: bool,
}

impl<'a> Backbuffer<'a> {
    pub fn new(conn: &'a Connection, frontbuffer: XId, dialog: Dialog<'a>) -> Result<Self> {
        conn.extension_information(present::X11_EXTENSION_NAME)?
            .ok_or_else(|| Error::Unsupported("x11 present extension required".into()))?;
        let present_version = conn
            .present_query_version(1, 0)?
            .reply()
            .map_err(|e| conn.xerr_from(e))?;
        let caps = conn
            .present_query_capabilities(frontbuffer)?
            .reply()
            .map_err(|e| conn.xerr_from(e))?;

        debug!(
            "present version: {}.{}, capabilities: async {}, fence: {}",
            present_version.major_version,
            present_version.minor_version,
            caps.capabilities & u32::from(present::Capability::ASYNC) != 0,
            caps.capabilities & u32::from(present::Capability::FENCE) != 0,
        );

        let eid = conn
            .generate_id()
            .map_err(|e| conn.xerr_from(e))?;
        conn.present_select_input(
            eid,
            frontbuffer,
            present::EventMask::COMPLETE_NOTIFY | present::EventMask::IDLE_NOTIFY,
        )?;

        Ok(Self {
            conn,
            frontbuffer,
            dialog,
            eid,
            serial: 0,
            last_completed_serial: 0,
            last_updated: 1,
            last_presented: 0,
            update_pending: false,
            backbuffer_idle_event_received: false,
        })
    }

    pub fn update(&mut self) -> Result<bool> {
        if self.backbuffer_idle_event_received {
            trace!("update");
            self.dialog.update()?;
            self.last_updated += 1;
            self.update_pending = false;
            self.present()?;
            Ok(true)
        } else {
            trace!("update: backbuffer not idle");
            self.update_pending = true;
            Ok(false)
        }
    }

    // TODO on some hardware might not become ready until next swap completes?
    pub fn on_idle_notify(&mut self, ev: &present::IdleNotifyEvent) -> Result<()> {
        if ev.serial == self.serial {
            self.backbuffer_idle_event_received = true;
            trace!("idle notify: backbuffer became idle");
            if self.update_pending {
                assert!(self.update()?);
            }
        } else {
            trace!("idle notify: not idle");
        }
        Ok(())
    }

    pub fn on_vsync_completed(&mut self, serial: u32) {
        trace!("vsync completed");
        assert!(
            serial > self.last_completed_serial,
            format!(
                "serial <= self.last_completed_serial. ({} <= {}) number wrapped?",
                serial, self.last_completed_serial
            )
        );
        self.last_completed_serial = serial;
    }

    pub fn present(&mut self) -> Result<()> {
        if self.last_completed_serial != self.serial && self.last_presented == self.last_updated {
            trace!("redraw for the current frame already pending: last completed request {}, current request {}",
                self.last_completed_serial, self.serial);
            return Ok(());
        }
        trace!("present");
        self.serial += 1;
        self.last_presented = self.last_updated;
        self.backbuffer_idle_event_received = false;
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
            present::Option::NONE.into(), // options
            0,                            // target_msc
            0,                            // divisor
            0,                            // remainder
            &[],                          // notifiers
        )?;
        Ok(())
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
