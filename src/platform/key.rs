//! Raw key events: every press and release, with the physical key, the text
//! it types, left/right modifiers and the repeat flag.
//!
//! Backends deliver [`Event::RawKey`] before their usual translation into
//! [`Event::Key`] / [`Event::Text`] (after input methods and dead keys have
//! had their look, so a key an IME or a compose sequence takes isn't raw).
//! The event handler can call [`consume_key`] while handling it; the backend
//! then skips its own translation for that key. The App does this for an
//! element with [`Element::on_raw_key`](crate::core::Element::on_raw_key)
//! that handles the key. See `docs/keyboard.md`.

use crate::platform::ControlFlow;
use crate::platform::event::{ButtonState, Event, KeyCode, Modifiers};
use std::cell::Cell;

/// A key's position on the keyboard, independent of the layout: a USB HID
/// usage (page 7, "Keyboard/Keypad"), as the W3C UI Events `code` values
/// are defined. `KeyA` is the key right of Caps Lock on any layout; on AZERTY
/// it types `q`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PhysicalKey(pub u16);

/// Where a key sits, for keys the keyboard has twice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum KeyLocation {
    /// The only key of its kind, or the main block's one.
    #[default]
    Standard,
    /// The left one of a modifier pair.
    Left,
    /// The right one of a modifier pair.
    Right,
    /// On the numeric keypad (digits, operators, keypad Enter).
    Numpad,
}

/// Which physical modifier keys are held, left and right apart, as the bits
/// of the USB HID modifier byte. Tracked from the modifier keys' own presses
/// and releases (or read from the OS where it says), so it can lag behind a
/// modifier pressed while another window had focus.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ModifierSides(pub u8);

impl ModifierSides {
    pub const LEFT_CTRL: u8 = 0x01;
    pub const LEFT_SHIFT: u8 = 0x02;
    pub const LEFT_ALT: u8 = 0x04;
    pub const LEFT_META: u8 = 0x08;
    pub const RIGHT_CTRL: u8 = 0x10;
    pub const RIGHT_SHIFT: u8 = 0x20;
    pub const RIGHT_ALT: u8 = 0x40;
    pub const RIGHT_META: u8 = 0x80;

    pub fn left_ctrl(self) -> bool {
        self.0 & Self::LEFT_CTRL != 0
    }
    pub fn right_ctrl(self) -> bool {
        self.0 & Self::RIGHT_CTRL != 0
    }
    pub fn left_shift(self) -> bool {
        self.0 & Self::LEFT_SHIFT != 0
    }
    pub fn right_shift(self) -> bool {
        self.0 & Self::RIGHT_SHIFT != 0
    }
    /// The left Alt key (Option on macOS).
    pub fn left_alt(self) -> bool {
        self.0 & Self::LEFT_ALT != 0
    }
    /// The right Alt key (right Option on macOS; AltGr on many layouts,
    /// which then doesn't count as [`Modifiers::alt`]).
    pub fn right_alt(self) -> bool {
        self.0 & Self::RIGHT_ALT != 0
    }
    /// The left logo key (Command, Windows, Super).
    pub fn left_meta(self) -> bool {
        self.0 & Self::LEFT_META != 0
    }
    pub fn right_meta(self) -> bool {
        self.0 & Self::RIGHT_META != 0
    }

    /// The bit of a modifier key's physical position, if it is one.
    pub fn bit_of(key: PhysicalKey) -> Option<u8> {
        matches!(key.0, 0xe0..=0xe7).then(|| 1u8 << (key.0 - 0xe0))
    }

    /// Drop the sides of modifiers `modifiers` says aren't held (a release
    /// that went to another window).
    pub fn reconcile(self, modifiers: Modifiers) -> ModifierSides {
        let mut bits = self.0;
        let pairs = [
            (modifiers.ctrl, Self::LEFT_CTRL | Self::RIGHT_CTRL),
            (modifiers.shift, Self::LEFT_SHIFT | Self::RIGHT_SHIFT),
            // AltGr is the right Alt key without the Alt modifier.
            (modifiers.alt, Self::LEFT_ALT),
            (modifiers.meta, Self::LEFT_META | Self::RIGHT_META),
        ];
        for (held, mask) in pairs {
            if !held {
                bits &= !mask;
            }
        }
        ModifierSides(bits)
    }
}

/// Which Option keys act as Meta on macOS: with such an Option key held, a
/// key isn't composed by the keyboard layout (Option+B types `b`, not `∫`)
/// and doesn't go to the input method, so a terminal can send `ESC b`. Set
/// for the focused element with
/// [`Element::option_as_meta`](crate::core::Element::option_as_meta). Other
/// platforms don't compose with Alt, so it changes nothing there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum OptionAsMeta {
    /// Both Option keys compose (the macOS default).
    #[default]
    None,
    /// The left Option key is Meta; the right one composes.
    Left,
    /// The right Option key is Meta; the left one composes.
    Right,
    /// Both Option keys are Meta.
    Both,
}

impl OptionAsMeta {
    /// Whether, with `sides` held, Option acts as Meta.
    pub fn applies(self, sides: ModifierSides) -> bool {
        match self {
            OptionAsMeta::None => false,
            OptionAsMeta::Left => sides.left_alt(),
            OptionAsMeta::Right => sides.right_alt(),
            OptionAsMeta::Both => sides.left_alt() || sides.right_alt(),
        }
    }
}

