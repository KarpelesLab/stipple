//! Pure input-translation tables shared by the native backends.
//!
//! The Win32 and AppKit backends only compile on their own targets, so their
//! key and scroll mappings live here, where they build (and are unit-tested) on
//! every host. Each backend reads the raw values from the OS and hands them to
//! these functions.

use crate::geometry::Point;
use crate::platform::event::{KeyCode, Modifiers, ScrollDelta, Touch, TouchPhase};
use crate::platform::window::CursorIcon;

/// Logical pixels scrolled per wheel notch ("line"). Every backend uses the
/// same value, so a mouse wheel scrolls the same distance everywhere.
pub(crate) const LINE_PX: f64 = 40.0;

// ---- Win32 ------------------------------------------------------------------

/// Translate a Win32 virtual-key code (`WM_KEYDOWN`'s `wParam`) into a named
/// [`KeyCode`]. Printable keys arrive as text through `WM_CHAR` instead.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn win32_vk(vk: usize) -> Option<KeyCode> {
    Some(match vk {
        0x08 => KeyCode::Backspace, // VK_BACK
        0x09 => KeyCode::Tab,       // VK_TAB
        0x0D => KeyCode::Enter,     // VK_RETURN
        0x1B => KeyCode::Escape,    // VK_ESCAPE
        0x21 => KeyCode::PageUp,    // VK_PRIOR
        0x22 => KeyCode::PageDown,  // VK_NEXT
        0x23 => KeyCode::End,       // VK_END
        0x24 => KeyCode::Home,      // VK_HOME
        0x25 => KeyCode::ArrowLeft, // VK_LEFT
        0x26 => KeyCode::ArrowUp,   // VK_UP
        0x27 => KeyCode::ArrowRight,
        0x28 => KeyCode::ArrowDown,
        0x2D => KeyCode::Insert,      // VK_INSERT
        0x2E => KeyCode::Delete,      // VK_DELETE
        0x12 => KeyCode::Alt,         // VK_MENU
        0x5D => KeyCode::ContextMenu, // VK_APPS
        0x70 => KeyCode::F1,          // VK_F1 … VK_F12 are consecutive
        0x71 => KeyCode::F2,
        0x72 => KeyCode::F3,
        0x73 => KeyCode::F4,
        0x74 => KeyCode::F5,
        0x75 => KeyCode::F6,
        0x76 => KeyCode::F7,
        0x77 => KeyCode::F8,
        0x78 => KeyCode::F9,
        0x79 => KeyCode::F10,
        0x7A => KeyCode::F11,
        0x7B => KeyCode::F12,
        _ => return None,
    })
}

/// The character a letter or digit virtual key stands for, used for Ctrl/Win
/// shortcuts (`WM_CHAR` only delivers a control code for Ctrl+C). Letters are
/// lower-case unless Shift is held, like the keysym X11 reports.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn win32_shortcut_char(vk: usize, shift: bool) -> Option<char> {
    match vk {
        0x30..=0x39 => Some(vk as u8 as char), // '0'..'9'
        0x41..=0x5A => {
            let c = vk as u8 as char; // 'A'..'Z'
            Some(if shift { c } else { c.to_ascii_lowercase() })
        }
        _ => None,
    }
}

/// The scroll delta for a `WM_MOUSEWHEEL` / `WM_MOUSEHWHEEL` `wParam`. The high
/// word is a signed multiple of `WHEEL_DELTA` (120) per notch, positive away
/// from the user (up) or to the right; precision touchpads send fractions.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn win32_wheel(wp: usize, horizontal: bool) -> ScrollDelta {
    let raw = ((wp >> 16) & 0xffff) as u16 as i16;
    let px = f64::from(raw) / 120.0 * LINE_PX;
    if horizontal {
        ScrollDelta { dx: px, dy: 0.0 }
    } else {
        // Stipple's dy grows downwards; a wheel turned up scrolls towards the top.
        ScrollDelta { dx: 0.0, dy: -px }
    }
}

