use std::convert::TryInto;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::os::unix::ffi::OsStrExt as _;

use log::{debug, trace};
use x11rb::connection::RequestConnection;
use x11rb::protocol::xkb::{self as xkbrb, ConnectionExt as _};

use crate::errors::{bail, Unsupported};

mod ffi;
pub mod ffi_keysyms;
pub mod ffi_names;

pub use ffi::xkb_compose_feed_result;
pub use ffi::xkb_compose_status;
pub use ffi::xkb_state_component;
pub use ffi_keysyms as keysyms;
pub use ffi_names as names;

pub type Keycode = ffi::xkb_keycode_t;
pub type Keysym = ffi::xkb_keysym_t;

use crate::errors::{Result, X11ErrorString as _};

pub struct Keyboard<'a> {
    state: *mut ffi::xkb_state,
    context: *mut ffi::xkb_context,
    pub(super) compose: Option<Compose>,
    map_parts: u16,
    events: u16,
    conn: &'a crate::Connection,
}

impl<'a> Keyboard<'a> {
    pub fn new(conn: &'a crate::Connection) -> Result<Self> {
        conn.extension_information(xkbrb::X11_EXTENSION_NAME)?
            .ok_or_else(|| Unsupported("x11 xkb extension required".into()))?;
        let xkb_use = conn
            .xkb_use_extension(
                ffi::XKB_X11_MIN_MAJOR_XKB_VERSION as u16,
                ffi::XKB_X11_MIN_MINOR_XKB_VERSION as u16,
            )?
            .reply()
            .map_xerr(conn)?;
        if !xkb_use.supported {
            bail!(Unsupported("too old xkb?".into()));
        }

        let map_parts = xkbrb::MapPart::KEY_TYPES
            | xkbrb::MapPart::KEY_SYMS
            | xkbrb::MapPart::MODIFIER_MAP
            | xkbrb::MapPart::EXPLICIT_COMPONENTS
            | xkbrb::MapPart::KEY_ACTIONS
            | xkbrb::MapPart::KEY_BEHAVIORS
            | xkbrb::MapPart::VIRTUAL_MODS
            | xkbrb::MapPart::VIRTUAL_MOD_MAP;

        let events = xkbrb::EventType::NEW_KEYBOARD_NOTIFY
            | xkbrb::EventType::MAP_NOTIFY
            | xkbrb::EventType::STATE_NOTIFY;
        //let events = 0xFFF; //XkbAllEventsMask

        conn.xkb_select_events(
            xkbrb::ID::USE_CORE_KBD.into(),
            0u16,
            events,
            map_parts,
            map_parts,
            &xkbrb::SelectEventsAux::new(),
        )?;

        let context = unsafe { ffi::xkb_context_new(ffi::xkb_keysym_flags::XKB_KEYSYM_NO_FLAGS) };
        if context.is_null() {
            panic!("xkb context creation failed");
        }

        let compose = match Compose::new(context) {
            Err(err) => {
                debug!("compose: {}", err);
                None
            }
            Ok(compose) => Some(compose),
        };

        let state = Self::create_xkb_state(conn, context);

        let me = Self {
            state,
            context,
            map_parts: map_parts.into(),
            events: events.into(),
            compose,
            conn,
        };
        Ok(me)
    }

    pub fn create_xkb_state(
        conn: &crate::Connection,
        context: *mut ffi::xkb_context,
    ) -> *mut ffi::xkb_state {
        let device_id = unsafe {
            ffi::xkb_x11_get_core_keyboard_device_id(conn.get_raw_xcb_connection() as *mut _)
        };
        if device_id == -1 {
            panic!("xkb get core keyboard device id failed");
        }
        let keymap = unsafe {
            ffi::xkb_x11_keymap_new_from_device(
                context,
                conn.get_raw_xcb_connection() as *mut _,
                device_id,
                ffi::xkb_keymap_compile_flags::XKB_KEYMAP_COMPILE_NO_FLAGS,
            )
        };
        if keymap.is_null() {
            panic!("xkb keymap creation failed");
        };
        let state = unsafe {
            ffi::xkb_x11_state_new_from_device(
                keymap,
                conn.get_raw_xcb_connection() as *mut _,
                device_id,
            )
        };

        // xkb_keymap is no longer referenced directly
        unsafe { ffi::xkb_keymap_unref(keymap) }

        if state.is_null() {
            panic!("xkb state creation failed");
        };

        state
    }

    pub fn reload_keymap(&mut self) {
        unsafe { ffi::xkb_state_unref(self.state) }
        self.state = Self::create_xkb_state(self.conn, self.context);
    }