/// One key press or release, with everything the platform says about it.
///
/// In the spirit of the W3C `code` / `key` split: [`physical`](Self::physical)
/// is where the key is, [`logical`](Self::logical) what it means under the
/// layout, and [`text`](Self::text) what it types.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub state: ButtonState,
    /// The key under the current layout: a named key ([`KeyCode::Enter`],
    /// [`KeyCode::F5`], [`KeyCode::Shift`], …) or [`KeyCode::Char`] with the
    /// character the key gives with Shift as held but without Ctrl, Alt or
    /// AltGr (`Char('a')` for A, `Char('A')` for Shift+A, `Char('1')` for
    /// keypad 1 with Num Lock). The same identity [`Event::Key`] and
    /// [`Shortcut`](crate::platform::Shortcut)s use.
    pub logical: KeyCode,
    /// The key's position on the keyboard, whatever the layout.
    pub physical: PhysicalKey,
    /// The left/right/keypad position, from [`physical`](Self::physical).
    pub location: KeyLocation,
    pub modifiers: Modifiers,
    /// Left and right modifier keys held.
    pub sides: ModifierSides,
    /// A press generated by holding the key down.
    pub repeat: bool,
    /// What the key types with the modifiers held, as a text field would
    /// receive it: with Shift, AltGr and (macOS) Option applied, but not Ctrl
    /// or the logo key. `None` for keys that type nothing (named keys and
    /// releases), and never a control character.
    pub text: Option<String>,
    /// What the key types with only Shift applied (AppKit's
    /// `charactersIgnoringModifiers`): `b` for Option+B on macOS, where
    /// [`text`](Self::text) is `∫`. What a terminal sends after `ESC` for
    /// "Option as Meta".
    pub text_ignoring_modifiers: Option<String>,
    /// The platform's own code for the key: the X11 or xkb keycode, the
    /// Windows scan code (`0xe0` prefix for extended keys), the macOS virtual
    /// key code, the Android key code. For logging and debugging.
    pub scancode: u32,
}

impl KeyEvent {
    /// A press of `logical` at `physical`, with no text: the starting point
    /// backends fill in.
    pub fn new(state: ButtonState, logical: KeyCode, physical: PhysicalKey) -> KeyEvent {
        KeyEvent {
            state,
            logical,
            physical,
            location: physical.location(),
            modifiers: Modifiers::default(),
            sides: ModifierSides::default(),
            repeat: false,
            text: None,
            text_ignoring_modifiers: None,
            scancode: 0,
        }
    }

    /// Whether this is a press (including a repeat).
    pub fn is_press(&self) -> bool {
        self.state == ButtonState::Pressed
    }
}

/// `s` as key text (for the backends that get text as a string): `None` when empty, or when it is only control characters
/// or the private-use code points macOS gives function keys.
#[allow(dead_code)]
pub(crate) fn key_text(s: &str) -> Option<String> {
    let printable = !s.is_empty()
        && !s
            .chars()
            .all(|c| c.is_control() || ('\u{F700}'..='\u{F8FF}').contains(&c));
    printable.then(|| s.to_string())
}

// ---- Consuming a raw key -----------------------------------------------------

thread_local! {
    static CONSUMED: Cell<bool> = const { Cell::new(false) };
}

/// Call while handling an [`Event::RawKey`] to say the key is used: the
/// backend then skips its own translation of it ([`Event::Key`],
/// [`Event::Text`], dead keys, the platform's default handling where it
/// can). Has no effect at any other time.
pub fn consume_key() {
    CONSUMED.with(|c| c.set(true));
}

/// Hand `event` to `handler` as an [`Event::RawKey`]. Returns whether the
/// handler consumed it (see [`consume_key`]) and its control flow.
pub(crate) fn deliver_raw(
    event: KeyEvent,
    handler: impl FnOnce(Event) -> ControlFlow,
) -> (bool, ControlFlow) {
    CONSUMED.with(|c| c.set(false));
    let flow = handler(Event::RawKey(event));
    (CONSUMED.with(|c| c.replace(false)), flow)
}

// ---- Side tracking -----------------------------------------------------------

/// Left/right modifier state for backends whose events only carry the
/// combined modifiers: updated from each modifier key's press and release.
#[derive(Clone, Copy, Debug, Default)]
#[allow(dead_code)] // backends that read the sides from the OS don't need it
pub(crate) struct SideTracker(u8);

#[allow(dead_code)]
impl SideTracker {
    /// Note `key` going down or up, and return the sides held now, as far as
    /// `modifiers` (the state with this key) agrees.
    pub(crate) fn update(
        &mut self,
        key: PhysicalKey,
        state: ButtonState,
        modifiers: Modifiers,
    ) -> ModifierSides {
        if let Some(bit) = ModifierSides::bit_of(key) {
            match state {
                ButtonState::Pressed => self.0 |= bit,
                ButtonState::Released => self.0 &= !bit,
            }
        }
        // A modifier's own press reports the state from before it on X11;
        // keep the key's bit while it is down.
        let own = match state {
            ButtonState::Pressed => ModifierSides::bit_of(key).unwrap_or(0),
            ButtonState::Released => 0,
        };
        let reconciled = ModifierSides(self.0 & !own).reconcile(modifiers).0 | own;
        self.0 = reconciled;
        ModifierSides(reconciled)
    }

    /// Forget everything (focus left the window).
    pub(crate) fn clear(&mut self) {
        self.0 = 0;
    }
}

// ---- Physical keys -----------------------------------------------------------