/// The byte offset in `s` of UTF-16 code-unit index `units`, as IMM32 and
/// AppKit count positions in composition strings. An index inside a surrogate
/// pair, or past the end, rounds up to the next char boundary.
#[cfg_attr(not(any(target_os = "windows", target_os = "macos")), allow(dead_code))]
pub(crate) fn utf16_to_byte(s: &str, units: usize) -> usize {
    let mut seen = 0;
    for (i, c) in s.char_indices() {
        if seen >= units {
            return i;
        }
        seen += c.len_utf16();
    }
    s.len()
}

/// Joins the UTF-16 code units of `WM_CHAR`, which delivers a character
/// outside the Basic Multilingual Plane (an emoji, a rare CJK ideograph) as two
/// messages, one per surrogate.
#[derive(Debug, Default)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) struct Utf16Joiner {
    high: Option<u16>,
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
impl Utf16Joiner {
    /// Feed one code unit; returns the character once it's complete.
    pub(crate) fn push(&mut self, unit: u16) -> Option<char> {
        match unit {
            0xD800..=0xDBFF => {
                self.high = Some(unit);
                None
            }
            0xDC00..=0xDFFF => {
                let high = self.high.take()?;
                char::decode_utf16([high, unit]).next()?.ok()
            }
            _ => {
                self.high = None;
                char::from_u32(unit as u32)
            }
        }
    }
}

// ---- AppKit -----------------------------------------------------------------

/// Translate a macOS virtual key code (`-[NSEvent keyCode]`, the `kVK_*`
/// constants) into a named [`KeyCode`]. Other keys arrive as `characters`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn mac_keycode(kc: u16) -> Option<KeyCode> {
    Some(match kc {
        36 | 76 => KeyCode::Enter, // Return, keypad Enter
        48 => KeyCode::Tab,
        51 => KeyCode::Backspace, // kVK_Delete
        53 => KeyCode::Escape,
        114 => KeyCode::Insert, // kVK_Help sits where Insert is on PC keyboards
        115 => KeyCode::Home,
        116 => KeyCode::PageUp,
        117 => KeyCode::Delete, // kVK_ForwardDelete
        119 => KeyCode::End,
        121 => KeyCode::PageDown,
        122 => KeyCode::F1,
        120 => KeyCode::F2,
        99 => KeyCode::F3,
        118 => KeyCode::F4,
        96 => KeyCode::F5,
        97 => KeyCode::F6,
        98 => KeyCode::F7,
        100 => KeyCode::F8,
        101 => KeyCode::F9,
        109 => KeyCode::F10,
        103 => KeyCode::F11,
        111 => KeyCode::F12,
        123 => KeyCode::ArrowLeft,
        124 => KeyCode::ArrowRight,
        125 => KeyCode::ArrowDown,
        126 => KeyCode::ArrowUp,
        _ => return None,
    })
}

/// Decode `-[NSEvent modifierFlags]`. Command is reported as `meta`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn mac_modifiers(flags: u64) -> Modifiers {
    Modifiers {
        shift: flags & (1 << 17) != 0, // NSEventModifierFlagShift
        ctrl: flags & (1 << 18) != 0,  // NSEventModifierFlagControl
        alt: flags & (1 << 19) != 0,   // NSEventModifierFlagOption
        meta: flags & (1 << 20) != 0,  // NSEventModifierFlagCommand
    }
}

/// The scroll delta for an `NSEventTypeScrollWheel` from its
/// `scrollingDeltaX/Y`. Trackpads and Magic Mice report precise deltas in
/// points (logical pixels); classic wheels report lines. AppKit's deltas are
/// positive towards the top/left (content follows the fingers), the opposite of
/// Stipple's.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn mac_scroll(dx: f64, dy: f64, precise: bool) -> ScrollDelta {
    let unit = if precise { 1.0 } else { LINE_PX };
    ScrollDelta {
        dx: -dx * unit,
        dy: -dy * unit,
    }
}

/// Whether a `characters` string is an AppKit function-key code point
/// (`NSUpArrowFunctionKey` … in U+F700–U+F8FF) rather than typed text.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn mac_is_function_key(s: &str) -> bool {
    s.chars().any(|c| ('\u{F700}'..='\u{F8FF}').contains(&c))
}

// ---- Web --------------------------------------------------------------------

