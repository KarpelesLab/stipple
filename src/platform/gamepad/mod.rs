//! Gamepads, per platform: the public API is `stipple::gamepad`, which
//! re-exports this module's items.

use std::sync::Mutex;
use std::time::Duration;

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) mod android;
#[cfg(any(target_os = "macos", target_os = "ios"))]
#[allow(unsafe_code)]
mod apple;
#[cfg(any(target_os = "linux", test))]
#[cfg_attr(target_os = "linux", allow(unsafe_code))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod evdev;
#[cfg(any(target_arch = "wasm32", test))]
#[cfg_attr(target_arch = "wasm32", allow(unsafe_code))]
pub(crate) mod web;
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(target_os = "windows", allow(unsafe_code))]
mod xinput;

/// Identifies a connected gamepad. Ids are never reused within a process: a
/// pad that disconnects and comes back gets a new one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GamepadId(pub(crate) u32);

impl GamepadId {
    /// The id as a number (1 for the first pad seen, then counting up).
    pub fn get(self) -> u32 {
        self.0
    }
}

impl std::fmt::Display for GamepadId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "gamepad {}", self.0)
    }
}

/// A button on the standard layout.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GamepadButton {
    /// The bottom face button: Xbox A, PlayStation ✕, Nintendo B.
    South,
    /// The right face button: Xbox B, PlayStation ○, Nintendo A.
    East,
    /// The left face button: Xbox X, PlayStation □, Nintendo Y.
    West,
    /// The top face button: Xbox Y, PlayStation △, Nintendo X.
    North,
    /// The left bumper (LB, L1, L).
    LeftShoulder,
    /// The right bumper (RB, R1, R).
    RightShoulder,
    /// The left trigger (LT, L2, ZL), as a button; its travel is
    /// [`GamepadAxis::LeftTrigger`].
    LeftTrigger,
    /// The right trigger (RT, R2, ZR), as a button; its travel is
    /// [`GamepadAxis::RightTrigger`].
    RightTrigger,
    /// Select / Back / View / Share / Minus.
    Select,
    /// Start / Menu / Options / Plus.
    Start,
    /// The Guide / Home / PS button. Often taken by the system.
    Guide,
    /// Pressing the left stick in (L3).
    LeftStick,
    /// Pressing the right stick in (R3).
    RightStick,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
}

impl GamepadButton {
    /// Every button, in declaration order.
    pub const ALL: [GamepadButton; 17] = [
        Self::South,
        Self::East,
        Self::West,
        Self::North,
        Self::LeftShoulder,
        Self::RightShoulder,
        Self::LeftTrigger,
        Self::RightTrigger,
        Self::Select,
        Self::Start,
        Self::Guide,
        Self::LeftStick,
        Self::RightStick,
        Self::DPadUp,
        Self::DPadDown,
        Self::DPadLeft,
        Self::DPadRight,
    ];

    fn bit(self) -> u32 {
        1 << self as u32
    }
}

/// An axis on the standard layout. Sticks run from −1 to 1, +X right and +Y
/// **down**; triggers from 0 to 1.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GamepadAxis {
    LeftX,
    /// The left stick's vertical position: positive is down.
    LeftY,
    RightX,
    /// The right stick's vertical position: positive is down.
    RightY,
    /// How far the left trigger is pulled, 0 to 1.
    LeftTrigger,
    /// How far the right trigger is pulled, 0 to 1.
    RightTrigger,
}

impl GamepadAxis {
    /// Every axis, in declaration order.
    pub const ALL: [GamepadAxis; 6] = [
        Self::LeftX,
        Self::LeftY,
        Self::RightX,
        Self::RightY,
        Self::LeftTrigger,
        Self::RightTrigger,
    ];

    /// Whether this is a trigger (0 to 1) rather than a stick axis (−1 to 1).
    pub fn is_trigger(self) -> bool {
        matches!(self, Self::LeftTrigger | Self::RightTrigger)
    }

    fn clamp(self, value: f32) -> f32 {
        let value = if value.is_nan() { 0.0 } else { value };
        if self.is_trigger() {
            value.clamp(0.0, 1.0)
        } else {
            value.clamp(-1.0, 1.0)
        }
    }
}

/// What the platform says about a pad.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct GamepadInfo {
    /// The pad's name, as the platform gives it ("Xbox Wireless Controller").
    pub name: String,
    /// The USB (or Bluetooth) vendor id, where known.
    pub vendor_id: Option<u16>,
    /// The USB (or Bluetooth) product id, where known.
    pub product_id: Option<u16>,
}

