//! Pure input-translation tables shared by the native backends.
//!
//! The Win32 and AppKit backends only compile on their own targets, so their
//! key and scroll mappings live here, where they build (and are unit-tested) on
//! every host. Each backend reads the raw values from the OS and hands them to
//! these functions.

use crate::platform::event::{KeyCode, Modifiers, ScrollDelta};

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
        0x2D => KeyCode::Insert, // VK_INSERT
        0x2E => KeyCode::Delete, // VK_DELETE
        0x70 => KeyCode::F1,     // VK_F1 … VK_F12 are consecutive
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
