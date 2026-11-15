//! Polled input for games: what is held now, and what was pressed and
//! released since the last update.

use super::TouchControls;
use crate::geometry::{Point, Size, Vec2};
use crate::platform::gamepad::{GamepadAxis, GamepadButton, GamepadEvent, GamepadId};
use crate::platform::{
    ButtonState, Event, KeyCode, KeyEvent, Modifiers, PhysicalKey, PointerButton, Touch, TouchPhase,
};

/// How far a stick or trigger must move for the pad to count as the
/// [last device used](InputState::last_device).
const AXIS_ACTIVE: f32 = 0.5;

/// The kind of device the player used last: what a game shows prompts for
/// ("press A" or "press Space"), and when [touch controls](TouchControls)
/// show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum InputDevice {
    /// A key went down.
    Keyboard,
    /// A mouse button went down (moving the mouse doesn't count).
    Mouse,
    /// A gamepad button went down, or a stick or trigger moved past half way.
    Gamepad,
    /// A finger touched the screen.
    Touch,
}

/// A finger on the screen, as [`InputState::touches`] reports it.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TouchPoint {
    /// Identifies the finger while it's down (ids may be reused after).
    pub id: u64,
    /// Where it went down, in logical window coordinates.
    pub start: Point,
    /// Where it is now (or lifted).
    pub position: Point,
}

/// One pad's buttons (bits by [`GamepadButton`] order) and axes, with the
/// edges since the last update.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct PadInput {
    /// `None` for the touch controls' pad.
    id: Option<GamepadId>,
    held: u32,
    pressed: u32,
    released: u32,
    axes: [f32; 6],
    /// The axes as of the last update.
    prev: [f32; 6],
    /// Disconnected: kept until the update has seen its releases.
    gone: bool,
}

pub(crate) fn bit(b: GamepadButton) -> u32 {
    1 << b as u32
}

impl PadInput {
    /// A button went down or up. Returns whether it was a press.
    fn button(&mut self, b: GamepadButton, down: bool) -> bool {
        let was = self.held & bit(b) != 0;
        if down && !was {
            self.held |= bit(b);
            self.pressed |= bit(b);
        } else if !down && was {
            self.held &= !bit(b);
            self.released |= bit(b);
        }
        down && !was
    }

    /// Set every button at once (the touch controls' pad), with edges.
    fn set_buttons(&mut self, held: u32) {
        self.pressed |= held & !self.held;
        self.released |= self.held & !held;
        self.held = held;
    }

    fn release_all(&mut self) {
        self.released |= self.held;
        self.held = 0;
        self.axes = [0.0; 6];
    }

    fn end_update(&mut self) {
        self.pressed = 0;
        self.released = 0;
        self.prev = self.axes;
    }

    /// Axis `axis` now and as of the last update.
    pub(crate) fn axis(&self, axis: GamepadAxis) -> (f32, f32) {
        (self.axes[axis as usize], self.prev[axis as usize])
    }

    /// Whether this is a real pad (not the touch controls).
    pub(crate) fn is_real(&self) -> bool {
        self.id.is_some()
    }
}