impl PhysicalKey {
    /// A key the platform didn't identify.
    pub const UNIDENTIFIED: PhysicalKey = PhysicalKey(0);

    /// Whether the platform identified the key.
    pub fn is_identified(self) -> bool {
        self.0 != 0
    }

    /// The W3C UI Events `code` name: `"KeyA"`, `"Digit1"`, `"Numpad1"`,
    /// `"ShiftLeft"`, `"F5"`…
    pub fn name(self) -> Option<&'static str> {
        HID_NAMES
            .iter()
            .find(|(usage, _)| *usage == self.0)
            .map(|(_, name)| *name)
    }

    /// The key a W3C `code` names (what a browser's `KeyboardEvent.code`
    /// holds).
    pub fn from_name(name: &str) -> Option<PhysicalKey> {
        HID_NAMES
            .iter()
            .find(|(_, n)| *n == name)
            .map(|(usage, _)| PhysicalKey(*usage))
            .or(match name {
                // Older names some browsers still send.
                "OSLeft" => Some(PhysicalKey(0xe3)),
                "OSRight" => Some(PhysicalKey(0xe7)),
                "NumpadClear" => Some(PhysicalKey(0x53)),
                _ => None,
            })
    }

    /// Where this key sits: left/right for modifiers, numpad for the keypad.
    pub fn location(self) -> KeyLocation {
        match self.0 {
            0xe0..=0xe3 => KeyLocation::Left,
            0xe4..=0xe7 => KeyLocation::Right,
            0x54..=0x63 | 0x67 | 0x85 | 0x86 | 0x8c | 0xb0..=0xdd => KeyLocation::Numpad,
            _ => KeyLocation::Standard,
        }
    }

    /// Whether this is a modifier key (Shift, Ctrl, Alt or the logo key).
    pub fn is_modifier(self) -> bool {
        ModifierSides::bit_of(self).is_some()
    }

    /// The key with a Linux evdev code (`KEY_*` in `input-event-codes.h`):
    /// what Wayland and Android report, and X11's keycode minus 8.
    pub fn from_evdev(code: u32) -> PhysicalKey {
        let usage = match code {
            1 => 0x29,
            2..=10 => 0x1e + (code - 2) as u16,
            11 => 0x27,
            12 => 0x2d,
            13 => 0x2e,
            14 => 0x2a,
            15 => 0x2b,
            16..=25 => {
                [0x14, 0x1a, 0x08, 0x15, 0x17, 0x1c, 0x18, 0x0c, 0x12, 0x13][(code - 16) as usize]
            }
            26 => 0x2f,
            27 => 0x30,
            28 => 0x28,
            29 => 0xe0,
            30..=38 => [0x04, 0x16, 0x07, 0x09, 0x0a, 0x0b, 0x0d, 0x0e, 0x0f][(code - 30) as usize],
            39 => 0x33,
            40 => 0x34,
            41 => 0x35,
            42 => 0xe1,
            43 => 0x31,
            44..=50 => [0x1d, 0x1b, 0x06, 0x19, 0x05, 0x11, 0x10][(code - 44) as usize],
            51 => 0x36,
            52 => 0x37,
            53 => 0x38,
            54 => 0xe5,
            55 => 0x55,
            56 => 0xe2,
            57 => 0x2c,
            58 => 0x39,
            59..=68 => 0x3a + (code - 59) as u16,
            69 => 0x53,
            70 => 0x47,
            71..=83 => [
                0x5f, 0x60, 0x61, 0x56, 0x5c, 0x5d, 0x5e, 0x57, 0x59, 0x5a, 0x5b, 0x62, 0x63,
            ][(code - 71) as usize],
            85 => 0x94,
            86 => 0x64,
            87 => 0x44,
            88 => 0x45,
            89 => 0x87,
            90 => 0x92,
            91 => 0x93,
            92 => 0x8a,
            93 => 0x88,
            94 => 0x8b,
            95 => 0x8c,
            96 => 0x58,
            97 => 0xe4,
            98 => 0x54,
            99 => 0x46,
            100 => 0xe6,
            102 => 0x4a,
            103 => 0x52,
            104 => 0x4b,
            105 => 0x50,
            106 => 0x4f,
            107 => 0x4d,
            108 => 0x51,
            109 => 0x4e,
            110 => 0x49,
            111 => 0x4c,
            113 => 0x7f,
            114 => 0x81,
            115 => 0x80,
            116 => 0x66,
            117 => 0x67,
            119 => 0x48,
            121 => 0x85,
            122 => 0x90,
            123 => 0x91,
            124 => 0x89,
            125 => 0xe3,
            126 => 0xe7,
            127 => 0x65,
            138 => 0x75,
            183..=194 => 0x68 + (code - 183) as u16,
            _ => 0,
        };
        PhysicalKey(usage)
    }

    /// The key with a Windows scan code (`WM_KEYDOWN`'s `lParam` bits 16–23)
    /// and extended-key flag (bit 24).
    pub fn from_win32_scancode(scan: u32, extended: bool) -> PhysicalKey {
        // Set 1 scan codes are evdev codes for the main block.
        let evdev = match (scan, extended) {
            (0x45, false) => 119, // Pause
            (0x45, true) => 69,   // Num Lock
            (0x01..=0x58, false) => scan,
            (0x59, false) => 117,
            (0x64..=0x6e, false) => 183 + (scan - 0x64),
            (0x76, false) => 194,
            (0x70, false) => 93,
            (0x71, false) => 123,
            (0x72, false) => 122,
            (0x73, false) => 89,
            (0x79, false) => 92,
            (0x7b, false) => 94,
            (0x7d, false) => 124,
            (0x7e, false) => 121,
            (0x1c, true) => 96,
            (0x1d, true) => 97,
            (0x20, true) => 113,
            (0x2e, true) => 114,
            (0x30, true) => 115,
            (0x35, true) => 98,
            (0x37, true) => 99,
            (0x38, true) => 100,
            (0x47, true) => 102,
            (0x48, true) => 103,
            (0x49, true) => 104,
            (0x4b, true) => 105,
            (0x4d, true) => 106,
            (0x4f, true) => 107,
            (0x50, true) => 108,
            (0x51, true) => 109,
            (0x52, true) => 110,
            (0x53, true) => 111,
            (0x5b, true) => 125,
            (0x5c, true) => 126,
            (0x5d, true) => 127,
            _ => 0,
        };
        PhysicalKey::from_evdev(evdev)
    }

    /// The key with a macOS virtual key code (`NSEvent.keyCode`, `kVK_*`).
    pub fn from_mac_keycode(code: u16) -> PhysicalKey {
        let usage = MAC_KEYCODES
            .iter()
            .find(|(kvk, _)| *kvk == code)
            .map_or(0, |(_, usage)| *usage);
        PhysicalKey(usage)
    }
}

