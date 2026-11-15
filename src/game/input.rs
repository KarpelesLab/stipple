//! Polled input for games: what is held now, and what was pressed and
//! released since the last update.

use crate::geometry::{Point, Vec2};
use crate::platform::{
    ButtonState, Event, KeyCode, KeyEvent, Modifiers, PhysicalKey, PointerButton,
};

/// The keyboard and mouse as a game reads them once per update.
///
/// The [game loop](crate::game::GameLoop) keeps one up to date from the
/// window's events and hands it to each update as
/// [`Frame::input`](crate::game::Frame::input):
///
/// - **Held**: the keys (by physical position, and by the logical key they
///   gave when pressed) and mouse buttons down now.
/// - **Edges**: the keys and buttons pressed and released since the previous
///   update. They are latched, so a key pressed and released between two
///   updates still shows as both [`key_pressed`](Self::key_pressed) and
///   [`key_released`](Self::key_released). Key repeats aren't presses.
/// - **The pointer**: where it is in the window (`None` outside), how far it
///   moved and how far the wheel turned since the previous update.
///
/// When the window loses focus every held key and button is let go (and
/// reported released), so nothing sticks while another window has the
/// keyboard.
///
/// A game can also feed one itself (from `on_raw_key` and pointer handlers,
/// or in tests) with [`key`](Self::key), [`button`](Self::button),
/// [`pointer_moved`](Self::pointer_moved), [`wheel_by`](Self::wheel_by) and
/// [`end_update`](Self::end_update).
#[derive(Clone, Debug, Default)]
pub struct InputState {
    /// Keys held: where each is, and the logical key it gave when pressed.
    held: Vec<(PhysicalKey, KeyCode)>,
    pressed: Vec<(PhysicalKey, KeyCode)>,
    released: Vec<(PhysicalKey, KeyCode)>,
    buttons: Vec<PointerButton>,
    buttons_pressed: Vec<PointerButton>,
    buttons_released: Vec<PointerButton>,
    pointer: Option<Point>,
    motion: Vec2,
    wheel: Vec2,
    modifiers: Modifiers,
}

impl InputState {
    /// Nothing held, nothing pressed.
    pub fn new() -> InputState {
        InputState::default()
    }

    // ---- Keys ---------------------------------------------------------------

    /// Whether the key at `key` is down.
    pub fn key_held(&self, key: PhysicalKey) -> bool {
        self.held.iter().any(|(p, _)| *p == key)
    }

    /// Whether the key at `key` went down since the last update.
    pub fn key_pressed(&self, key: PhysicalKey) -> bool {
        self.pressed.iter().any(|(p, _)| *p == key)
    }

    /// Whether the key at `key` came up since the last update.
    pub fn key_released(&self, key: PhysicalKey) -> bool {
        self.released.iter().any(|(p, _)| *p == key)
    }

    /// Whether a key that gave `key` under the layout when pressed is down.
    /// Letters compare without case, so `Char('w')` matches a W pressed with
    /// Shift held.
    pub fn logical_held(&self, key: KeyCode) -> bool {
        self.held.iter().any(|(_, l)| same_logical(*l, key))
    }

    /// Whether a key giving `key` went down since the last update.
    pub fn logical_pressed(&self, key: KeyCode) -> bool {
        self.pressed.iter().any(|(_, l)| same_logical(*l, key))
    }

    /// Whether a key giving `key` came up since the last update.
    pub fn logical_released(&self, key: KeyCode) -> bool {
        self.released.iter().any(|(_, l)| same_logical(*l, key))
    }

