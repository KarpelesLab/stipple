//! Raw key events on X11 ([`Event::RawKey`](crate::platform::Event::RawKey)):
//! the physical key (an evdev keycode is the X keycode minus 8 under every
//! modern server's keymap), the text at the Shift, Caps Lock, Num Lock and
//! AltGr levels of the core keyboard mapping, left/right modifiers, and the
//! repeat flag.
//!
//! The X server repeats a held key by sending a release and a press with the
//! same timestamp. Stipple asks XKB for *detectable* auto-repeat
//! (`XkbPerClientFlags`), which drops the fake releases, so a repeat is a
//! press of a key that is already down; without XKB, a press stamped with the
//! time of the key's release just before it is a repeat.
//!
//! A client that has initialized XKB no longer gets the core
//! `MappingNotify` for a keyboard mapping change unless it asks for XKB's
//! own notifications, so it selects `XkbNewKeyboardNotify` and
//! `XkbMapNotify`, which the loop treats as `MappingNotify`.

use super::{Conn, Keymap, OP_QUERY_EXTENSION, dbg, finish, pad4, rd_u16, rd_u32};
use crate::platform::event::{ButtonState, KeyCode, Modifiers};
use crate::platform::key::{SideTracker, keysym_logical, keysym_text};
use crate::platform::{KeyEvent, PhysicalKey};
use std::io::{self, Read};

/// `XkbUseExtension`, `XkbSelectEvents` and `XkbPerClientFlags` minor
/// opcodes.
const XKB_USE_EXTENSION: u8 = 0;
const XKB_SELECT_EVENTS: u8 = 1;
const XKB_PER_CLIENT_FLAGS: u8 = 21;
/// `XkbNewKeyboardNotify` and `XkbMapNotify`: event types and mask bits.
const XKB_NEW_KEYBOARD_NOTIFY: u8 = 0;
const XKB_MAP_NOTIFY: u8 = 1;
/// `XkbAllMapComponentsMask`.
const XKB_ALL_MAP_COMPONENTS: u16 = 0xff;
/// `XkbUseCoreKbd`.
const XKB_USE_CORE_KBD: u16 = 0x100;
/// `XkbPCF_DetectableAutoRepeatMask`.
const XKB_PCF_DETECTABLE_AUTO_REPEAT: u32 = 1;

/// The core modifier bits: Lock (Caps Lock), Mod2 (Num Lock) and Mod5 (AltGr,
/// `ISO_Level3_Shift`) as every xkb keymap binds them.
const MASK_LOCK: u16 = 0x0002;
const MASK_NUM_LOCK: u16 = 0x0010;
const MASK_ALT_GR: u16 = 0x0080;

/// `XkbUseExtension(1, 0)`, which must come before any other XKB request.
fn use_extension_request(major: u8) -> Vec<u8> {
    let mut req = vec![major, XKB_USE_EXTENSION, 0, 0];
    req.extend_from_slice(&1u16.to_le_bytes());
    req.extend_from_slice(&0u16.to_le_bytes());
    finish(req)
}

/// `XkbSelectEvents` for keyboard and keymap changes (all their details).
fn select_events_request(major: u8) -> Vec<u8> {
    let which = 1u16 << XKB_NEW_KEYBOARD_NOTIFY | 1 << XKB_MAP_NOTIFY;
    let mut req = vec![major, XKB_SELECT_EVENTS, 0, 0];
    for half in [
        XKB_USE_CORE_KBD,
        which,                  // affectWhich
        0,                      // clear
        which,                  // selectAll
        XKB_ALL_MAP_COMPONENTS, // affectMap
        XKB_ALL_MAP_COMPONENTS, // map
    ] {
        req.extend_from_slice(&half.to_le_bytes());
    }
    finish(req)
}

/// `XkbPerClientFlags` turning detectable auto-repeat on for the core
/// keyboard.
fn per_client_flags_request(major: u8) -> Vec<u8> {
    let mut req = vec![major, XKB_PER_CLIENT_FLAGS, 0, 0];
    req.extend_from_slice(&XKB_USE_CORE_KBD.to_le_bytes());
    req.extend_from_slice(&[0, 0]);
    for word in [
        XKB_PCF_DETECTABLE_AUTO_REPEAT, // change
        XKB_PCF_DETECTABLE_AUTO_REPEAT, // value
        0,                              // ctrlsToChange
        0,                              // autoCtrls
        0,                              // autoCtrlValues
    ] {
        req.extend_from_slice(&word.to_le_bytes());
    }
    finish(req)
}