/// macOS virtual key codes (`kVK_*`) and their HID usages.
const MAC_KEYCODES: &[(u16, u16)] = &[
    (0x00, 0x04), // A
    (0x01, 0x16), // S
    (0x02, 0x07), // D
    (0x03, 0x09), // F
    (0x04, 0x0b), // H
    (0x05, 0x0a), // G
    (0x06, 0x1d), // Z
    (0x07, 0x1b), // X
    (0x08, 0x06), // C
    (0x09, 0x19), // V
    (0x0a, 0x64), // ISO section
    (0x0b, 0x05), // B
    (0x0c, 0x14), // Q
    (0x0d, 0x1a), // W
    (0x0e, 0x08), // E
    (0x0f, 0x15), // R
    (0x10, 0x1c), // Y
    (0x11, 0x17), // T
    (0x12, 0x1e), // 1
    (0x13, 0x1f), // 2
    (0x14, 0x20), // 3
    (0x15, 0x21), // 4
    (0x16, 0x23), // 6
    (0x17, 0x22), // 5
    (0x18, 0x2e), // =
    (0x19, 0x26), // 9
    (0x1a, 0x24), // 7
    (0x1b, 0x2d), // -
    (0x1c, 0x25), // 8
    (0x1d, 0x27), // 0
    (0x1e, 0x30), // ]
    (0x1f, 0x12), // O
    (0x20, 0x18), // U
    (0x21, 0x2f), // [
    (0x22, 0x0c), // I
    (0x23, 0x13), // P
    (0x24, 0x28), // Return
    (0x25, 0x0f), // L
    (0x26, 0x0d), // J
    (0x27, 0x34), // '
    (0x28, 0x0e), // K
    (0x29, 0x33), // ;
    (0x2a, 0x31), // backslash
    (0x2b, 0x36), // ,
    (0x2c, 0x38), // /
    (0x2d, 0x11), // N
    (0x2e, 0x10), // M
    (0x2f, 0x37), // .
    (0x30, 0x2b), // Tab
    (0x31, 0x2c), // Space
    (0x32, 0x35), // `
    (0x33, 0x2a), // Delete (Backspace)
    (0x35, 0x29), // Escape
    (0x36, 0xe7), // Right Command
    (0x37, 0xe3), // Command
    (0x38, 0xe1), // Shift
    (0x39, 0x39), // Caps Lock
    (0x3a, 0xe2), // Option
    (0x3b, 0xe0), // Control
    (0x3c, 0xe5), // Right Shift
    (0x3d, 0xe6), // Right Option
    (0x3e, 0xe4), // Right Control
    (0x40, 0x6c), // F17
    (0x41, 0x63), // keypad .
    (0x43, 0x55), // keypad *
    (0x45, 0x57), // keypad +
    (0x47, 0x53), // keypad Clear (Num Lock's place)
    (0x48, 0x80), // Volume Up
    (0x49, 0x81), // Volume Down
    (0x4a, 0x7f), // Mute
    (0x4b, 0x54), // keypad /
    (0x4c, 0x58), // keypad Enter
    (0x4e, 0x56), // keypad -
    (0x4f, 0x6d), // F18
    (0x50, 0x6e), // F19
    (0x51, 0x67), // keypad =
    (0x52, 0x62), // keypad 0
    (0x53, 0x59), // keypad 1
    (0x54, 0x5a), // keypad 2
    (0x55, 0x5b), // keypad 3
    (0x56, 0x5c), // keypad 4
    (0x57, 0x5d), // keypad 5
    (0x58, 0x5e), // keypad 6
    (0x59, 0x5f), // keypad 7
    (0x5a, 0x6f), // F20
    (0x5b, 0x60), // keypad 8
    (0x5c, 0x61), // keypad 9
    (0x5d, 0x89), // JIS Yen
    (0x5e, 0x87), // JIS Underscore (Ro)
    (0x5f, 0x85), // JIS keypad comma
    (0x60, 0x3e), // F5
    (0x61, 0x3f), // F6
    (0x62, 0x40), // F7
    (0x63, 0x3c), // F3
    (0x64, 0x41), // F8
    (0x65, 0x42), // F9
    (0x66, 0x91), // JIS Eisu (Lang2)
    (0x67, 0x44), // F11
    (0x68, 0x90), // JIS Kana (Lang1)
    (0x69, 0x68), // F13
    (0x6a, 0x6b), // F16
    (0x6b, 0x69), // F14
    (0x6d, 0x43), // F10
    (0x6e, 0x65), // Context menu
    (0x6f, 0x45), // F12
    (0x71, 0x6a), // F15
    (0x72, 0x49), // Help (Insert's place)
    (0x73, 0x4a), // Home
    (0x74, 0x4b), // Page Up
    (0x75, 0x4c), // Forward Delete
    (0x76, 0x3d), // F4
    (0x77, 0x4d), // End
    (0x78, 0x3b), // F2
    (0x79, 0x4e), // Page Down
    (0x7a, 0x3a), // F1
    (0x7b, 0x50), // Left
    (0x7c, 0x4f), // Right
    (0x7d, 0x51), // Down
    (0x7e, 0x52), // Up
];