    /// The keys down, in the order they went down.
    pub fn keys_held(&self) -> impl Iterator<Item = PhysicalKey> + '_ {
        self.held.iter().map(|(p, _)| *p)
    }

    /// The keys pressed since the last update, in order.
    pub fn keys_pressed(&self) -> impl Iterator<Item = PhysicalKey> + '_ {
        self.pressed.iter().map(|(p, _)| *p)
    }

    /// The modifiers as of the last key event.
    pub fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    // ---- Mouse --------------------------------------------------------------

    /// Whether mouse button `b` is down.
    pub fn button_held(&self, b: PointerButton) -> bool {
        self.buttons.contains(&b)
    }

    /// Whether mouse button `b` went down since the last update.
    pub fn button_pressed(&self, b: PointerButton) -> bool {
        self.buttons_pressed.contains(&b)
    }

    /// Whether mouse button `b` came up since the last update.
    pub fn button_released(&self, b: PointerButton) -> bool {
        self.buttons_released.contains(&b)
    }

    /// The mouse buttons pressed since the last update, in order.
    pub fn buttons_pressed(&self) -> impl Iterator<Item = PointerButton> + '_ {
        self.buttons_pressed.iter().copied()
    }

    /// Where the pointer is, in the window's logical coordinates, or `None`
    /// while it is outside the window.
    pub fn pointer(&self) -> Option<Point> {
        self.pointer
    }

    /// How far the pointer moved since the last update, in logical pixels.
    pub fn motion(&self) -> Vec2 {
        self.motion
    }

    /// How far the wheel scrolled since the last update, in logical pixels
    /// (lines count as [`SCROLL_LINE_PX`](crate::platform::SCROLL_LINE_PX)).
    /// Positive `dy` scrolls down.
    pub fn wheel(&self) -> Vec2 {
        self.wheel
    }

    /// Whether anything was pressed since the last update.
    pub fn any_pressed(&self) -> bool {
        !self.pressed.is_empty() || !self.buttons_pressed.is_empty()
    }

    // ---- Feeding ------------------------------------------------------------

    /// Take a raw key event. Repeats keep the key held but aren't presses.
    pub fn key(&mut self, ev: &KeyEvent) {
        self.modifiers = ev.modifiers;
        // Unidentified positions (rare: some Wayland setups) go by the
        // logical key instead.
        let same = |(p, l): &(PhysicalKey, KeyCode)| {
            if ev.physical.is_identified() {
                *p == ev.physical
            } else {
                !p.is_identified() && same_logical(*l, ev.logical)
            }
        };
        match ev.state {
            ButtonState::Pressed => {
                if self.held.iter().any(same) {
                    return; // a repeat, or a press reported twice
                }
                let entry = (ev.physical, ev.logical);
                self.held.push(entry);
                if !ev.repeat {
                    self.pressed.push(entry);
                }
            }
            ButtonState::Released => {
                if let Some(i) = self.held.iter().position(same) {
                    let entry = self.held.remove(i);
                    self.released.push(entry);
                }
            }
        }
    }

    /// Take a mouse button press or release.
    pub fn button(&mut self, b: PointerButton, state: ButtonState) {
        match state {
            ButtonState::Pressed => {
                if !self.buttons.contains(&b) {
                    self.buttons.push(b);
                    self.buttons_pressed.push(b);
                }
            }
            ButtonState::Released => {
                if let Some(i) = self.buttons.iter().position(|x| *x == b) {
                    self.buttons.remove(i);
                    self.buttons_released.push(b);
                }
            }
        }
    }

    /// The pointer moved to `at` (logical window coordinates). The first
    /// position after entering the window is no motion.
    pub fn pointer_moved(&mut self, at: Point) {
        if let Some(was) = self.pointer {
            self.motion = self.motion + (at - was);
        }
        self.pointer = Some(at);
    }

    /// The pointer left the window.
    pub fn pointer_left(&mut self) {
        self.pointer = None;
    }

    /// Relative motion that isn't a change of position (a locked pointer).
    pub fn add_motion(&mut self, delta: Vec2) {
        self.motion = self.motion + delta;
    }

    /// The wheel scrolled by `delta` logical pixels.
    pub fn wheel_by(&mut self, delta: Vec2) {
        self.wheel = self.wheel + delta;
    }

    /// The window lost focus: let go of every key and button (reporting them
    /// released) so none stays held.
    pub fn release_all(&mut self) {
        self.released.append(&mut self.held);
        self.buttons_released.append(&mut self.buttons);
        self.modifiers = Modifiers::default();
    }

    /// An update read the edges: forget presses, releases, motion and wheel.
    /// What is held stays.
    pub fn end_update(&mut self) {
        self.pressed.clear();
        self.released.clear();
        self.buttons_pressed.clear();
        self.buttons_released.clear();
        self.motion = Vec2::ZERO;
        self.wheel = Vec2::ZERO;
    }

    /// Take what concerns games in a window event. Returns whether it did.
    pub fn event(&mut self, event: &Event) -> bool {
        match event {
            Event::RawKey(ev) => self.key(ev),
            Event::PointerButton { button, state, .. } => self.button(*button, *state),
            Event::PointerMoved { position } => self.pointer_moved(*position),
            Event::PointerLeft => self.pointer_left(),
            Event::Scroll { delta } => {
                let (dx, dy) = delta.in_pixels();
                self.wheel_by(Vec2::new(dx, dy));
            }
            Event::FocusChanged(false) => self.release_all(),
            _ => return false,
        }
        true
    }
}