    pub fn key_get_utf8(&self, key: Keycode, buf: &mut [u8]) -> usize {
        unsafe {
            ffi::xkb_state_key_get_utf8(
                self.state,
                key,
                buf.as_mut_ptr() as *mut c_char,
                buf.len().try_into().unwrap(),
            )
            .try_into()
            .unwrap()
        }
    }

    pub fn key_get_one_sym(&self, key: Keycode) -> Keysym {
        unsafe { ffi::xkb_state_key_get_one_sym(self.state, key) }
    }

    pub fn mod_name_is_active(&self, modifier: &[u8], mod_type: xkb_state_component::Type) -> bool {
        unsafe {
            ffi::xkb_state_mod_name_is_active(self.state, modifier as *const _ as _, mod_type) == 1
        }
    }

    pub fn update_mask(&mut self, ev: &xkbrb::StateNotifyEvent) {
        trace!("update mask");
        unsafe {
            ffi::xkb_state_update_mask(
                self.state,
                ev.base_mods as u32,
                ev.latched_mods as u32,
                ev.locked_mods as u32,
                ev.base_group.try_into().unwrap(),
                ev.latched_group.try_into().unwrap(),
                ev.locked_group.into(),
            );
        };
    }
}

impl<'a> Drop for Keyboard<'a> {
    fn drop(&mut self) {
        debug!("dropping keyboard");
        if let Err(err) = self.conn.xkb_select_events(
            xkbrb::ID::USE_CORE_KBD.into(),
            !0u16,                          // clear
            self.events,                    // select_all
            self.map_parts,                 // affect_map
            self.map_parts,                 // map
            &xkbrb::SelectEventsAux::new(), // details TODO like affect (a mask) except automatically set to include the flags in select_all and clear
        ) {
            debug!("clear xkb_select_events failed: {}", err);
        }
        unsafe { ffi::xkb_state_unref(self.state) }
        unsafe { ffi::xkb_context_unref(self.context) }
    }
}

pub struct Compose {
    state: *mut ffi::xkb_compose_state,
}

impl Compose {
    fn new(context: *mut ffi::xkb_context) -> Result<Self> {
        debug!("loading compose table");
        let locale = ["LC_ALL", "LC_CTYPE", "LANG"].iter().find_map(|l| {
            if let Some(locale) = std::env::var_os(l) {
                let bytes = locale.as_bytes();
                if !bytes.is_empty() {
                    return CString::new(bytes).ok();
                }
            }
            None
        });
        let compose_table = unsafe {
            ffi::xkb_compose_table_new_from_locale(
                context,
                locale
                    .as_deref()
                    .map(CStr::as_ptr)
                    .unwrap_or(b"C\0".as_ptr() as _),
                ffi::xkb_compose_compile_flags::XKB_COMPOSE_COMPILE_NO_FLAGS,
            )
        };
        if compose_table.is_null() {
            bail!("xkb_compose_table_new_from_locale failed");
        }

        let state = unsafe {
            ffi::xkb_compose_state_new(
                compose_table,
                ffi::xkb_compose_state_flags::XKB_COMPOSE_STATE_NO_FLAGS,
            )
        };
        if state.is_null() {
            bail!("xkb_compose_state_new failed");
        }

        unsafe { ffi::xkb_compose_table_unref(compose_table) }
        debug!("compose table loaded");

        Ok(Self { state })
    }

    pub fn state_feed(&self, key_sym: Keysym) -> xkb_compose_feed_result::Type {
        unsafe { ffi::xkb_compose_state_feed(self.state, key_sym) }
    }

    pub fn state_get_status(&self) -> xkb_compose_status::Type {
        unsafe { ffi::xkb_compose_state_get_status(self.state) }
    }

    pub fn state_get_one_sym(&self) -> Keysym {
        unsafe { ffi::xkb_compose_state_get_one_sym(self.state) }
    }

    pub fn state_reset(&self) {
        unsafe { ffi::xkb_compose_state_reset(self.state) }
    }

    pub fn compose_state_get_utf8(&self, buf: &mut [u8]) -> usize {
        unsafe {
            ffi::xkb_compose_state_get_utf8(
                self.state,
                buf.as_mut_ptr() as *mut c_char,
                buf.len().try_into().unwrap(),
            )
            .try_into()
            .unwrap()
        }
    }
}

impl Drop for Compose {
    fn drop(&mut self) {
        debug!("dropping compose");
        unsafe { ffi::xkb_compose_state_unref(self.state) }
    }
}
