//! macOS and iOS: `GameController.framework`, loaded at run time.
//!
//! A timer on the main run loop polls `+[GCController controllers]` and each
//! one's `extendedGamepad` profile: 60 times a second while a pad is
//! connected, twice a second otherwise. [`state`](super::state) polls too,
//! when called on the main thread. Pads with only a micro gamepad profile
//! (the Siri Remote) are left out. GameController has no vendor or product
//! ids, and rumble needs Core Haptics, so neither is offered.

use super::{GamepadAxis, GamepadButton, GamepadId, GamepadInfo, GamepadState, update};
use crate::platform::native::objc::{Id, class, msg, on_main_thread, rust_string, sel, with_pool};
use std::ffi::c_void;
use std::sync::Mutex;
use std::time::Duration;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopCommonModes: *const c_void;
    fn CFRunLoopGetMain() -> *mut c_void;
    fn CFAbsoluteTimeGetCurrent() -> f64;
    fn CFRunLoopTimerCreate(
        allocator: *const c_void,
        fire_date: f64,
        interval: f64,
        flags: usize,
        order: isize,
        callout: unsafe extern "C" fn(*mut c_void, *mut c_void),
        context: *mut c_void,
    ) -> *mut c_void;
    fn CFRunLoopAddTimer(rl: *mut c_void, timer: *mut c_void, mode: *const c_void);
    fn CFRunLoopTimerSetNextFireDate(timer: *mut c_void, date: f64);
}

unsafe extern "C" {
    fn dlopen(path: *const std::ffi::c_char, mode: i32) -> *mut c_void;
}

const FAST: f64 = 1.0 / 60.0;
const SLOW: f64 = 0.5;

/// The controllers seen, retained, and their ids.
struct Known(Vec<(usize, GamepadId)>);
static KNOWN: Mutex<Known> = Mutex::new(Known(Vec::new()));

pub(super) fn start() {
    const RTLD_LAZY: i32 = 1;
    let path = c"/System/Library/Frameworks/GameController.framework/GameController";
    if unsafe { dlopen(path.as_ptr(), RTLD_LAZY) }.is_null() || class("GCController").is_null() {
        return;
    }
    if on_main_thread() {
        poll();
    }
    unsafe {
        let timer = CFRunLoopTimerCreate(
            std::ptr::null(),
            CFAbsoluteTimeGetCurrent() + FAST,
            FAST,
            0,
            0,
            fire,
            std::ptr::null_mut(),
        );
        if !timer.is_null() {
            CFRunLoopAddTimer(CFRunLoopGetMain(), timer, kCFRunLoopCommonModes);
        }
    }
}

unsafe extern "C" fn fire(timer: *mut c_void, _info: *mut c_void) {
    if !poll() {
        unsafe { CFRunLoopTimerSetNextFireDate(timer, CFAbsoluteTimeGetCurrent() + SLOW) };
    }
}

pub(super) fn refresh() {
    if on_main_thread() && !KNOWN.lock().unwrap_or_else(|e| e.into_inner()).0.is_empty() {
        poll();
    }
}

pub(super) fn set_rumble(_: GamepadId, _: f32, _: f32, _: Duration) -> bool {
    false
}

fn responds(obj: Id, name: &str) -> bool {
    let s = sel(name);
    !obj.is_null() && unsafe { msg!(obj, "respondsToSelector:", (s: Id) -> i8) } != 0
}

/// `obj.name`, or nil when `obj` doesn't have it (older systems).
fn get(obj: Id, name: &str) -> Id {
    if !responds(obj, name) {
        return std::ptr::null_mut();
    }
    unsafe { msg!(obj, name, () -> Id) }
}

/// A `GCControllerButtonInput`'s pressed flag.
fn pressed(button: Id) -> bool {
    !button.is_null() && unsafe { msg!(button, "isPressed", () -> i8) } != 0
}

/// A `GCControllerButtonInput`'s or `GCControllerAxisInput`'s value.
fn value(input: Id) -> f32 {
    if input.is_null() {
        return 0.0;
    }
    unsafe { msg!(input, "value", () -> f32) }
}

/// An extended gamepad's state. GameController's Y is up; ours is down.
fn read(pad: Id) -> GamepadState {
    let mut s = GamepadState::default();
    let buttons = [
        ("buttonA", GamepadButton::South),
        ("buttonB", GamepadButton::East),
        ("buttonX", GamepadButton::West),
        ("buttonY", GamepadButton::North),
        ("leftShoulder", GamepadButton::LeftShoulder),
        ("rightShoulder", GamepadButton::RightShoulder),
        ("leftTrigger", GamepadButton::LeftTrigger),
        ("rightTrigger", GamepadButton::RightTrigger),
        ("buttonOptions", GamepadButton::Select),
        ("buttonMenu", GamepadButton::Start),
        ("buttonHome", GamepadButton::Guide),
        ("leftThumbstickButton", GamepadButton::LeftStick),
        ("rightThumbstickButton", GamepadButton::RightStick),
    ];
    for (name, button) in buttons {
        s.set_button(button, pressed(get(pad, name)));
    }
    let dpad = get(pad, "dpad");
    for (name, button) in [
        ("up", GamepadButton::DPadUp),
        ("down", GamepadButton::DPadDown),
        ("left", GamepadButton::DPadLeft),
        ("right", GamepadButton::DPadRight),
    ] {
        s.set_button(button, pressed(get(dpad, name)));
    }
    for (stick, x, y) in [
        ("leftThumbstick", GamepadAxis::LeftX, GamepadAxis::LeftY),
        ("rightThumbstick", GamepadAxis::RightX, GamepadAxis::RightY),
    ] {
        let stick = get(pad, stick);
        s.set_axis(x, value(get(stick, "xAxis")));
        s.set_axis(y, -value(get(stick, "yAxis")));
    }
    s.set_axis(GamepadAxis::LeftTrigger, value(get(pad, "leftTrigger")));
    s.set_axis(GamepadAxis::RightTrigger, value(get(pad, "rightTrigger")));
    s
}

/// Poll the controllers. Returns whether any is connected.
fn poll() -> bool {
    with_pool(|| {
        let mut known = KNOWN.lock().unwrap_or_else(|e| e.into_inner());
        let list = unsafe { msg!(class("GCController"), "controllers", () -> Id) };
        let count = if list.is_null() {
            0
        } else {
            unsafe { msg!(list, "count", () -> usize) }
        };
        let mut seen = Vec::with_capacity(count);
        for i in 0..count {
            let ctrl = unsafe { msg!(list, "objectAtIndex:", (i: usize) -> Id) };
            let pad = get(ctrl, "extendedGamepad");
            if pad.is_null() {
                continue;
            }
            let state = read(pad);
            let id = match known.0.iter().find(|(c, _)| *c == ctrl as usize) {
                Some(&(_, id)) => {
                    update(|r| r.set_state(id, state));
                    id
                }
                None => {
                    let name = rust_string(get(ctrl, "vendorName"))
                        .unwrap_or_else(|| "Game controller".to_owned());
                    unsafe { msg!(ctrl, "retain", () -> Id) };
                    let id = update(|r| r.connect(GamepadInfo::new(name), state));
                    known.0.push((ctrl as usize, id));
                    id
                }
            };
            seen.push(id);
        }
        known.0.retain(|&(ctrl, id)| {
            if seen.contains(&id) {
                return true;
            }
            update(|r| r.disconnect(id));
            unsafe { msg!(ctrl as Id, "release", () -> ()) };
            false
        });
        !known.0.is_empty()
    })
}