impl GamepadInfo {
    /// A pad called `name`, ids unknown.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            vendor_id: None,
            product_id: None,
        }
    }

    /// With its vendor and product ids.
    pub fn with_ids(mut self, vendor: u16, product: u16) -> Self {
        self.vendor_id = Some(vendor);
        self.product_id = Some(product);
        self
    }
}

/// A pad's buttons and axes at one moment.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GamepadState {
    buttons: u32,
    axes: [f32; 6],
}

impl GamepadState {
    /// Whether `button` is down.
    pub fn pressed(&self, button: GamepadButton) -> bool {
        self.buttons & button.bit() != 0
    }

    /// Where `axis` is: −1 to 1 for a stick (+Y down), 0 to 1 for a trigger.
    pub fn axis(&self, axis: GamepadAxis) -> f32 {
        self.axes[axis as usize]
    }

    /// The left stick as `(x, y)`, +Y down.
    pub fn left_stick(&self) -> (f32, f32) {
        (self.axis(GamepadAxis::LeftX), self.axis(GamepadAxis::LeftY))
    }

    /// The right stick as `(x, y)`, +Y down.
    pub fn right_stick(&self) -> (f32, f32) {
        (
            self.axis(GamepadAxis::RightX),
            self.axis(GamepadAxis::RightY),
        )
    }

    /// The buttons that are down.
    pub fn pressed_buttons(&self) -> impl Iterator<Item = GamepadButton> + '_ {
        GamepadButton::ALL.into_iter().filter(|b| self.pressed(*b))
    }

    /// This state with `button` down or up.
    pub fn with_button(mut self, button: GamepadButton, pressed: bool) -> Self {
        self.set_button(button, pressed);
        self
    }

    /// This state with `axis` at `value` (clamped to the axis's range).
    pub fn with_axis(mut self, axis: GamepadAxis, value: f32) -> Self {
        self.set_axis(axis, value);
        self
    }

    fn set_button(&mut self, button: GamepadButton, pressed: bool) {
        if pressed {
            self.buttons |= button.bit();
        } else {
            self.buttons &= !button.bit();
        }
    }

    fn set_axis(&mut self, axis: GamepadAxis, value: f32) {
        self.axes[axis as usize] = axis.clamp(value);
    }
}

/// Something that happened on a gamepad.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum GamepadEvent {
    /// A pad connected (or was already connected when gamepad support
    /// started).
    Connected { id: GamepadId, info: GamepadInfo },
    /// A pad went away. Its id isn't used again.
    Disconnected { id: GamepadId },
    /// A button went down (`pressed`) or up.
    Button {
        id: GamepadId,
        button: GamepadButton,
        pressed: bool,
    },
    /// An axis moved to `value`.
    Axis {
        id: GamepadId,
        axis: GamepadAxis,
        value: f32,
    },
}

impl GamepadEvent {
    /// The pad this is about.
    pub fn id(&self) -> GamepadId {
        match self {
            Self::Connected { id, .. }
            | Self::Disconnected { id }
            | Self::Button { id, .. }
            | Self::Axis { id, .. } => *id,
        }
    }
}

/// How far a trigger travels before it counts as pressed, on pads with no
/// digital trigger of their own (XInput's `XINPUT_GAMEPAD_TRIGGER_THRESHOLD`,
/// 30 of 255).
#[cfg_attr(
    any(target_os = "macos", target_os = "ios", target_arch = "wasm32"),
    allow(dead_code)
)]
pub(crate) const TRIGGER_PRESSED: f32 = 30.0 / 255.0;

/// One change a backend read from a pad.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Change {
    Button(GamepadButton, bool),
    Axis(GamepadAxis, f32),
}

#[derive(Debug)]
struct Pad {
    id: GamepadId,
    info: GamepadInfo,
    state: GamepadState,
}

/// The connected pads, and the events not yet delivered.
#[derive(Debug, Default)]
pub(crate) struct Registry {
    pads: Vec<Pad>,
    last_id: u32,
    events: Vec<GamepadEvent>,
}

// Polling backends diff snapshots; event-driven ones apply single changes.
#[cfg_attr(any(target_os = "android", target_arch = "wasm32"), allow(dead_code))]
impl Registry {
    pub(crate) const fn new() -> Self {
        Self {
            pads: Vec::new(),
            last_id: 0,
            events: Vec::new(),
        }
    }

