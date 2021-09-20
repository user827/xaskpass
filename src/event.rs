use log::{debug, trace, warn};
use tokio::io::unix::AsyncFd;
use tokio::time::Instant;
use x11rb::connection::Connection as _;
use x11rb::connection::RequestConnection as _;
use x11rb::protocol::xproto::{self, ConnectionExt as _, CursorWrapper, WindowWrapper};
use x11rb::protocol::xfixes::{self, ConnectionExt as _};
use x11rb::protocol::Event as XEvent;
use zeroize::Zeroize;

use crate::backbuffer::Backbuffer;
use crate::dialog::{Action, Dialog};
use crate::errors::{Error, Result, Unsupported};
use crate::keyboard::{Keyboard, Keycode};
use crate::secret::Passphrase;
use crate::Connection;

enum State {
    Continue,
    Ready,
    Cancelled,
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
    pub xfd: &'a AsyncFd<Connection>,
    pub backbuffer: Backbuffer<'a>,
    pub(super) window: WindowWrapper<'a, Connection>,
    pub keyboard: Keyboard<'a>,
    pub(super) atoms: crate::AtomCollection,
    pub(super) width: u16,
    pub(super) height: u16,
    pub(super) grab_keyboard: bool,
    pub(super) startup_time: Instant,
    pub(super) keyboard_grabbed: bool,
    pub(super) first_expose_received: bool,
    pub(super) input_cursor: Option<CursorWrapper<'a, Connection>>,
    pub(super) compositor_atom: xproto::Atom,
}

impl<'a> XContext<'a> {
    pub fn conn(&self) -> &Connection {
        self.xfd.get_ref()
    }

    pub fn init(&self) -> Result<()> {
        self.conn().extension_information(xfixes::X11_EXTENSION_NAME)?
            .ok_or_else(|| Unsupported("x11 xfixes extension required".into()))?;
        let (major, minor) = xfixes::X11_XML_VERSION;
        let version_cookie = self.conn().xfixes_query_version(major, minor)?;
        if log::log_enabled!(log::Level::Debug) {
            let version = version_cookie.reply()?;
            debug!("xfixes version {}.{}", version.major_version, version.minor_version);
        }

        self.conn().xfixes_select_selection_input(
            self.window.window(),
            self.compositor_atom,
            xfixes::SelectionEventMask::SET_SELECTION_OWNER |
            xfixes::SelectionEventMask::SELECTION_WINDOW_DESTROY |
            xfixes::SelectionEventMask::SELECTION_CLIENT_CLOSE
        )?;
        Ok(())
    }

    pub async fn run_events(&mut self, mut dialog: Dialog) -> Result<Option<Passphrase>> {
        tokio::pin! { let xevents_ready = self.xfd.readable(); }
        dialog.init_events();
        loop {
            self.conn().flush()?;
            tokio::select! {
                action = dialog.handle_events() => {
                    if matches!(action, Action::Cancel) {
                        return Ok(None)
                    }
                }
                xevents_guard = &mut xevents_ready => {
                    if let Some(xevent) = self.conn().poll_for_event()? {
                        //silly!("xevent {:?}", xevent);
                        match self.handle_xevent(&mut dialog, xevent)? {
                            State::Continue => {},
                            State::Ready => { return Ok(Some(dialog.indicator.into_pass())) },
                            State::Cancelled => { return Ok(None) },
                        }
                    } else {
                        xevents_guard.unwrap().clear_ready();
                    }
                    xevents_ready.set(self.xfd.readable());
                }
            }
            if dialog.dirty() {
                self.backbuffer.update(&mut dialog)?;
            } else if self.backbuffer.backbuffer_dirty {
                self.backbuffer.present()?;
            }
            tokio::task::yield_now().await;
        }
    }

    pub fn set_default_cursor(&self) -> Result<()> {
        self.conn().change_window_attributes(
            self.window.window(),
            &xproto::ChangeWindowAttributesAux::new().cursor(x11rb::NONE),
        )?;
        Ok(())
    }

    pub fn set_input_cursor(&self) -> Result<()> {
        trace!("set input cursor");
        if let Some(ref cursor) = self.input_cursor {
            self.conn().change_window_attributes(
                self.window.window(),
                &xproto::ChangeWindowAttributesAux::new().cursor(cursor.cursor()),
            )?;
            trace!("input cursor set");
        }
        Ok(())
    }

    pub fn paste_primary(&self) -> Result<()> {
        trace!("PRIMARY selection");
        self.conn().convert_selection(
            self.window.window(),
            xproto::AtomEnum::PRIMARY.into(),
            self.atoms.UTF8_STRING,
            self.atoms.XSEL_DATA,
            x11rb::CURRENT_TIME,
        )?;
        Ok(())
    }

