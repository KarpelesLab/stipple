//! Pointer odds and ends for the X11 backend: the double-click settings the
//! X settings daemon publishes (XSETTINGS), the blank cursor that
//! [`CursorIcon::Hidden`](crate::platform::CursorIcon::Hidden) shows, the
//! pointer lock (an active pointer grab, and warping the pointer back to the
//! window's center after each move), and the modifiers and buttons of core
//! pointer events.

use std::io;
use std::time::Duration;

use super::{Conn, finish, intern_atom, rd_u32, refresh::round_trip};
use crate::platform::{Modifiers, PointerButton, ScrollDelta};

const OP_GET_SELECTION_OWNER: u8 = 23;
const OP_CREATE_PIXMAP: u8 = 53;
const OP_FREE_PIXMAP: u8 = 54;
const OP_FREE_GC: u8 = 60;
const OP_POLY_FILL_RECTANGLE: u8 = 70;
const OP_CREATE_CURSOR: u8 = 93;
const OP_GRAB_POINTER: u8 = 26;
const OP_UNGRAB_POINTER: u8 = 27;
const OP_WARP_POINTER: u8 = 41;

/// `EnterNotify` and `LeaveNotify` event codes, and their event-mask bits.
pub(super) const X_ENTER_NOTIFY: u8 = 7;
pub(super) const X_LEAVE_NOTIFY: u8 = 8;
pub(super) const EV_ENTER_WINDOW: u32 = 0x0000_0010;
pub(super) const EV_LEAVE_WINDOW: u32 = 0x0000_0020;

/// The modifiers in a core input event's state mask (offset 28): Shift 1,
/// Control 4, Mod1 (Alt) 8, Mod4 (Super) 0x40.
pub(super) fn event_modifiers(buf: &[u8; 32]) -> Modifiers {
    let mask = u16::from_le_bytes([buf[28], buf[29]]);
    Modifiers {
        shift: mask & 0x0001 != 0,
        ctrl: mask & 0x0004 != 0,
        alt: mask & 0x0008 != 0,
        meta: mask & 0x0040 != 0,
    }
}

/// The button a core `ButtonPress`/`ButtonRelease` detail names (4–7 are the
/// wheel: see [`wheel_button_delta`]).
pub(super) fn button(detail: u8) -> PointerButton {
    match detail {
        1 => PointerButton::Left,
        2 => PointerButton::Middle,
        3 => PointerButton::Right,
        8 => PointerButton::Back,
        9 => PointerButton::Forward,
        n => PointerButton::Other(n as u16),
    }
}

/// The scroll for a core-protocol wheel "button": 4/5 scroll up/down, 6/7
/// left/right, a line per click. `None` for real buttons.
pub(super) fn wheel_button_delta(detail: u8) -> Option<ScrollDelta> {
    let (dx, dy) = match detail {
        4 => (0.0, -1.0),
        5 => (0.0, 1.0),
        6 => (-1.0, 0.0),
        7 => (1.0, 0.0),
        _ => return None,
    };
    Some(ScrollDelta::lines(dx, dy))
}

/// Whether a `LeaveNotify` means the pointer left the window: not when it
/// went into a child window (detail `NotifyInferior`).
pub(super) fn leaves_window(buf: &[u8; 32]) -> bool {
    const NOTIFY_INFERIOR: u8 = 2;
    buf[1] != NOTIFY_INFERIOR
}