/// HID usages and their W3C `code` names.
const HID_NAMES: &[(u16, &str)] = &[
    (0x04, "KeyA"),
    (0x05, "KeyB"),
    (0x06, "KeyC"),
    (0x07, "KeyD"),
    (0x08, "KeyE"),
    (0x09, "KeyF"),
    (0x0a, "KeyG"),
    (0x0b, "KeyH"),
    (0x0c, "KeyI"),
    (0x0d, "KeyJ"),
    (0x0e, "KeyK"),
    (0x0f, "KeyL"),
    (0x10, "KeyM"),
    (0x11, "KeyN"),
    (0x12, "KeyO"),
    (0x13, "KeyP"),
    (0x14, "KeyQ"),
    (0x15, "KeyR"),
    (0x16, "KeyS"),
    (0x17, "KeyT"),
    (0x18, "KeyU"),
    (0x19, "KeyV"),
    (0x1a, "KeyW"),
    (0x1b, "KeyX"),
    (0x1c, "KeyY"),
    (0x1d, "KeyZ"),
    (0x1e, "Digit1"),
    (0x1f, "Digit2"),
    (0x20, "Digit3"),
    (0x21, "Digit4"),
    (0x22, "Digit5"),
    (0x23, "Digit6"),
    (0x24, "Digit7"),
    (0x25, "Digit8"),
    (0x26, "Digit9"),
    (0x27, "Digit0"),
    (0x28, "Enter"),
    (0x29, "Escape"),
    (0x2a, "Backspace"),
    (0x2b, "Tab"),
    (0x2c, "Space"),
    (0x2d, "Minus"),
    (0x2e, "Equal"),
    (0x2f, "BracketLeft"),
    (0x30, "BracketRight"),
    (0x31, "Backslash"),
    (0x32, "IntlHash"),
    (0x33, "Semicolon"),
    (0x34, "Quote"),
    (0x35, "Backquote"),
    (0x36, "Comma"),
    (0x37, "Period"),
    (0x38, "Slash"),
    (0x39, "CapsLock"),
    (0x3a, "F1"),
    (0x3b, "F2"),
    (0x3c, "F3"),
    (0x3d, "F4"),
    (0x3e, "F5"),
    (0x3f, "F6"),
    (0x40, "F7"),
    (0x41, "F8"),
    (0x42, "F9"),
    (0x43, "F10"),
    (0x44, "F11"),
    (0x45, "F12"),
    (0x46, "PrintScreen"),
    (0x47, "ScrollLock"),
    (0x48, "Pause"),
    (0x49, "Insert"),
    (0x4a, "Home"),
    (0x4b, "PageUp"),
    (0x4c, "Delete"),
    (0x4d, "End"),
    (0x4e, "PageDown"),
    (0x4f, "ArrowRight"),
    (0x50, "ArrowLeft"),
    (0x51, "ArrowDown"),
    (0x52, "ArrowUp"),
    (0x53, "NumLock"),
    (0x54, "NumpadDivide"),
    (0x55, "NumpadMultiply"),
    (0x56, "NumpadSubtract"),
    (0x57, "NumpadAdd"),
    (0x58, "NumpadEnter"),
    (0x59, "Numpad1"),
    (0x5a, "Numpad2"),
    (0x5b, "Numpad3"),
    (0x5c, "Numpad4"),
    (0x5d, "Numpad5"),
    (0x5e, "Numpad6"),
    (0x5f, "Numpad7"),
    (0x60, "Numpad8"),
    (0x61, "Numpad9"),
    (0x62, "Numpad0"),
    (0x63, "NumpadDecimal"),
    (0x64, "IntlBackslash"),
    (0x65, "ContextMenu"),
    (0x66, "Power"),
    (0x67, "NumpadEqual"),
    (0x68, "F13"),
    (0x69, "F14"),
    (0x6a, "F15"),
    (0x6b, "F16"),
    (0x6c, "F17"),
    (0x6d, "F18"),
    (0x6e, "F19"),
    (0x6f, "F20"),
    (0x70, "F21"),
    (0x71, "F22"),
    (0x72, "F23"),
    (0x73, "F24"),
    (0x75, "Help"),
    (0x7f, "AudioVolumeMute"),
    (0x80, "AudioVolumeUp"),
    (0x81, "AudioVolumeDown"),
    (0x85, "NumpadComma"),
    (0x87, "IntlRo"),
    (0x88, "KanaMode"),
    (0x89, "IntlYen"),
    (0x8a, "Convert"),
    (0x8b, "NonConvert"),
    (0x8c, "NumpadComma"),
    (0x90, "Lang1"),
    (0x91, "Lang2"),
    (0x92, "Lang3"),
    (0x93, "Lang4"),
    (0x94, "Lang5"),
    (0xe0, "ControlLeft"),
    (0xe1, "ShiftLeft"),
    (0xe2, "AltLeft"),
    (0xe3, "MetaLeft"),
    (0xe4, "ControlRight"),
    (0xe5, "ShiftRight"),
    (0xe6, "AltRight"),
    (0xe7, "MetaRight"),
];