    pub fn paste_clipboard(&self) -> Result<()> {
        self.conn().convert_selection(
            self.window.window(),
            self.atoms.CLIPBOARD,
            self.atoms.UTF8_STRING,
            self.atoms.XSEL_DATA,
            x11rb::CURRENT_TIME,
        )?;
        Ok(())
    }

    fn handle_xevent(&mut self, dialog: &mut Dialog, event: XEvent) -> Result<State> {
        match event {
            XEvent::Error(error) => {
                return Err(Error::X11(error));
            }
            XEvent::Expose(expose_event) => {
                if expose_event.count > 0 {
                    return Ok(State::Continue);
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
                            .conn()
                            .grab_keyboard(
                                false,
                                self.window.window(),
                                x11rb::CURRENT_TIME,
                                xproto::GrabMode::ASYNC,
                                xproto::GrabMode::ASYNC,
                            )?
                            .reply()?
                            .status;
                        if matches!(grabbed, xproto::GrabStatus::SUCCESS) {
                            // TODO should set_focus(true) if focus event is not implied
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
                    self.backbuffer.update(dialog)?;
                }
            }
            // minimized
            XEvent::UnmapNotify(..) => {}
            // unminimized
            XEvent::MapNotify(..) => {}
            XEvent::ReparentNotify(..) => {}
            XEvent::MotionNotify(me) => {
                if me.same_screen {
                    let (x, y) = self
                        .backbuffer
                        .cr
                        .device_to_user(me.event_x as f64, me.event_y as f64)
                        .expect("cairo device_to_user");
                    dialog.handle_motion(x, y, self)?;
                } else {
                    trace!("not same screen");
                }
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
                } else {
                    let (x, y) = self
                        .backbuffer
                        .cr
                        .device_to_user(bp.event_x as f64, bp.event_y as f64)
                        .expect("cairo device_to_user");
                    let action =
                        dialog.handle_button_press(bp.detail.into(), x, y, isrelease, self)?;
                    match action {
                        Action::Ok => return Ok(State::Ready),
                        Action::Cancel => return Ok(State::Cancelled),
                        Action::Nothing => {}
                        _ => unreachable!(),
                    }
                }
            }
            XEvent::KeyRelease(..) => {}
            XEvent::KeyPress(key_press) => {
                let action = dialog.handle_key_press(Keypress { key_press }, self)?;
                trace!("action {:?}", action);
                match action {
                    Action::Ok => return Ok(State::Ready),
                    Action::Cancel => return Ok(State::Cancelled),
                    Action::Nothing => {}
                    _ => unreachable!(),
                }
            }
            XEvent::SelectionNotify(sn) => {
                if sn.property == x11rb::NONE {
                    warn!("selection failed?");
                } else {
                    let reply = self
                        .conn()
                        .get_property(
                            false,
                            sn.requestor,
                            sn.property,
                            xproto::GetPropertyType::ANY,
                            0,
                            u32::MAX,
                        )?
                        .reply()?;
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
                            Ok(mut val) => {
                                dialog.indicator.pass_insert(&val, true);
                                val.zeroize();
                            }
                        }
                    }
                }
            }
            XEvent::FocusIn(..) => {
                dialog.indicator.set_focused(true);
            }
            XEvent::FocusOut(fe) => {
                if fe.mode != xproto::NotifyMode::GRAB
                    && fe.mode != xproto::NotifyMode::WHILE_GRABBED
                {
                    dialog.indicator.set_focused(false);
                }
            }
            XEvent::ClientMessage(client_message) => {
                debug!("client message");
                if client_message.format == 32
                    && client_message.data.as_data32()[0] == self.atoms.WM_DELETE_WINDOW
                {
                    debug!("close requested");
                    return Ok(State::Cancelled);
                }
            }
            XEvent::PresentIdleNotify(ev) => {
                self.backbuffer.on_idle_notify(&ev);
            }
            XEvent::PresentCompleteNotify(ev) => {
                dialog.on_displayed(self.backbuffer.on_vsync_completed(ev));
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
            XEvent::XfixesSelectionNotify(sn) => {
                debug!("selection notify: {:?}", sn);
                dialog.set_transparency(sn.subtype == xfixes::SelectionEvent::SET_SELECTION_OWNER);
            }
            event => {
                debug!("unexpected event {:?}", event);
            }
        }
        Ok(State::Continue)
    }
}

impl<'a> Drop for XContext<'a> {
    fn drop(&mut self) {
        if self.keyboard_grabbed {
            if let Err(err) = self.conn().ungrab_keyboard(x11rb::CURRENT_TIME) {
                debug!("ungrab keyboard failed: {}", err);
            }
        }
        if let Err(err) = xfixes::select_selection_input(
            self.conn(),
            self.window.window(),
            self.compositor_atom,
            0u32
        ) {
            debug!("clear select selection failed: {}", err);
        }
        debug!("dropping XContext");
        if let Err(err) = self.conn().flush() {
            debug!("conn flush failed: {}", err);
        }
    }
}
