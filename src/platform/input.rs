//! Pure input-translation tables shared by the native backends.
//!
//! The Win32 and AppKit backends only compile on their own targets, so their
//! key and scroll mappings live here, where they build (and are unit-tested) on
//! every host. Each backend reads the raw values from the OS and hands them to
//! these functions.

use crate::geometry::Point;
use crate::platform::event::{
    ButtonState, KeyCode, Modifiers, PointerButton, ScrollDelta, ScrollPhase, Touch, TouchPhase,
};
use crate::platform::key::{ModifierSides, PhysicalKey};
use crate::platform::window::CursorIcon;

/// Logical pixels scrolled per wheel notch ("line"). Every backend uses the
/// same value, so a mouse wheel scrolls the same distance everywhere.
pub(crate) use crate::platform::event::SCROLL_LINE_PX as LINE_PX;

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
    let lines = f64::from(raw) / 120.0;
    if horizontal {
        ScrollDelta::lines(lines, 0.0)
    } else {
        // Stipple's dy grows downwards; a wheel turned up scrolls towards the top.
        ScrollDelta::lines(0.0, -lines)
    }
}

/// Whether any mouse button is still down after a Win32 button message, from
/// the `MK_*` key-state flags in its `wParam` (left, right, middle, and the
/// two side buttons).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn win32_any_button_down(wp: usize) -> bool {
    const MK_BUTTONS: usize = 0x0001 | 0x0002 | 0x0010 | 0x0020 | 0x0040;
    wp & MK_BUTTONS != 0
}

/// The side button a `WM_XBUTTONDOWN` / `WM_XBUTTONUP` is about, from the
/// high word of its `wParam` (`XBUTTON1` back, `XBUTTON2` forward).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn win32_xbutton(wp: usize) -> PointerButton {
    match (wp >> 16) & 0xffff {
        1 => PointerButton::Back,
        2 => PointerButton::Forward,
        n => PointerButton::Other(n as u16),
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
    if precise {
        ScrollDelta::pixels(-dx, -dy)
    } else {
        ScrollDelta::lines(-dx, -dy)
    }
}

/// The gesture phase of an `NSEventTypeScrollWheel` from its `phase` and
/// `momentumPhase` (`NSEventPhase` bits: began 1, stationary 2, changed 4,
/// ended 8, cancelled 16, may-begin 32). A wheel reports neither.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn mac_scroll_phase(phase: u64, momentum: u64) -> ScrollPhase {
    if momentum != 0 {
        return ScrollPhase::Momentum;
    }
    match phase {
        0 => ScrollPhase::None,
        1 | 32 => ScrollPhase::Started,
        8 | 16 => ScrollPhase::Ended,
        _ => ScrollPhase::Moved,
    }
}

/// The button of an `otherMouseDown/Up/Dragged` from its `buttonNumber`
/// (2 middle, 3 back, 4 forward).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn mac_other_button(number: i64) -> PointerButton {
    match number {
        2 => PointerButton::Middle,
        3 => PointerButton::Back,
        4 => PointerButton::Forward,
        n => PointerButton::Other(n as u16),
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
    match mode {
        1 => ScrollDelta::lines(dx, dy),
        2 => {
            let page = page.max(LINE_PX);
            ScrollDelta::pixels(dx * page, dy * page)
        }
        _ => ScrollDelta::pixels(dx, dy),
    }
}