    /// A pad connected, already in `state`.
    pub(crate) fn connect(&mut self, info: GamepadInfo, state: GamepadState) -> GamepadId {
        self.last_id += 1;
        let id = GamepadId(self.last_id);
        self.pads.push(Pad {
            id,
            info: info.clone(),
            state,
        });
        self.events.push(GamepadEvent::Connected { id, info });
        id
    }

    pub(crate) fn disconnect(&mut self, id: GamepadId) {
        if let Some(i) = self.pads.iter().position(|p| p.id == id) {
            self.pads.remove(i);
            self.events.push(GamepadEvent::Disconnected { id });
        }
    }

    fn pad(&mut self, id: GamepadId) -> Option<&mut Pad> {
        self.pads.iter_mut().find(|p| p.id == id)
    }

    /// Apply one change, with an event if it changed anything.
    pub(crate) fn apply(&mut self, id: GamepadId, change: Change) {
        let Some(pad) = self.pads.iter_mut().find(|p| p.id == id) else {
            return;
        };
        match change {
            Change::Button(button, pressed) => {
                if pad.state.pressed(button) != pressed {
                    pad.state.set_button(button, pressed);
                    self.events.push(GamepadEvent::Button {
                        id,
                        button,
                        pressed,
                    });
                }
            }
            Change::Axis(axis, value) => {
                let value = axis.clamp(value);
                if pad.state.axis(axis) != value {
                    pad.state.set_axis(axis, value);
                    self.events.push(GamepadEvent::Axis { id, axis, value });
                }
            }
        }
    }

    /// Bring a pad to `state`, with an event for each difference (buttons
    /// first, then axes): for backends that poll a whole snapshot.
    pub(crate) fn set_state(&mut self, id: GamepadId, state: GamepadState) {
        let Some(old) = self.pad(id).map(|p| p.state) else {
            return;
        };
        if old == state {
            return;
        }
        for button in GamepadButton::ALL {
            if old.pressed(button) != state.pressed(button) {
                self.apply(id, Change::Button(button, state.pressed(button)));
            }
        }
        for axis in GamepadAxis::ALL {
            self.apply(id, Change::Axis(axis, state.axis(axis)));
        }
    }

    pub(crate) fn state(&self, id: GamepadId) -> Option<GamepadState> {
        self.pads.iter().find(|p| p.id == id).map(|p| p.state)
    }

    pub(crate) fn take_events(&mut self) -> Vec<GamepadEvent> {
        std::mem::take(&mut self.events)
    }
}

static REGISTRY: Mutex<Registry> = Mutex::new(Registry::new());

/// Change the registry; if that queued events, wake the loop to deliver them.
pub(crate) fn update<R>(f: impl FnOnce(&mut Registry) -> R) -> R {
    let (r, woke) = {
        let mut reg = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        let r = f(&mut reg);
        (r, !reg.events.is_empty())
    };
    if woke {
        crate::platform::wake::wake();
    }
    r
}

fn read<R>(f: impl FnOnce(&Registry) -> R) -> R {
    f(&REGISTRY.lock().unwrap_or_else(|e| e.into_inner()))
}

/// The events not yet delivered. The backends' loops hand them to the
/// handler as [`Event::Gamepad`](crate::platform::Event::Gamepad) when they
/// wake.
pub(crate) fn take_events() -> Vec<GamepadEvent> {
    REGISTRY
        .lock()
        .map(|mut r| r.take_events())
        .unwrap_or_default()
}

/// Start gamepad support: open the platform's devices and begin listening.
/// Called on first use by [`gamepads`], [`state`] and
/// [`App::on_gamepad`](crate::App::on_gamepad); calling it again does
/// nothing. Pads already connected arrive as
/// [`Connected`](GamepadEvent::Connected) events.
pub fn start() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(os::start);
}

/// The connected pads, oldest first, with what the platform says about each.
pub fn gamepads() -> Vec<(GamepadId, GamepadInfo)> {
    start();
    os::refresh();
    read(|r| r.pads.iter().map(|p| (p.id, p.info.clone())).collect())
}

/// What the platform says about pad `id`, if it's connected.
pub fn info(id: GamepadId) -> Option<GamepadInfo> {
    read(|r| r.pads.iter().find(|p| p.id == id).map(|p| p.info.clone()))
}

