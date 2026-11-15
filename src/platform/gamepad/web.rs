//! The web: the Gamepad API.
//!
//! `web/stipple.js` polls `navigator.getGamepads()` each animation frame
//! while a pad is connected and hands each pad's buttons and axes to the
//! module (`stipple_gamepad_*`), indexed as the standard mapping indexes
//! them. Pads the browser doesn't map to the standard layout are read as if
//! it did. Rumble goes through `vibrationActuator.playEffect("dual-rumble")`.

use super::{Change, GamepadAxis, GamepadButton, GamepadId, GamepadInfo, GamepadState, update};
use std::sync::Mutex;

/// The standard mapping's button indexes.
const BUTTONS: [GamepadButton; 17] = [
    GamepadButton::South,
    GamepadButton::East,
    GamepadButton::West,
    GamepadButton::North,
    GamepadButton::LeftShoulder,
    GamepadButton::RightShoulder,
    GamepadButton::LeftTrigger,
    GamepadButton::RightTrigger,
    GamepadButton::Select,
    GamepadButton::Start,
    GamepadButton::LeftStick,
    GamepadButton::RightStick,
    GamepadButton::DPadUp,
    GamepadButton::DPadDown,
    GamepadButton::DPadLeft,
    GamepadButton::DPadRight,
    GamepadButton::Guide,
];

/// The changes standard button `index` makes, at `value` (0 to 1) and
/// `pressed`. Buttons 6 and 7, the triggers, are axes too.
pub(crate) fn button(index: u32, value: f64, pressed: bool) -> Vec<Change> {
    let Some(&b) = BUTTONS.get(index as usize) else {
        return Vec::new();
    };
    let mut out = vec![Change::Button(b, pressed)];
    match index {
        6 => out.push(Change::Axis(GamepadAxis::LeftTrigger, value as f32)),
        7 => out.push(Change::Axis(GamepadAxis::RightTrigger, value as f32)),
        _ => {}
    }
    out
}

/// The change standard axis `index` makes (the web's Y is down, like ours).
pub(crate) fn axis(index: u32, value: f64) -> Option<Change> {
    let axis = match index {
        0 => GamepadAxis::LeftX,
        1 => GamepadAxis::LeftY,
        2 => GamepadAxis::RightX,
        3 => GamepadAxis::RightY,
        _ => return None,
    };
    Some(Change::Axis(axis, value as f32))
}

/// A pad's name and ids from its `Gamepad.id`: Chromium's
/// `"Name (STANDARD GAMEPAD Vendor: 045e Product: 028e)"` or Firefox's
/// `"045e-028e-Name"`.
pub(crate) fn parse_id(id: &str) -> GamepadInfo {
    let hex = |s: &str| u16::from_str_radix(s.trim(), 16).ok();
    let mut parts = id.splitn(3, '-');
    if let (Some(v), Some(p), Some(name)) = (parts.next(), parts.next(), parts.next())
        && v.len() == 4
        && p.len() == 4
        && let (Some(v), Some(p)) = (hex(v), hex(p))
    {
        return GamepadInfo::new(name.trim()).ids(v, p);
    }
    let name = id.split(" (").next().unwrap_or(id).trim();
    let after = |key: &str| {
        let i = id.find(key)? + key.len();
        let rest = id[i..].trim_start();
        hex(rest.get(..4)?)
    };
    let info = GamepadInfo::new(if name.is_empty() { "Gamepad" } else { name });
    match (after("Vendor:"), after("Product:")) {
        (Some(v), Some(p)) => info.ids(v, p),
        _ => info,
    }
}

/// The page's gamepad indexes and the pads they are.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
static PADS: Mutex<Vec<(u32, GamepadId)>> = Mutex::new(Vec::new());

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn pad(index: u32) -> Option<GamepadId> {
    let pads = PADS.lock().unwrap_or_else(|e| e.into_inner());
    pads.iter().find(|(i, _)| *i == index).map(|(_, id)| *id)
}

/// The page saw pad `index` connect, with this `Gamepad.id`.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn connect(index: u32, id: &str) {
    disconnect(index);
    let gid = update(|r| r.connect(parse_id(id), GamepadState::default()));
    PADS.lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((index, gid));
}

/// The page saw pad `index` go.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn disconnect(index: u32) {
    let mut pads = PADS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(i) = pads.iter().position(|(n, _)| *n == index) {
        let (_, id) = pads.remove(i);
        drop(pads);
        update(|r| r.disconnect(id));
    }
}

/// Standard button `button_index` of pad `index` is at `value`, `pressed`.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn set_button(index: u32, button_index: u32, value: f64, pressed: bool) {
    if let Some(id) = pad(index) {
        let changes = button(button_index, value, pressed);
        update(|r| changes.into_iter().for_each(|c| r.apply(id, c)));
    }
}

/// Standard axis `axis_index` of pad `index` is at `value`.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn set_axis(index: u32, axis_index: u32, value: f64) {
    if let (Some(id), Some(c)) = (pad(index), axis(axis_index, value)) {
        update(|r| r.apply(id, c));
    }
}

#[cfg(target_arch = "wasm32")]
pub(super) fn start() {}

#[cfg(target_arch = "wasm32")]
pub(super) fn refresh() {}

#[cfg(target_arch = "wasm32")]
pub(super) fn set_rumble(
    id: GamepadId,
    low: f32,
    high: f32,
    duration: std::time::Duration,
) -> bool {
    let index = {
        let pads = PADS.lock().unwrap_or_else(|e| e.into_inner());
        pads.iter().find(|(_, g)| *g == id).map(|(i, _)| *i)
    };
    let Some(index) = index else {
        return false;
    };
    crate::platform::backend::web::gamepad_rumble(index, low, high, duration.as_secs_f64() * 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_standard_buttons_and_axes() {
        assert_eq!(
            button(0, 1.0, true),
            [Change::Button(GamepadButton::South, true)]
        );
        assert_eq!(
            button(16, 0.0, false),
            [Change::Button(GamepadButton::Guide, false)]
        );
        assert_eq!(
            button(7, 0.75, true),
            [
                Change::Button(GamepadButton::RightTrigger, true),
                Change::Axis(GamepadAxis::RightTrigger, 0.75)
            ]
        );
        assert!(button(17, 1.0, true).is_empty());
        assert_eq!(axis(1, 0.5), Some(Change::Axis(GamepadAxis::LeftY, 0.5)));
        assert_eq!(axis(4, 0.5), None);
    }

    #[test]
    fn parses_ids() {
        let chrome =
            parse_id("Xbox 360 Controller (XInput STANDARD GAMEPAD Vendor: 045e Product: 028e)");
        assert_eq!(chrome.name, "Xbox 360 Controller");
        assert_eq!(
            (chrome.vendor_id, chrome.product_id),
            (Some(0x045e), Some(0x028e))
        );
        let firefox = parse_id("054c-09cc-Wireless Controller");
        assert_eq!(firefox.name, "Wireless Controller");
        assert_eq!(
            (firefox.vendor_id, firefox.product_id),
            (Some(0x054c), Some(0x09cc))
        );
        assert_eq!(parse_id("Some Pad"), GamepadInfo::new("Some Pad"));
    }
}