/// The XKB extension as this client uses it.
#[derive(Clone, Copy, Debug)]
pub(super) struct Xkb {
    /// Its first event code.
    first_event: u8,
    /// Whether repeats come without fake releases.
    pub(super) detectable_repeat: bool,
}

impl Xkb {
    /// Whether `buf` is an XKB notification that the keyboard mapping
    /// changed (a new keyboard, or new keysyms).
    pub(super) fn mapping_changed(&self, buf: &[u8; 32]) -> bool {
        buf[0] & 0x7f == self.first_event
            && matches!(buf[1], XKB_NEW_KEYBOARD_NOTIFY | XKB_MAP_NOTIFY)
    }
}

/// Initialize XKB and ask for detectable auto-repeat and for its keymap
/// notifications. Run at startup, before any window exists, as it reads its
/// replies directly. `None` when the server has no XKB (or
/// `STIPPLE_X11_NO_XKB_REPEAT` is set): then the core protocol's
/// `MappingNotify` and fake releases stay.
pub(super) fn init_xkb(conn: &mut Conn) -> io::Result<Option<Xkb>> {
    if std::env::var_os("STIPPLE_X11_NO_XKB_REPEAT").is_some() {
        return Ok(None);
    }
    let name = b"XKEYBOARD";
    let mut req = vec![OP_QUERY_EXTENSION, 0, 0, 0];
    req.extend_from_slice(&(name.len() as u16).to_le_bytes());
    req.extend_from_slice(&[0u8; 2]);
    req.extend_from_slice(name);
    pad4(&mut req);
    conn.send(&finish(req))?;
    let mut reply = [0u8; 32];
    conn.stream.read_exact(&mut reply)?;
    // Reply: present (8), major opcode (9), first event (10).
    if reply[0] != 1 || reply[8] == 0 {
        return Ok(None);
    }
    let (major, first_event) = (reply[9], reply[10]);
    conn.send(&use_extension_request(major))?;
    conn.stream.read_exact(&mut reply)?;
    // Reply: byte 1 = supported.
    if reply[0] != 1 || reply[1] == 0 {
        return Ok(None);
    }
    // No reply to this one; an error would come before the next reply.
    conn.send(&select_events_request(major))?;
    conn.send(&per_client_flags_request(major))?;
    conn.stream.read_exact(&mut reply)?;
    if reply[0] != 1 {
        dbg(format_args!("xkb: error {} setting up", reply[1]));
        return Ok(None);
    }
    // Reply: value (12).
    let detectable_repeat = rd_u32(&reply, 12).unwrap_or(0) & XKB_PCF_DETECTABLE_AUTO_REPEAT != 0;
    dbg(format_args!(
        "xkb: events from {first_event}, detectable auto-repeat: {detectable_repeat}"
    ));
    Ok(Some(Xkb {
        first_event,
        detectable_repeat,
    }))
}

/// What the loop knows about the keyboard between key events: which keys
/// are down, for the repeat flag, and which modifier sides.
pub(super) struct KeyTracker {
    /// Whether XKB drops the fake releases between repeats.
    detectable: bool,
    /// Per keycode: down, and the time of its last press.
    down: [bool; 256],
    pressed_at: [u32; 256],
    /// The last release (keycode, time), for servers without XKB.
    last_release: Option<(u8, u32)>,
    sides: SideTracker,
}

impl KeyTracker {
    pub(super) fn new(detectable: bool) -> KeyTracker {
        KeyTracker {
            detectable,
            down: [false; 256],
            pressed_at: [0; 256],
            last_release: None,
            sides: SideTracker::default(),
        }
    }

    /// Note a key event from the server (not one an input method sent back)
    /// and say whether it is a repeat.
    pub(super) fn observe(&mut self, buf: &[u8; 32]) -> bool {
        let press = buf[0] & 0x7f == super::X_KEY_PRESS;
        let keycode = buf[1] as usize;
        let time = rd_u32(buf, 4).unwrap_or(0);
        if !press {
            self.down[keycode] = false;
            self.last_release = Some((buf[1], time));
            return false;
        }
        let repeat = if self.detectable {
            // A press of a key already down; one whose release we missed
            // (it went to another window) is only a repeat soon after.
            self.down[keycode] && time.wrapping_sub(self.pressed_at[keycode]) < 1000
        } else {
            self.last_release == Some((buf[1], time))
        };
        self.down[keycode] = true;
        self.pressed_at[keycode] = time;
        self.last_release = None;
        repeat
    }