/// The button of a DOM pointer event's `button` (0 left, 1 middle, 2 right,
/// 3 back, 4 forward).
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn web_button(button: u32) -> PointerButton {
    match button {
        0 => PointerButton::Left,
        1 => PointerButton::Middle,
        2 => PointerButton::Right,
        3 => PointerButton::Back,
        4 => PointerButton::Forward,
        b => PointerButton::Other(b as u16),
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
        CursorIcon::Hidden => "none",
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
        _ => return None,
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
    ScrollDelta::lines(f64::from(h), -f64::from(v))
}

/// The buttons in an `AMotionEvent_getButtonState` mask (primary 1,
/// secondary 2, tertiary 4, back 8, forward 16), in that order.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn android_buttons(mask: i32) -> impl Iterator<Item = PointerButton> {
    [
        (1, PointerButton::Left),
        (2, PointerButton::Right),
        (4, PointerButton::Middle),
        (8, PointerButton::Back),
        (16, PointerButton::Forward),
    ]
    .into_iter()
    .filter(move |(bit, _)| mask & bit != 0)
    .map(|(_, b)| b)
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

// ---- Raw key events (shared) --------------------------------------------------

/// The named key at a physical position (HID usage), for keys every layout
/// agrees on: modifiers, locks, function and editing keys. `None` for keys
/// whose meaning comes from the layout.
#[allow(dead_code)]
pub(crate) fn hid_named(usage: u16) -> Option<KeyCode> {
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
    Some(match usage {
        0x28 | 0x58 => KeyCode::Enter,
        0x29 => KeyCode::Escape,
        0x2a => KeyCode::Backspace,
        0x2b => KeyCode::Tab,
        0x2c => KeyCode::Space,
        0x39 => KeyCode::CapsLock,
        0x3a..=0x45 => F_KEYS[(usage - 0x3a) as usize],
        0x46 => KeyCode::PrintScreen,
        0x47 => KeyCode::ScrollLock,
        0x48 => KeyCode::Pause,
        0x49 => KeyCode::Insert,
        0x4a => KeyCode::Home,
        0x4b => KeyCode::PageUp,
        0x4c => KeyCode::Delete,
        0x4d => KeyCode::End,
        0x4e => KeyCode::PageDown,
        0x4f => KeyCode::ArrowRight,
        0x50 => KeyCode::ArrowLeft,
        0x51 => KeyCode::ArrowDown,
        0x52 => KeyCode::ArrowUp,
        0x53 => KeyCode::NumLock,
        0x65 => KeyCode::ContextMenu,
        0xe0 | 0xe4 => KeyCode::Control,
        0xe1 | 0xe5 => KeyCode::Shift,
        0xe2 | 0xe6 => KeyCode::Alt,
        0xe3 | 0xe7 => KeyCode::Meta,
        _ => return None,
    })
}

/// The logical key of a raw key event: a named key from the platform's own
/// mapping (`named`), else from the physical key, else the character the key
/// gives with Shift as held (`shifted`), else unidentified with `scancode`.
#[allow(dead_code)]
pub(crate) fn raw_logical(
    named: Option<KeyCode>,
    physical: PhysicalKey,
    shifted: Option<&str>,
    scancode: u32,
) -> KeyCode {
    if let Some(code) = named.or_else(|| hid_named(physical.0)) {
        return code;
    }
    let mut chars = shifted.unwrap_or("").chars();
    match (chars.next(), chars.next()) {
        (Some(' '), None) => KeyCode::Space,
        (Some(c), None) if !c.is_control() && !('\u{F700}'..='\u{F8FF}').contains(&c) => {
            KeyCode::Char(c)
        }
        _ => KeyCode::Unidentified(scancode),
    }
}

/// Left/right modifiers from AppKit's device-dependent `modifierFlags` bits.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn mac_sides(flags: u64) -> ModifierSides {
    let pairs = [
        (0x0001, ModifierSides::LEFT_CTRL),
        (0x0002, ModifierSides::LEFT_SHIFT),
        (0x0004, ModifierSides::RIGHT_SHIFT),
        (0x0008, ModifierSides::LEFT_META),
        (0x0010, ModifierSides::RIGHT_META),
        (0x0020, ModifierSides::LEFT_ALT),
        (0x0040, ModifierSides::RIGHT_ALT),
        (0x2000, ModifierSides::RIGHT_CTRL),
    ];
    ModifierSides(
        pairs
            .iter()
            .filter(|(flag, _)| flags & flag != 0)
            .fold(0, |bits, (_, side)| bits | side),
    )
}

/// An `NSEventTypeFlagsChanged` for the modifier key `kc`: whether it went
/// down or up, read from its own device-dependent bit in `flags` (Caps Lock
/// from its lock bit). `None` for a key that isn't a modifier.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn mac_flags_changed(kc: u16, flags: u64) -> Option<ButtonState> {
    let bit: u64 = match kc {
        0x3b => 0x0001,  // Control
        0x38 => 0x0002,  // Shift
        0x3c => 0x0004,  // Right Shift
        0x37 => 0x0008,  // Command
        0x36 => 0x0010,  // Right Command
        0x3a => 0x0020,  // Option
        0x3d => 0x0040,  // Right Option
        0x3e => 0x2000,  // Right Control
        0x39 => 1 << 16, // Caps Lock
        _ => return None,
    };
    Some(if flags & bit != 0 {
        ButtonState::Pressed
    } else {
        ButtonState::Released
    })
}

/// A web key's text: `key` when it is one printable character, unless Ctrl
/// or Meta is held (Ctrl+Alt is AltGr on Windows, which types).
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn web_text(key: &str, modifiers: Modifiers) -> Option<String> {
    let command = (modifiers.ctrl && !modifiers.alt) || modifiers.meta;
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if !command && !c.is_control() => Some(c.to_string()),
        _ => None,
    }
}

