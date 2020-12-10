use std::time::Duration;

use anyhow::anyhow;
use log::{debug, info, trace, warn};
use tokio::time::{sleep, Instant, Sleep};
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{self, ConnectionExt as _};
use x11rb::protocol::Event;
use zeroize::Zeroize;

use crate::backbuffer::Backbuffer;
use crate::dialog;
use crate::errors::{Result, X11ErrorString as _};
use crate::keyboard::Keyboard;
use crate::secret::{Passphrase, SecBuf};
use crate::{Connection, XId};

pub struct XContext<'a> {
    pub conn: &'a Connection,
    pub backbuffer: Backbuffer<'a>,
    pub window: xproto::Window,
    pub keyboard: Keyboard<'a>,
    pub atoms: crate::AtomCollection,
    pub colormap: XId,
    pub(super) own_colormap: bool,
    pub input_timeout: Option<Duration>,
    pub width: u16,
    pub height: u16,
    pub grab_keyboard: bool,
    pub debug: bool,
}

struct EventContext<'a> {
    input_timeout: Sleep,
    blink_timeout: Sleep,
    show_selection_timeout: Sleep,
    keyboard_grabbed: bool,
    conn: &'a Connection,
    middle_mouse_pressed: bool,
}

impl<'a> Drop for EventContext<'a> {
    fn drop(&mut self) {
        debug!("dropping EventContext");
        if self.keyboard_grabbed {
            if let Err(err) = self.conn.ungrab_keyboard(x11rb::CURRENT_TIME) {
                debug!("ungrab keyboard failed: {}", err);
            }
        }
    }
}

enum State {
    Continue,
    Completed,
    Cancelled,
}

impl<'a> XContext<'a> {
    pub async fn run_xevents<'b>(&mut self) -> Result<Option<Passphrase>> {
        let mut pass = SecBuf::new(vec!['X'; 512]);
        let mut evctx = EventContext {
            input_timeout: sleep(self.input_timeout.unwrap_or_else(|| Duration::from_secs(0))),
            blink_timeout: self.backbuffer.dialog.indicator.init_blink(),
            show_selection_timeout: sleep(Duration::from_millis(0)),
            keyboard_grabbed: false,
            conn: self.conn,
            middle_mouse_pressed: false,
        };

        tokio::pin! { let xfd_readable = self.conn.xfd.readable(); }

