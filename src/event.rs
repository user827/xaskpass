use anyhow::Context;
use log::{debug, trace, warn};
use tokio::io::unix::AsyncFd;
use tokio::time::Instant;
use x11rb::connection::Connection as _;
use x11rb::connection::RequestConnection;
use x11rb::protocol::xfixes::{self, ConnectionExt as _};
use x11rb::protocol::xproto::EventMask;
use x11rb::protocol::xproto::{self, ConnectionExt as _, CursorWrapper, WindowWrapper};
use x11rb::protocol::Event;
use zeroize::Zeroize;

use crate::backbuffer::Backbuffer;
use crate::dialog::{Action, Dialog};
use crate::errors::{Error, Result, Unsupported};
use crate::keyboard::Keyboard;
use crate::secret::Passphrase;
use crate::Connection;

enum State {
    Continue,
    Ready,
    Cancelled,
}

pub struct Config<'a> {
    pub xfd: &'a AsyncFd<Connection>,
    pub backbuffer: Backbuffer<'a>,
    pub window: WindowWrapper<'a, Connection>,
    pub keyboard: Keyboard<'a>,
    pub atoms: crate::AtomCollection,
    pub width: u16,
    pub height: u16,
    pub grab_keyboard: bool,
    pub startup_time: Instant,
    pub input_cursor: Option<CursorWrapper<'a, Connection>>,
    pub compositor_atom: Option<xproto::Atom>,
    pub debug: bool,
    pub cycle_deadline: u128,
    pub root: xproto::Window,
}

#[allow(clippy::struct_excessive_bools)]
pub struct XContext<'a> {
    config: Config<'a>,
    keyboard_grabbed: bool,
    first_expose_received: bool,
    xsel_in_progress: bool,
    xfd_eagain: bool,
    xcb_events_queued_maybe: bool,
    x_unflushed_count: u32,
    max_work_time: u128,
}

impl<'a> Config<'a> {
    pub fn conn(&self) -> &'a Connection {
        self.xfd.get_ref()
    }
}