/// Pad `id`'s buttons and axes now, or `None` if it isn't connected. Read
/// it each frame from a game's update; on Windows and Apple platforms this
/// also polls the pad.
pub fn state(id: GamepadId) -> Option<GamepadState> {
    start();
    os::refresh();
    read(|r| r.state(id))
}

/// Rumble pad `id` for `duration`: `low` drives the low-frequency (strong,
/// left) motor and `high` the high-frequency (weak, right) one, each from 0
/// (off) to 1. A zero duration, or both strengths 0, stops it. Returns
/// whether the pad can rumble here (see the table in the [module
/// docs](crate::gamepad)).
pub fn set_rumble(id: GamepadId, low: f32, high: f32, duration: Duration) -> bool {
    let (low, high) = (low.clamp(0.0, 1.0), high.clamp(0.0, 1.0));
    if read(|r| !r.pads.iter().any(|p| p.id == id)) {
        return false;
    }
    os::set_rumble(id, low, high, duration)
}

#[cfg(target_os = "windows")]
use xinput as os;

#[cfg(target_os = "linux")]
use evdev as os;

#[cfg(any(target_os = "macos", target_os = "ios"))]
use apple as os;

#[cfg(target_arch = "wasm32")]
use web as os;

#[cfg(target_os = "android")]
use android as os;

#[cfg(not(any(
    target_os = "windows",
    target_os = "linux",
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_arch = "wasm32"
)))]
mod os {
    pub(super) fn start() {}
    pub(super) fn refresh() {}
    pub(super) fn set_rumble(_: super::GamepadId, _: f32, _: f32, _: super::Duration) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_reports_only_changes() {
        let mut r = Registry::new();
        let id = r.connect(GamepadInfo::new("pad"), GamepadState::default());
        assert_eq!(
            r.take_events(),
            [GamepadEvent::Connected {
                id,
                info: GamepadInfo::new("pad")
            }]
        );
        r.apply(id, Change::Button(GamepadButton::South, true));
        r.apply(id, Change::Button(GamepadButton::South, true));
        r.apply(id, Change::Axis(GamepadAxis::LeftX, 2.0));
        r.apply(id, Change::Axis(GamepadAxis::LeftTrigger, -1.0));
        assert_eq!(
            r.take_events(),
            [
                GamepadEvent::Button {
                    id,
                    button: GamepadButton::South,
                    pressed: true
                },
                GamepadEvent::Axis {
                    id,
                    axis: GamepadAxis::LeftX,
                    value: 1.0
                },
            ]
        );
        let s = r.state(id).unwrap();
        assert!(s.pressed(GamepadButton::South));
        assert_eq!(s.left_stick(), (1.0, 0.0));
        r.disconnect(id);
        r.disconnect(id);
        assert_eq!(r.take_events(), [GamepadEvent::Disconnected { id }]);
        assert!(r.state(id).is_none());
        // Ids aren't reused.
        let again = r.connect(GamepadInfo::new("pad"), GamepadState::default());
        assert_ne!(again, id);
    }

    #[test]
    fn set_state_diffs_snapshots() {
        let mut r = Registry::new();
        let id = r.connect(GamepadInfo::default(), GamepadState::default());
        r.take_events();
        let s = GamepadState::default()
            .with_button(GamepadButton::Start, true)
            .with_axis(GamepadAxis::RightY, -0.5);
        r.set_state(id, s);
        r.set_state(id, s);
        assert_eq!(
            r.take_events(),
            [
                GamepadEvent::Button {
                    id,
                    button: GamepadButton::Start,
                    pressed: true
                },
                GamepadEvent::Axis {
                    id,
                    axis: GamepadAxis::RightY,
                    value: -0.5
                },
            ]
        );
        r.set_state(id, GamepadState::default());
        assert_eq!(r.take_events().len(), 2);
    }

    #[test]
    fn state_accessors() {
        let s = GamepadState::default()
            .with_button(GamepadButton::DPadLeft, true)
            .with_button(GamepadButton::North, true)
            .with_axis(GamepadAxis::LeftTrigger, 0.25)
            .with_axis(GamepadAxis::LeftY, f32::NAN);
        assert_eq!(
            s.pressed_buttons().collect::<Vec<_>>(),
            [GamepadButton::North, GamepadButton::DPadLeft]
        );
        assert_eq!(s.axis(GamepadAxis::LeftTrigger), 0.25);
        assert_eq!(s.axis(GamepadAxis::LeftY), 0.0);
        assert!(GamepadAxis::RightTrigger.is_trigger() && !GamepadAxis::RightX.is_trigger());
    }
}