/// Translate a DOM `KeyboardEvent.key` into a [`KeyCode`]. Named keys map to
/// their variants, a single character to [`KeyCode::Char`]; modifier keys on
/// their own and anything else unknown return `None`.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn web_key(key: &str) -> Option<KeyCode> {
    Some(match key {
        "Enter" => KeyCode::Enter,
        "Escape" | "Esc" => KeyCode::Escape,
        "Backspace" => KeyCode::Backspace,
        "Tab" => KeyCode::Tab,
        " " | "Spacebar" => KeyCode::Space,
        "ArrowLeft" | "Left" => KeyCode::ArrowLeft,
        "ArrowRight" | "Right" => KeyCode::ArrowRight,
        "ArrowUp" | "Up" => KeyCode::ArrowUp,
        "ArrowDown" | "Down" => KeyCode::ArrowDown,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        "PageUp" => KeyCode::PageUp,
        "PageDown" => KeyCode::PageDown,
        "Insert" => KeyCode::Insert,
        "Delete" | "Del" => KeyCode::Delete,
        "F1" => KeyCode::F1,
        "F2" => KeyCode::F2,
        "F3" => KeyCode::F3,
        "F4" => KeyCode::F4,
        "F5" => KeyCode::F5,
        "F6" => KeyCode::F6,
        "F7" => KeyCode::F7,
        "F8" => KeyCode::F8,
        "F9" => KeyCode::F9,
        "F10" => KeyCode::F10,
        "F11" => KeyCode::F11,
        "F12" => KeyCode::F12,
        _ => {
            let mut chars = key.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => KeyCode::Char(c),
                _ => return None,
            }
        }
    })
}

/// The modifier bits the web glue packs: 1 Shift, 2 Ctrl, 4 Alt, 8 Meta.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn web_modifiers(bits: u32) -> Modifiers {
    Modifiers {
        shift: bits & 1 != 0,
        ctrl: bits & 2 != 0,
        alt: bits & 4 != 0,
        meta: bits & 8 != 0,
    }
}

/// The scroll delta for a DOM `WheelEvent`: `mode` is its `deltaMode` (0
/// pixels, 1 lines, 2 pages, a page being `page` logical pixels). The DOM's
/// deltas already grow downwards and to the right, as Stipple's do.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn web_wheel(dx: f64, dy: f64, mode: u32, page: f64) -> ScrollDelta {
    let unit = match mode {
        1 => LINE_PX,
        2 => page.max(LINE_PX),
        _ => 1.0,
    };
    ScrollDelta {
        dx: dx * unit,
        dy: dy * unit,
    }
}

/// The CSS `cursor` value for a [`CursorIcon`].
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn css_cursor(icon: CursorIcon) -> &'static str {
    match icon {
        CursorIcon::Default => "default",
        CursorIcon::Text => "text",
        CursorIcon::Pointer => "pointer",
        CursorIcon::Crosshair => "crosshair",
        CursorIcon::NotAllowed => "not-allowed",
        CursorIcon::Grab => "grab",
        CursorIcon::Grabbing => "grabbing",
        CursorIcon::Move => "move",
        CursorIcon::EwResize => "ew-resize",
        CursorIcon::NsResize => "ns-resize",
        CursorIcon::NwseResize => "nwse-resize",
        CursorIcon::NeswResize => "nesw-resize",
    }
}

