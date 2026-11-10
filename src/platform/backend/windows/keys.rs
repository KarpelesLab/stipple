//! Raw key events on Windows: a [`KeyEvent`] for every `WM_(SYS)KEYDOWN` and
//! `WM_(SYS)KEYUP`, queued before the coarse [`Event::Key`].
//!
//! The scan code (with the extended-key flag) gives the physical key; the
//! text comes from `ToUnicode` with flag 4, which leaves the keyboard state
//! (a pending dead key) alone, so `TranslateMessage` still composes as
//! usual. Keys the IME takes (`VK_PROCESSKEY`) and characters injected with
//! `SendInput` (`VK_PACKET`) aren't raw.
//!
//! When the handler consumes a raw key, the message loop drops the events
//! queued after it for the same key, and [`super::WinCtx::drop_chars`] makes
//! the `WM_CHAR`s `TranslateMessage` posted for it go unheard.

use super::{GetKeyState, Lparam};
use crate::platform::event::{ButtonState, Event, Modifiers};
use crate::platform::input;
use crate::platform::key::{KeyEvent, ModifierSides, PhysicalKey, key_text};

#[link(name = "user32")]
unsafe extern "system" {
    fn GetKeyboardState(state: *mut u8) -> i32;
    fn ToUnicode(vk: u32, scan: u32, state: *const u8, buf: *mut u16, len: i32, flags: u32) -> i32;
}

/// `VK_PROCESSKEY`: the key went to the IME.
const VK_PROCESSKEY: usize = 0xE5;
/// `VK_PACKET`: a character injected by `SendInput`, not a key.
pub(super) const VK_PACKET: usize = 0xE7;
const VK_SHIFT: usize = 0x10;
const VK_CONTROL: usize = 0x11;
const VK_MENU: usize = 0x12;
const VK_CAPITAL: usize = 0x14;
const VK_LCONTROL: usize = 0xA2;
const VK_RCONTROL: usize = 0xA3;
const VK_LMENU: usize = 0xA4;
const VK_RMENU: usize = 0xA5;
/// `ToUnicode`'s "don't change the keyboard state" flag (Windows 10 1607+).
const TU_NO_STATE_CHANGE: u32 = 4;

/// Left/right modifiers held, from `GetKeyState`.
fn sides() -> ModifierSides {
    let down = |vk| unsafe { GetKeyState(vk) } < 0;
    let pairs = [
        (0xA2, ModifierSides::LEFT_CTRL),
        (0xA3, ModifierSides::RIGHT_CTRL),
        (0xA0, ModifierSides::LEFT_SHIFT),
        (0xA1, ModifierSides::RIGHT_SHIFT),
        (0xA4, ModifierSides::LEFT_ALT),
        (0xA5, ModifierSides::RIGHT_ALT),
        (0x5B, ModifierSides::LEFT_META),
        (0x5C, ModifierSides::RIGHT_META),
    ];
    ModifierSides(
        pairs
            .iter()
            .filter(|(vk, _)| down(*vk))
            .fold(0, |bits, (_, side)| bits | side),
    )
}

/// What `vk` types with the keyboard state `state`, without touching the
/// dead-key state. `None` for a dead key or no text.
fn to_unicode(vk: usize, scan: u32, state: &[u8; 256]) -> Option<String> {
    let mut buf = [0u16; 8];
    let n = unsafe {
        ToUnicode(
            vk as u32,
            scan,
            state.as_ptr(),
            buf.as_mut_ptr(),
            buf.len() as i32,
            TU_NO_STATE_CHANGE,
        )
    };
    if n <= 0 {
        return None;
    }
    key_text(&String::from_utf16_lossy(&buf[..n as usize]))
}

/// The raw key event for a `WM_(SYS)KEYDOWN`/`UP` with virtual key `vk` and
/// `lParam` `lp`, or `None` for keys that aren't raw.
pub(super) fn raw_event(
    vk: usize,
    lp: Lparam,
    state: ButtonState,
    modifiers: Modifiers,
) -> Option<Event> {
    if vk == VK_PROCESSKEY || vk == VK_PACKET {
        return None;
    }
    let scan = ((lp >> 16) & 0xff) as u32;
    let extended = lp & (1 << 24) != 0;
    let physical = PhysicalKey::from_win32_scancode(scan, extended);
    let pressed = state == ButtonState::Pressed;
    let (text, ignoring) = if pressed {
        let mut kb = [0u8; 256];
        unsafe { GetKeyboardState(kb.as_mut_ptr()) };
        // Ctrl+Alt is AltGr, which types; Ctrl or Alt alone doesn't change
        // the character.
        let altgr = modifiers.ctrl && modifiers.alt;
        let mut typing = kb;
        if !altgr {
            for vk in [
                VK_CONTROL,
                VK_LCONTROL,
                VK_RCONTROL,
                VK_MENU,
                VK_LMENU,
                VK_RMENU,
            ] {
                typing[vk] = 0;
            }
        }
        let mut shifted = [0u8; 256];
        shifted[VK_SHIFT] = kb[VK_SHIFT] & 0x80;
        shifted[VK_CAPITAL] = kb[VK_CAPITAL] & 0x01;
        (
            to_unicode(vk, scan, &typing),
            to_unicode(vk, scan, &shifted),
        )
    } else {
        (None, None)
    };
    let logical = input::raw_logical(
        input::win32_named(vk),
        physical,
        ignoring.as_deref(),
        scan | if extended { 0xe000 } else { 0 },
    );
    let mut ev = KeyEvent::new(state, logical, physical);
    ev.modifiers = modifiers;
    ev.sides = sides();
    ev.repeat = pressed && lp & (1 << 30) != 0;
    ev.text = text;
    ev.text_ignoring_modifiers = ignoring;
    ev.scancode = scan | if extended { 0xe000 } else { 0 };
    Some(Event::RawKey(ev))
}
