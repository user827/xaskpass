use std::collections::VecDeque;

use log::{debug, trace, warn};
use tokio::io::unix::AsyncFd;
use tokio::time::Instant;
use x11rb::connection::Connection as _;
use x11rb::connection::RequestConnection;
use x11rb::connection::SequenceNumber;
use x11rb::cookie::Cookie;
use x11rb::cookie::PollableCookie;
use x11rb::protocol::xfixes::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{self, ConnectionExt as _, CursorWrapper, WindowWrapper};
use x11rb::protocol::Event;
use x11rb::x11_utils::TryParse;
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

#[derive(Debug)]
enum Reply {
    GrabKeyboard(xproto::GrabKeyboardReply),
    Selection(xproto::GetPropertyReply),
}

enum CookieType<'a> {
    GrabKeyboard(Cookie<'a, Connection, xproto::GrabKeyboardReply>),
    Selection(Cookie<'a, Connection, xproto::GetPropertyReply>),
}

impl<'a> CookieType<'a> {
    fn sequence_number(&self) -> SequenceNumber {
        match self {
            Self::GrabKeyboard(cookie) => cookie.sequence_number(),
            Self::Selection(cookie) => cookie.sequence_number(),
        }
    }

    fn do_poll_reply<R, F, G>(
        cookie: Cookie<'a, Connection, R>,
        ct: F,
        rt: G,
        orig: &mut Option<Self>,
    ) -> Result<Option<Reply>>
    where
        R: TryParse,
        F: FnOnce(Cookie<'a, Connection, R>) -> Self,
        G: FnOnce(R) -> Reply,
    {
        let mut cookie = Some(cookie);
        let reply = cookie.poll_reply()?.map(rt);
        *orig = cookie.map(ct);
        Ok(reply)
    }

    fn poll_reply(me: &mut Option<Self>) -> Result<Option<Reply>> {
        let val = me.take();
        match val {
            Some(CookieType::Selection(cookie)) => {
                Self::do_poll_reply(cookie, Self::Selection, Reply::Selection, me)
            }
            Some(CookieType::GrabKeyboard(cookie)) => {
                Self::do_poll_reply(cookie, Self::GrabKeyboard, Reply::GrabKeyboard, me)
            }
            None => panic!("panic!"),
        }
    }
}

type Callback<'a> = fn(&mut XContext<'a>, &mut Dialog, reply: Reply) -> Result<State>;

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
}

#[allow(clippy::struct_excessive_bools)]
pub struct XContext<'a> {
    pub config: Config<'a>,
    keyboard_grabbed: bool,
    first_expose_received: bool,
    cookies: VecDeque<(CookieType<'a>, Callback<'a>)>,
    grab_keyboard_requested: bool,
    poll_for_event_called: bool,
    /// Whether xfd must have received EAGAIN
    xfd_eagain: bool,
    xsel_in_progress: bool,
}

impl<'a> Config<'a> {
    pub fn conn(&self) -> &'a Connection {
        self.xfd.get_ref()
    }
}