    /// The raw key event for a key press or release `buf`.
    pub(super) fn event(&mut self, keymap: &Keymap, buf: &[u8; 32], repeat: bool) -> KeyEvent {
        let state = if buf[0] & 0x7f == super::X_KEY_PRESS {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        };
        let keycode = buf[1];
        let mask = rd_u16(buf, 28).unwrap_or(0);
        let modifiers = Modifiers {
            shift: mask & 0x0001 != 0,
            ctrl: mask & 0x0004 != 0,
            alt: mask & 0x0008 != 0,
            meta: mask & 0x0040 != 0,
        };
        let physical = PhysicalKey::from_evdev(u32::from(keycode).saturating_sub(8));
        let levels = Levels::of(keymap, keycode);
        let shifted = levels.at(mask, false);
        let mut event = KeyEvent::new(state, keysym_logical(shifted), physical);
        event.modifiers = modifiers;
        event.sides = self.sides.update(physical, state, modifiers);
        event.repeat = repeat && state == ButtonState::Pressed;
        event.scancode = u32::from(keycode);
        if state == ButtonState::Pressed {
            event.text = keysym_text(levels.at(mask, true));
            event.text_ignoring_modifiers = keysym_text(shifted);
        }
        if let KeyCode::Unidentified(_) = event.logical {
            dbg(format_args!("raw key {keycode}: no keysym"));
        }
        event
    }
}

/// A keycode's keysyms in the core mapping: group 1's levels 1 and 2, and
/// (as XKB lays the core mapping out) its levels 3 and 4 in columns 5 and 6.
struct Levels([u32; 4]);

impl Levels {
    fn of(keymap: &Keymap, keycode: u8) -> Levels {
        let col = |c: usize| keymap.keysym_column(keycode, c);
        Levels([col(0), col(1), col(4), col(5)])
    }