/// The keyboard, mouse, gamepads and touch screen as a game reads them once
/// per update.
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
///   moved and how far the wheel turned since the previous update. While the
///   pointer is [locked](crate::core::AppHandle::set_pointer_lock),
///   [`motion`](Self::motion) is the mouse's relative motion instead.
/// - **Gamepads**: each connected pad's buttons (held, and latched edges like
///   keys) and axes. Queries take `Some(id)` for one pad or `None` for any
///   pad (a single-player game), the [touch controls](TouchControls)
///   included.
/// - **Touches**: the fingers down, where each started and is now, and those
///   that went down and lifted since the previous update. The pointer events
///   the first finger also produces aren't taken as the mouse.
///
/// When the window loses focus every held key, button and finger is let go
/// (and reported released), and every pad's buttons and axes too, so nothing
/// sticks while another window has the keyboard. A disconnected pad's
/// buttons are reported released on the next update.
///
/// A game can also feed one itself (from `on_raw_key` and pointer handlers,
/// or in tests) with [`key`](Self::key), [`button`](Self::button),
/// [`pointer_moved`](Self::pointer_moved), [`wheel_by`](Self::wheel_by),
/// [`gamepad`](Self::gamepad), [`touch`](Self::touch), [`event`](Self::event)
/// and [`end_update`](Self::end_update).
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
    /// Whether the pointer is locked: moves aren't motion, deltas are.
    pointer_locked: bool,
    /// The connected pads (and those gone since the last update).
    pads: Vec<PadInput>,
    /// The touch controls, as a pad.
    touch_pad: PadInput,
    touches: Vec<TouchPoint>,
    touches_started: Vec<TouchPoint>,
    touches_ended: Vec<TouchPoint>,
    /// The finger that is also the pointer, and whether its pointer release
    /// is still to come: those pointer events aren't the mouse's.
    touch_pointer: Option<u64>,
    touch_release_pending: bool,
    last_device: Option<InputDevice>,
    controls: Option<TouchControls>,
    /// The window's size (logical), where the touch controls sit.
    area: Size,
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

    /// Whether anything was pressed since the last update: a key, a mouse
    /// button, a gamepad button or a finger.
    pub fn any_pressed(&self) -> bool {
        !self.pressed.is_empty()
            || !self.buttons_pressed.is_empty()
            || self.all_pads().any(|p| p.pressed != 0)
            || !self.touches_started.is_empty()
    }

    /// The kind of device used last (`None` before any): see
    /// [`InputDevice`].
    pub fn last_device(&self) -> Option<InputDevice> {
        self.last_device
    }

    /// Whether the pointer is locked, as far as this input knows (the game
    /// loop keeps it up to date).
    pub fn pointer_locked(&self) -> bool {
        self.pointer_locked
    }

    // ---- Gamepads -----------------------------------------------------------

    /// The connected pads, in the order they connected. The touch controls
    /// aren't among them.
    pub fn gamepads(&self) -> impl Iterator<Item = GamepadId> + '_ {
        self.pads.iter().filter(|p| !p.gone).filter_map(|p| p.id)
    }

    /// Every pad, the touch controls' last.
    fn all_pads(&self) -> impl Iterator<Item = &PadInput> {
        self.pads.iter().chain(std::iter::once(&self.touch_pad))
    }

    /// Pad `pad`, or with `None` every pad (the touch controls included).
    pub(crate) fn pads_matching(&self, pad: Option<GamepadId>) -> impl Iterator<Item = &PadInput> {
        self.all_pads()
            .filter(move |p| pad.is_none() || (p.id == pad && !p.gone))
    }

    /// Whether `button` is down on pad `pad` (`None`: on any pad, the touch
    /// controls included).
    pub fn gamepad_held(&self, pad: Option<GamepadId>, button: GamepadButton) -> bool {
        self.pads_matching(pad).any(|p| p.held & bit(button) != 0)
    }

    /// Whether `button` went down since the last update (on `pad`, or any).
    pub fn gamepad_pressed(&self, pad: Option<GamepadId>, button: GamepadButton) -> bool {
        self.pads_matching(pad)
            .any(|p| p.pressed & bit(button) != 0)
    }

    /// Whether `button` came up since the last update (on `pad`, or any).
    pub fn gamepad_released(&self, pad: Option<GamepadId>, button: GamepadButton) -> bool {
        self.pads_matching(pad)
            .any(|p| p.released & bit(button) != 0)
    }

    /// Where `axis` is on `pad` (−1 to 1 for a stick, +Y down; 0 to 1 for a
    /// trigger), with no dead zone; with `None`, on the pad pushing it
    /// furthest. 0 for a pad that isn't connected.
    pub fn gamepad_axis(&self, pad: Option<GamepadId>, axis: GamepadAxis) -> f32 {
        self.pads_matching(pad)
            .map(|p| p.axis(axis).0)
            .fold(0.0, |a, v| if v.abs() > a.abs() { v } else { a })
    }

    /// The buttons pressed since the last update on the real pads (not the
    /// touch controls), pad by pad.
    pub fn gamepad_buttons_pressed(&self) -> impl Iterator<Item = GamepadButton> + '_ {
        self.pads.iter().flat_map(|p| {
            GamepadButton::ALL
                .into_iter()
                .filter(move |b| p.pressed & bit(*b) != 0)
        })
    }

    // ---- Touch --------------------------------------------------------------

    /// The fingers down, in the order they went down.
    pub fn touches(&self) -> &[TouchPoint] {
        &self.touches
    }

    /// The fingers that went down since the last update (some may have
    /// lifted already).
    pub fn touches_started(&self) -> &[TouchPoint] {
        &self.touches_started
    }

    /// The fingers that lifted (or were cancelled) since the last update,
    /// where they lifted.
    pub fn touches_ended(&self) -> &[TouchPoint] {
        &self.touches_ended
    }

    /// The touch controls, with their state (which fingers hold what), if
    /// any are set.
    pub fn touch_controls(&self) -> Option<&TouchControls> {
        self.controls.as_ref()
    }

    /// Set the touch controls: from then on their sticks and buttons are a
    /// pad, read by the any-pad queries and the gamepad bindings. The game
    /// loop sets them from
    /// [`GameLoop::touch_controls`](crate::game::GameLoop::touch_controls).
    pub fn set_touch_controls(&mut self, controls: Option<TouchControls>) {
        self.controls = controls;
        if let Some(c) = &mut self.controls {
            c.set_area(self.area);
            if let Some(device) = self.last_device {
                c.device_used(device);
            }
        }
        self.sync_touch_pad();
    }

    /// Set the area the touch controls are placed in: the window's logical
    /// size (the game loop keeps it up to date).
    pub fn set_area(&mut self, area: Size) {
        if self.area != area {
            self.area = area;
            if let Some(c) = &mut self.controls {
                c.set_area(area);
            }
        }
    }

    /// The area the touch controls are placed in.
    pub fn area(&self) -> Size {
        self.area
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
                    self.set_last_device(InputDevice::Keyboard);
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
                    self.set_last_device(InputDevice::Mouse);
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

    /// Relative motion that isn't a change of position (a locked pointer's
    /// [`Event::PointerDelta`]).
    pub fn add_motion(&mut self, delta: Vec2) {
        self.motion = self.motion + delta;
    }

    /// The pointer was locked or released. While it's locked, moves aren't
    /// motion (the pointer is held in place; [`add_motion`](Self::add_motion)
    /// is); either way the next move starts afresh, so the jump to or from
    /// where the pointer is held isn't motion.
    pub fn set_pointer_locked(&mut self, locked: bool) {
        if self.pointer_locked != locked {
            self.pointer_locked = locked;
            self.pointer = None;
        }
    }

    /// Take a gamepad event: a pad connecting, a button, an axis, a pad
    /// going away (its held buttons are then reported released).
    pub fn gamepad(&mut self, ev: &GamepadEvent) {
        let id = ev.id();
        let i = match self.pads.iter().position(|p| p.id == Some(id) && !p.gone) {
            Some(i) => i,
            None => {
                if matches!(ev, GamepadEvent::Disconnected { .. }) {
                    return;
                }
                self.pads.push(PadInput {
                    id: Some(id),
                    ..PadInput::default()
                });
                self.pads.len() - 1
            }
        };
        let pad = &mut self.pads[i];
        match *ev {
            GamepadEvent::Button {
                button, pressed, ..
            } => {
                if pad.button(button, pressed) {
                    self.set_last_device(InputDevice::Gamepad);
                }
            }
            GamepadEvent::Axis { axis, value, .. } => {
                let was = pad.axes[axis as usize];
                pad.axes[axis as usize] = value;
                if value.abs() >= AXIS_ACTIVE && was.abs() < AXIS_ACTIVE {
                    self.set_last_device(InputDevice::Gamepad);
                }
            }
            GamepadEvent::Disconnected { .. } => {
                pad.release_all();
                pad.gone = true;
            }
            _ => {}
        }
    }

    /// Take a finger's touch (logical window coordinates). It feeds the
    /// touch controls, if any.
    pub fn touch(&mut self, t: &Touch) {
        let point = TouchPoint {
            id: t.id,
            start: t.position,
            position: t.position,
        };
        let at = self.touches.iter().position(|p| p.id == t.id);
        match (t.phase, at) {
            (TouchPhase::Started, None) => {
                if self.touches.is_empty() {
                    // The first finger is also the pointer.
                    self.touch_pointer = Some(t.id);
                    self.touch_release_pending = false;
                    self.pointer = None;
                }
                self.touches.push(point);
                self.touches_started.push(point);
                self.set_last_device(InputDevice::Touch);
            }
            (TouchPhase::Started | TouchPhase::Moved, Some(i)) => {
                self.touches[i].position = t.position;
            }
            (TouchPhase::Ended | TouchPhase::Cancelled, Some(i)) => {
                let mut p = self.touches.remove(i);
                p.position = t.position;
                self.touches_ended.push(p);
                if self.touch_pointer == Some(t.id) {
                    self.touch_pointer = None;
                    self.touch_release_pending = true;
                }
            }
            _ => return,
        }
        if let Some(c) = &mut self.controls {
            c.touch(t);
            self.sync_touch_pad();
        }
    }

    /// Bring the touch controls' pad to what the controls hold.
    fn sync_touch_pad(&mut self) {
        let (held, axes) = match &self.controls {
            Some(c) => c.pad(),
            None => (0, [0.0; 6]),
        };
        self.touch_pad.set_buttons(held);
        self.touch_pad.axes = axes;
    }

    fn set_last_device(&mut self, device: InputDevice) {
        self.last_device = Some(device);
        if let Some(c) = &mut self.controls {
            c.device_used(device);
        }
    }

    /// Whether a pointer button or move comes from the finger that is also
    /// the pointer, rather than the mouse.
    fn pointer_from_touch(&self) -> bool {
        self.touch_pointer.is_some() || self.touch_release_pending
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
        for pad in &mut self.pads {
            pad.release_all();
        }
        self.touches_ended.append(&mut self.touches);
        self.touch_pointer = None;
        self.touch_release_pending = false;
        if let Some(c) = &mut self.controls {
            c.release_all();
        }
        self.sync_touch_pad();
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
        self.pads.retain(|p| !p.gone);
        for pad in &mut self.pads {
            pad.end_update();
        }
        self.touch_pad.end_update();
        self.touches_started.clear();
        self.touches_ended.clear();
    }

    /// Take what concerns games in a window event. Returns whether it did.
    ///
    /// The pointer events the first finger on a touch screen also produces
    /// (they follow its [`Event::Touch`]) aren't taken as the mouse. While
    /// the pointer is locked, moves aren't motion but
    /// [`Event::PointerDelta`]s are.
    pub fn event(&mut self, event: &Event) -> bool {
        match event {
            Event::RawKey(ev) => self.key(ev),
            Event::PointerButton { state, .. } if self.pointer_from_touch() => {
                if *state == ButtonState::Released {
                    self.touch_release_pending = false;
                } else if self.touch_pointer.is_none() {
                    // A press with no finger down: the touch's release never
                    // came, and this is the mouse.
                    self.touch_release_pending = false;
                    return self.event(event);
                }
            }
            Event::PointerButton { button, state, .. } => self.button(*button, *state),
            Event::PointerMoved { .. } if self.pointer_from_touch() || self.pointer_locked => {}
            Event::PointerMoved { position } => self.pointer_moved(*position),
            Event::PointerDelta { dx, dy } => self.add_motion(Vec2::new(*dx, *dy)),
            Event::Touch(t) => self.touch(t),
            Event::Gamepad(ev) => self.gamepad(ev),
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

    fn pad(n: u32) -> GamepadId {
        GamepadId(n)
    }

    fn pad_button(id: GamepadId, button: GamepadButton, pressed: bool) -> Event {
        Event::Gamepad(GamepadEvent::Button {
            id,
            button,
            pressed,
        })
    }

    fn pad_axis(id: GamepadId, axis: GamepadAxis, value: f32) -> Event {
        Event::Gamepad(GamepadEvent::Axis { id, axis, value })
    }

    fn finger(id: u64, phase: TouchPhase, x: f64, y: f64) -> Event {
        Event::Touch(Touch {
            id,
            phase,
            position: Point::new(x, y),
        })
    }

    #[test]
    fn gamepad_buttons_have_latched_edges_per_pad_and_on_any() {
        use GamepadButton::{East, South};
        let mut i = InputState::new();
        let (a, b) = (pad(1), pad(2));
        i.event(&Event::Gamepad(GamepadEvent::Connected {
            id: a,
            info: crate::gamepad::GamepadInfo::new("one"),
        }));
        // A pad heard of first through a button is taken as connected.
        assert!(i.event(&pad_button(b, East, true)));
        assert_eq!(i.gamepads().collect::<Vec<_>>(), [a, b]);
        assert_eq!(i.last_device(), Some(InputDevice::Gamepad));
        i.end_update();
        // A tap between updates is both pressed and released.
        i.event(&pad_button(a, South, true));
        i.event(&pad_button(a, South, false));
        assert!(i.gamepad_pressed(Some(a), South) && i.gamepad_released(None, South));
        assert!(!i.gamepad_held(None, South));
        assert!(!i.gamepad_pressed(Some(b), South), "the other pad");
        assert!(i.gamepad_held(Some(b), East) && i.gamepad_held(None, East));
        assert!(!i.gamepad_pressed(None, East), "held from before");
        assert!(i.any_pressed());
        assert_eq!(i.gamepad_buttons_pressed().collect::<Vec<_>>(), [South]);
        i.end_update();
        assert!(!i.gamepad_pressed(None, South) && !i.gamepad_released(None, South));
        // A repeated press isn't a second one.
        i.event(&pad_button(b, East, true));
        assert!(!i.gamepad_pressed(None, East));
    }

    #[test]
    fn gamepad_axes_any_pad_and_letting_go() {
        let mut i = InputState::new();
        let (a, b) = (pad(1), pad(2));
        i.event(&pad_axis(a, GamepadAxis::LeftX, 0.3));
        i.event(&pad_axis(b, GamepadAxis::LeftX, -0.7));
        i.event(&pad_button(b, GamepadButton::North, true));
        assert_eq!(i.gamepad_axis(Some(a), GamepadAxis::LeftX), 0.3);
        assert_eq!(i.gamepad_axis(None, GamepadAxis::LeftX), -0.7, "furthest");
        assert_eq!(i.gamepad_axis(Some(pad(9)), GamepadAxis::LeftX), 0.0);
        i.end_update();
        // Disconnected: its buttons released on this update, gone after.
        i.event(&Event::Gamepad(GamepadEvent::Disconnected { id: b }));
        assert!(i.gamepad_released(None, GamepadButton::North));
        assert_eq!(i.gamepad_axis(None, GamepadAxis::LeftX), 0.3);
        assert_eq!(i.gamepads().collect::<Vec<_>>(), [a]);
        i.end_update();
        assert_eq!(i.pads.len(), 1);
        // Focus loss lets go of the rest.
        i.event(&pad_button(a, GamepadButton::Start, true));
        i.end_update();
        i.event(&Event::FocusChanged(false));
        assert!(i.gamepad_released(Some(a), GamepadButton::Start));
        assert!(!i.gamepad_held(None, GamepadButton::Start));
        assert_eq!(i.gamepad_axis(None, GamepadAxis::LeftX), 0.0);
    }

    #[test]
    fn pointer_deltas_are_motion_and_locked_moves_are_not() {
        let mut i = InputState::new();
        i.event(&Event::PointerMoved {
            position: Point::new(10.0, 10.0),
        });
        i.event(&Event::PointerMoved {
            position: Point::new(14.0, 10.0),
        });
        // Locked (the backend warps the pointer to the window's center).
        i.set_pointer_locked(true);
        assert!(i.pointer_locked());
        i.event(&Event::PointerMoved {
            position: Point::new(100.0, 50.0),
        });
        i.event(&Event::PointerDelta { dx: 3.0, dy: -2.0 });
        i.event(&Event::PointerDelta { dx: 1.5, dy: 0.5 });
        assert_eq!(
            i.motion(),
            Vec2::new(8.5, -1.5),
            "moves before, then deltas"
        );
        i.end_update();
        assert_eq!(i.motion(), Vec2::ZERO);
        // Released: the cursor shows again where it was held, which is no
        // motion; moves from there are.
        i.set_pointer_locked(false);
        i.event(&Event::PointerMoved {
            position: Point::new(100.0, 50.0),
        });
        i.event(&Event::PointerMoved {
            position: Point::new(101.0, 50.0),
        });
        assert_eq!(i.motion(), Vec2::new(1.0, 0.0));
    }

    #[test]
    fn touches_and_the_pointer_they_also_make() {
        let mut i = InputState::new();
        i.event(&finger(7, TouchPhase::Started, 20.0, 30.0));
        // The first finger's pointer events follow; they aren't the mouse.
        i.event(&Event::PointerMoved {
            position: Point::new(20.0, 30.0),
        });
        i.event(&Event::PointerButton {
            button: PointerButton::Left,
            state: ButtonState::Pressed,
            position: Point::new(20.0, 30.0),
        });
        i.event(&finger(8, TouchPhase::Started, 300.0, 30.0));
        i.event(&finger(7, TouchPhase::Moved, 25.0, 40.0));
        assert!(!i.button_held(PointerButton::Left) && i.motion() == Vec2::ZERO);
        assert_eq!(i.last_device(), Some(InputDevice::Touch));
        assert_eq!(i.touches().len(), 2);
        assert_eq!(i.touches()[0].start, Point::new(20.0, 30.0));
        assert_eq!(i.touches()[0].position, Point::new(25.0, 40.0));
        assert_eq!(i.touches_started().len(), 2);
        assert!(i.any_pressed());
        i.end_update();
        i.event(&finger(7, TouchPhase::Ended, 25.0, 40.0));
        i.event(&Event::PointerButton {
            button: PointerButton::Left,
            state: ButtonState::Released,
            position: Point::new(25.0, 40.0),
        });
        assert!(!i.button_released(PointerButton::Left));
        assert_eq!(i.touches_ended()[0].id, 7);
        assert_eq!(i.touches().len(), 1);
        // The mouse, after: a click is a click again.
        i.event(&Event::PointerButton {
            button: PointerButton::Left,
            state: ButtonState::Pressed,
            position: Point::new(5.0, 5.0),
        });
        assert!(i.button_pressed(PointerButton::Left));
        assert_eq!(i.last_device(), Some(InputDevice::Mouse));
        // Focus loss ends the remaining finger.
        i.event(&Event::FocusChanged(false));
        assert!(i.touches().is_empty());
        assert_eq!(i.touches_ended().len(), 2);
    }

    #[test]
    fn touch_controls_are_a_pad() {
        use crate::game::{TouchButton, TouchControls, TouchStick};
        let mut i = InputState::new();
        i.set_area(Size::new(800.0, 400.0));
        i.set_touch_controls(Some(
            TouchControls::new()
                .stick(TouchStick::left().radius(50.0).dead_zone(0.0))
                .button(TouchButton::new(GamepadButton::South, "A")),
        ));
        assert!(!i.touch_controls().unwrap().visible());
        // A quick tap on A between updates is seen; the left thumb steers.
        i.event(&finger(1, TouchPhase::Started, 720.0, 300.0));
        i.event(&finger(1, TouchPhase::Ended, 720.0, 300.0));
        i.event(&finger(2, TouchPhase::Started, 100.0, 300.0));
        i.event(&finger(2, TouchPhase::Moved, 75.0, 300.0));
        assert!(i.touch_controls().unwrap().visible());
        assert!(i.gamepad_pressed(None, GamepadButton::South));
        assert!(i.gamepad_released(None, GamepadButton::South));
        assert_eq!(i.gamepad_axis(None, GamepadAxis::LeftX), -0.5);
        assert_eq!(i.gamepads().count(), 0, "not a connected pad");
        assert_eq!(i.gamepad_buttons_pressed().count(), 0);
        i.end_update();
        // Held with one finger while the other steers.
        i.event(&finger(3, TouchPhase::Started, 700.0, 310.0));
        assert!(i.gamepad_held(None, GamepadButton::South));
        assert!(i.gamepad_pressed(None, GamepadButton::South));
        // A key hides them; the fingers still count.
        i.key(&press(PhysicalKey::KEY_W));
        assert!(!i.touch_controls().unwrap().visible());
        assert!(i.gamepad_held(None, GamepadButton::South));
        i.event(&Event::FocusChanged(false));
        assert!(!i.gamepad_held(None, GamepadButton::South));
        assert_eq!(i.gamepad_axis(None, GamepadAxis::LeftX), 0.0);
    }
}
