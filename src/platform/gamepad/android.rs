//! Android: the activity's input events from gamepad and joystick sources.
//!
//! The NativeActivity backend hands key events from `AINPUT_SOURCE_GAMEPAD`
//! (`AKEYCODE_BUTTON_*` and the D-pad keys) and motion events from
//! `AINPUT_SOURCE_JOYSTICK` (`AXIS_X`/`Y`, `Z`/`RZ` for the right stick,
//! `HAT_X`/`HAT_Y`, `LTRIGGER`/`RTRIGGER` or `BRAKE`/`GAS`) here. A pad is
//! connected when its first event arrives, named after its `InputDevice`.

use super::{Change, GamepadAxis, GamepadButton, GamepadId, GamepadInfo, TRIGGER_PRESSED};
use super::{GamepadState, update};
use std::sync::Mutex;

pub(crate) const AINPUT_SOURCE_GAMEPAD: i32 = 0x0000_0401;
pub(crate) const AINPUT_SOURCE_JOYSTICK: i32 = 0x0100_0010;

const AXIS_X: i32 = 0;
const AXIS_Y: i32 = 1;
const AXIS_Z: i32 = 11;
const AXIS_RZ: i32 = 14;
const AXIS_HAT_X: i32 = 15;
const AXIS_HAT_Y: i32 = 16;
const AXIS_LTRIGGER: i32 = 17;
const AXIS_RTRIGGER: i32 = 18;
const AXIS_GAS: i32 = 22;
const AXIS_BRAKE: i32 = 23;

/// The gamepad button an `AKEYCODE_*` is.
pub(crate) fn button(keycode: i32) -> Option<GamepadButton> {
    Some(match keycode {
        96 => GamepadButton::South,  // BUTTON_A
        97 => GamepadButton::East,   // BUTTON_B
        99 => GamepadButton::West,   // BUTTON_X
        100 => GamepadButton::North, // BUTTON_Y
        102 => GamepadButton::LeftShoulder,
        103 => GamepadButton::RightShoulder,
        104 => GamepadButton::LeftTrigger,
        105 => GamepadButton::RightTrigger,
        106 => GamepadButton::LeftStick,
        107 => GamepadButton::RightStick,
        108 => GamepadButton::Start,
        109 => GamepadButton::Select,
        110 => GamepadButton::Guide, // BUTTON_MODE
        19 => GamepadButton::DPadUp,
        20 => GamepadButton::DPadDown,
        21 => GamepadButton::DPadLeft,
        22 => GamepadButton::DPadRight,
        _ => return None,
    })
}

/// The changes a joystick motion event makes, reading its axes with `axis`.
pub(crate) fn motion(axis: impl Fn(i32) -> f32) -> Vec<Change> {
    let lt = axis(AXIS_LTRIGGER).max(axis(AXIS_BRAKE));
    let rt = axis(AXIS_RTRIGGER).max(axis(AXIS_GAS));
    let (hx, hy) = (axis(AXIS_HAT_X), axis(AXIS_HAT_Y));
    vec![
        Change::Axis(GamepadAxis::LeftX, axis(AXIS_X)),
        Change::Axis(GamepadAxis::LeftY, axis(AXIS_Y)),
        Change::Axis(GamepadAxis::RightX, axis(AXIS_Z)),
        Change::Axis(GamepadAxis::RightY, axis(AXIS_RZ)),
        Change::Axis(GamepadAxis::LeftTrigger, lt),
        Change::Axis(GamepadAxis::RightTrigger, rt),
        Change::Button(GamepadButton::LeftTrigger, lt > TRIGGER_PRESSED),
        Change::Button(GamepadButton::RightTrigger, rt > TRIGGER_PRESSED),
        Change::Button(GamepadButton::DPadLeft, hx < -0.5),
        Change::Button(GamepadButton::DPadRight, hx > 0.5),
        Change::Button(GamepadButton::DPadUp, hy < -0.5),
        Change::Button(GamepadButton::DPadDown, hy > 0.5),
    ]
}

/// Input device ids and the pads they are.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
static DEVICES: Mutex<Vec<(i32, GamepadId)>> = Mutex::new(Vec::new());

/// Apply `changes` from input device `device`, connecting it first (named by
/// `info`) if it's new.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn input(device: i32, info: impl FnOnce() -> GamepadInfo, changes: &[Change]) {
    let mut devices = DEVICES.lock().unwrap_or_else(|e| e.into_inner());
    let id = match devices.iter().find(|(d, _)| *d == device) {
        Some(&(_, id)) => id,
        None => {
            let id = update(|r| r.connect(info(), GamepadState::default()));
            devices.push((device, id));
            id
        }
    };
    update(|r| {
        for &c in changes {
            r.apply(id, c);
        }
    });
}

#[cfg(target_os = "android")]
pub(super) fn start() {}

#[cfg(target_os = "android")]
pub(super) fn refresh() {}

#[cfg(target_os = "android")]
pub(super) fn set_rumble(_: GamepadId, _: f32, _: f32, _: std::time::Duration) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_keycodes() {
        assert_eq!(button(96), Some(GamepadButton::South));
        assert_eq!(button(99), Some(GamepadButton::West));
        assert_eq!(button(100), Some(GamepadButton::North));
        assert_eq!(button(110), Some(GamepadButton::Guide));
        assert_eq!(button(21), Some(GamepadButton::DPadLeft));
        assert_eq!(button(62), None); // SPACE
    }

    #[test]
    fn maps_axes() {
        let changes = motion(|a| match a {
            AXIS_X => -1.0,
            AXIS_RZ => 0.5,
            AXIS_GAS => 0.8,
            AXIS_HAT_Y => 1.0,
            _ => 0.0,
        });
        assert!(changes.contains(&Change::Axis(GamepadAxis::LeftX, -1.0)));
        assert!(changes.contains(&Change::Axis(GamepadAxis::RightY, 0.5)));
        assert!(changes.contains(&Change::Axis(GamepadAxis::RightTrigger, 0.8)));
        assert!(changes.contains(&Change::Button(GamepadButton::RightTrigger, true)));
        assert!(changes.contains(&Change::Button(GamepadButton::DPadDown, true)));
        assert!(changes.contains(&Change::Button(GamepadButton::DPadUp, false)));
    }
}