/// An `NSMenuItem` key equivalent for `shortcut`: the `keyEquivalent` string
/// and its `keyEquivalentModifierMask`. `None` for keys a menu can't show.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn mac_key_equivalent(shortcut: &crate::platform::Shortcut) -> Option<(String, u64)> {
    let function = |n: u32| char::from_u32(0xF704 + n).map(String::from);
    let key = match shortcut.key {
        KeyCode::Char(c) => c.to_lowercase().collect(),
        KeyCode::Space => " ".into(),
        KeyCode::Enter => "\r".into(),
        KeyCode::Escape => "\u{1b}".into(),
        KeyCode::Backspace => "\u{8}".into(),
        KeyCode::Tab => "\t".into(),
        KeyCode::ArrowUp => "\u{F700}".into(),
        KeyCode::ArrowDown => "\u{F701}".into(),
        KeyCode::ArrowLeft => "\u{F702}".into(),
        KeyCode::ArrowRight => "\u{F703}".into(),
        KeyCode::Insert => "\u{F727}".into(),
        KeyCode::Delete => "\u{F728}".into(),
        KeyCode::Home => "\u{F729}".into(),
        KeyCode::End => "\u{F72B}".into(),
        KeyCode::PageUp => "\u{F72C}".into(),
        KeyCode::PageDown => "\u{F72D}".into(),
        KeyCode::F1 => function(0)?,
        KeyCode::F2 => function(1)?,
        KeyCode::F3 => function(2)?,
        KeyCode::F4 => function(3)?,
        KeyCode::F5 => function(4)?,
        KeyCode::F6 => function(5)?,
        KeyCode::F7 => function(6)?,
        KeyCode::F8 => function(7)?,
        KeyCode::F9 => function(8)?,
        KeyCode::F10 => function(9)?,
        KeyCode::F11 => function(10)?,
        KeyCode::F12 => function(11)?,
        KeyCode::Alt | KeyCode::ContextMenu | KeyCode::Unidentified(_) => return None,
    };
    let mut mask = 0;
    if shortcut.shift {
        mask |= 1 << 17; // NSEventModifierFlagShift
    }
    if shortcut.ctrl {
        mask |= 1 << 18; // NSEventModifierFlagControl
    }
    if shortcut.alt {
        mask |= 1 << 19; // NSEventModifierFlagOption
    }
    if shortcut.command {
        mask |= 1 << 20; // NSEventModifierFlagCommand
    }
    Some((key, mask))
}

// ---- Android ----------------------------------------------------------------

/// Translate an Android key code (`AKeyEvent_getKeyCode`, the `AKEYCODE_*`
/// constants) into a named [`KeyCode`]. Keys that type a character are read
/// through the device's key character map instead.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn android_keycode(kc: i32) -> Option<KeyCode> {
    Some(match kc {
        19 => KeyCode::ArrowUp, // AKEYCODE_DPAD_UP
        20 => KeyCode::ArrowDown,
        21 => KeyCode::ArrowLeft,
        22 => KeyCode::ArrowRight,
        61 => KeyCode::Tab,
        66 | 160 => KeyCode::Enter, // ENTER, NUMPAD_ENTER
        67 => KeyCode::Backspace,   // AKEYCODE_DEL
        92 => KeyCode::PageUp,
        93 => KeyCode::PageDown,
        111 => KeyCode::Escape,
        112 => KeyCode::Delete, // AKEYCODE_FORWARD_DEL
        122 => KeyCode::Home,   // AKEYCODE_MOVE_HOME
        123 => KeyCode::End,
        124 => KeyCode::Insert,
        // AKEYCODE_F1 … AKEYCODE_F12 are consecutive.
        131..=142 => ANDROID_FUNCTION_KEYS[(kc - 131) as usize],
        _ => return None,
    })
}

const ANDROID_FUNCTION_KEYS: [KeyCode; 12] = [
    KeyCode::F1,
    KeyCode::F2,
    KeyCode::F3,
    KeyCode::F4,
    KeyCode::F5,
    KeyCode::F6,
    KeyCode::F7,
    KeyCode::F8,
    KeyCode::F9,
    KeyCode::F10,
    KeyCode::F11,
    KeyCode::F12,
];

/// Decode an Android meta state (`AKeyEvent_getMetaState`).
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn android_modifiers(meta: i32) -> Modifiers {
    Modifiers {
        shift: meta & 0x1 != 0,    // AMETA_SHIFT_ON
        alt: meta & 0x2 != 0,      // AMETA_ALT_ON
        ctrl: meta & 0x1000 != 0,  // AMETA_CTRL_ON
        meta: meta & 0x10000 != 0, // AMETA_META_ON
    }
}

/// The character a key types on a US layout, for when the device's key
/// character map can't be read.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn android_ascii(kc: i32, shift: bool) -> Option<char> {
    const SHIFTED_DIGITS: &[u8; 10] = b")!@#$%^&*(";
    let pick = |plain: u8, shifted: u8| Some(if shift { shifted } else { plain } as char);
    match kc {
        7..=16 => {
            let i = (kc - 7) as usize;
            pick(b'0' + i as u8, SHIFTED_DIGITS[i])
        }
        29..=54 => {
            let c = b'a' + (kc - 29) as u8;
            pick(c, c.to_ascii_uppercase())
        }
        17 => Some('*'),
        18 => Some('#'),
        55 => pick(b',', b'<'),
        56 => pick(b'.', b'>'),
        62 => Some(' '),
        68 => pick(b'`', b'~'),
        69 => pick(b'-', b'_'),
        70 => pick(b'=', b'+'),
        71 => pick(b'[', b'{'),
        72 => pick(b']', b'}'),
        73 => pick(b'\\', b'|'),
        74 => pick(b';', b':'),
        75 => pick(b'\'', b'"'),
        76 => pick(b'/', b'?'),
        77 => Some('@'),
        81 => Some('+'),
        _ => None,
    }
}

