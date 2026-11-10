//! Keys on Wayland: raw key events
//! ([`Event::RawKey`](crate::platform::Event::RawKey)) and client-side key
//! repeat.
//!
//! The compositor reports each key once, as an evdev code, and leaves
//! repeating to the client: `wl_keyboard.repeat_info` gives the rate and
//! delay. While a key that repeats is held, the event loop waits until
//! [`KeyRepeat::deadline`] along with the connection, and each repeat goes
//! through the same path as the press (input method aside: a key the
//! compositor's input method took never reaches the client).

use super::{XkbKeymap, evdev_to_event, keysym_name_to_event, keysym_value, wl_modifiers};
use crate::platform::ControlFlow;
use crate::platform::compose::ComposeState;
use crate::platform::event::{ButtonState, Event, KeyCode};
use crate::platform::key::{KeyRepeat, SideTracker, keysym_logical, keysym_text};
use crate::platform::{KeyEvent, PhysicalKey};

/// What happened to a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stroke {
    Press,
    Release,
    /// Held long enough to repeat (generated here, not by the compositor).
    Repeat,
}

/// The keyboard's state between messages.
#[derive(Default)]
pub(super) struct Keys {
    /// Dead keys and compose sequences: the client's job on Wayland.
    pub(super) compose: ComposeState,
    pub(super) repeat: KeyRepeat,
    sides: SideTracker,
}

impl Keys {
    /// `wl_keyboard.leave`, or the keyboard went away: nothing is held any
    /// more.
    pub(super) fn reset(&mut self) {
        self.repeat.cancel();
        self.sides.clear();
    }

    /// Key `key` (evdev) went down or up, or repeated, with the modifier
    /// state `mods` (xkb's depressed | latched | locked). Delivers the raw
    /// key and then, unless the app took it, the compose result or the
    /// key's own event. Starts or stops the repeat.
    pub(super) fn key(
        &mut self,
        keymap: Option<&XkbKeymap>,
        mods: u32,
        (key, stroke): (u32, Stroke),
        now: std::time::Instant,
        handler: &mut dyn FnMut(Event) -> ControlFlow,
    ) -> ControlFlow {
        let pressed = stroke != Stroke::Release;
        let repeat = stroke == Stroke::Repeat;
        let modifiers = wl_modifiers(mods, keymap);
        let sym = keymap.and_then(|m| m.sym(key + 8, mods));
        let ks = sym.and_then(keysym_value);
        let state = if pressed {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        };
        // A virtual keyboard's codes aren't positions: say so rather than
        // name the wrong key.
        let physical = match keymap {
            Some(m) if !m.evdev_keycodes => PhysicalKey::UNIDENTIFIED,
            _ => PhysicalKey::from_evdev(key),
        };
        let logical = match ks {
            Some(ks) => keysym_logical(ks),
            // No keymap: the layout-independent table's idea of the key.
            None => match evdev_to_event(key, true, modifiers) {
                Some(Event::Key { code, .. }) => code,
                Some(Event::Text(t)) => t
                    .chars()
                    .next()
                    .map_or(KeyCode::Unidentified(key), KeyCode::Char),
                _ => KeyCode::Unidentified(key),
            },
        };
        if !repeat {
            if pressed {
                self.repeat.press(key, repeats(physical, logical), now);
            } else {
                self.repeat.release(key);
            }
        }
        let shortcut = modifiers.ctrl || modifiers.meta;
        let composed = ks
            .filter(|_| pressed)
            .and_then(|ks| crate::platform::compose::feed_event(&mut self.compose, ks, shortcut));
        if let Some(ime) = composed {
            return handler(Event::Ime(ime));
        }
        let mut raw = KeyEvent::new(state, logical, physical);
        raw.modifiers = modifiers;
        raw.sides = self.sides.update(physical, state, modifiers);
        raw.repeat = repeat;
        raw.scancode = key + 8;
        if pressed {
            raw.text = match ks {
                Some(ks) => keysym_text(ks),
                None => match logical {
                    KeyCode::Char(c) => Some(c.to_string()),
                    KeyCode::Space => Some(" ".into()),
                    _ => None,
                },
            };
            raw.text_ignoring_modifiers = raw.text.clone();
        }
        let (used, flow) = crate::platform::key::deliver_raw(raw, &mut *handler);
        if used || flow == ControlFlow::Exit {
            return flow;
        }
        let ev = match sym {
            Some(sym) => keysym_name_to_event(sym, pressed, modifiers),
            None => evdev_to_event(key, pressed, modifiers),
        };
        match ev {
            Some(ev) => handler(ev),
            None => ControlFlow::Wait,
        }
    }
}