        debug!("starting event loop");
        loop {
            self.conn.flush()?;
            tokio::select! {
                _ = &mut evctx.input_timeout, if self.input_timeout.is_some() => {
                    info!("input timeout");
                    return Ok(None)
                }
                _ = &mut evctx.blink_timeout, if self.backbuffer.dialog.indicator.blink_do => {
                    if self.backbuffer.dialog.indicator.on_blink_timeout(&mut evctx.blink_timeout) {
                        self.backbuffer.update()?;
                    }
                }
                _ = &mut evctx.show_selection_timeout, if self.backbuffer.dialog.indicator.show_selection_do => {
                    if self.backbuffer.dialog.indicator.on_show_selection_timeout() {
                        self.backbuffer.update()?;
                    }
                }
                xfd_guard = &mut xfd_readable => {
                    let mut xfd_guard = xfd_guard.unwrap();
                    let xevent = self.conn.poll_for_event()?;
                    match xevent {
                        None => {
                            trace!("poll_for_event not ready");
                            xfd_guard.clear_ready();
                        }
                        Some(xevent) => {
                            let state = self.handle_xevent(&mut evctx, &mut pass, xevent);
                            match state? {
                                State::Completed => return Ok(Some(Passphrase(pass))),
                                State::Cancelled => return Ok(None),
                                // Ensure other tasks have a change after processing each event as
                                // the await for asyncfd does not allow this if the fd is already
                                // readable.
                                // Also do the whole select loop again after this instead of only
                                // polling for all X11 events first as the other futures where cancelled after
                                // select returned here so the yield below does not give them a
                                // chance.
                                State::Continue  => tokio::task::yield_now().await,
                            }
                        }
                    }
                    xfd_readable.set(self.conn.xfd.readable());
                }
            }
        }
    }

    fn handle_xevent(
        &mut self,
        evctx: &mut EventContext,
        pass: &mut SecBuf<char>,
        event: Event,
    ) -> Result<State> {
        match event {
            Event::Error(error) => {
                return Err(anyhow::Error::new(self.conn.xerr.from(error))
                    .context("error event")
                    .into());
            }
            Event::Expose(expose_event) => {
                trace!("EXPOSE");
                if expose_event.count > 0 {
                    return Ok(State::Continue);
                }

                self.backbuffer.present()?;

                if self.grab_keyboard && !evctx.keyboard_grabbed {
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
                        evctx.keyboard_grabbed = true;
                        debug!("keyboard grab succeeded");
                    } else {
                        return Err(anyhow!("keyboard grab failed: {:?}", grabbed).into());
                    }
                }
            }
            Event::ConfigureNotify(ev) => {
                trace!("configure notify");
                if self.width != ev.width || self.height != ev.height {
                    trace!("resized");
                    self.width = ev.width;
                    self.height = ev.height;
                    self.backbuffer.dialog.resize_requested = Some((ev.width, ev.height));
                    self.backbuffer.update()?;
                }
            }
            // minimized
            Event::UnmapNotify(_) => {
                trace!("unmap notify");
            }
            // unminimized
            Event::MapNotify(_) => {
                trace!("map notify");
            }
            Event::ReparentNotify(_) => {
                trace!("reparent notify");
            }
            Event::MotionNotify(me) => {
                trace!("motion notify");
                if !me.same_screen {
                    trace!("not same screen");
                    return Ok(State::Continue);
                }
                let (action, repaint) =
                    self.backbuffer.dialog.handle_motion(me.event_x, me.event_y);
                match action {
                    dialog::Action::Ok => return Ok(State::Completed),
                    dialog::Action::Cancel => return Ok(State::Cancelled),
                    dialog::Action::NoAction => {}
                }
                if repaint {
                    self.backbuffer.update()?;
                }
            }
            // both events have the same structure
            Event::ButtonPress(bp) | Event::ButtonRelease(bp) => {
                let isrelease = matches!(event, Event::ButtonRelease(_));
                trace!("button {}", if isrelease { "release" } else { "press" });
                if !bp.same_screen {
                    trace!("not same screen");
                    return Ok(State::Continue);
                }
                if !isrelease && bp.detail == xproto::ButtonIndex::M2.into() {
                    evctx.middle_mouse_pressed = true;
                } else if evctx.middle_mouse_pressed && bp.detail == xproto::ButtonIndex::M2.into()
                {
                    trace!("PRIMARY selection");
                    evctx.middle_mouse_pressed = false;
                    self.conn.convert_selection(
                        self.window,
                        xproto::AtomEnum::PRIMARY.into(),
                        self.atoms.UTF8_STRING,
                        self.atoms.XSEL_DATA,
                        x11rb::CURRENT_TIME,
                    )?;
                } else if bp.detail != xproto::ButtonIndex::M1.into() {
                    trace!("not the left mouse button");
                } else {
                    let (action, repaint) = self
                        .backbuffer
                        .dialog
                        .handle_button_press(bp.event_x, bp.event_y, isrelease);
                    match action {
                        dialog::Action::Ok => return Ok(State::Completed),
                        dialog::Action::Cancel => return Ok(State::Cancelled),
                        dialog::Action::NoAction => {}
                    }
                    if repaint {
                        self.backbuffer.update()?;
                    }
                }
            }
            Event::KeyRelease(_) => {
                trace!("key release");
            }
            Event::KeyPress(mut key_press) => {
                let buf = self.keyboard.secure_key_get_utf8(key_press.detail);
                let s = buf.unsecure();
                if !s.is_empty() {
                    for letter in s.chars() {
                        if self.debug {
                            debug!("letter: {:?}", letter);
                        } else {
                            debug!("letter");
                        }
                        if let Some(timeout) = self.input_timeout {
                            evctx
                                .input_timeout
                                .reset(Instant::now().checked_add(timeout).unwrap());
                        }
                        match letter {
                            '\r' | '\n' => return Ok(State::Completed),
                            '\x1b' => return Ok(State::Cancelled),
                            // backspace
                            '\x08' | '\x7f' => {
                                if pass.len > 0 {
                                    pass.len -= 1;
                                }
                            }
                            // ctrl-u
                            '\u{15}' => {
                                pass.len = 0;
                            }
                            // ctrl-v
                            '\u{16}' => {
                                self.conn.convert_selection(
                                    self.window,
                                    self.atoms.CLIPBOARD,
                                    self.atoms.UTF8_STRING,
                                    self.atoms.XSEL_DATA,
                                    x11rb::CURRENT_TIME,
                                )?;
                            }
                            l => {
                                pass.buf.unsecure_mut()[pass.len] = l;
                                pass.len += 1;
                                key_press.detail.zeroize();
                            }
                        }
                    }
                    if self
                        .backbuffer
                        .dialog
                        .indicator
                        .passphrase_updated(pass.len)
                    {
                        self.backbuffer.update()?;
                    }
                } else {
                    debug!("key press {}", key_press.detail);
                }
            }
            Event::SelectionNotify(sn) => {
                trace!("selection notify");
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
                            Ok(mut val) => {
                                for l in val.chars() {
                                    pass.buf.unsecure_mut()[pass.len] = l;
                                    pass.len += 1;
                                }
                                val.zeroize();
                                if self
                                    .backbuffer
                                    .dialog
                                    .indicator
                                    .show_selection(pass.len, &mut evctx.show_selection_timeout)
                                {
                                    self.backbuffer.update()?;
                                }
                            }
                        }
                    }
                }
            }
            Event::FocusIn(fe) => {
                trace!("focus in {:?}", fe);
                if self.backbuffer.dialog.indicator.set_focused(true, &mut evctx.blink_timeout) {
                    self.backbuffer.update()?;
                }
            }
            Event::FocusOut(fe) => {
                trace!("focus out {:?}", fe);
                if fe.mode != xproto::NotifyMode::GRAB
                    && fe.mode != xproto::NotifyMode::WHILE_GRABBED
                    && self.backbuffer.dialog.indicator.set_focused(false, &mut evctx.blink_timeout)
                {
                    self.backbuffer.update()?;
                }
            }
            Event::ClientMessage(client_message) => {
                debug!("client message");
                if client_message.format == 32
                    && client_message.data.as_data32()[0] == self.atoms.WM_DELETE_WINDOW
                {
                    debug!("close requested");
                    return Ok(State::Cancelled);
                }
            }
            Event::GeGeneric(ge) => {
                debug!("unknown generic event for extension {}", ge.extension);
            }
            Event::PresentIdleNotify(ein) => {
                self.backbuffer.on_idle_notify(&ein)?;
            }
            Event::PresentCompleteNotify(ein) => {
                trace!("complete notify {}", ein.serial);
                self.backbuffer.on_vsync_completed(ein.serial);
            }
            Event::XkbStateNotify(key) => {
                trace!("xkb state notify");
                self.keyboard.update_mask(&key);
            }
            // TODO needs more testing
            Event::XkbNewKeyboardNotify(_) => {
                debug!("xkb new keyboard notify");
                self.keyboard.reload_keymap()?;
            }
            // TODO needs more testing
            Event::XkbMapNotify(_) => {
                debug!("xkb map notify");
                self.keyboard.reload_keymap()?;
            }
            event => {
                debug!("unexpected event {}", event.response_type());
            }
        }
        Ok(State::Continue)
    }
}

impl<'a> Drop for XContext<'a> {
    fn drop(&mut self) {
        debug!("dropping XContext");
        if let Err(err) = self.conn.destroy_window(self.window) {
            debug!("destroy window failed: {}", err);
        }
        if self.own_colormap {
            if let Err(err) = self.conn.free_colormap(self.colormap) {
                debug!("free colormap failed: {}", err);
            }
        }
        if let Err(err) = self.conn.flush() {
            debug!("conn flush failed: {}", err);
        }
    }
}