/// A web key's text with only Shift applied, from its `code` where that
/// fixes it (letters, and digits without Shift), else its text.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn web_text_ignoring_modifiers(
    code: &str,
    shift: bool,
    text: Option<&str>,
) -> Option<String> {
    if let Some(l) = code.strip_prefix("Key")
        && l.len() == 1
        && l.as_bytes()[0].is_ascii_uppercase()
    {
        return Some(if shift {
            l.to_string()
        } else {
            l.to_ascii_lowercase()
        });
    }
    if let Some(d) = code.strip_prefix("Digit")
        && d.len() == 1
        && !shift
    {
        return Some(d.to_string());
    }
    if code == "Space" {
        return Some(" ".into());
    }
    text.map(str::to_string)
}

/// Left/right modifiers from an Android meta state.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn android_sides(meta: i32) -> ModifierSides {
    let pairs = [
        (0x10, ModifierSides::LEFT_ALT),
        (0x20, ModifierSides::RIGHT_ALT),
        (0x40, ModifierSides::LEFT_SHIFT),
        (0x80, ModifierSides::RIGHT_SHIFT),
        (0x2000, ModifierSides::LEFT_CTRL),
        (0x4000, ModifierSides::RIGHT_CTRL),
        (0x20000, ModifierSides::LEFT_META),
        (0x40000, ModifierSides::RIGHT_META),
    ];
    ModifierSides(
        pairs
            .iter()
            .filter(|(flag, _)| meta & flag != 0)
            .fold(0, |bits, (_, side)| bits | side),
    )
}

/// The named key for an Android key code, modifiers and locks included.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn android_named(kc: i32) -> Option<KeyCode> {
    android_keycode(kc).or(match kc {
        59 | 60 => Some(KeyCode::Shift),
        113 | 114 => Some(KeyCode::Control),
        57 | 58 => Some(KeyCode::Alt),
        117 | 118 => Some(KeyCode::Meta),
        115 => Some(KeyCode::CapsLock),
        143 => Some(KeyCode::NumLock),
        116 => Some(KeyCode::ScrollLock),
        120 => Some(KeyCode::PrintScreen),
        121 => Some(KeyCode::Pause),
        62 => Some(KeyCode::Space),
        _ => None,
    })
}

