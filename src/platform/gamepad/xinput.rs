//! Windows: XInput.
//!
//! `xinput1_4.dll` (Windows 8 and later), else `xinput9_1_0.dll`, loaded at
//! run time. A thread polls the four user slots: connected ones every 8 ms,
//! empty ones every second (`XInputGetState` on an empty slot is slow), and
//! [`state`](super::state) polls the connected ones again when a game reads
//! them. The Guide button comes from the undocumented `XInputGetStateEx`
//! (ordinal 100 of `xinput1_4.dll`) where it's there.

use super::{GamepadAxis, GamepadButton, GamepadState, TRIGGER_PRESSED};

/// `XINPUT_GAMEPAD`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub(crate) struct XGamepad {
    pub buttons: u16,
    pub left_trigger: u8,
    pub right_trigger: u8,
    pub thumb_lx: i16,
    pub thumb_ly: i16,
    pub thumb_rx: i16,
    pub thumb_ry: i16,
}

/// `wButtons` bits, in [`GamepadButton`] terms.
const BUTTONS: [(u16, GamepadButton); 15] = [
    (0x0001, GamepadButton::DPadUp),
    (0x0002, GamepadButton::DPadDown),
    (0x0004, GamepadButton::DPadLeft),
    (0x0008, GamepadButton::DPadRight),
    (0x0010, GamepadButton::Start),
    (0x0020, GamepadButton::Select),
    (0x0040, GamepadButton::LeftStick),
    (0x0080, GamepadButton::RightStick),
    (0x0100, GamepadButton::LeftShoulder),
    (0x0200, GamepadButton::RightShoulder),
    // XInputGetStateEx only.
    (0x0400, GamepadButton::Guide),
    (0x1000, GamepadButton::South),
    (0x2000, GamepadButton::East),
    (0x4000, GamepadButton::West),
    (0x8000, GamepadButton::North),
];

fn stick(v: i16) -> f32 {
    (v as f32 / 32767.0).clamp(-1.0, 1.0)
}

/// A pad's state from its `XINPUT_GAMEPAD`. XInput's Y is up; ours is down.
pub(crate) fn to_state(g: &XGamepad) -> GamepadState {
    let mut s = GamepadState::default();
    for (bit, button) in BUTTONS {
        s.set_button(button, g.buttons & bit != 0);
    }
    let lt = g.left_trigger as f32 / 255.0;
    let rt = g.right_trigger as f32 / 255.0;
    s.set_button(GamepadButton::LeftTrigger, lt > TRIGGER_PRESSED);
    s.set_button(GamepadButton::RightTrigger, rt > TRIGGER_PRESSED);
    s.set_axis(GamepadAxis::LeftX, stick(g.thumb_lx));
    s.set_axis(GamepadAxis::LeftY, -stick(g.thumb_ly));
    s.set_axis(GamepadAxis::RightX, stick(g.thumb_rx));
    s.set_axis(GamepadAxis::RightY, -stick(g.thumb_ry));
    s.set_axis(GamepadAxis::LeftTrigger, lt);
    s.set_axis(GamepadAxis::RightTrigger, rt);
    s
}

#[cfg(target_os = "windows")]
pub(super) use os::{refresh, set_rumble, start};