impl<'a> XContext<'a> {
    pub fn keyboard(&self) -> &Keyboard<'a> {
        &self.config.keyboard
    }

    pub fn conn(&self) -> &'a Connection {
        self.config.conn()
    }

    pub fn new(config: Config<'a>) -> Result<Self> {
        if let Some(compositor_atom) = config.compositor_atom {
            config
                .conn()
                .extension_information(xfixes::X11_EXTENSION_NAME)?
                .ok_or_else(|| Unsupported("x11 xfixes extension required".into()))?;
            let (major, minor) = xfixes::X11_XML_VERSION;
            let version_cookie = config.conn().xfixes_query_version(major, minor)?;
            if log::log_enabled!(log::Level::Debug) {
                let version = version_cookie.reply()?;
                debug!(
                    "xfixes version {}.{}",
                    version.major_version, version.minor_version
                );
            }

            config.conn().xfixes_select_selection_input(
                config.window.window(),
                compositor_atom,
                xfixes::SelectionEventMask::SET_SELECTION_OWNER
                    | xfixes::SelectionEventMask::SELECTION_WINDOW_DESTROY
                    | xfixes::SelectionEventMask::SELECTION_CLIENT_CLOSE,
            )?;
        }
        Ok(Self {
            config,
            keyboard_grabbed: false,
            first_expose_received: false,
            xsel_in_progress: false,
            xfd_eagain: false,
            xcb_events_queued_maybe: true, // assume there are to be safe
            x_unflushed_count: 0,
            max_work_time: 0,
        })
    }

    // Handles an event from X server.
    // TODO errors for discarded replies might still be pending in x11rb/xcb after this returns Ok(None)
    fn xcb_dequeue(&mut self, dialog: &mut Dialog) -> Result<Option<State>> {
        let mut state = None;
        while state.is_none() && self.xcb_dirty() {
            self.xcb_events_queued_maybe = true;
            // poll_for_event might not read from the fd until EAGAIN if there were pending errors
            if let Some(event) = self.conn().poll_for_event()? {
                // TODO after poll_for_event there might be pending errors queued by the xcb
                if self.config.debug {
                    trace!("event {:?}", event);
                }
                state = Some(self.handle_event(dialog, event)?);
            } else {
                self.xfd_eagain = true;
                self.xcb_events_queued_maybe = false;
            }
            if state.is_some() {
                self.x_unflushed_count += 1;
            }
            // Flush once all the events in the queue have been handled
            if (state.is_none() && self.x_unflushed_count > 0) || self.x_unflushed_count > 10 {
                self.flush(dialog)?;
            } else {
                trace!("not flushing");
            }
        }
        Ok(state)
    }

    fn xcb_dirty(&self) -> bool {
        !self.xfd_eagain || self.xcb_events_queued_maybe
    }

    fn flush(&mut self, dialog: &mut Dialog) -> Result<()> {
        trace!("flush after {} events/replies", self.x_unflushed_count);
        // Xcb might queue something on flush and other commands
        self.xcb_events_queued_maybe = true;
        // TODO do not draw if the window is not exposed at all
        self.config.backbuffer.commit(dialog)?;
        self.conn().flush()?;
        self.x_unflushed_count = 0;
        Ok(())
    }

    fn stopwatch_stop(&mut self, timestamp: Instant) {
        let duration = timestamp.elapsed().as_micros();
        if duration > self.max_work_time {
            self.max_work_time = duration;
            debug!("event cycle took {}μs, new max", duration);
        } else {
            trace!(
                "event cycle took {}μs, max {}μs",
                duration,
                self.max_work_time
            );
        }
        if duration > self.config.cycle_deadline {
            warn!("event cycle took {}μs", duration);
        }
    }

    pub async fn run_events(&mut self, mut dialog: Dialog) -> Result<Option<Passphrase>> {
        dialog.init_events();
        self.flush(&mut dialog)?;
        tokio::pin! { let events_ready = self.config.xfd.readable(); }
        let mut xcb_fd_guard = None;
        let mut state = State::Continue;
        while matches!(state, State::Continue) {
            trace!("event loop cycle start: xcb_dirty: {}", self.xcb_dirty());
            tokio::select! {
                action = dialog.handle_events() => {
                    let timestamp = Instant::now();
                    self.flush(&mut dialog)?;
                    if matches!(action, Action::Cancel) {
                        state = State::Cancelled;
                    }
                    self.stopwatch_stop(timestamp);
                }
                events_guard = &mut events_ready, if !self.xcb_dirty() => {
                    trace!("xfd returned ready");
                    self.xfd_eagain = false;
                    xcb_fd_guard = Some(events_guard.context("xfd poll")?);
                    events_ready.set(self.config.xfd.readable());
                }
                _ = async {}, if self.xcb_dirty() => {
                    let timestamp = Instant::now();
                    if let Some(s) = self.xcb_dequeue(&mut dialog)? {
                        state = s;
                    } else {
                        assert!(
                            !self.xcb_dirty(),
                            "nothing found but still dirty: eagain {}, events {}",
                            self.xfd_eagain,
                            self.xcb_events_queued_maybe,
                            );
                        if let Some(ref mut guard) = xcb_fd_guard {
                            guard.clear_ready();
                            trace!("xfd clear ready");
                            xcb_fd_guard = None;
                        }

                    }
                    self.stopwatch_stop(timestamp);
                }
            }
            tokio::task::yield_now().await;
        }
        match state {
            State::Continue => unreachable!(),
            State::Ready => Ok(Some(dialog.indicator.into_pass())),
            State::Cancelled => Ok(None),
        }
    }

    pub fn set_default_cursor(&self) -> Result<()> {
        self.conn().change_window_attributes(
            self.config.window.window(),
            &xproto::ChangeWindowAttributesAux::new().cursor(x11rb::NONE),
        )?;
        Ok(())
    }

    pub fn set_input_cursor(&self) -> Result<()> {
        trace!("set input cursor");
        if let Some(ref cursor) = self.config.input_cursor {
            self.conn().change_window_attributes(
                self.config.window.window(),
                &xproto::ChangeWindowAttributesAux::new().cursor(cursor.cursor()),
            )?;
            trace!("input cursor set");
        }
        Ok(())
    }

    pub fn paste_primary(&mut self) -> Result<()> {
        trace!("PRIMARY selection");
        if self.xsel_in_progress {
            warn!("xsel already in progress");
            return Ok(());
        }
        self.conn().convert_selection(
            self.config.window.window(),
            xproto::AtomEnum::PRIMARY.into(),
            self.config.atoms.UTF8_STRING,
            self.config.atoms.XSEL_DATA,
            x11rb::CURRENT_TIME,
        )?;
        self.xsel_in_progress = true;
        Ok(())
    }

    pub fn paste_clipboard(&mut self) -> Result<()> {
        trace!("CLIPBOARD selection");
        if self.xsel_in_progress {
            warn!("xsel already in progress");
            return Ok(());
        }
        self.conn().convert_selection(
            self.config.window.window(),
            self.config.atoms.CLIPBOARD,
            self.config.atoms.UTF8_STRING,
            self.config.atoms.XSEL_DATA,
            x11rb::CURRENT_TIME,
        )?;
        self.xsel_in_progress = true;
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    fn handle_event(&mut self, dialog: &mut Dialog, event: Event) -> Result<State> {
        match event {
            Event::Error(error) => {
                return Err(Error::X11(error));
            }
            Event::Expose(expose_event) => {
                if expose_event.count > 0 {
                    return Ok(State::Continue);
                }

                self.config.backbuffer.set_exposed();

                if !self.first_expose_received {
                    debug!(
                        "time until first expose {}ms",
                        self.config.startup_time.elapsed().as_millis()
                    );
                    self.first_expose_received = true;
                }

                if self.config.grab_keyboard && !self.keyboard_grabbed {
                    debug!("grabbing keyboard");
                    let gk = self
                        .conn()
                        .grab_keyboard(
                            false,
                            self.config.window.window(),
                            x11rb::CURRENT_TIME,
                            xproto::GrabMode::ASYNC,
                            xproto::GrabMode::ASYNC,
                        )?
                        .reply()?;
                    let grabbed = gk.status;
                    match grabbed {
                        xproto::GrabStatus::SUCCESS => debug!("keyboard grab succeeded"),
                        xproto::GrabStatus::ALREADY_GRABBED => debug!("keyboard already grabbed"),
                        _ => warn!("keyboard grab failed: {:?}", grabbed),
                    }
                }
            }
            Event::ConfigureNotify(ev) => {
                if self.config.width != ev.width || self.config.height != ev.height {
                    trace!("resize event w: {}, h: {}", ev.width, ev.height);
                    self.config.width = ev.width;
                    self.config.height = ev.height;
                    self.config.backbuffer.resize_requested = Some((ev.width, ev.height));
                }
            }
            Event::MotionNotify(me) => {
                if me.same_screen {
                    let (x, y) = self
                        .config
                        .backbuffer
                        .cr
                        .device_to_user(f64::from(me.event_x), f64::from(me.event_y))
                        .expect("cairo device_to_user");
                    dialog.handle_motion(x, y, self)?;
                } else {
                    trace!("not same screen");
                }
            }
            // both events have the same structure
            Event::ButtonPress(bp) | Event::ButtonRelease(bp) => {
                let isrelease = matches!(event, Event::ButtonRelease(_));
                trace!(
                    "button {}: {:?}",
                    if isrelease { "release" } else { "press" },
                    bp
                );
                if !bp.same_screen {
                    trace!("not same screen");
                    return Ok(State::Continue);
                }
                let (x, y) = self
                    .config
                    .backbuffer
                    .cr
                    .device_to_user(f64::from(bp.event_x), f64::from(bp.event_y))
                    .expect("cairo device_to_user");
                let action = dialog.handle_button_press(bp.detail.into(), x, y, isrelease, self)?;
                match action {
                    Action::Ok => return Ok(State::Ready),
                    Action::Cancel => return Ok(State::Cancelled),
                    Action::Nothing => {}
                    _ => unreachable!(),
                }
            }
            Event::KeyPress(key_press) => {
                let action = dialog.handle_key_press(key_press.detail.into(), self)?;
                trace!("action {:?}", action);
                match action {
                    Action::Ok => return Ok(State::Ready),
                    Action::Cancel => return Ok(State::Cancelled),
                    Action::Nothing => {}
                    _ => unreachable!(),
                }
            }
            Event::SelectionNotify(sn) => {
                if !self.xsel_in_progress {
                    warn!("got selection notify but xsel not in progress");
                }
                if sn.property == x11rb::NONE {
                    warn!("invalid selection");
                    self.xsel_in_progress = false;
                    return Ok(State::Continue);
                }
                let selection = self
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
                self.xsel_in_progress = false;
                if selection.format != 8 {
                    warn!("invalid selection format {}", selection.format);
                    return Ok(State::Continue);
                // TODO
                } else if selection.type_ == self.config.atoms.INCR {
                    warn!("Selection too big and INCR selection not implemented");
                    return Ok(State::Continue);
                }
                match String::from_utf8(selection.value) {
                    Err(err) => {
                        warn!("selection is not valid utf8: {}", err);
                        err.into_bytes().zeroize();
                    }
                    Ok(mut val) => {
                        dialog.indicator.pass_insert(&val, true);
                        val.zeroize();
                    }
                }
            }
            Event::FocusIn(fe) => {
                if fe.mode == xproto::NotifyMode::GRAB {
                    self.keyboard_grabbed = true;
                } else if fe.mode == xproto::NotifyMode::UNGRAB {
                    self.keyboard_grabbed = false;
                }
                dialog.indicator.set_focused(true);
            }
            Event::FocusOut(fe) => {
                if fe.mode == xproto::NotifyMode::GRAB {
                    self.keyboard_grabbed = true;
                } else if fe.mode == xproto::NotifyMode::UNGRAB {
                    self.keyboard_grabbed = false;
                }
                if fe.mode != xproto::NotifyMode::GRAB
                    && fe.mode != xproto::NotifyMode::WHILE_GRABBED
                {
                    dialog.indicator.set_focused(false);
                }
            }
            Event::ClientMessage(mut client_message) => {
                trace!("client message");
                if client_message.type_ == self.config.atoms.WM_PROTOCOLS
                    && client_message.format == 32
                {
                    if client_message.data.as_data32()[0] == self.config.atoms.WM_DELETE_WINDOW {
                        debug!("close requested");
                        return Ok(State::Cancelled);
                    } else if client_message.data.as_data32()[0] == self.config.atoms._NET_WM_PING {
                        trace!("ping");
                        client_message.window = self.config.root;
                        self.config.conn().send_event(
                            false,
                            self.config.root,
                            EventMask::STRUCTURE_NOTIFY,
                            client_message,
                        )?;
                    }
                } else {
                    debug!("unknown client message");
                }
            }
            Event::PresentIdleNotify(ev) => {
                self.config.backbuffer.on_idle_notify(&ev);
            }
            Event::PresentCompleteNotify(ev) => {
                self.config.backbuffer.on_vsync_completed(ev);
            }
            Event::XkbStateNotify(key) => {
                self.config.keyboard.update_mask(&key);
            }
            // TODO needs more testing
            Event::XkbNewKeyboardNotify(..) => {
                debug!("xkb new keyboard notify");
                self.config.keyboard.reload_keymap();
            }
            // TODO needs more testing
            Event::XkbMapNotify(..) => {
                debug!("xkb map notify");
                self.config.keyboard.reload_keymap();
            }
            Event::XfixesSelectionNotify(sn) => {
                debug!("selection notify: {:?}", sn);
                dialog.set_transparency(sn.subtype == xfixes::SelectionEvent::SET_SELECTION_OWNER);
            }
            // minimized
            Event::UnmapNotify(..) => {
                debug!("set invisible");
                self.config.backbuffer.visible = false;
            }
            // Ignored events:
            // unminimized
            Event::MapNotify(..) | Event::ReparentNotify(..) | Event::KeyRelease(..) => {
                trace!("ignored event {:?}", event);
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
        if let Some(compositor_atom) = self.config.compositor_atom {
            if let Err(err) = xfixes::select_selection_input(
                self.conn(),
                self.config.window.window(),
                compositor_atom,
                (0_u32).into(),
            ) {
                debug!("clear select selection failed: {}", err);
            }
        }
        debug!("dropping XContext");
        if let Err(err) = self.conn().flush() {
            debug!("conn flush failed: {}", err);
        }
    }
}