/// The named key for a Win32 virtual key, modifiers and locks included.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn win32_named(vk: usize) -> Option<KeyCode> {
    win32_vk(vk).or(match vk {
        0x10 | 0xA0 | 0xA1 => Some(KeyCode::Shift),
        0x11 | 0xA2 | 0xA3 => Some(KeyCode::Control),
        0xA4 | 0xA5 => Some(KeyCode::Alt),
        0x5B | 0x5C => Some(KeyCode::Meta),
        0x14 => Some(KeyCode::CapsLock),
        0x90 => Some(KeyCode::NumLock),
        0x91 => Some(KeyCode::ScrollLock),
        0x2C => Some(KeyCode::PrintScreen),
        0x13 => Some(KeyCode::Pause),
        0x20 => Some(KeyCode::Space),
        _ => None,
    })
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
        // One notch up: +120 in the high word → a line towards the top.
        let up = 120usize << 16;
        assert_eq!(win32_wheel(up, false), ScrollDelta::lines(0.0, -1.0));
        assert_eq!(win32_wheel(up, false).in_pixels(), (0.0, -40.0));
        // One notch down: -120 as a 16-bit word.
        let down = ((-120i16) as u16 as usize) << 16;
        assert_eq!(win32_wheel(down, false), ScrollDelta::lines(0.0, 1.0));
        // Tilt right; low-word key flags are ignored.
        assert_eq!(win32_wheel(up | 0x0008, true), ScrollDelta::lines(1.0, 0.0));
        // A precision touchpad's partial notch.
        assert_eq!(
            win32_wheel(((-30i16) as u16 as usize) << 16, false).dy,
            0.25
        );
    }

    #[test]
    fn win32_buttons() {
        assert!(win32_any_button_down(0x0010)); // MK_MBUTTON
        assert!(win32_any_button_down(0x0040)); // MK_XBUTTON2
        assert!(!win32_any_button_down(0x0004 | 0x0008)); // Shift, Ctrl only
        assert_eq!(win32_xbutton(1 << 16), PointerButton::Back);
        assert_eq!(win32_xbutton(2 << 16 | 0x20), PointerButton::Forward);
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
        assert_eq!(android_scroll(0.0, 1.0), ScrollDelta::lines(0.0, -1.0));
        let held: Vec<_> = android_buttons(1 | 4 | 16).collect();
        assert_eq!(
            held,
            [
                PointerButton::Left,
                PointerButton::Middle,
                PointerButton::Forward
            ]
        );
    }

    #[test]
    fn mac_scroll_units() {
        // A trackpad swipe up (content follows the fingers) scrolls down.
        assert_eq!(mac_scroll(0.0, -12.5, true), ScrollDelta::pixels(0.0, 12.5));
        // A wheel notch towards the user is one line down.
        assert_eq!(mac_scroll(0.0, -1.0, false), ScrollDelta::lines(0.0, 1.0));
        assert_eq!(mac_scroll(2.0, 0.0, false).in_pixels(), (-80.0, 0.0));
        assert_eq!(mac_scroll_phase(0, 0), ScrollPhase::None);
        assert_eq!(mac_scroll_phase(1, 0), ScrollPhase::Started);
        assert_eq!(mac_scroll_phase(4, 0), ScrollPhase::Moved);
        assert_eq!(mac_scroll_phase(8, 0), ScrollPhase::Ended);
        assert_eq!(mac_scroll_phase(0, 4), ScrollPhase::Momentum);
        assert_eq!(mac_other_button(2), PointerButton::Middle);
        assert_eq!(mac_other_button(3), PointerButton::Back);
        assert_eq!(mac_other_button(7), PointerButton::Other(7));
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
        assert_eq!(web_wheel(0.0, 3.0, 1, 300.0), ScrollDelta::lines(0.0, 3.0));
        assert_eq!(
            web_wheel(0.0, -53.0, 0, 300.0),
            ScrollDelta::pixels(0.0, -53.0)
        );
        assert_eq!(web_wheel(0.0, 1.0, 2, 300.0).dy, 300.0);
        assert_eq!(web_button(1), PointerButton::Middle);
        assert_eq!(web_button(4), PointerButton::Forward);
        assert_eq!(css_cursor(CursorIcon::Text), "text");
        assert_eq!(css_cursor(CursorIcon::NeswResize), "nesw-resize");
    }

    #[test]
    fn raw_key_helpers() {
        let p = |n| PhysicalKey::from_name(n).unwrap();
        assert_eq!(raw_logical(None, p("ShiftRight"), None, 0), KeyCode::Shift);
        assert_eq!(
            raw_logical(None, p("KeyA"), Some("q"), 0),
            KeyCode::Char('q')
        );
        assert_eq!(raw_logical(None, p("Space"), Some(" "), 0), KeyCode::Space);
        assert_eq!(
            raw_logical(None, PhysicalKey(0), Some("\u{F704}"), 7),
            KeyCode::Unidentified(7)
        );
        assert_eq!(
            raw_logical(Some(KeyCode::Home), p("Numpad7"), Some("7"), 0),
            KeyCode::Home
        );
        // macOS.
        let s = mac_sides(0x20 | 0x2000 | (1 << 19));
        assert!(s.left_alt() && s.right_ctrl() && !s.right_alt());
        assert_eq!(mac_flags_changed(0x3d, 0x40), Some(ButtonState::Pressed));
        assert_eq!(mac_flags_changed(0x3d, 0x20), Some(ButtonState::Released));
        assert_eq!(mac_flags_changed(0x39, 1 << 16), Some(ButtonState::Pressed));
        assert_eq!(mac_flags_changed(0x00, 0), None);
        // Web.
        let none = Modifiers::default();
        let ctrl = Modifiers { ctrl: true, ..none };
        let altgr = Modifiers {
            ctrl: true,
            alt: true,
            ..none
        };
        assert_eq!(web_text("a", none).as_deref(), Some("a"));
        assert_eq!(web_text("a", ctrl), None);
        assert_eq!(web_text("@", altgr).as_deref(), Some("@"));
        assert_eq!(web_text("Enter", none), None);
        let ign = web_text_ignoring_modifiers;
        assert_eq!(ign("KeyB", false, Some("\u{222b}")).as_deref(), Some("b"));
        assert_eq!(ign("KeyB", true, None).as_deref(), Some("B"));
        assert_eq!(ign("Digit1", false, Some("\u{a1}")).as_deref(), Some("1"));
        assert_eq!(ign("Digit1", true, Some("!")).as_deref(), Some("!"));
        // Android.
        let s = android_sides(0x20 | 0x40);
        assert!(s.right_alt() && s.left_shift() && !s.left_alt());
        assert_eq!(android_named(60), Some(KeyCode::Shift));
        assert_eq!(android_named(62), Some(KeyCode::Space));
        assert_eq!(android_named(29), None);
        // Win32.
        assert_eq!(win32_named(0x12), Some(KeyCode::Alt));
        assert_eq!(win32_named(0xA5), Some(KeyCode::Alt));
        assert_eq!(win32_named(0x10), Some(KeyCode::Shift));
        assert_eq!(win32_named(0x5B), Some(KeyCode::Meta));
        assert_eq!(win32_named(0x41), None);
    }
}
