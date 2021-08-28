use log::{debug, trace, warn};
use tokio::time::Instant;
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{self, ConnectionExt as _};
use x11rb::protocol::Event as XEvent;
use zeroize::Zeroize;

use crate::backbuffer::{Backbuffer, FrameId};
use crate::errors::{Context as _, Result, X11ErrorString as _};
use crate::keyboard::{Keyboard, Keycode};
use crate::{Connection, XId};

pub enum Event {
    Motion {
        x: f64,
        y: f64,
    },
    KeyPress(Keypress),
    ButtonPress {
        button: xproto::ButtonIndex,
        x: f64,
        y: f64,
        isrelease: bool,
    },
    Paste(String),
    Focus(bool),
    PendingUpdate,
    VsyncCompleted(FrameId),
    Exit,
}

pub struct Keypress {
    key_press: xproto::KeyPressEvent,
}

impl Keypress {
    pub fn get_key(&self) -> Keycode {
        self.key_press.detail.into()
    }
}

impl Drop for Keypress {
    fn drop(&mut self) {
        self.key_press.detail.zeroize();
    }
}

pub struct XContext<'a> {
    pub conn: &'a Connection,
    pub backbuffer: Backbuffer<'a>,
    pub(super) window: xproto::Window,
    pub keyboard: Keyboard<'a>,
    pub(super) atoms: crate::AtomCollection,
    pub(super) colormap: XId,
    pub(super) own_colormap: bool,
    pub(super) width: u16,
    pub(super) height: u16,
    pub(super) grab_keyboard: bool,
    pub(super) startup_time: Instant,
    pub(super) keyboard_grabbed: bool,
    pub(super) first_expose_received: bool,
    pub(super) input_cursor: Option<XId>,
}

impl<'a> XContext<'a> {
    pub fn poll_for_event(&mut self) -> Result<Option<Event>> {
        loop {
            if let Some(xevent) = self.conn.poll_for_event()? {
                trace!("xevent {:?}", xevent);
                if let Some(event) = self.handle_xevent(xevent)? {
                    return Ok(Some(event));
                }
            } else {
                return Ok(None);
            }
        }
    }

    pub fn set_default_cursor(&self) -> Result<()> {
        self.conn.change_window_attributes(
            self.window,
            &xproto::ChangeWindowAttributesAux::new().cursor(x11rb::NONE),
        )?;
        Ok(())
    }

    pub fn set_input_cursor(&self) -> Result<()> {
        trace!("set input cursor");
        if let Some(cursor) = self.input_cursor {
            self.conn.change_window_attributes(
                self.window,
                &xproto::ChangeWindowAttributesAux::new().cursor(cursor),
            )?;
            trace!("input cursor set");
        }
        Ok(())
    }

    pub fn paste_primary(&self) -> Result<()> {
        self.conn.convert_selection(
            self.window,
            xproto::AtomEnum::PRIMARY.into(),
            self.atoms.UTF8_STRING,
            self.atoms.XSEL_DATA,
            x11rb::CURRENT_TIME,
        )?;
        Ok(())
    }

    pub fn paste_clipboard(&self) -> Result<()> {
        self.conn.convert_selection(
            self.window,
            self.atoms.CLIPBOARD,
            self.atoms.UTF8_STRING,
            self.atoms.XSEL_DATA,
            x11rb::CURRENT_TIME,
        )?;
        Ok(())
    }