/// Read the double-click time and distance from the XSETTINGS manager of
/// screen 0 (`Net/DoubleClickTime`, `Net/DoubleClickDistance`, as GNOME, Xfce
/// and other desktops' settings daemons publish), and record them. Nothing
/// happens without a settings manager. Call before any window exists, as
/// replies are read directly.
pub(super) fn read_double_click(conn: &mut Conn) -> io::Result<()> {
    let selection = intern_atom(conn, b"_XSETTINGS_S0")?;
    let mut req = vec![OP_GET_SELECTION_OWNER, 0, 0, 0];
    req.extend_from_slice(&selection.to_le_bytes());
    let Some(reply) = round_trip(conn, req)? else {
        return Ok(());
    };
    let owner = rd_u32(&reply, 8).unwrap_or(0);
    if owner == 0 {
        return Ok(());
    }
    let property = intern_atom(conn, b"_XSETTINGS_SETTINGS")?;
    let mut req = vec![super::OP_GET_PROPERTY, 0, 0, 0];
    req.extend_from_slice(&owner.to_le_bytes());
    req.extend_from_slice(&property.to_le_bytes());
    req.extend_from_slice(&0u32.to_le_bytes()); // any type
    req.extend_from_slice(&0u32.to_le_bytes()); // offset
    req.extend_from_slice(&(64u32 * 1024).to_le_bytes()); // length, in words
    let Some(reply) = round_trip(conn, req)? else {
        return Ok(());
    };
    let len = rd_u32(&reply, 16).unwrap_or(0) as usize;
    let data = reply.get(32..32 + len).unwrap_or(&[]);
    let time = xsettings_int(data, b"Net/DoubleClickTime")
        .filter(|&ms| ms > 0)
        .map(|ms| Duration::from_millis(ms as u64));
    let distance = xsettings_int(data, b"Net/DoubleClickDistance")
        .filter(|&px| px > 0)
        .map(f64::from);
    super::dbg(format_args!(
        "XSETTINGS double click: {time:?}, {distance:?} px"
    ));
    if time.is_some() || distance.is_some() {
        crate::platform::set_double_click(time, distance);
    }
    Ok(())
}

/// The integer setting `name` in an `_XSETTINGS_SETTINGS` property value:
/// a byte-order byte, 3 pad, a serial and a count, then each setting (type,
/// pad, name length, the name padded to 4, its serial, then an INT32, a
/// length-prefixed padded string, or 4 CARD16s of a color).
pub(super) fn xsettings_int(data: &[u8], name: &[u8]) -> Option<i32> {
    let big = *data.first()? == 1;
    let u16_at = |at: usize| -> Option<u16> {
        let b: [u8; 2] = data.get(at..at + 2)?.try_into().ok()?;
        Some(if big {
            u16::from_be_bytes(b)
        } else {
            u16::from_le_bytes(b)
        })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let b: [u8; 4] = data.get(at..at + 4)?.try_into().ok()?;
        Some(if big {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        })
    };
    let pad = |n: usize| n.div_ceil(4) * 4;
    let count = u32_at(8)?;
    let mut at = 12;
    for _ in 0..count {
        let kind = *data.get(at)?;
        let name_len = u16_at(at + 2)? as usize;
        let this = data.get(at + 4..at + 4 + name_len)?;
        let value = at + 4 + pad(name_len) + 4; // past the serial
        let size = match kind {
            0 => 4,
            1 => 4 + pad(u32_at(value)? as usize),
            2 => 8,
            _ => return None,
        };
        if kind == 0 && this == name {
            return u32_at(value).map(|v| v as i32);
        }
        at = value + size;
    }
    None
}

