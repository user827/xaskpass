use std::convert::TryInto;
use std::os::raw::c_char;

use anyhow::anyhow;
use log::{debug, trace};
use x11rb::connection::RequestConnection;
use x11rb::protocol::xkb::{self as xkbrb, ConnectionExt as _};
use x11rb::protocol::xproto;

mod ffi;
pub mod ffi_keysyms;
pub mod ffi_names;

pub use ffi::xkb_state_component;
pub use ffi_keysyms as keysyms;
pub use ffi_names as names;

use crate::errors::{Error, Result, X11ErrorString as _};
use crate::secret::SecBuf;

pub struct SecUtf8Mut(SecBuf<u8>);
impl SecUtf8Mut {
    pub fn unsecure(&self) -> &str {
        unsafe { std::str::from_utf8_unchecked(self.0.unsecure()) }
    }
}

pub struct Keyboard<'a> {
    state: *mut ffi::xkb_state,
    context: *mut ffi::xkb_context,
    map_parts: u16,
    events: u16,
    conn: &'a crate::Connection,
}

impl<'a> Keyboard<'a> {
    pub fn new(conn: &'a crate::Connection) -> Result<Self> {
        conn.extension_information(xkbrb::X11_EXTENSION_NAME)?
            .ok_or_else(|| Error::Unsupported("x11 xkb extension required".into()))?;
        let xkb_use = conn
            .xkb_use_extension(
                ffi::XKB_X11_MIN_MAJOR_XKB_VERSION as u16,
                ffi::XKB_X11_MIN_MINOR_XKB_VERSION as u16,
            )?
            .reply()
            .map_xerr(conn)?;
        if !xkb_use.supported {
            return Err(Error::Unsupported("too old xkb?".into()));
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
            return Err(anyhow!("xkb context creation failed").into());
        }

        let state = Self::create_xkb_state(conn, context).map_err(|err| {
            unsafe { ffi::xkb_context_unref(context) }
            err
        })?;

        let me = Self {
            state,
            context,
            map_parts: map_parts.into(),
            events: events.into(),
            conn,
        };
        Ok(me)
    }

    pub fn create_xkb_state(
        conn: &crate::Connection,
        context: *mut ffi::xkb_context,
    ) -> Result<*mut ffi::xkb_state> {
        let device_id = unsafe {
            ffi::xkb_x11_get_core_keyboard_device_id(conn.get_raw_xcb_connection() as *mut _)
        };
        if device_id == -1 {
            return Err(anyhow!("xkb get core keyboard device id failed").into());
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
            return Err(anyhow!("xkb keymap creation failed").into());
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
            return Err(anyhow!("xkb state creation failed").into());
        };

        Ok(state)
    }

    pub fn reload_keymap(&mut self) -> Result<()> {
        unsafe { ffi::xkb_state_unref(self.state) }
        self.state = Self::create_xkb_state(self.conn, self.context)?;
        Ok(())
    }

    pub fn key_get_utf8(&self, key: xproto::Keycode, buf: &mut [u8]) -> usize {
        unsafe {
            ffi::xkb_state_key_get_utf8(
                self.state,
                key as ffi::xkb_keycode_t,
                buf.as_mut_ptr() as *mut c_char,
                buf.len().try_into().unwrap(),
            )
            .try_into()
            .unwrap()
        }
    }

    pub fn secure_key_get_utf8(&self, key: xproto::Keycode) -> SecUtf8Mut {
        let mut buf = SecBuf::new(vec![0; 60]);
        buf.len = self.key_get_utf8(key, buf.buf.unsecure_mut());
        if buf.len > buf.unsecure().len() {
            buf = SecBuf::new(vec![0; buf.len]);
            buf.len = self.key_get_utf8(key, buf.buf.unsecure_mut())
        }
        SecUtf8Mut(buf)
    }

    pub fn key_get_one_sym(&self, key: xproto::Keycode) -> ffi::xkb_keysym_t {
        unsafe { ffi::xkb_state_key_get_one_sym(self.state, key.into()) }
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