// ---- X11 / xkb keysyms -------------------------------------------------------

/// The logical key for an X11 keysym (the one X11 and Wayland's xkb keymap
/// give at the Shift level held): named keys, keypad keys as their characters
/// (or as the navigation keys they are without Num Lock), and printable
/// keysyms as [`KeyCode::Char`].
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn keysym_logical(ks: u32) -> KeyCode {
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
    match ks {
        0xff08 => KeyCode::Backspace,
        0xff09 | 0xfe20 | 0xff89 => KeyCode::Tab, // Tab, ISO_Left_Tab, KP_Tab
        0xff0d | 0xff8d => KeyCode::Enter,        // Return, KP_Enter
        0xff1b => KeyCode::Escape,
        0x20 | 0xff80 => KeyCode::Space,
        0xff51 | 0xff96 => KeyCode::ArrowLeft,
        0xff52 | 0xff97 => KeyCode::ArrowUp,
        0xff53 | 0xff98 => KeyCode::ArrowRight,
        0xff54 | 0xff99 => KeyCode::ArrowDown,
        0xff50 | 0xff95 => KeyCode::Home,
        0xff57 | 0xff9c => KeyCode::End,
        0xff55 | 0xff9a => KeyCode::PageUp,
        0xff56 | 0xff9b => KeyCode::PageDown,
        0xff63 | 0xff9e => KeyCode::Insert,
        0xffff | 0xff9f => KeyCode::Delete,
        0xffbe..=0xffc9 => F_KEYS[(ks - 0xffbe) as usize],
        0xffe1 | 0xffe2 => KeyCode::Shift,
        0xffe3 | 0xffe4 => KeyCode::Control,
        0xffe7..=0xffea => KeyCode::Alt,      // Meta_L/R, Alt_L/R
        0xffeb..=0xffee => KeyCode::Meta,     // Super_L/R, Hyper_L/R
        0xfe03 | 0xff7e => KeyCode::AltGraph, // ISO_Level3_Shift, Mode_switch
        0xffe5 => KeyCode::CapsLock,
        0xff7f => KeyCode::NumLock,
        0xff14 => KeyCode::ScrollLock,
        0xff61 => KeyCode::PrintScreen,
        0xff13 => KeyCode::Pause,
        0xff67 => KeyCode::ContextMenu,
        0xffb0..=0xffb9 => KeyCode::Char((b'0' + (ks - 0xffb0) as u8) as char),
        0xffaa => KeyCode::Char('*'),
        0xffab => KeyCode::Char('+'),
        0xffac => KeyCode::Char(','),
        0xffad => KeyCode::Char('-'),
        0xffae => KeyCode::Char('.'),
        0xffaf => KeyCode::Char('/'),
        0xffbd => KeyCode::Char('='),
        _ => match crate::platform::compose::keysym_to_char(ks) {
            Some(c) => KeyCode::Char(c),
            None => KeyCode::Unidentified(ks),
        },
    }
}

/// The text an X11 keysym types (for [`KeyEvent::text`]): a printable
/// character's, including the keypad's.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn keysym_text(ks: u32) -> Option<String> {
    match keysym_logical(ks) {
        KeyCode::Char(c) => Some(c.to_string()),
        KeyCode::Space => Some(" ".into()),
        _ => None,
    }
}

// ---- Client-side key repeat (Wayland) ----------------------------------------

/// Client-side key repeat, for Wayland, where the compositor doesn't repeat
/// keys: it sends `wl_keyboard.repeat_info` (rate per second and delay in
/// milliseconds), and the client repeats the last key pressed while it is
/// held. The event loop waits until [`deadline`](Self::deadline) and then
/// takes [`due`](Self::due) keys.
#[derive(Clone, Debug)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) struct KeyRepeat {
    /// Repeats per second; 0 turns repeating off.
    rate: u32,
    delay: std::time::Duration,
    /// The key held, and when it next repeats.
    held: Option<(u32, std::time::Instant)>,
}