    fn handle_xevent(&mut self, event: XEvent) -> Result<Option<Event>> {
        match event {
            XEvent::Error(error) => {
                return Err(self.conn.xerr.from(error)).context("error event");
            }
            XEvent::Expose(expose_event) => {
                if expose_event.count > 0 {
                    return Ok(None);
                }

                self.backbuffer.present()?;

                if !self.first_expose_received {
                    debug!(
                        "time until first expose {}ms",
                        self.startup_time.elapsed().as_millis()
                    );
                    self.first_expose_received = true;

                    if self.grab_keyboard {
                        let grabbed = self
                            .conn
                            .grab_keyboard(
                                false,
                                self.window,
                                x11rb::CURRENT_TIME,
                                xproto::GrabMode::ASYNC,
                                xproto::GrabMode::ASYNC,
                            )?
                            .reply()
                            .map_xerr(self.conn)?
                            .status;
                        if matches!(grabbed, xproto::GrabStatus::SUCCESS) {
                            self.keyboard_grabbed = true;
                            debug!("keyboard grab succeeded");
                        } else {
                            warn!("keyboard grab failed: {:?}", grabbed);
                        }
                    }
                }
            }
            XEvent::ConfigureNotify(ev) => {
                if self.width != ev.width || self.height != ev.height {
                    trace!("resize event w: {}, h: {}", ev.width, ev.height);
                    self.width = ev.width;
                    self.height = ev.height;
                    self.backbuffer.resize_requested = Some((ev.width, ev.height));
                    return Ok(Some(Event::PendingUpdate));
                }
            }
            // minimized
            XEvent::UnmapNotify(..) => {}
            // unminimized
            XEvent::MapNotify(..) => {}
            XEvent::ReparentNotify(..) => {}
            XEvent::MotionNotify(me) => {
                if !me.same_screen {
                    trace!("not same screen");
                    return Ok(None);
                }
                let (x, y) = self
                    .backbuffer
                    .cr
                    .device_to_user(me.event_x as f64, me.event_y as f64)
                    .expect("cairo device_to_user");
                return Ok(Some(Event::Motion { x, y }));
            }
            // both events have the same structure
            XEvent::ButtonPress(bp) | XEvent::ButtonRelease(bp) => {
                let isrelease = matches!(event, XEvent::ButtonRelease(_));
                trace!(
                    "button {}: {:?}",
                    if isrelease { "release" } else { "press" },
                    bp
                );
                if !bp.same_screen {
                    trace!("not same screen");
                    return Ok(None);
                }
                let (x, y) = self
                    .backbuffer
                    .cr
                    .device_to_user(bp.event_x as f64, bp.event_y as f64)
                    .expect("cairo device_to_user");
                return Ok(Some(Event::ButtonPress {
                    button: bp.detail.into(),
                    x,
                    y,
                    isrelease,
                }));
            }
            XEvent::KeyRelease(..) => {}
            XEvent::KeyPress(key_press) => {
                return Ok(Some(Event::KeyPress(Keypress { key_press })));
            }
            XEvent::SelectionNotify(sn) => {
                if sn.property == x11rb::NONE {
                    warn!("selection failed?");
                } else {
                    let reply = self
                        .conn
                        .get_property(
                            false,
                            sn.requestor,
                            sn.property,
                            xproto::GetPropertyType::ANY,
                            0,
                            u32::MAX,
                        )?
                        .reply()
                        .map_xerr(self.conn)?;
                    if reply.format != 8 {
                        warn!("invalid selection format {}", reply.format);
                    // TODO
                    } else if reply.type_ == self.atoms.INCR {
                        warn!("Data too large and INCR mechanism not implemented");
                    } else {
                        match String::from_utf8(reply.value) {
                            Err(err) => {
                                warn!("selection not valid utf8: {}", err);
                                err.into_bytes().zeroize();
                            }
                            Ok(val) => {
                                return Ok(Some(Event::Paste(val)));
                            }
                        }
                    }
                }
            }
            XEvent::FocusIn(..) => {
                return Ok(Some(Event::Focus(true)));
            }
            XEvent::FocusOut(fe) => {
                if fe.mode != xproto::NotifyMode::GRAB
                    && fe.mode != xproto::NotifyMode::WHILE_GRABBED
                {
                    return Ok(Some(Event::Focus(false)));
                }
            }
            XEvent::ClientMessage(client_message) => {
                debug!("client message");
                if client_message.format == 32
                    && client_message.data.as_data32()[0] == self.atoms.WM_DELETE_WINDOW
                {
                    debug!("close requested");
                    return Ok(Some(Event::Exit));
                }
            }
            XEvent::PresentIdleNotify(ev) => {
                if self.backbuffer.on_idle_notify(&ev) {
                    return Ok(Some(Event::PendingUpdate));
                }
            }
            XEvent::PresentCompleteNotify(ev) => {
                return Ok(Some(Event::VsyncCompleted(
                    self.backbuffer.on_vsync_completed(ev),
                )));
            }
            XEvent::XkbStateNotify(key) => {
                self.keyboard.update_mask(&key);
            }
            // TODO needs more testing
            XEvent::XkbNewKeyboardNotify(..) => {
                debug!("xkb new keyboard notify");
                self.keyboard.reload_keymap();
            }
            // TODO needs more testing
            XEvent::XkbMapNotify(..) => {
                debug!("xkb map notify");
                self.keyboard.reload_keymap();
            }
            event => {
                debug!("unexpected event {:?}", event);
            }
        }
        Ok(None)
    }
}

impl<'a> Drop for XContext<'a> {
    fn drop(&mut self) {
        if self.keyboard_grabbed {
            if let Err(err) = self.conn.ungrab_keyboard(x11rb::CURRENT_TIME) {
                debug!("ungrab keyboard failed: {}", err);
            }
        }
        debug!("dropping XContext");
        if let Err(err) = self.conn.destroy_window(self.window) {
            debug!("destroy window failed: {}", err);
        }
        if self.own_colormap {
            if let Err(err) = self.conn.free_colormap(self.colormap) {
                debug!("free colormap failed: {}", err);
            }
        }
        if let Some(cursor) = self.input_cursor {
            if let Err(err) = self.conn.free_cursor(cursor) {
                debug!("free cursor failed: {}", err);
            }
        }
        if let Err(err) = self.conn.flush() {
            debug!("conn flush failed: {}", err);
        }
    }
}