/// The scroll delta for an `AMOTION_EVENT_ACTION_SCROLL`, from its
/// `AXIS_HSCROLL` / `AXIS_VSCROLL` values: notches, with a positive vertical
/// value meaning the wheel turned up (away from the user).
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn android_scroll(h: f32, v: f32) -> ScrollDelta {
    ScrollDelta {
        dx: f64::from(h) * LINE_PX,
        dy: -f64::from(v) * LINE_PX,
    }
}

/// The touches an `AMotionEvent` from a finger or stylus reports, from its
/// masked action, the action's pointer index, and every pointer's id and
/// position: the finger that went down or lifted, every finger on a move
/// (Android doesn't say which moved), and all of them on a cancel.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn android_touches(action: i32, index: usize, pointers: &[(i32, Point)]) -> Vec<Touch> {
    const DOWN: i32 = 0;
    const UP: i32 = 1;
    const MOVE: i32 = 2;
    const CANCEL: i32 = 3;
    const POINTER_DOWN: i32 = 5;
    const POINTER_UP: i32 = 6;
    let touch = |&(id, position): &(i32, Point), phase| Touch {
        id: id as u64,
        phase,
        position,
    };
    match action {
        DOWN | POINTER_DOWN | UP | POINTER_UP => {
            let phase = if matches!(action, DOWN | POINTER_DOWN) {
                TouchPhase::Started
            } else {
                TouchPhase::Ended
            };
            pointers
                .get(index)
                .map(|p| touch(p, phase))
                .into_iter()
                .collect()
        }
        MOVE => pointers
            .iter()
            .map(|p| touch(p, TouchPhase::Moved))
            .collect(),
        CANCEL => pointers
            .iter()
            .map(|p| touch(p, TouchPhase::Cancelled))
            .collect(),
        _ => Vec::new(),
    }
}

// ---- iOS ---------------------------------------------------------------------

/// The modifiers held, from a `UIKey`'s `modifierFlags`
/// (`UIKeyModifierFlags`, the same bits as AppKit's).
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(crate) fn ios_modifiers(flags: usize) -> Modifiers {
    Modifiers {
        shift: flags & (1 << 17) != 0,
        ctrl: flags & (1 << 18) != 0,
        alt: flags & (1 << 19) != 0,
        meta: flags & (1 << 20) != 0,
    }
}