impl Default for KeyRepeat {
    /// Until the compositor says otherwise: 25 per second after 600 ms, the
    /// usual defaults (and what Weston and sway use).
    fn default() -> Self {
        KeyRepeat {
            rate: 25,
            delay: std::time::Duration::from_millis(600),
            held: None,
        }
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl KeyRepeat {
    /// `wl_keyboard.repeat_info`: `rate` keys per second (0 disables
    /// repeat), after `delay` milliseconds.
    pub(crate) fn set_info(&mut self, rate: i32, delay: i32) {
        self.rate = rate.max(0) as u32;
        self.delay = std::time::Duration::from_millis(delay.max(0) as u64);
        if self.rate == 0 {
            self.held = None;
        }
    }

    /// `key` went down at `now`. A key that repeats starts its delay; any
    /// other key (a modifier) leaves a held key repeating, as xkb does.
    pub(crate) fn press(&mut self, key: u32, repeats: bool, now: std::time::Instant) {
        if self.rate > 0 && repeats {
            self.held = Some((key, now + self.delay));
        }
    }

    /// `key` went up: it stops repeating.
    pub(crate) fn release(&mut self, key: u32) {
        if self.held.is_some_and(|(k, _)| k == key) {
            self.held = None;
        }
    }

    /// Stop repeating (focus left the surface, the keyboard went away).
    pub(crate) fn cancel(&mut self) {
        self.held = None;
    }

    /// When the held key next repeats.
    pub(crate) fn deadline(&self) -> Option<std::time::Instant> {
        self.held.map(|(_, at)| at)
    }

    /// The held key, if a repeat of it is due at `now`; schedules the next.
    /// A loop that fell behind repeats once and catches up, rather than
    /// sending a burst.
    pub(crate) fn due(&mut self, now: std::time::Instant) -> Option<u32> {
        let (key, at) = self.held?;
        if at > now {
            return None;
        }
        let interval = std::time::Duration::from_micros(1_000_000 / u64::from(self.rate.max(1)));
        let mut next = at + interval;
        if next <= now {
            next = now + interval;
        }
        self.held = Some((key, next));
        Some(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn physical_keys_have_w3c_names() {
        assert_eq!(PhysicalKey::from_evdev(30).name(), Some("KeyA"));
        assert_eq!(PhysicalKey::from_evdev(16).name(), Some("KeyQ"));
        assert_eq!(PhysicalKey::from_evdev(2).name(), Some("Digit1"));
        assert_eq!(PhysicalKey::from_evdev(11).name(), Some("Digit0"));
        assert_eq!(PhysicalKey::from_evdev(79).name(), Some("Numpad1"));
        assert_eq!(PhysicalKey::from_evdev(82).name(), Some("Numpad0"));
        assert_eq!(PhysicalKey::from_evdev(96).name(), Some("NumpadEnter"));
        assert_eq!(PhysicalKey::from_evdev(59).name(), Some("F1"));
        assert_eq!(PhysicalKey::from_evdev(88).name(), Some("F12"));
        assert_eq!(PhysicalKey::from_evdev(183).name(), Some("F13"));
        assert_eq!(PhysicalKey::from_evdev(56).name(), Some("AltLeft"));
        assert_eq!(PhysicalKey::from_evdev(100).name(), Some("AltRight"));
        assert_eq!(PhysicalKey::from_evdev(125).name(), Some("MetaLeft"));
        assert_eq!(PhysicalKey::from_evdev(127).name(), Some("ContextMenu"));
        assert_eq!(PhysicalKey::from_evdev(0), PhysicalKey::UNIDENTIFIED);
        assert_eq!(PhysicalKey::from_name("KeyZ"), Some(PhysicalKey(0x1d)));
        assert_eq!(PhysicalKey::from_name("OSLeft"), Some(PhysicalKey(0xe3)));
        assert_eq!(PhysicalKey::from_name("Nope"), None);
        // Every letter of the evdev table is a distinct key.
        let letters: std::collections::HashSet<_> = (16..=25)
            .chain(30..=38)
            .chain(44..=50)
            .map(PhysicalKey::from_evdev)
            .collect();
        assert_eq!(letters.len(), 26);
        assert!(letters.iter().all(|k| k.name().unwrap().starts_with("Key")));
    }

    #[test]
    fn locations_come_from_the_key() {
        assert_eq!(PhysicalKey(0xe1).location(), KeyLocation::Left);
        assert_eq!(PhysicalKey(0xe6).location(), KeyLocation::Right);
        assert_eq!(PhysicalKey(0x59).location(), KeyLocation::Numpad);
        assert_eq!(PhysicalKey(0x58).location(), KeyLocation::Numpad);
        assert_eq!(PhysicalKey(0x28).location(), KeyLocation::Standard);
        assert!(PhysicalKey(0xe7).is_modifier());
        assert!(!PhysicalKey(0x04).is_modifier());
    }

    #[test]
    fn windows_scancodes_map_through_evdev() {
        let k = |s, e| PhysicalKey::from_win32_scancode(s, e).name();
        assert_eq!(k(0x1e, false), Some("KeyA"));
        assert_eq!(k(0x1c, false), Some("Enter"));
        assert_eq!(k(0x1c, true), Some("NumpadEnter"));
        assert_eq!(k(0x1d, true), Some("ControlRight"));
        assert_eq!(k(0x38, true), Some("AltRight"));
        assert_eq!(k(0x4b, false), Some("Numpad4"));
        assert_eq!(k(0x4b, true), Some("ArrowLeft"));
        assert_eq!(k(0x45, false), Some("Pause"));
        assert_eq!(k(0x45, true), Some("NumLock"));
        assert_eq!(k(0x5b, true), Some("MetaLeft"));
        assert_eq!(k(0x57, false), Some("F11"));
        assert_eq!(k(0x64, false), Some("F13"));
    }

    #[test]
    fn mac_keycodes_map_to_usages() {
        let k = |c| PhysicalKey::from_mac_keycode(c).name();
        assert_eq!(k(0x00), Some("KeyA"));
        assert_eq!(k(0x0b), Some("KeyB"));
        assert_eq!(k(0x3a), Some("AltLeft"));
        assert_eq!(k(0x3d), Some("AltRight"));
        assert_eq!(k(0x53), Some("Numpad1"));
        assert_eq!(k(0x4c), Some("NumpadEnter"));
        assert_eq!(k(0x24), Some("Enter"));
        assert_eq!(k(0x7a), Some("F1"));
        assert_eq!(k(0x7e), Some("ArrowUp"));
        // No two mac codes give one key.
        let usages: std::collections::HashSet<_> = MAC_KEYCODES.iter().map(|(_, u)| u).collect();
        assert_eq!(usages.len(), MAC_KEYCODES.len());
    }

    #[test]
    fn keysyms_give_logical_keys_and_text() {
        assert_eq!(keysym_logical(0x61), KeyCode::Char('a'));
        assert_eq!(keysym_logical(0x41), KeyCode::Char('A'));
        assert_eq!(keysym_logical(0xffbe), KeyCode::F1);
        assert_eq!(keysym_logical(0xffb1), KeyCode::Char('1'));
        assert_eq!(keysym_logical(0xff9c), KeyCode::End); // KP_End
        assert_eq!(keysym_logical(0xff8d), KeyCode::Enter);
        assert_eq!(keysym_logical(0xfe20), KeyCode::Tab);
        assert_eq!(keysym_logical(0xffe1), KeyCode::Shift);
        assert_eq!(keysym_logical(0xffe9), KeyCode::Alt);
        assert_eq!(keysym_logical(0xfe03), KeyCode::AltGraph);
        assert_eq!(keysym_text(0xffb1).as_deref(), Some("1"));
        assert_eq!(keysym_text(0x20).as_deref(), Some(" "));
        assert_eq!(keysym_text(0xe9).as_deref(), Some("é"));
        assert_eq!(keysym_text(0xff0d), None);
        assert_eq!(key_text("\u{F704}"), None); // NSF1FunctionKey
        assert_eq!(key_text("\u{3}"), None);
        assert_eq!(key_text("∫").as_deref(), Some("∫"));
    }

    #[test]
    fn sides_track_modifier_keys() {
        let mut t = SideTracker::default();
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        // X11 reports the state before the key: Shift isn't set on its press.
        let s = t.update(
            PhysicalKey(0xe5),
            ButtonState::Pressed,
            Modifiers::default(),
        );
        assert!(s.right_shift() && !s.left_shift());
        let s = t.update(PhysicalKey(0x04), ButtonState::Pressed, shift);
        assert!(s.right_shift());
        let s = t.update(PhysicalKey(0xe5), ButtonState::Released, shift);
        assert_eq!(s, ModifierSides(0));
        // A release we missed: the modifiers say Shift is up.
        t.update(
            PhysicalKey(0xe1),
            ButtonState::Pressed,
            Modifiers::default(),
        );
        let s = t.update(
            PhysicalKey(0x04),
            ButtonState::Pressed,
            Modifiers::default(),
        );
        assert!(!s.left_shift());
        assert!(OptionAsMeta::Left.applies(ModifierSides(ModifierSides::LEFT_ALT)));
        assert!(!OptionAsMeta::Left.applies(ModifierSides(ModifierSides::RIGHT_ALT)));
        assert!(OptionAsMeta::Both.applies(ModifierSides(ModifierSides::RIGHT_ALT)));
        assert!(!OptionAsMeta::None.applies(ModifierSides(0xff)));
    }

    #[test]
    fn consuming_is_scoped_to_one_delivery() {
        let ev = KeyEvent::new(ButtonState::Pressed, KeyCode::F1, PhysicalKey(0x3a));
        let (used, _) = deliver_raw(ev.clone(), |_| {
            consume_key();
            ControlFlow::Wait
        });
        assert!(used);
        let (used, _) = deliver_raw(ev, |e| {
            assert!(matches!(e, Event::RawKey(k) if k.logical == KeyCode::F1));
            ControlFlow::Wait
        });
        assert!(!used);
        // A stray call outside a delivery doesn't leak into the next one.
        consume_key();
        let ev = KeyEvent::new(ButtonState::Pressed, KeyCode::F2, PhysicalKey(0x3b));
        assert!(!deliver_raw(ev, |_| ControlFlow::Wait).0);
    }

    #[test]
    fn repeat_waits_the_delay_then_runs_at_the_rate() {
        let t0 = Instant::now();
        let mut r = KeyRepeat::default();
        r.set_info(10, 300);
        r.press(30, true, t0);
        assert_eq!(r.deadline(), Some(t0 + Duration::from_millis(300)));
        assert_eq!(r.due(t0 + Duration::from_millis(299)), None);
        assert_eq!(r.due(t0 + Duration::from_millis(300)), Some(30));
        assert_eq!(r.deadline(), Some(t0 + Duration::from_millis(400)));
        // Late: one repeat, then the next a full interval on.
        assert_eq!(r.due(t0 + Duration::from_millis(1000)), Some(30));
        assert_eq!(r.deadline(), Some(t0 + Duration::from_millis(1100)));
        // A modifier doesn't stop it; releasing another key doesn't either.
        r.press(42, false, t0);
        r.release(31);
        assert!(r.deadline().is_some());
        // Another key takes over; releasing it stops repeating.
        r.press(31, true, t0);
        r.release(31);
        assert_eq!(r.deadline(), None);
        // Rate 0 turns repeat off.
        r.set_info(0, 300);
        r.press(30, true, t0);
        assert_eq!(r.deadline(), None);
        r.set_info(25, 600);
        r.press(30, true, t0);
        r.cancel();
        assert_eq!(r.due(t0 + Duration::from_secs(5)), None);
    }
}