/// Whether two logical keys are the same key, letters without case.
fn same_logical(a: KeyCode, b: KeyCode) -> bool {
    match (a, b) {
        (KeyCode::Char(x), KeyCode::Char(y)) => x.to_lowercase().eq(y.to_lowercase()),
        _ => a == b,
    }
}

/// Positions of common game keys, as named in the W3C `code` list.
impl PhysicalKey {
    pub const KEY_A: PhysicalKey = PhysicalKey(0x04);
    pub const KEY_B: PhysicalKey = PhysicalKey(0x05);
    pub const KEY_C: PhysicalKey = PhysicalKey(0x06);
    pub const KEY_D: PhysicalKey = PhysicalKey(0x07);
    pub const KEY_E: PhysicalKey = PhysicalKey(0x08);
    pub const KEY_F: PhysicalKey = PhysicalKey(0x09);
    pub const KEY_G: PhysicalKey = PhysicalKey(0x0a);
    pub const KEY_H: PhysicalKey = PhysicalKey(0x0b);
    pub const KEY_I: PhysicalKey = PhysicalKey(0x0c);
    pub const KEY_J: PhysicalKey = PhysicalKey(0x0d);
    pub const KEY_K: PhysicalKey = PhysicalKey(0x0e);
    pub const KEY_L: PhysicalKey = PhysicalKey(0x0f);
    pub const KEY_M: PhysicalKey = PhysicalKey(0x10);
    pub const KEY_N: PhysicalKey = PhysicalKey(0x11);
    pub const KEY_O: PhysicalKey = PhysicalKey(0x12);
    pub const KEY_P: PhysicalKey = PhysicalKey(0x13);
    pub const KEY_Q: PhysicalKey = PhysicalKey(0x14);
    pub const KEY_R: PhysicalKey = PhysicalKey(0x15);
    pub const KEY_S: PhysicalKey = PhysicalKey(0x16);
    pub const KEY_T: PhysicalKey = PhysicalKey(0x17);
    pub const KEY_U: PhysicalKey = PhysicalKey(0x18);
    pub const KEY_V: PhysicalKey = PhysicalKey(0x19);
    pub const KEY_W: PhysicalKey = PhysicalKey(0x1a);
    pub const KEY_X: PhysicalKey = PhysicalKey(0x1b);
    pub const KEY_Y: PhysicalKey = PhysicalKey(0x1c);
    pub const KEY_Z: PhysicalKey = PhysicalKey(0x1d);
    pub const DIGIT_1: PhysicalKey = PhysicalKey(0x1e);
    pub const DIGIT_2: PhysicalKey = PhysicalKey(0x1f);
    pub const DIGIT_3: PhysicalKey = PhysicalKey(0x20);
    pub const DIGIT_4: PhysicalKey = PhysicalKey(0x21);
    pub const DIGIT_5: PhysicalKey = PhysicalKey(0x22);
    pub const DIGIT_6: PhysicalKey = PhysicalKey(0x23);
    pub const DIGIT_7: PhysicalKey = PhysicalKey(0x24);
    pub const DIGIT_8: PhysicalKey = PhysicalKey(0x25);
    pub const DIGIT_9: PhysicalKey = PhysicalKey(0x26);
    pub const DIGIT_0: PhysicalKey = PhysicalKey(0x27);
    pub const ENTER: PhysicalKey = PhysicalKey(0x28);
    pub const ESCAPE: PhysicalKey = PhysicalKey(0x29);
    pub const BACKSPACE: PhysicalKey = PhysicalKey(0x2a);
    pub const TAB: PhysicalKey = PhysicalKey(0x2b);
    pub const SPACE: PhysicalKey = PhysicalKey(0x2c);
    pub const F1: PhysicalKey = PhysicalKey(0x3a);
    pub const ARROW_RIGHT: PhysicalKey = PhysicalKey(0x4f);
    pub const ARROW_LEFT: PhysicalKey = PhysicalKey(0x50);
    pub const ARROW_DOWN: PhysicalKey = PhysicalKey(0x51);
    pub const ARROW_UP: PhysicalKey = PhysicalKey(0x52);
    pub const CONTROL_LEFT: PhysicalKey = PhysicalKey(0xe0);
    pub const SHIFT_LEFT: PhysicalKey = PhysicalKey(0xe1);
    pub const ALT_LEFT: PhysicalKey = PhysicalKey(0xe2);
    pub const CONTROL_RIGHT: PhysicalKey = PhysicalKey(0xe4);
    pub const SHIFT_RIGHT: PhysicalKey = PhysicalKey(0xe5);
    pub const ALT_RIGHT: PhysicalKey = PhysicalKey(0xe6);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(k: PhysicalKey) -> KeyEvent {
        KeyEvent::new(ButtonState::Pressed, KeyCode::Char('w'), k)
    }