#[cfg(target_os = "windows")]
mod os {
    use super::{XGamepad, to_state};
    use crate::platform::gamepad::{GamepadId, GamepadInfo, update};
    use std::ffi::c_void;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *const c_void;
    }

    /// `XINPUT_STATE`, with room for `XInputGetStateEx`'s extra field.
    #[repr(C)]
    #[derive(Default)]
    struct XState {
        packet: u32,
        pad: XGamepad,
        _reserved: u32,
    }

    /// `XINPUT_VIBRATION`.
    #[repr(C)]
    struct XVibration {
        left: u16,
        right: u16,
    }

    type GetState = unsafe extern "system" fn(u32, *mut XState) -> u32;
    type SetState = unsafe extern "system" fn(u32, *mut XVibration) -> u32;

    const ERROR_SUCCESS: u32 = 0;
    const SLOTS: usize = 4;

    struct Api {
        get: GetState,
        set: Option<SetState>,
    }

    #[derive(Default)]
    struct Slot {
        id: Option<GamepadId>,
        packet: u32,
        /// When an empty slot was last tried.
        tried: Option<Instant>,
        /// When the current rumble ends.
        rumble_until: Option<Instant>,
    }

    struct XInput {
        api: Api,
        slots: [Slot; SLOTS],
    }

    static XINPUT: Mutex<Option<XInput>> = Mutex::new(None);

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn load() -> Option<Api> {
        for dll in ["xinput1_4.dll", "xinput9_1_0.dll", "xinput1_3.dll"] {
            let module = unsafe { LoadLibraryW(wide(dll).as_ptr()) };
            if module.is_null() {
                continue;
            }
            // XInputGetStateEx (ordinal 100) also reports the Guide button.
            let ex = unsafe { GetProcAddress(module, 100usize as *const u8) };
            let get = if ex.is_null() {
                unsafe { GetProcAddress(module, c"XInputGetState".as_ptr().cast()) }
            } else {
                ex
            };
            if get.is_null() {
                continue;
            }
            let set = unsafe { GetProcAddress(module, c"XInputSetState".as_ptr().cast()) };
            return Some(Api {
                get: unsafe { std::mem::transmute::<*const c_void, GetState>(get) },
                set: (!set.is_null())
                    .then(|| unsafe { std::mem::transmute::<*const c_void, SetState>(set) }),
            });
        }
        None
    }

    /// Poll the slots: connected ones always, empty ones once a second (or
    /// all of them with `all`). Returns whether any pad is connected.
    fn poll(x: &mut XInput, all: bool) -> bool {
        let now = Instant::now();
        let mut any = false;
        for (i, slot) in x.slots.iter_mut().enumerate() {
            if slot.id.is_none()
                && !all
                && slot
                    .tried
                    .is_some_and(|t| now.duration_since(t) < Duration::from_secs(1))
            {
                continue;
            }
            let mut st = XState::default();
            let ok = unsafe { (x.api.get)(i as u32, &mut st) } == ERROR_SUCCESS;
            match (ok, slot.id) {
                (true, Some(id)) => {
                    any = true;
                    if st.packet != slot.packet {
                        slot.packet = st.packet;
                        update(|r| r.set_state(id, to_state(&st.pad)));
                    }
                }
                (true, None) => {
                    any = true;
                    slot.packet = st.packet;
                    let info = GamepadInfo::new(format!("XInput controller {}", i + 1));
                    slot.id = Some(update(|r| r.connect(info, to_state(&st.pad))));
                }
                (false, Some(id)) => {
                    slot.id = None;
                    slot.rumble_until = None;
                    slot.tried = Some(now);
                    update(|r| r.disconnect(id));
                }
                (false, None) => slot.tried = Some(now),
            }
            // A rumble that ran its time.
            if slot.rumble_until.is_some_and(|t| t <= now) {
                slot.rumble_until = None;
                if let Some(set) = x.api.set {
                    let mut v = XVibration { left: 0, right: 0 };
                    unsafe { set(i as u32, &mut v) };
                }
            }
        }
        any
    }

    pub(in crate::platform::gamepad) fn start() {
        let Some(api) = load() else {
            return;
        };
        let mut x = XInput {
            api,
            slots: Default::default(),
        };
        poll(&mut x, true);
        *XINPUT.lock().unwrap_or_else(|e| e.into_inner()) = Some(x);
        let _ = std::thread::Builder::new()
            .name("stipple-xinput".into())
            .spawn(|| {
                loop {
                    let any = match XINPUT.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                        Some(x) => poll(x, false),
                        None => return,
                    };
                    std::thread::sleep(Duration::from_millis(if any { 8 } else { 250 }));
                }
            });
    }

    pub(in crate::platform::gamepad) fn refresh() {
        let mut guard = XINPUT.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(x) = guard.as_mut() {
            poll(x, false);
        }
    }

    pub(in crate::platform::gamepad) fn set_rumble(
        id: GamepadId,
        low: f32,
        high: f32,
        duration: Duration,
    ) -> bool {
        let mut guard = XINPUT.lock().unwrap_or_else(|e| e.into_inner());
        let Some(x) = guard.as_mut() else {
            return false;
        };
        let Some(set) = x.api.set else {
            return false;
        };
        let Some(i) = x.slots.iter().position(|s| s.id == Some(id)) else {
            return false;
        };
        let stop = duration.is_zero() || (low == 0.0 && high == 0.0);
        let mut v = if stop {
            XVibration { left: 0, right: 0 }
        } else {
            XVibration {
                left: (low * 65535.0) as u16,
                right: (high * 65535.0) as u16,
            }
        };
        unsafe { set(i as u32, &mut v) };
        x.slots[i].rumble_until = (!stop).then(|| Instant::now() + duration);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::gamepad::{GamepadEvent, GamepadInfo, Registry};

    #[test]
    fn maps_buttons_sticks_and_triggers() {
        let g = XGamepad {
            buttons: 0x1000 | 0x4000 | 0x0001 | 0x0400 | 0x0020,
            left_trigger: 255,
            right_trigger: 20,
            thumb_lx: -32768,
            thumb_ly: 32767,
            thumb_rx: 0,
            thumb_ry: -16384,
        };
        let s = to_state(&g);
        assert_eq!(
            s.pressed_buttons().collect::<Vec<_>>(),
            [
                GamepadButton::South,
                GamepadButton::West,
                GamepadButton::LeftTrigger,
                GamepadButton::Select,
                GamepadButton::Guide,
                GamepadButton::DPadUp,
            ]
        );
        // Up on the stick is negative Y.
        assert_eq!(s.left_stick(), (-1.0, -1.0));
        assert!((s.axis(GamepadAxis::RightY) - 0.5).abs() < 1e-3);
        assert_eq!(s.axis(GamepadAxis::LeftTrigger), 1.0);
        assert!(!s.pressed(GamepadButton::RightTrigger));
    }

    #[test]
    fn polls_diff_into_events() {
        let mut r = Registry::new();
        let id = r.connect(GamepadInfo::new("x"), to_state(&XGamepad::default()));
        r.take_events();
        r.set_state(
            id,
            to_state(&XGamepad {
                buttons: 0x2000,
                right_trigger: 200,
                ..Default::default()
            }),
        );
        let events = r.take_events();
        assert_eq!(
            events[..2],
            [
                GamepadEvent::Button {
                    id,
                    button: GamepadButton::East,
                    pressed: true
                },
                GamepadEvent::Button {
                    id,
                    button: GamepadButton::RightTrigger,
                    pressed: true
                },
            ]
        );
        assert!(matches!(
            events[2],
            GamepadEvent::Axis { axis: GamepadAxis::RightTrigger, value, .. } if (value - 200.0 / 255.0).abs() < 1e-6
        ));
        assert_eq!(events.len(), 3);
    }
}
