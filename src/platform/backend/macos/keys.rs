//! Raw key events on macOS: a [`KeyEvent`] for every key-down, key-up and
//! modifier change (`NSEventTypeFlagsChanged`, where a modifier key alone
//! goes down or up).
//!
//! `keyCode` gives the physical key, the device-dependent bits of
//! `modifierFlags` the left/right modifiers, `isARepeat` the repeat flag,
//! `characters` the text and `charactersIgnoringModifiers` the text with
//! only Shift applied. With an Option key held that acts as Meta (see
//! [`OptionAsMeta`](crate::platform::OptionAsMeta)), the text is the latter:
//! Option+B gives `b`, not `∫`.

use super::{CTX, Id, event_string, msg_bool, msg_u16, msg_u64, sel};
use crate::platform::event::ButtonState;
use crate::platform::input;
use crate::platform::key::{KeyEvent, PhysicalKey, key_text};

/// `NSEventTypeFlagsChanged`.
pub(super) const NS_FLAGS_CHANGED: u64 = 12;
const NS_KEY_DOWN: u64 = 10;
const NS_KEY_UP: u64 = 11;

/// The raw key event for a key-down, key-up or flags-changed `NSEvent` of
/// type `etype`; `None` for other events, and for a flags change that isn't
/// a modifier key's.
///
/// # Safety
/// `ev` must be a live `NSEvent` of type `etype`.
pub(super) unsafe fn raw_event(etype: u64, ev: Id) -> Option<KeyEvent> {
    let kc = unsafe { msg_u16(ev, sel("keyCode")) };
    let flags = unsafe { msg_u64(ev, sel("modifierFlags")) };
    let state = match etype {
        NS_KEY_DOWN => ButtonState::Pressed,
        NS_KEY_UP => ButtonState::Released,
        NS_FLAGS_CHANGED => input::mac_flags_changed(kc, flags)?,
        _ => return None,
    };
    let modifiers = input::mac_modifiers(flags);
    let sides = input::mac_sides(flags);
    let physical = PhysicalKey::from_mac_keycode(kc);
    // A flags-changed event has no characters (asking raises) and no repeat.
    let is_key = etype != NS_FLAGS_CHANGED;
    let ignoring = if is_key {
        unsafe { event_string(ev, "charactersIgnoringModifiers") }
    } else {
        None
    };
    let logical = input::raw_logical(
        input::mac_keycode(kc),
        physical,
        ignoring.as_deref(),
        u32::from(kc),
    );
    let mut key = KeyEvent::new(state, logical, physical);
    key.modifiers = modifiers;
    key.sides = sides;
    key.scancode = u32::from(kc);
    if is_key && state == ButtonState::Pressed {
        key.repeat = unsafe { msg_bool(ev, sel("isARepeat")) };
        let meta = option_is_meta(&key);
        key.text_ignoring_modifiers = ignoring.as_deref().and_then(key_text);
        key.text = if modifiers.ctrl || modifiers.meta || meta {
            key.text_ignoring_modifiers.clone()
        } else {
            unsafe { event_string(ev, "characters") }
                .as_deref()
                .and_then(key_text)
        };
    }
    Some(key)
}

/// Whether `key` is held with an Option key acting as Meta, so it skips the
/// input method.
pub(super) fn option_is_meta(key: &KeyEvent) -> bool {
    key.modifiers.alt && CTX.with(|c| c.borrow().option_as_meta).applies(key.sides)
}