    fn release(k: PhysicalKey) -> KeyEvent {
        KeyEvent::new(ButtonState::Released, KeyCode::Char('w'), k)
    }

    #[test]
    fn the_physical_constants_match_their_names() {
        assert_eq!(PhysicalKey::KEY_W.name(), Some("KeyW"));
        assert_eq!(PhysicalKey::DIGIT_0.name(), Some("Digit0"));
        assert_eq!(PhysicalKey::SPACE.name(), Some("Space"));
        assert_eq!(PhysicalKey::F1.name(), Some("F1"));
        assert_eq!(PhysicalKey::ARROW_UP.name(), Some("ArrowUp"));
        assert_eq!(PhysicalKey::SHIFT_RIGHT.name(), Some("ShiftRight"));
    }

    #[test]
    fn a_tap_between_updates_is_still_seen() {
        let mut i = InputState::new();
        i.key(&press(PhysicalKey::KEY_W));
        i.key(&release(PhysicalKey::KEY_W));
        assert!(!i.key_held(PhysicalKey::KEY_W));
        assert!(i.key_pressed(PhysicalKey::KEY_W));
        assert!(i.key_released(PhysicalKey::KEY_W));
        i.end_update();
        assert!(!i.key_pressed(PhysicalKey::KEY_W));
        assert!(!i.key_released(PhysicalKey::KEY_W));
    }

    #[test]
    fn held_keys_stay_held_and_repeats_are_not_presses() {
        let mut i = InputState::new();
        i.key(&press(PhysicalKey::KEY_W));
        assert!(i.key_pressed(PhysicalKey::KEY_W));
        assert!(i.logical_held(KeyCode::Char('W')), "letters without case");
        i.end_update();
        let mut repeat = press(PhysicalKey::KEY_W);
        repeat.repeat = true;
        i.key(&repeat);
        assert!(i.key_held(PhysicalKey::KEY_W));
        assert!(!i.key_pressed(PhysicalKey::KEY_W));
        assert_eq!(i.keys_held().count(), 1);
    }

    #[test]
    fn buttons_pointer_and_wheel() {
        let mut i = InputState::new();
        assert!(i.event(&Event::PointerMoved {
            position: Point::new(10.0, 10.0)
        }));
        assert_eq!(i.motion(), Vec2::ZERO, "entering is no motion");
        i.pointer_moved(Point::new(13.0, 6.0));
        i.pointer_moved(Point::new(15.0, 6.0));
        assert_eq!(i.motion(), Vec2::new(5.0, -4.0));
        i.button(PointerButton::Left, ButtonState::Pressed);
        i.wheel_by(Vec2::new(0.0, 40.0));
        assert!(i.button_pressed(PointerButton::Left) && i.button_held(PointerButton::Left));
        assert_eq!(i.wheel().dy, 40.0);
        i.end_update();
        assert_eq!(i.motion(), Vec2::ZERO);
        assert_eq!(i.wheel(), Vec2::ZERO);
        assert!(i.button_held(PointerButton::Left));
        i.button(PointerButton::Left, ButtonState::Released);
        assert!(i.button_released(PointerButton::Left));
        i.pointer_left();
        assert_eq!(i.pointer(), None);
    }

    #[test]
    fn losing_focus_lets_go_of_everything() {
        let mut i = InputState::new();
        i.key(&press(PhysicalKey::ARROW_LEFT));
        i.button(PointerButton::Right, ButtonState::Pressed);
        i.end_update();
        i.event(&Event::FocusChanged(false));
        assert!(!i.key_held(PhysicalKey::ARROW_LEFT));
        assert!(!i.button_held(PointerButton::Right));
        assert!(i.key_released(PhysicalKey::ARROW_LEFT), "reported released");
        assert!(i.button_released(PointerButton::Right));
        // The release that went to another window changes nothing.
        i.key(&release(PhysicalKey::ARROW_LEFT));
        assert_eq!(i.released.len(), 1);
    }
}