/// The blank cursor (created the first time it's needed): a 1×1 cursor whose
/// mask is empty, so nothing shows.
pub(super) fn blank_cursor(conn: &mut Conn) -> io::Result<u32> {
    if conn.blank_cursor != 0 {
        return Ok(conn.blank_cursor);
    }
    let root = conn.setup.root;
    // A 1×1 bitmap, cleared through a GC (a new pixmap's content is
    // undefined).
    let pixmap = conn.new_id();
    let mut req = vec![OP_CREATE_PIXMAP, 1 /* depth */, 0, 0];
    req.extend_from_slice(&pixmap.to_le_bytes());
    req.extend_from_slice(&root.to_le_bytes());
    req.extend_from_slice(&1u16.to_le_bytes());
    req.extend_from_slice(&1u16.to_le_bytes());
    conn.send(&finish(req))?;
    let gc = conn.new_id();
    let mut req = vec![super::OP_CREATE_GC, 0, 0, 0];
    req.extend_from_slice(&gc.to_le_bytes());
    req.extend_from_slice(&pixmap.to_le_bytes());
    req.extend_from_slice(&4u32.to_le_bytes()); // GCForeground
    req.extend_from_slice(&0u32.to_le_bytes());
    conn.send(&finish(req))?;
    let mut req = vec![OP_POLY_FILL_RECTANGLE, 0, 0, 0];
    req.extend_from_slice(&pixmap.to_le_bytes());
    req.extend_from_slice(&gc.to_le_bytes());
    for v in [0u16, 0, 1, 1] {
        req.extend_from_slice(&v.to_le_bytes());
    }
    conn.send(&finish(req))?;
    let cursor = conn.new_id();
    let mut req = vec![OP_CREATE_CURSOR, 0, 0, 0];
    req.extend_from_slice(&cursor.to_le_bytes());
    req.extend_from_slice(&pixmap.to_le_bytes()); // source
    req.extend_from_slice(&pixmap.to_le_bytes()); // mask: all clear
    req.extend_from_slice(&[0; 12]); // foreground and background colors
    req.extend_from_slice(&[0; 4]); // hotspot
    conn.send(&finish(req))?;
    let mut req = vec![OP_FREE_GC, 0, 0, 0];
    req.extend_from_slice(&gc.to_le_bytes());
    conn.send(&finish(req))?;
    let mut req = vec![OP_FREE_PIXMAP, 0, 0, 0];
    req.extend_from_slice(&pixmap.to_le_bytes());
    conn.send(&finish(req))?;
    conn.blank_cursor = cursor;
    Ok(cursor)
}

/// A `GrabPointer` request: every pointer event (buttons and motion) to
/// `window` (not owner events), asynchronously, the pointer confined to
/// `window` and showing `cursor`, from now on.
pub(super) fn grab_request(window: u32, cursor: u32) -> Vec<u8> {
    const MASK: u16 =
        (super::EV_BUTTON_PRESS | super::EV_BUTTON_RELEASE | super::EV_POINTER_MOTION) as u16;
    const ASYNC: u8 = 1;
    let mut req = vec![OP_GRAB_POINTER, 0 /* owner-events: no */, 0, 0];
    req.extend_from_slice(&window.to_le_bytes());
    req.extend_from_slice(&MASK.to_le_bytes());
    req.extend_from_slice(&[ASYNC, ASYNC]); // pointer and keyboard modes
    req.extend_from_slice(&window.to_le_bytes()); // confine-to
    req.extend_from_slice(&cursor.to_le_bytes());
    req.extend_from_slice(&0u32.to_le_bytes()); // CurrentTime
    finish(req)
}

/// An `UngrabPointer` request, now.
pub(super) fn ungrab_request() -> Vec<u8> {
    let mut req = vec![OP_UNGRAB_POINTER, 0, 0, 0];
    req.extend_from_slice(&0u32.to_le_bytes()); // CurrentTime
    finish(req)
}

/// A `WarpPointer` request moving the pointer to `(x, y)` in `window`,
/// wherever it is.
pub(super) fn warp_request(window: u32, x: i16, y: i16) -> Vec<u8> {
    let mut req = vec![OP_WARP_POINTER, 0, 0, 0];
    req.extend_from_slice(&0u32.to_le_bytes()); // src-window: None
    req.extend_from_slice(&window.to_le_bytes());
    req.extend_from_slice(&[0; 8]); // src x, y, width, height
    req.extend_from_slice(&x.to_le_bytes());
    req.extend_from_slice(&y.to_le_bytes());
    finish(req)
}