/// Whether holding a key repeats it: not the modifiers and locks.
fn repeats(physical: PhysicalKey, logical: KeyCode) -> bool {
    !physical.is_modifier()
        && !matches!(
            logical,
            KeyCode::Shift
                | KeyCode::Control
                | KeyCode::Alt
                | KeyCode::Meta
                | KeyCode::AltGraph
                | KeyCode::CapsLock
                | KeyCode::NumLock
                | KeyCode::ScrollLock
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn run(
        keys: &mut Keys,
        key: u32,
        pressed: bool,
        repeat: bool,
        take_raw: bool,
        t: Instant,
    ) -> Vec<Event> {
        let mut out = Vec::new();
        let stroke = match (pressed, repeat) {
            (_, true) => Stroke::Repeat,
            (true, false) => Stroke::Press,
            (false, false) => Stroke::Release,
        };
        keys.key(None, 0, (key, stroke), t, &mut |e| {
            if take_raw && matches!(e, Event::RawKey(_)) {
                crate::platform::consume_key();
            }
            out.push(e);
            ControlFlow::Wait
        });
        out
    }

    #[test]
    fn keys_go_out_raw_then_translated() {
        let mut keys = Keys::default();
        let t0 = Instant::now();
        let out = run(&mut keys, 30, true, false, false, t0); // KEY_A
        assert_eq!(out.len(), 2);
        let Event::RawKey(raw) = &out[0] else {
            panic!("raw first: {out:?}")
        };
        assert_eq!(raw.logical, KeyCode::Char('a'));
        assert_eq!(raw.physical.name(), Some("KeyA"));
        assert_eq!(raw.text.as_deref(), Some("a"));
        assert!(!raw.repeat);
        assert_eq!(out[1], Event::Text("a".into()));
        // Taken raw: nothing else.
        let out = run(&mut keys, 59, true, false, true, t0); // F1
        assert_eq!(out.len(), 1);
        assert!(matches!(&out[0], Event::RawKey(k) if k.logical == KeyCode::F1));
    }

    #[test]
    fn a_held_key_repeats_until_released() {
        let mut keys = Keys::default();
        keys.repeat.set_info(20, 200);
        let t0 = Instant::now();
        run(&mut keys, 14, true, false, false, t0); // Backspace
        assert_eq!(
            keys.repeat.deadline(),
            Some(t0 + Duration::from_millis(200))
        );
        let at = t0 + Duration::from_millis(200);
        let key = keys.repeat.due(at).expect("due");
        let out = run(&mut keys, key, true, true, false, at);
        assert!(matches!(&out[0], Event::RawKey(k) if k.repeat && k.logical == KeyCode::Backspace));
        assert!(matches!(
            &out[1],
            Event::Key {
                code: KeyCode::Backspace,
                ..
            }
        ));
        // The next one a rate interval later.
        assert_eq!(keys.repeat.deadline(), Some(at + Duration::from_millis(50)));
        // A modifier pressed meanwhile doesn't stop it; releasing the key does.
        run(&mut keys, 42, true, false, false, at); // Left Shift
        assert!(keys.repeat.deadline().is_some());
        run(&mut keys, 14, false, false, false, at);
        assert_eq!(keys.repeat.deadline(), None);
        // Leaving the surface stops it too.
        run(&mut keys, 30, true, false, false, at);
        assert!(keys.repeat.deadline().is_some());
        keys.reset();
        assert_eq!(keys.repeat.deadline(), None);
    }

    /// The keymap `wtype -k Tab -k F5 -M ctrl -k d -m ctrl -k Tab -P b`
    /// uploads: its own keycodes, in the order the keys are first typed, and
    /// keysyms by value.
    const WTYPE_KEYMAP: &str = "\
xkb_keymap {
xkb_keycodes \"(unnamed)\" {
minimum = 8;
maximum = 13;
<K1> = 9;
<K2> = 10;
<K3> = 11;
<K4> = 12;
};
xkb_types \"(unnamed)\" { include \"complete\" };
xkb_compatibility \"(unnamed)\" { include \"complete\" };
xkb_symbols \"(unnamed)\" {
key <K1> {[0xff09]};
key <K2> {[0xffc2]};
key <K3> {[0x64]};
key <K4> {[0x62]};
};
};";

    #[test]
    fn a_virtual_keyboard_types_every_key_with_unknown_positions() {
        let map = super::super::parse_xkb(WTYPE_KEYMAP);
        assert!(!map.evdev_keycodes);
        let mut keys = Keys::default();
        let t0 = Instant::now();
        let ctrl = super::super::XKB_CONTROL;
        let mut raw = Vec::new();
        let strokes = [
            (1, 0),    // Tab
            (2, 0),    // F5
            (3, ctrl), // Ctrl+d
            (1, 0),    // Tab
            (4, 0),    // b
        ];
        for (key, mods) in strokes {
            for stroke in [Stroke::Press, Stroke::Release] {
                keys.key(Some(&map), mods, (key, stroke), t0, &mut |e| {
                    if let Event::RawKey(k) = e {
                        raw.push(k);
                    }
                    ControlFlow::Wait
                });
            }
        }
        let pressed: Vec<_> = raw
            .iter()
            .filter(|k| k.is_press())
            .map(|k| (k.logical, k.modifiers.ctrl))
            .collect();
        assert_eq!(
            pressed,
            [
                (KeyCode::Tab, false),
                (KeyCode::F5, false),
                (KeyCode::Char('d'), true),
                (KeyCode::Tab, false),
                (KeyCode::Char('b'), false),
            ]
        );
        assert_eq!(raw.len(), 10);
        // wtype's key 1 isn't Escape, and its key 4 isn't the 3 key.
        assert!(raw.iter().all(|k| k.physical == PhysicalKey::UNIDENTIFIED));
        assert_eq!(raw[0].scancode, 9);
    }
}