/// The key a hardware keyboard press (`UIPress.key`) should send as
/// [`Event::Key`](crate::platform::Event::Key), from its HID usage
/// (`keyCode`), `charactersIgnoringModifiers` and modifiers; `None` leaves the
/// press to UIKit's text system, which types it through `UIKeyInput`
/// (`insertText:` for characters, Return and Space, `deleteBackward` for
/// Backspace).
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(crate) fn ios_key(usage: isize, chars: &str, modifiers: Modifiers) -> Option<KeyCode> {
    const F_KEYS: [KeyCode; 12] = [
        KeyCode::F1,
        KeyCode::F2,
        KeyCode::F3,
        KeyCode::F4,
        KeyCode::F5,
        KeyCode::F6,
        KeyCode::F7,
        KeyCode::F8,
        KeyCode::F9,
        KeyCode::F10,
        KeyCode::F11,
        KeyCode::F12,
    ];
    let shortcut = modifiers.ctrl || modifiers.meta;
    let named = match usage {
        0x28 => KeyCode::Enter,
        0x29 => KeyCode::Escape,
        0x2A => KeyCode::Backspace,
        0x2B => KeyCode::Tab,
        0x2C => KeyCode::Space,
        0x3A..=0x45 => F_KEYS[(usage - 0x3A) as usize],
        0x49 => KeyCode::Insert,
        0x4A => KeyCode::Home,
        0x4B => KeyCode::PageUp,
        0x4C => KeyCode::Delete,
        0x4D => KeyCode::End,
        0x4E => KeyCode::PageDown,
        0x4F => KeyCode::ArrowRight,
        0x50 => KeyCode::ArrowLeft,
        0x51 => KeyCode::ArrowDown,
        0x52 => KeyCode::ArrowUp,
        // Shortcuts: a character key with Ctrl or Command.
        _ if shortcut => {
            let c = chars.chars().next()?;
            return (!c.is_control()).then(|| KeyCode::Char(c.to_ascii_lowercase()));
        }
        _ => return None,
    };
    // What the text system types (Return, Space, Backspace) goes through it,
    // unless a shortcut modifier is held.
    let typed = matches!(named, KeyCode::Enter | KeyCode::Space | KeyCode::Backspace);
    (!typed || shortcut).then_some(named)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ios_hardware_keys() {
        let none = Modifiers::default();
        let cmd = ios_modifiers(1 << 20);
        assert!(cmd.meta && !cmd.ctrl && !cmd.shift && !cmd.alt);
        let shift = ios_modifiers(1 << 17);
        assert!(shift.shift);
        assert_eq!(ios_key(0x50, "", none), Some(KeyCode::ArrowLeft));
        assert_eq!(ios_key(0x50, "", shift), Some(KeyCode::ArrowLeft));
        assert_eq!(ios_key(0x2B, "\t", none), Some(KeyCode::Tab));
        assert_eq!(ios_key(0x29, "", none), Some(KeyCode::Escape));
        assert_eq!(ios_key(0x3A, "", none), Some(KeyCode::F1));
        assert_eq!(ios_key(0x45, "", none), Some(KeyCode::F12));
        assert_eq!(ios_key(0x4C, "", none), Some(KeyCode::Delete));
        // Typing goes through UIKeyInput.
        assert_eq!(ios_key(0x04, "a", none), None);
        assert_eq!(ios_key(0x04, "A", shift), None);
        assert_eq!(ios_key(0x28, "\r", none), None);
        assert_eq!(ios_key(0x2A, "", none), None);
        assert_eq!(ios_key(0x2C, " ", none), None);
        // ...except with a shortcut modifier.
        assert_eq!(ios_key(0x06, "c", cmd), Some(KeyCode::Char('c')));
        assert_eq!(ios_key(0x19, "V", cmd), Some(KeyCode::Char('v')));
        assert_eq!(ios_key(0x2A, "", cmd), Some(KeyCode::Backspace));
        // Modifier keys alone send nothing.
        assert_eq!(ios_key(0xE1, "", shift), None);
    }

    #[test]
    fn android_touches_follow_each_finger() {
        let a = (4, Point::new(10.0, 20.0));
        let b = (7, Point::new(30.0, 40.0));
        let t = |id, phase, p: Point| Touch {
            id,
            phase,
            position: p,
        };
        assert_eq!(
            android_touches(0, 0, &[a]),
            [t(4, TouchPhase::Started, a.1)]
        );
        // A second finger: only it starts.
        assert_eq!(
            android_touches(5, 1, &[a, b]),
            [t(7, TouchPhase::Started, b.1)]
        );
        assert_eq!(
            android_touches(2, 0, &[a, b]),
            [t(4, TouchPhase::Moved, a.1), t(7, TouchPhase::Moved, b.1)]
        );
        assert_eq!(
            android_touches(6, 0, &[a, b]),
            [t(4, TouchPhase::Ended, a.1)]
        );
        assert_eq!(android_touches(1, 0, &[b]), [t(7, TouchPhase::Ended, b.1)]);
        assert_eq!(
            android_touches(3, 0, &[a, b]),
            [
                t(4, TouchPhase::Cancelled, a.1),
                t(7, TouchPhase::Cancelled, b.1)
            ]
        );
        // Hover and scroll aren't touches.
        assert!(android_touches(7, 0, &[a]).is_empty());
        assert!(android_touches(8, 0, &[a]).is_empty());
    }

    #[test]
    fn mac_key_equivalents() {
        use crate::platform::Shortcut;
        assert_eq!(
            mac_key_equivalent(&Shortcut::cmd('S').shift()),
            Some(("s".to_string(), (1 << 20) | (1 << 17)))
        );
        assert_eq!(
            mac_key_equivalent(&Shortcut::key(KeyCode::F5)),
            Some(("\u{F708}".to_string(), 0))
        );
        assert_eq!(mac_key_equivalent(&Shortcut::key(KeyCode::Alt)), None);
    }

    #[test]
    fn win32_virtual_keys() {
        assert_eq!(win32_vk(0x08), Some(KeyCode::Backspace));
        assert_eq!(win32_vk(0x25), Some(KeyCode::ArrowLeft));
        assert_eq!(win32_vk(0x27), Some(KeyCode::ArrowRight));
        assert_eq!(win32_vk(0x21), Some(KeyCode::PageUp));
        assert_eq!(win32_vk(0x22), Some(KeyCode::PageDown));
        assert_eq!(win32_vk(0x24), Some(KeyCode::Home));
        assert_eq!(win32_vk(0x23), Some(KeyCode::End));
        assert_eq!(win32_vk(0x2D), Some(KeyCode::Insert));
        assert_eq!(win32_vk(0x2E), Some(KeyCode::Delete));
        assert_eq!(win32_vk(0x70), Some(KeyCode::F1));
        assert_eq!(win32_vk(0x7B), Some(KeyCode::F12));
        assert_eq!(win32_vk(0x7C), None); // F13 is not mapped
        assert_eq!(win32_vk(0x41), None); // letters come through WM_CHAR
        assert_eq!(win32_shortcut_char(0x43, false), Some('c'));
        assert_eq!(win32_shortcut_char(0x43, true), Some('C'));
        assert_eq!(win32_shortcut_char(0x31, false), Some('1'));
        assert_eq!(win32_shortcut_char(0x20, false), None);
    }

    #[test]
    fn win32_wheel_notches() {
        // One notch up: +120 in the high word → scroll 40 px towards the top.
        let up = 120usize << 16;
        assert_eq!(win32_wheel(up, false), ScrollDelta { dx: 0.0, dy: -40.0 });
        // One notch down: -120 as a 16-bit word.
        let down = ((-120i16) as u16 as usize) << 16;
        assert_eq!(win32_wheel(down, false), ScrollDelta { dx: 0.0, dy: 40.0 });
        // Tilt right; low-word key flags are ignored.
        assert_eq!(
            win32_wheel(up | 0x0008, true),
            ScrollDelta { dx: 40.0, dy: 0.0 }
        );
        // A precision touchpad's partial notch.
        assert_eq!(
            win32_wheel(((-30i16) as u16 as usize) << 16, false).dy,
            10.0
        );
    }

    #[test]
    fn utf16_positions_and_surrogates() {
        // "a" (1 unit), "日" (1 unit, 3 bytes), "😀" (2 units, 4 bytes).
        let s = "a日😀b";
        assert_eq!(utf16_to_byte(s, 0), 0);
        assert_eq!(utf16_to_byte(s, 1), 1);
        assert_eq!(utf16_to_byte(s, 2), 4);
        assert_eq!(utf16_to_byte(s, 3), 8); // inside the pair: rounds up
        assert_eq!(utf16_to_byte(s, 4), 8);
        assert_eq!(utf16_to_byte(s, 5), 9);
        assert_eq!(utf16_to_byte(s, 99), 9);
        let mut j = Utf16Joiner::default();
        assert_eq!(j.push(0x61), Some('a'));
        assert_eq!(j.push(0xD83D), None);
        assert_eq!(j.push(0xDE00), Some('😀'));
        // A lone low surrogate is dropped.
        assert_eq!(j.push(0xDE00), None);
    }

    #[test]
    fn mac_key_codes_and_flags() {
        assert_eq!(mac_keycode(36), Some(KeyCode::Enter));
        assert_eq!(mac_keycode(126), Some(KeyCode::ArrowUp));
        assert_eq!(mac_keycode(115), Some(KeyCode::Home));
        assert_eq!(mac_keycode(119), Some(KeyCode::End));
        assert_eq!(mac_keycode(116), Some(KeyCode::PageUp));
        assert_eq!(mac_keycode(121), Some(KeyCode::PageDown));
        assert_eq!(mac_keycode(117), Some(KeyCode::Delete));
        assert_eq!(mac_keycode(122), Some(KeyCode::F1));
        assert_eq!(mac_keycode(111), Some(KeyCode::F12));
        assert_eq!(mac_keycode(0), None); // 'a' comes through `characters`
        assert_eq!(
            mac_modifiers((1 << 17) | (1 << 20)),
            Modifiers {
                shift: true,
                meta: true,
                ..Modifiers::default()
            }
        );
        // Caps Lock (1 << 16) and device-dependent bits are ignored.
        assert_eq!(mac_modifiers((1 << 16) | 0x100), Modifiers::default());
        assert!(mac_is_function_key("\u{F729}")); // NSHomeFunctionKey
        assert!(!mac_is_function_key("a"));
    }

    #[test]
    fn android_keys_and_meta() {
        assert_eq!(android_keycode(67), Some(KeyCode::Backspace));
        assert_eq!(android_keycode(112), Some(KeyCode::Delete));
        assert_eq!(android_keycode(21), Some(KeyCode::ArrowLeft));
        assert_eq!(android_keycode(160), Some(KeyCode::Enter));
        assert_eq!(android_keycode(131), Some(KeyCode::F1));
        assert_eq!(android_keycode(142), Some(KeyCode::F12));
        assert_eq!(android_keycode(29), None); // 'a' types a character
        assert_eq!(android_keycode(4), None); // Back stays with the system
        assert_eq!(
            android_modifiers(0x1 | 0x1000),
            Modifiers {
                shift: true,
                ctrl: true,
                ..Modifiers::default()
            }
        );
        assert!(android_modifiers(0x10000).meta);
        assert_eq!(android_ascii(29, false), Some('a'));
        assert_eq!(android_ascii(54, true), Some('Z'));
        assert_eq!(android_ascii(8, true), Some('!'));
        assert_eq!(android_ascii(73, false), Some('\\'));
        assert_eq!(android_ascii(62, true), Some(' '));
        assert_eq!(android_ascii(66, false), None);
        assert_eq!(android_scroll(0.0, 1.0), ScrollDelta { dx: 0.0, dy: -40.0 });
    }

    #[test]
    fn mac_scroll_units() {
        // A trackpad swipe up (content follows the fingers) scrolls down.
        assert_eq!(
            mac_scroll(0.0, -12.5, true),
            ScrollDelta { dx: 0.0, dy: 12.5 }
        );
        // A wheel notch towards the user is one line down.
        assert_eq!(
            mac_scroll(0.0, -1.0, false),
            ScrollDelta { dx: 0.0, dy: 40.0 }
        );
        assert_eq!(
            mac_scroll(2.0, 0.0, false),
            ScrollDelta { dx: -80.0, dy: 0.0 }
        );
    }

    #[test]
    fn web_keys_wheel_and_cursors() {
        assert_eq!(web_key("ArrowLeft"), Some(KeyCode::ArrowLeft));
        assert_eq!(web_key("PageDown"), Some(KeyCode::PageDown));
        assert_eq!(web_key("F12"), Some(KeyCode::F12));
        assert_eq!(web_key(" "), Some(KeyCode::Space));
        assert_eq!(web_key("a"), Some(KeyCode::Char('a')));
        assert_eq!(web_key("\u{e9}"), Some(KeyCode::Char('\u{e9}')));
        assert_eq!(web_key("Shift"), None);
        assert_eq!(web_key("Dead"), None);
        assert_eq!(
            web_modifiers(2 | 8),
            Modifiers {
                ctrl: true,
                meta: true,
                ..Modifiers::default()
            }
        );
        assert_eq!(
            web_wheel(0.0, 3.0, 1, 300.0),
            ScrollDelta { dx: 0.0, dy: 120.0 }
        );
        assert_eq!(
            web_wheel(0.0, -53.0, 0, 300.0),
            ScrollDelta { dx: 0.0, dy: -53.0 }
        );
        assert_eq!(web_wheel(0.0, 1.0, 2, 300.0).dy, 300.0);
        assert_eq!(css_cursor(CursorIcon::Text), "text");
        assert_eq!(css_cursor(CursorIcon::NeswResize), "nesw-resize");
    }
}