/// Grab the pointer for `window`, showing `cursor` (the blank one), and
/// wait for the answer. Events that come first are kept in `stray`, for the
/// event loop. Returns whether the grab took (`GrabSuccess`; not when another
/// client holds a grab, or the window isn't viewable).
pub(super) fn grab(
    conn: &mut Conn,
    window: u32,
    cursor: u32,
    stray: &mut Vec<[u8; 32]>,
) -> io::Result<bool> {
    conn.send(&grab_request(window, cursor))?;
    loop {
        let mut packet = [0u8; 32];
        io::Read::read_exact(&mut conn.stream, &mut packet)?;
        match packet[0] {
            1 => {
                super::dbg(format_args!("GrabPointer status {}", packet[1]));
                return Ok(packet[1] == 0);
            }
            0 if packet[10] == OP_GRAB_POINTER => return Ok(false),
            _ => stray.push(packet),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_lock_requests_are_encoded() {
        let grab = grab_request(0x0040_0001, 0x0040_0009);
        assert_eq!(grab.len(), 24);
        assert_eq!(&grab[..4], &[26, 0, 6, 0]);
        assert_eq!(&grab[4..8], &0x0040_0001u32.to_le_bytes());
        // ButtonPress | ButtonRelease | PointerMotion, both modes async.
        assert_eq!(&grab[8..12], &[0x4c, 0, 1, 1]);
        assert_eq!(&grab[12..16], &0x0040_0001u32.to_le_bytes(), "confined");
        assert_eq!(&grab[16..20], &0x0040_0009u32.to_le_bytes());
        assert_eq!(&grab[20..24], &[0; 4]);
        assert_eq!(ungrab_request(), [27, 0, 2, 0, 0, 0, 0, 0]);
        let warp = warp_request(0x0040_0001, 320, -2);
        assert_eq!(warp.len(), 24);
        assert_eq!(&warp[..4], &[41, 0, 6, 0]);
        assert_eq!(&warp[4..8], &[0; 4]);
        assert_eq!(&warp[8..12], &0x0040_0001u32.to_le_bytes());
        assert_eq!(&warp[12..20], &[0; 8]);
        assert_eq!(&warp[20..24], &[0x40, 0x01, 0xfe, 0xff]);
    }

    /// An XSETTINGS blob with a string, a color and two integers.
    fn settings(big: bool) -> Vec<u8> {
        let u16b = |v: u16| {
            if big {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            }
        };
        let u32b = |v: u32| {
            if big {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            }
        };
        let mut d = vec![u8::from(big), 0, 0, 0];
        d.extend(u32b(7)); // serial
        d.extend(u32b(4)); // count
        let mut entry = |kind: u8, name: &[u8], value: &[u8]| {
            d.push(kind);
            d.push(0);
            d.extend(u16b(name.len() as u16));
            d.extend(name);
            while d.len() % 4 != 0 {
                d.push(0);
            }
            d.extend(u32b(0)); // last-change serial
            d.extend(value);
        };
        let mut s = u32b(5).to_vec();
        s.extend(b"Adwaita\0\0\0"[..8].iter());
        entry(1, b"Net/ThemeName", &s);
        entry(2, b"Gtk/Color", &[1, 2, 3, 4, 5, 6, 7, 8]);
        entry(0, b"Net/DoubleClickTime", &u32b(250));
        entry(0, b"Net/DoubleClickDistance", &u32b(8));
        d
    }

    #[test]
    fn xsettings_integers_are_found() {
        for big in [false, true] {
            let d = settings(big);
            assert_eq!(xsettings_int(&d, b"Net/DoubleClickTime"), Some(250));
            assert_eq!(xsettings_int(&d, b"Net/DoubleClickDistance"), Some(8));
            assert_eq!(xsettings_int(&d, b"Net/Missing"), None);
            // The string isn't an integer.
            assert_eq!(xsettings_int(&d, b"Net/ThemeName"), None);
        }
        assert_eq!(xsettings_int(&[], b"x"), None);
        assert_eq!(
            xsettings_int(&settings(false)[..30], b"Net/DoubleClickTime"),
            None
        );
    }

    #[test]
    fn buttons_and_wheel() {
        assert_eq!(button(2), PointerButton::Middle);
        assert_eq!(button(8), PointerButton::Back);
        assert_eq!(button(9), PointerButton::Forward);
        assert_eq!(button(12), PointerButton::Other(12));
        assert_eq!(wheel_button_delta(6), Some(ScrollDelta::lines(-1.0, 0.0)));
        assert_eq!(wheel_button_delta(5), Some(ScrollDelta::lines(0.0, 1.0)));
        assert_eq!(wheel_button_delta(2), None);
        let mut ev = [0u8; 32];
        ev[28] = 0x05; // Shift | Control
        let m = event_modifiers(&ev);
        assert!(m.shift && m.ctrl && !m.alt);
        ev[1] = 2;
        assert!(!leaves_window(&ev));
        ev[1] = 0;
        assert!(leaves_window(&ev));
    }
}