    /// The keysym at the level `mask` selects: Shift (inverted by Caps Lock
    /// for letters, by Num Lock on the keypad), and AltGr if `alt_gr` and
    /// the key has a third level. Empty levels fall back to the one below.
    fn at(&self, mask: u16, alt_gr: bool) -> u32 {
        let [l1, l2, l3, l4] = self.0;
        let mut shift = mask & 0x0001 != 0;
        let keypad = (0xff80..=0xffbd).contains(&l2) && (0xff80..=0xffbd).contains(&l1);
        if keypad && mask & MASK_NUM_LOCK != 0 {
            shift = !shift;
        }
        let letter = crate::platform::compose::keysym_to_char(l1)
            .is_some_and(|c| c.is_alphabetic() && c.is_lowercase());
        if letter && mask & MASK_LOCK != 0 {
            shift = !shift;
        }
        let pick = |a: u32, b: u32| if shift && b != 0 { b } else { a };
        if alt_gr && mask & MASK_ALT_GR != 0 && l3 != 0 {
            return pick(l3, l4);
        }
        pick(l1, l2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_event(press: bool, keycode: u8, time: u32, mask: u16) -> [u8; 32] {
        let mut buf = [0u8; 32];
        buf[0] = if press { 2 } else { 3 };
        buf[1] = keycode;
        buf[4..8].copy_from_slice(&time.to_le_bytes());
        buf[28..30].copy_from_slice(&mask.to_le_bytes());
        buf
    }

    /// A keymap with 6 keysyms per keycode, from keycode 8.
    fn keymap(keys: &[(u8, [u32; 6])]) -> Keymap {
        let mut syms = vec![0u32; 6 * 248];
        for (kc, row) in keys {
            let base = (*kc as usize - 8) * 6;
            syms[base..base + 6].copy_from_slice(row);
        }
        Keymap {
            syms,
            per: 6,
            min: 8,
        }
    }

    #[test]
    fn xkb_requests_are_laid_out() {
        let req = use_extension_request(135);
        assert_eq!(req, vec![135, 0, 2, 0, 1, 0, 0, 0]);
        let req = select_events_request(135);
        assert_eq!(&req[..4], &[135, 1, 4, 0]);
        assert_eq!(rd_u16(&req, 4), Some(0x100));
        assert_eq!(rd_u16(&req, 6), Some(3)); // affectWhich
        assert_eq!(rd_u16(&req, 10), Some(3)); // selectAll
        assert_eq!(rd_u16(&req, 14), Some(0xff)); // map
        let xkb = Xkb {
            first_event: 85,
            detectable_repeat: true,
        };
        let mut ev = [0u8; 32];
        ev[0] = 85;
        ev[1] = XKB_MAP_NOTIFY;
        assert!(xkb.mapping_changed(&ev));
        ev[1] = 2; // XkbStateNotify
        assert!(!xkb.mapping_changed(&ev));
        let req = per_client_flags_request(135);
        assert_eq!(&req[..4], &[135, 21, 7, 0]);
        assert_eq!(req.len(), 28);
        assert_eq!(rd_u16(&req, 4), Some(0x100));
        assert_eq!(rd_u32(&req, 8), Some(1));
        assert_eq!(rd_u32(&req, 12), Some(1));
        assert_eq!(rd_u32(&req, 16), Some(0));
    }

    #[test]
    fn repeats_are_presses_of_held_keys() {
        let mut t = KeyTracker::new(true);
        assert!(!t.observe(&key_event(true, 38, 100, 0)));
        assert!(t.observe(&key_event(true, 38, 600, 0)));
        assert!(t.observe(&key_event(true, 38, 640, 0)));
        assert!(!t.observe(&key_event(false, 38, 700, 0)));
        assert!(!t.observe(&key_event(true, 38, 800, 0)));
        // A release that went elsewhere: much later, it's a new press.
        assert!(!t.observe(&key_event(true, 38, 5000, 0)));
        // Without XKB: the fake release shares the press's timestamp.
        let mut t = KeyTracker::new(false);
        assert!(!t.observe(&key_event(true, 38, 100, 0)));
        assert!(!t.observe(&key_event(false, 38, 600, 0)));
        assert!(t.observe(&key_event(true, 38, 600, 0)));
        assert!(!t.observe(&key_event(false, 38, 900, 0)));
        assert!(!t.observe(&key_event(true, 38, 950, 0)));
    }

    #[test]
    fn raw_events_carry_levels_and_physical_keys() {
        // 'a' (keycode 38), '2' with AltGr '²', keypad 1 (87), F1 (67),
        // Shift_R (62).
        let map = keymap(&[
            (38, [0x61, 0x41, 0x61, 0x41, 0xe6, 0xc6]),
            (11, [0x32, 0x40, 0x32, 0x40, 0xb2, 0]),
            (87, [0xff9c, 0xffb1, 0xff9c, 0xffb1, 0, 0]),
            (67, [0xffbe, 0, 0xffbe, 0, 0, 0]),
            (62, [0xffe2, 0, 0xffe2, 0, 0, 0]),
        ]);
        let mut t = KeyTracker::new(true);
        let ev = t.event(&map, &key_event(true, 38, 1, 0), false);
        assert_eq!(ev.logical, KeyCode::Char('a'));
        assert_eq!(ev.physical.name(), Some("KeyA"));
        assert_eq!(ev.text.as_deref(), Some("a"));
        // Shift and Caps Lock.
        let ev = t.event(&map, &key_event(true, 38, 1, 1), false);
        assert_eq!(ev.logical, KeyCode::Char('A'));
        let ev = t.event(&map, &key_event(true, 38, 1, MASK_LOCK), false);
        assert_eq!(ev.text.as_deref(), Some("A"));
        // Ctrl: logical stays the letter, text too (Ctrl isn't applied).
        let ev = t.event(&map, &key_event(true, 38, 1, 4), false);
        assert!(ev.modifiers.ctrl);
        assert_eq!(ev.logical, KeyCode::Char('a'));
        assert_eq!(ev.text.as_deref(), Some("a"));
        // AltGr types level 3; the logical key and unmodified text don't.
        let ev = t.event(&map, &key_event(true, 11, 1, MASK_ALT_GR), false);
        assert_eq!(ev.logical, KeyCode::Char('2'));
        assert_eq!(ev.text.as_deref(), Some("²"));
        assert_eq!(ev.text_ignoring_modifiers.as_deref(), Some("2"));
        // The keypad, with and without Num Lock.
        let ev = t.event(&map, &key_event(true, 87, 1, 0), false);
        assert_eq!(ev.logical, KeyCode::End);
        assert_eq!(ev.location, crate::platform::KeyLocation::Numpad);
        let ev = t.event(&map, &key_event(true, 87, 1, MASK_NUM_LOCK), false);
        assert_eq!(ev.logical, KeyCode::Char('1'));
        assert_eq!(ev.text.as_deref(), Some("1"));
        // F1, released: no text.
        let ev = t.event(&map, &key_event(false, 67, 1, 0), true);
        assert_eq!(ev.logical, KeyCode::F1);
        assert_eq!(ev.state, ButtonState::Released);
        assert!(!ev.repeat && ev.text.is_none());
        // Right Shift is tracked as the right side.
        let ev = t.event(&map, &key_event(true, 62, 1, 0), false);
        assert_eq!(ev.logical, KeyCode::Shift);
        assert!(ev.sides.right_shift() && !ev.sides.left_shift());
        let ev = t.event(&map, &key_event(true, 38, 1, 1), true);
        assert!(ev.sides.right_shift() && ev.repeat);
    }
}