impl<'a> XContext<'a> {
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
            cookies: VecDeque::new(),
            grab_keyboard_requested: false,
            poll_for_event_called: false,
            xfd_eagain: false,
            xsel_in_progress: false,
        })
    }

    // Newest requests (with the highest sequence_number) go front
    fn add_cookie(&mut self, new_cookie: CookieType<'a>, f: Callback<'a>) {
        for (i, (cookie, _)) in self.cookies.iter().enumerate() {
            if new_cookie.sequence_number() > cookie.sequence_number() {
                self.cookies.insert(i, (new_cookie, f));
                return;
            }
        }
        self.cookies.push_back((new_cookie, f));
    }

    fn poll_for_reply(&mut self, dialog: &mut Dialog) -> Result<Option<State>> {
        // It is enough to check the cookie with the smallest sequence number. TODO number
        // wrapping.
        if let Some((cookie, f)) = self.cookies.pop_back() {
            let mut cookie = Some(cookie);
            if let Some(reply) = CookieType::poll_reply(&mut cookie)? {
                if self.config.debug {
                    trace!("reply {:?}", reply);
                }
                return Ok(Some(f(self, dialog, reply)?));
            }
            self.cookies.push_back((cookie.unwrap(), f));
        }
        Ok(None)
    }

    fn xcb_dequeue(&mut self, dialog: &mut Dialog) -> Result<Option<State>> {
        let mut state = None;
        if self.cookies.is_empty() {
            if !self.xfd_eagain {
                if let Some(event) = self.conn().poll_for_event()? {
                    self.poll_for_event_called = true;
                    if self.config.debug {
                        trace!("event {:?}", event);
                    }
                    state = Some(self.handle_event(dialog, event)?);
                }
            }
        } else {
            state = self.poll_for_reply(dialog)?;
            if state.is_none() {
                debug!("poll_for_reply: no replies");
            }
        }
        self.xfd_eagain = true;
        if state.is_none() {
            // poll_for_reply or poll_for_event might have had xcb queue more events
            while let Some(event) = self.conn().poll_for_queued_event()? {
                if self.config.debug {
                    if self.poll_for_event_called {
                        trace!(
                            "queued event {:?}, poll_for_event_called: {}",
                            event,
                            self.poll_for_event_called
                        );
                    } else {
                        debug!(
                            "queued event {:?}, poll_for_event_called: {}",
                            event, self.poll_for_event_called
                        );
                    }
                }
                state = Some(self.handle_event(dialog, event)?);
                if !matches!(state, Some(State::Continue)) {
                    break;
                }
            }
        }
        // We handled an event/reply
        if state.is_some() {
            // TODO do not draw if the window is not exposed at all
            self.config.backbuffer.commit(dialog)?;
            self.conn().flush()?;
            // Once again the xcb might have queued something
        }
        Ok(state)
    }

    pub async fn run_events(&mut self, mut dialog: Dialog) -> Result<Option<Passphrase>> {
        dialog.init_events();
        self.config.backbuffer.commit(&mut dialog)?;
        self.conn().flush()?;
        // Need to see if flush something else caused xcb to queue any events, to handle them
        // before waiting for fd to become readable again.
        let mut state = State::Continue;
        while let Some(s) = self.xcb_dequeue(&mut dialog)? {
            state = s;
            if !matches!(state, State::Continue) {
                break;
            }
        }
        tokio::pin! { let events_ready = self.config.xfd.readable(); }
        // Whether xcb queue might have events or its fd might have input
        let mut xcb_dirty = false;
        let mut xcb_fd_guard = None;
        while matches!(state, State::Continue) {
            tokio::select! {
                action = dialog.handle_events() => {
                    self.config.backbuffer.commit(&mut dialog)?;
                    self.conn().flush()?;
                    xcb_dirty = true;
                    if matches!(action, Action::Cancel) {
                        state = State::Cancelled;
                    }
                }
                events_guard = &mut events_ready, if !xcb_dirty => {
                    events_ready.set(self.config.xfd.readable());
                    xcb_fd_guard = Some(events_guard.unwrap());
                    xcb_dirty = true;
                    self.xfd_eagain = false;
                }
                _ = async {}, if xcb_dirty => {
                    if let Some(s) = self.xcb_dequeue(&mut dialog)? {
                        state = s;
                    } else {
                        xcb_dirty = false;
                        self.poll_for_event_called = false;
                        if let Some(ref mut xcb_fd_guard) = xcb_fd_guard {
                            // Only if we found no queued events, can we be sure that no replies or
                            // events have been queued by backbuffer.commit, handle_event, flush or something else.
                            xcb_fd_guard.clear_ready();
                        }
                    }
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
                    if self.grab_keyboard_requested {
                        debug!("grab keyboard already requested");
                    } else {
                        self.grab_keyboard_requested = true;
                        self.add_cookie(
                            CookieType::GrabKeyboard(self.conn().grab_keyboard(
                                false,
                                self.config.window.window(),
                                x11rb::CURRENT_TIME,
                                xproto::GrabMode::ASYNC,
                                xproto::GrabMode::ASYNC,
                            )?),
                            Self::on_grab_keyboard,
                        );
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
                self.add_cookie(
                    CookieType::Selection(self.conn().get_property(
                        false,
                        sn.requestor,
                        sn.property,
                        xproto::GetPropertyType::ANY,
                        0,
                        u32::MAX,
                    )?),
                    Self::on_selection,
                );
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
            Event::ClientMessage(client_message) => {
                debug!("client message");
                if client_message.format == 32
                    && client_message.data.as_data32()[0] == self.config.atoms.WM_DELETE_WINDOW
                {
                    debug!("close requested");
                    return Ok(State::Cancelled);
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

    #[allow(clippy::unnecessary_wraps)]
    #[allow(clippy::match_wildcard_for_single_variants)]
    #[allow(clippy::needless_pass_by_value)]
    fn on_grab_keyboard(&mut self, _: &mut Dialog, reply: Reply) -> Result<State> {
        match reply {
            Reply::GrabKeyboard(gk) => {
                self.grab_keyboard_requested = false;
                let grabbed = gk.status;
                match grabbed {
                    xproto::GrabStatus::SUCCESS => debug!("keyboard grab succeeded"),
                    xproto::GrabStatus::ALREADY_GRABBED => debug!("keyboard already grabbed"),
                    _ => warn!("keyboard grab failed: {:?}", grabbed),
                }
            }
            _ => unreachable!(),
        }
        Ok(State::Continue)
    }

    #[allow(clippy::unnecessary_wraps)]
    #[allow(clippy::match_wildcard_for_single_variants)]
    #[allow(clippy::needless_pass_by_value)]
    fn on_selection(&mut self, dialog: &mut Dialog, reply: Reply) -> Result<State> {
        if !self.xsel_in_progress {
            warn!("on_selection but xsel not in progress");
        }
        self.xsel_in_progress = false;
        match reply {
            Reply::Selection(selection) => {
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
            _ => unreachable!(),
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
                0_u32,
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
