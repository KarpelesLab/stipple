//! Linux: evdev.
//!
//! Every `/dev/input/event*` that has the gamepad buttons (`BTN_SOUTH`, a.k.a.
//! `BTN_GAMEPAD`) and absolute axes is a pad. A thread `poll`s the open
//! devices and an inotify watch on `/dev/input`, which reports pads plugged
//! in (re-scanning when a node is created or its permissions change, as udev
//! grants access a moment after creating it) and unplugged. Events are mapped
//! per the kernel's `Documentation/input/gamepad.rst`, with the usual
//! departures: triggers on `ABS_Z`/`ABS_RZ` (xpad, hid-playstation) or
//! `ABS_BRAKE`/`ABS_GAS`, a right stick on `ABS_Z`/`ABS_RZ` on pads without
//! `ABS_RX`/`ABS_RY`, and xpad's X/Y reported as `BTN_X`/`BTN_Y` (the codes
//! of `BTN_NORTH`/`BTN_WEST`). Devices the user can't open (not in the
//! `input` group, no udev `uaccess`) are skipped: no pads, no error.
//!
//! Rumble uploads an `FF_RUMBLE` effect (the device must open read-write).

use super::{Change, GamepadAxis, GamepadButton, GamepadState, TRIGGER_PRESSED};

pub(crate) const EV_SYN: u16 = 0x00;
pub(crate) const EV_KEY: u16 = 0x01;
pub(crate) const EV_ABS: u16 = 0x03;
pub(crate) const SYN_REPORT: u16 = 0;
pub(crate) const SYN_DROPPED: u16 = 3;

pub(crate) const BTN_SOUTH: u16 = 0x130;
const BTN_NORTH: u16 = 0x133;
const BTN_WEST: u16 = 0x134;
const BTN_TL2: u16 = 0x138;
const BTN_TR2: u16 = 0x139;

const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;
const ABS_Z: u16 = 0x02;
const ABS_RX: u16 = 0x03;
const ABS_RY: u16 = 0x04;
const ABS_RZ: u16 = 0x05;
const ABS_GAS: u16 = 0x09;
const ABS_BRAKE: u16 = 0x0a;
const ABS_HAT0X: u16 = 0x10;
const ABS_HAT0Y: u16 = 0x11;
const ABS_HAT2X: u16 = 0x14;
const ABS_HAT2Y: u16 = 0x15;
pub(crate) const ABS_CNT: usize = 0x40;
pub(crate) const KEY_CNT: usize = 0x300;

/// The key codes a pad reports, per gamepad.rst.
const KEYS: [(u16, GamepadButton); 17] = [
    (0x130, GamepadButton::South),
    (0x131, GamepadButton::East),
    (BTN_NORTH, GamepadButton::North),
    (BTN_WEST, GamepadButton::West),
    (0x136, GamepadButton::LeftShoulder),
    (0x137, GamepadButton::RightShoulder),
    (BTN_TL2, GamepadButton::LeftTrigger),
    (BTN_TR2, GamepadButton::RightTrigger),
    (0x13a, GamepadButton::Select),
    (0x13b, GamepadButton::Start),
    (0x13c, GamepadButton::Guide),
    (0x13d, GamepadButton::LeftStick),
    (0x13e, GamepadButton::RightStick),
    (0x220, GamepadButton::DPadUp),
    (0x221, GamepadButton::DPadDown),
    (0x222, GamepadButton::DPadLeft),
    (0x223, GamepadButton::DPadRight),
];

/// `struct input_absinfo`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub(crate) struct AbsInfo {
    pub value: i32,
    pub minimum: i32,
    pub maximum: i32,
    pub fuzz: i32,
    pub flat: i32,
    pub resolution: i32,
}

/// What an absolute axis means on this pad.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Stick(GamepadAxis),
    /// A trigger; `true` when the pad has no digital trigger button, so the
    /// axis also presses one.
    Trigger(GamepadAxis, bool),
    HatX,
    HatY,
}

/// Whether bit `n` is set in a kernel bitmask.
pub(crate) fn bit(bits: &[u8], n: usize) -> bool {
    bits.get(n / 8).is_some_and(|b| b & (1 << (n % 8)) != 0)
}

/// Whether a device with these key and absolute-axis bits is a gamepad.
pub(crate) fn is_gamepad(keys: &[u8], abs: &[u8]) -> bool {
    bit(keys, BTN_SOUTH as usize) && abs.iter().any(|&b| b != 0)
}

/// How one pad's events map onto the standard layout.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Map {
    abs: Vec<(u16, Role, AbsInfo)>,
    /// xpad: `BTN_X`/`BTN_Y` are the west and north buttons.
    swap_north_west: bool,
}

impl Map {
    /// The map for a device with these key bits and absolute axes (code and
    /// its `absinfo`). `xbox_labels`: the driver reports the Xbox X and Y
    /// buttons as `BTN_X` and `BTN_Y`, which share the codes of `BTN_NORTH`
    /// and `BTN_WEST` (xpad).
    pub(crate) fn new(keys: &[u8], abs: &[(u16, AbsInfo)], xbox_labels: bool) -> Self {
        let has = |code: u16| abs.iter().any(|(c, _)| *c == code);
        let analog_only_l = !bit(keys, BTN_TL2 as usize);
        let analog_only_r = !bit(keys, BTN_TR2 as usize);
        let lt = Role::Trigger(GamepadAxis::LeftTrigger, analog_only_l);
        let rt = Role::Trigger(GamepadAxis::RightTrigger, analog_only_r);
        let right_on_z = !(has(ABS_RX) && has(ABS_RY)) && has(ABS_Z) && has(ABS_RZ);
        let role = |code: u16| -> Option<Role> {
            Some(match code {
                ABS_X => Role::Stick(GamepadAxis::LeftX),
                ABS_Y => Role::Stick(GamepadAxis::LeftY),
                ABS_RX => Role::Stick(GamepadAxis::RightX),
                ABS_RY => Role::Stick(GamepadAxis::RightY),
                ABS_Z if right_on_z => Role::Stick(GamepadAxis::RightX),
                ABS_RZ if right_on_z => Role::Stick(GamepadAxis::RightY),
                ABS_Z | ABS_BRAKE | ABS_HAT2Y => lt,
                ABS_RZ | ABS_GAS | ABS_HAT2X => rt,
                ABS_HAT0X => Role::HatX,
                ABS_HAT0Y => Role::HatY,
                _ => return None,
            })
        };
        Self {
            abs: abs
                .iter()
                .filter(|(_, info)| info.maximum > info.minimum)
                .filter_map(|&(code, info)| role(code).map(|r| (code, r, info)))
                .collect(),
            swap_north_west: xbox_labels,
        }
    }

    fn button(&self, code: u16) -> Option<GamepadButton> {
        let code = match code {
            BTN_NORTH if self.swap_north_west => BTN_WEST,
            BTN_WEST if self.swap_north_west => BTN_NORTH,
            c => c,
        };
        KEYS.iter().find(|(c, _)| *c == code).map(|(_, b)| *b)
    }

    /// The changes one `input_event` makes.
    pub(crate) fn decode(&self, kind: u16, code: u16, value: i32, out: &mut Vec<Change>) {
        match kind {
            EV_KEY => {
                if let Some(b) = self.button(code) {
                    out.push(Change::Button(b, value != 0));
                }
            }
            EV_ABS => {
                if let Some(&(_, role, info)) = self.abs.iter().find(|(c, ..)| *c == code) {
                    decode_abs(role, info, value, out);
                }
            }
            _ => {}
        }
    }

    /// The whole state, from the keys held down (`EVIOCGKEY`) and each axis's
    /// current value (`EVIOCGABS`).
    pub(crate) fn state(&self, keys_down: &[u8], abs_value: impl Fn(u16) -> i32) -> GamepadState {
        let mut out = Vec::new();
        for (code, _) in KEYS {
            self.decode(EV_KEY, code, bit(keys_down, code as usize) as i32, &mut out);
        }
        for &(code, role, info) in &self.abs {
            decode_abs(role, info, abs_value(code), &mut out);
        }
        let mut s = GamepadState::default();
        for change in out {
            match change {
                Change::Button(b, p) => s.set_button(b, p || s.pressed(b) && is_dpad(b)),
                Change::Axis(a, v) => s.set_axis(a, v),
            }
        }
        s
    }
}

fn is_dpad(b: GamepadButton) -> bool {
    matches!(
        b,
        GamepadButton::DPadUp
            | GamepadButton::DPadDown
            | GamepadButton::DPadLeft
            | GamepadButton::DPadRight
    )
}

/// −1..1 for a stick (inside the reported `flat` is 0), 0..1 for a trigger.
pub(crate) fn normalize_stick(info: AbsInfo, value: i32) -> f32 {
    let (min, max) = (info.minimum as f64, info.maximum as f64);
    let center = (min + max) / 2.0;
    let off = value as f64 - center;
    if off.abs() <= info.flat as f64 {
        return 0.0;
    }
    (off / ((max - min) / 2.0)).clamp(-1.0, 1.0) as f32
}

pub(crate) fn normalize_trigger(info: AbsInfo, value: i32) -> f32 {
    let (min, max) = (info.minimum as f64, info.maximum as f64);
    ((value as f64 - min) / (max - min)).clamp(0.0, 1.0) as f32
}

fn decode_abs(role: Role, info: AbsInfo, value: i32, out: &mut Vec<Change>) {
    match role {
        Role::Stick(axis) => out.push(Change::Axis(axis, normalize_stick(info, value))),
        Role::Trigger(axis, presses) => {
            let v = normalize_trigger(info, value);
            out.push(Change::Axis(axis, v));
            if presses {
                let b = if axis == GamepadAxis::LeftTrigger {
                    GamepadButton::LeftTrigger
                } else {
                    GamepadButton::RightTrigger
                };
                out.push(Change::Button(b, v > TRIGGER_PRESSED));
            }
        }
        Role::HatX | Role::HatY => {
            let (neg, pos) = if role == Role::HatX {
                (GamepadButton::DPadLeft, GamepadButton::DPadRight)
            } else {
                (GamepadButton::DPadUp, GamepadButton::DPadDown)
            };
            // A hat is −1, 0 or 1 (or a wider range: past the middle third).
            let n = normalize_stick(AbsInfo { flat: 0, ..info }, value);
            out.push(Change::Button(neg, n < -0.5));
            out.push(Change::Button(pos, n > 0.5));
        }
    }
}

/// `ioctl` request numbers (`_IOC` on the common Linux ABIs).
pub(crate) const fn ioc(dir: u64, nr: u64, size: u64) -> u64 {
    (dir << 30) | (size << 16) | ((b'E' as u64) << 8) | nr
}
const IOC_WRITE: u64 = 1;
const IOC_READ: u64 = 2;
pub(crate) const fn eviocgbit(ev: u16, len: usize) -> u64 {
    ioc(IOC_READ, 0x20 + ev as u64, len as u64)
}
pub(crate) const fn eviocgabs(abs: u16) -> u64 {
    ioc(IOC_READ, 0x40 + abs as u64, 24)
}
pub(crate) const fn eviocgname(len: usize) -> u64 {
    ioc(IOC_READ, 0x06, len as u64)
}
pub(crate) const EVIOCGID: u64 = ioc(IOC_READ, 0x02, 8);
pub(crate) const fn eviocgkey(len: usize) -> u64 {
    ioc(IOC_READ, 0x18, len as u64)
}
pub(crate) const fn eviocsff(size: usize) -> u64 {
    ioc(IOC_WRITE, 0x80, size as u64)
}

#[cfg(target_os = "linux")]
pub(super) use os::{refresh, set_rumble, start};

#[cfg(target_os = "linux")]
mod os {
    use super::*;
    use crate::platform::gamepad::{GamepadId, GamepadInfo, update};
    use std::collections::HashMap;
    use std::ffi::{c_int, c_ulong, c_void};
    use std::fs::{File, OpenOptions};
    use std::io::{ErrorKind, Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::Duration;

    unsafe extern "C" {
        fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
        fn poll(fds: *mut PollFd, n: c_ulong, timeout: c_int) -> c_int;
        fn inotify_init1(flags: c_int) -> c_int;
        fn inotify_add_watch(fd: c_int, path: *const std::ffi::c_char, mask: u32) -> c_int;
    }

    const O_NONBLOCK: i32 = 0o4000;
    const IN_CLOEXEC: c_int = 0o2000000;
    const IN_ATTRIB: u32 = 0x4;
    const IN_MOVED_TO: u32 = 0x80;
    const IN_CREATE: u32 = 0x100;
    const IN_DELETE: u32 = 0x200;
    const POLLIN: i16 = 1;
    const EV_FF: u16 = 0x15;
    const FF_RUMBLE: usize = 0x50;
    const FF_CNT: usize = 0x80;

    #[repr(C)]
    struct PollFd {
        fd: c_int,
        events: i16,
        revents: i16,
    }

    /// `struct input_event` (a `timeval`, then type, code, value).
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct InputEvent {
        sec: std::ffi::c_long,
        usec: std::ffi::c_long,
        kind: u16,
        code: u16,
        value: i32,
    }

    /// `struct ff_effect` with its union laid out as `ff_periodic_effect`
    /// (the largest member, holding a pointer); `ff_rumble_effect`'s strong
    /// and weak magnitudes are its first two `u16`s.
    #[repr(C)]
    struct FfEffect {
        kind: u16,
        id: i16,
        direction: u16,
        trigger: [u16; 2],
        replay_length: u16,
        replay_delay: u16,
        u: FfUnion,
    }

    #[repr(C)]
    struct FfUnion {
        strong: u16,
        weak: u16,
        _periodic_rest: [u16; 7],
        _custom_len: u32,
        _custom_data: *const c_void,
    }

    #[cfg(target_pointer_width = "64")]
    const _: () =
        assert!(std::mem::size_of::<FfEffect>() == 48 && std::mem::size_of::<InputEvent>() == 24);

    struct Device {
        file: File,
        path: PathBuf,
        id: GamepadId,
        map: Map,
        /// After `SYN_DROPPED`: skip to the next report, then re-read it all.
        dropped: bool,
    }

    /// Devices that can rumble: their file and uploaded effect id, shared
    /// with [`set_rumble`] callers.
    struct Rumbler {
        file: File,
        effect: i16,
    }
    static RUMBLE: Mutex<Option<HashMap<GamepadId, Rumbler>>> = Mutex::new(None);

    const DIR: &str = "/dev/input";

    fn ioctl_buf(fd: c_int, req: u64, buf: &mut [u8]) -> bool {
        unsafe { ioctl(fd, req as c_ulong, buf.as_mut_ptr()) >= 0 }
    }

    /// Open `path` if it's a gamepad.
    fn open(path: &Path) -> Option<Device> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(O_NONBLOCK)
            .open(path)
            .or_else(|_| {
                OpenOptions::new()
                    .read(true)
                    .custom_flags(O_NONBLOCK)
                    .open(path)
            })
            .ok()?;
        let fd = file.as_raw_fd();
        let mut keys = [0u8; KEY_CNT / 8];
        let mut abs = [0u8; ABS_CNT / 8];
        if !ioctl_buf(fd, eviocgbit(EV_KEY, keys.len()), &mut keys)
            || !ioctl_buf(fd, eviocgbit(EV_ABS, abs.len()), &mut abs)
            || !is_gamepad(&keys, &abs)
        {
            return None;
        }
        let axes: Vec<(u16, AbsInfo)> = (0..ABS_CNT as u16)
            .filter(|&c| bit(&abs, c as usize))
            .filter_map(|c| absinfo(fd, c).map(|i| (c, i)))
            .collect();
        let mut name = [0u8; 256];
        let name = if ioctl_buf(fd, eviocgname(name.len()), &mut name) {
            let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
            String::from_utf8_lossy(&name[..end]).into_owned()
        } else {
            "Gamepad".to_owned()
        };
        let mut id = [0u8; 8];
        let mut info = GamepadInfo::new(name);
        if ioctl_buf(fd, EVIOCGID, &mut id) {
            let word = |i: usize| u16::from_ne_bytes([id[i], id[i + 1]]);
            info = info.with_ids(word(2), word(4));
        }
        let map = Map::new(&keys, &axes, xbox_labels(path, info.vendor_id));
        let state = read_state(fd, &map)?;
        let mut ff = [0u8; FF_CNT / 8];
        let rumble = ioctl_buf(fd, eviocgbit(EV_FF, ff.len()), &mut ff) && bit(&ff, FF_RUMBLE);
        let id = update(|r| r.connect(info, state));
        if rumble && let Ok(dup) = file.try_clone() {
            RUMBLE
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_or_insert_with(HashMap::new)
                .insert(
                    id,
                    Rumbler {
                        file: dup,
                        effect: -1,
                    },
                );
        }
        Some(Device {
            file,
            path: path.to_owned(),
            id,
            map,
            dropped: false,
        })
    }

    fn absinfo(fd: c_int, code: u16) -> Option<AbsInfo> {
        let mut info = AbsInfo::default();
        (unsafe { ioctl(fd, eviocgabs(code) as c_ulong, &mut info as *mut AbsInfo) } >= 0)
            .then_some(info)
    }

    fn read_state(fd: c_int, map: &Map) -> Option<GamepadState> {
        let mut down = [0u8; KEY_CNT / 8];
        if !ioctl_buf(fd, eviocgkey(down.len()), &mut down) {
            return None;
        }
        Some(map.state(&down, |c| absinfo(fd, c).map_or(0, |i| i.value)))
    }

    /// Whether the device's driver names the Xbox X and Y buttons `BTN_X`
    /// and `BTN_Y` (xpad, and Microsoft's pads).
    fn xbox_labels(path: &Path, vendor: Option<u16>) -> bool {
        let Some(node) = path.file_name() else {
            return false;
        };
        let driver = Path::new("/sys/class/input")
            .join(node)
            .join("device/device/driver");
        let driver = std::fs::read_link(driver).ok();
        driver
            .as_deref()
            .and_then(|d| d.file_name())
            .is_some_and(|d| d == "xpad")
            || vendor == Some(0x045e)
    }

    fn is_event_node(name: &std::ffi::OsStr) -> bool {
        name.to_str().is_some_and(|n| n.starts_with("event"))
    }

    /// Open every gamepad not open yet.
    fn scan(devices: &mut Vec<Device>) {
        let Ok(dir) = std::fs::read_dir(DIR) else {
            return;
        };
        let mut paths: Vec<PathBuf> = dir
            .flatten()
            .filter(|e| is_event_node(&e.file_name()))
            .map(|e| e.path())
            .filter(|p| !devices.iter().any(|d| &d.path == p))
            .collect();
        paths.sort();
        devices.extend(paths.iter().filter_map(|p| open(p)));
    }

    fn close(devices: &mut Vec<Device>, i: usize) {
        let dev = devices.remove(i);
        if let Some(map) = RUMBLE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            map.remove(&dev.id);
        }
        update(|r| r.disconnect(dev.id));
    }

    /// Read what `dev` has; `false` when it's gone.
    fn drain(dev: &mut Device) -> bool {
        const N: usize = 64;
        let size = std::mem::size_of::<InputEvent>();
        let mut buf = vec![0u8; N * size];
        loop {
            let n = match (&dev.file).read(&mut buf) {
                Ok(0) => return false,
                Ok(n) => n,
                Err(e) if e.kind() == ErrorKind::WouldBlock => return true,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(_) => return false,
            };
            let mut changes = Vec::new();
            let mut resync = false;
            for chunk in buf[..n].chunks_exact(size) {
                let ev: InputEvent =
                    unsafe { std::ptr::read_unaligned(chunk.as_ptr().cast::<InputEvent>()) };
                if ev.kind == EV_SYN {
                    if ev.code == SYN_DROPPED {
                        dev.dropped = true;
                        changes.clear();
                    } else if ev.code == SYN_REPORT && dev.dropped {
                        dev.dropped = false;
                        resync = true;
                    }
                } else if !dev.dropped {
                    dev.map.decode(ev.kind, ev.code, ev.value, &mut changes);
                }
            }
            let id = dev.id;
            let state = if resync {
                read_state(dev.file.as_raw_fd(), &dev.map)
            } else {
                None
            };
            update(|r| {
                for c in changes {
                    r.apply(id, c);
                }
                if let Some(s) = state {
                    r.set_state(id, s);
                }
            });
            if n < buf.len() {
                return true;
            }
        }
    }

    fn run(mut devices: Vec<Device>, inotify: Option<File>) {
        let mut buf = [0u8; 4096];
        loop {
            let mut fds: Vec<PollFd> = devices
                .iter()
                .map(|d| d.file.as_raw_fd())
                .chain(inotify.as_ref().map(|f| f.as_raw_fd()))
                .map(|fd| PollFd {
                    fd,
                    events: POLLIN,
                    revents: 0,
                })
                .collect();
            if fds.is_empty() {
                return;
            }
            let r = unsafe { poll(fds.as_mut_ptr(), fds.len() as c_ulong, -1) };
            if r < 0 {
                if std::io::Error::last_os_error().kind() == ErrorKind::Interrupted {
                    continue;
                }
                return;
            }
            // Devices, last first so removing one keeps the others' indices.
            for i in (0..devices.len()).rev() {
                if fds[i].revents != 0 && !drain(&mut devices[i]) {
                    close(&mut devices, i);
                }
            }
            if let Some(ino) = &inotify
                && fds.last().is_some_and(|f| f.revents != 0)
            {
                let mut changed = false;
                while let Ok(n) = (&*ino).read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    changed = true;
                }
                if changed {
                    // A node removed: its device reports the error itself,
                    // or here if it hasn't yet.
                    for i in (0..devices.len()).rev() {
                        if !devices[i].path.exists() {
                            close(&mut devices, i);
                        }
                    }
                    scan(&mut devices);
                }
            }
        }
    }

    pub(in crate::platform::gamepad) fn start() {
        let mut devices = Vec::new();
        let inotify = {
            let fd = unsafe { inotify_init1(IN_CLOEXEC | O_NONBLOCK) };
            (fd >= 0).then(|| unsafe { File::from_raw_fd(fd) })
        };
        let inotify = inotify.filter(|f| {
            let path = std::ffi::CString::new(DIR).expect("path");
            let mask = IN_CREATE | IN_ATTRIB | IN_DELETE | IN_MOVED_TO;
            unsafe { inotify_add_watch(f.as_raw_fd(), path.as_ptr(), mask) >= 0 }
        });
        scan(&mut devices);
        if devices.is_empty() && inotify.is_none() {
            return;
        }
        let _ = std::thread::Builder::new()
            .name("stipple-gamepads".into())
            .spawn(move || run(devices, inotify));
    }

    pub(in crate::platform::gamepad) fn refresh() {}

    pub(in crate::platform::gamepad) fn set_rumble(
        id: GamepadId,
        low: f32,
        high: f32,
        duration: Duration,
    ) -> bool {
        let mut guard = RUMBLE.lock().unwrap_or_else(|e| e.into_inner());
        let Some(r) = guard.as_mut().and_then(|m| m.get_mut(&id)) else {
            return false;
        };
        let fd = r.file.as_raw_fd();
        let play = |code: i16, value: i32| {
            let ev = InputEvent {
                sec: 0,
                usec: 0,
                kind: EV_FF,
                code: code as u16,
                value,
            };
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    (&ev as *const InputEvent).cast::<u8>(),
                    std::mem::size_of::<InputEvent>(),
                )
            };
            (&r.file).write_all(bytes).is_ok()
        };
        if duration.is_zero() || (low == 0.0 && high == 0.0) {
            return r.effect < 0 || play(r.effect, 0);
        }
        let mut effect = FfEffect {
            kind: FF_RUMBLE as u16,
            id: r.effect,
            direction: 0,
            trigger: [0; 2],
            replay_length: duration.as_millis().min(u16::MAX as u128) as u16,
            replay_delay: 0,
            u: FfUnion {
                strong: (low * 65535.0) as u16,
                weak: (high * 65535.0) as u16,
                _periodic_rest: [0; 7],
                _custom_len: 0,
                _custom_data: std::ptr::null(),
            },
        };
        let req = eviocsff(std::mem::size_of::<FfEffect>());
        if unsafe { ioctl(fd, req as c_ulong, &mut effect as *mut FfEffect) } < 0 {
            return false;
        }
        r.effect = effect.id;
        play(effect.id, 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(codes: &[u16], len: usize) -> Vec<u8> {
        let mut v = vec![0u8; len / 8];
        for &c in codes {
            v[c as usize / 8] |= 1 << (c % 8);
        }
        v
    }

    fn stick() -> AbsInfo {
        AbsInfo {
            minimum: -32768,
            maximum: 32767,
            flat: 128,
            ..Default::default()
        }
    }

    fn trigger() -> AbsInfo {
        AbsInfo {
            maximum: 1023,
            ..Default::default()
        }
    }

    fn hat() -> AbsInfo {
        AbsInfo {
            minimum: -1,
            maximum: 1,
            ..Default::default()
        }
    }

    /// An Xbox 360 pad under xpad.
    fn xpad() -> Map {
        let keys = bits(
            &[
                0x130, 0x131, 0x133, 0x134, 0x136, 0x137, 0x13a, 0x13b, 0x13c, 0x13d, 0x13e,
            ],
            KEY_CNT,
        );
        Map::new(
            &keys,
            &[
                (ABS_X, stick()),
                (ABS_Y, stick()),
                (ABS_Z, trigger()),
                (ABS_RX, stick()),
                (ABS_RY, stick()),
                (ABS_RZ, trigger()),
                (ABS_HAT0X, hat()),
                (ABS_HAT0Y, hat()),
            ],
            true,
        )
    }

    fn decode(map: &Map, kind: u16, code: u16, value: i32) -> Vec<Change> {
        let mut out = Vec::new();
        map.decode(kind, code, value, &mut out);
        out
    }

    #[test]
    fn ioctl_numbers_match_the_kernel_headers() {
        // Values from <linux/input.h> on x86_64 / aarch64.
        assert_eq!(EVIOCGID, 0x8008_4502);
        assert_eq!(eviocgname(256), 0x8100_4506);
        assert_eq!(eviocgabs(ABS_X), 0x8018_4540);
        assert_eq!(eviocgbit(EV_KEY, KEY_CNT / 8), 0x8060_4521);
        assert_eq!(eviocgkey(KEY_CNT / 8), 0x8060_4518);
        assert_eq!(eviocsff(48), 0x4030_4580);
    }

    #[test]
    fn recognizes_gamepads() {
        let abs = bits(&[ABS_X, ABS_Y], ABS_CNT);
        assert!(is_gamepad(&bits(&[BTN_SOUTH], KEY_CNT), &abs));
        // A keyboard, a joystick (BTN_TRIGGER), a pad without axes.
        assert!(!is_gamepad(&bits(&[30], KEY_CNT), &abs));
        assert!(!is_gamepad(&bits(&[0x120], KEY_CNT), &abs));
        assert!(!is_gamepad(
            &bits(&[BTN_SOUTH], KEY_CNT),
            &bits(&[], ABS_CNT)
        ));
    }

    #[test]
    fn decodes_buttons() {
        let map = xpad();
        assert_eq!(
            decode(&map, EV_KEY, 0x130, 1),
            [Change::Button(GamepadButton::South, true)]
        );
        // xpad's BTN_X (0x133) is the west button.
        assert_eq!(
            decode(&map, EV_KEY, 0x133, 1),
            [Change::Button(GamepadButton::West, true)]
        );
        assert_eq!(
            decode(&map, EV_KEY, 0x13c, 0),
            [Change::Button(GamepadButton::Guide, false)]
        );
        assert!(decode(&map, EV_KEY, 30, 1).is_empty());
        // Per gamepad.rst (hid-playstation), 0x133 is north.
        let ps = Map::new(&bits(&[0x130], KEY_CNT), &[(ABS_X, stick())], false);
        assert_eq!(
            decode(&ps, EV_KEY, 0x133, 1),
            [Change::Button(GamepadButton::North, true)]
        );
        assert_eq!(
            decode(&ps, EV_KEY, 0x221, 1),
            [Change::Button(GamepadButton::DPadDown, true)]
        );
    }

    #[test]
    fn normalizes_axes() {
        let map = xpad();
        assert_eq!(
            decode(&map, EV_ABS, ABS_X, 32767),
            [Change::Axis(GamepadAxis::LeftX, 1.0)]
        );
        assert_eq!(
            decode(&map, EV_ABS, ABS_RY, -32768),
            [Change::Axis(GamepadAxis::RightY, -1.0)]
        );
        // Inside `flat`: centred.
        assert_eq!(
            decode(&map, EV_ABS, ABS_Y, 100),
            [Change::Axis(GamepadAxis::LeftY, 0.0)]
        );
        // xpad has no BTN_TL2: the trigger presses the button too.
        assert_eq!(
            decode(&map, EV_ABS, ABS_Z, 1023),
            [
                Change::Axis(GamepadAxis::LeftTrigger, 1.0),
                Change::Button(GamepadButton::LeftTrigger, true)
            ]
        );
        assert_eq!(
            decode(&map, EV_ABS, ABS_RZ, 0),
            [
                Change::Axis(GamepadAxis::RightTrigger, 0.0),
                Change::Button(GamepadButton::RightTrigger, false)
            ]
        );
        assert_eq!(
            decode(&map, EV_ABS, ABS_HAT0X, -1),
            [
                Change::Button(GamepadButton::DPadLeft, true),
                Change::Button(GamepadButton::DPadRight, false)
            ]
        );
        assert_eq!(
            decode(&map, EV_ABS, ABS_HAT0Y, 1),
            [
                Change::Button(GamepadButton::DPadUp, false),
                Change::Button(GamepadButton::DPadDown, true)
            ]
        );
        let unit = AbsInfo {
            minimum: 0,
            maximum: 255,
            ..Default::default()
        };
        assert!((normalize_stick(unit, 255) - 1.0).abs() < 1e-6);
        assert!((normalize_stick(unit, 0) + 1.0).abs() < 1e-6);
        assert!((normalize_trigger(unit, 51) - 0.2).abs() < 1e-6);
    }

    #[test]
    fn right_stick_on_z_and_digital_triggers() {
        // A DirectInput-style pad: right stick on Z/RZ, digital L2/R2.
        let keys = bits(&[0x130, BTN_TL2, BTN_TR2], KEY_CNT);
        let map = Map::new(
            &keys,
            &[
                (ABS_X, stick()),
                (ABS_Y, stick()),
                (ABS_Z, stick()),
                (ABS_RZ, stick()),
            ],
            false,
        );
        assert_eq!(
            decode(&map, EV_ABS, ABS_RZ, 32767),
            [Change::Axis(GamepadAxis::RightY, 1.0)]
        );
        // Analog triggers per gamepad.rst, with digital buttons beside them.
        let map = Map::new(&keys, &[(ABS_HAT2Y, trigger())], false);
        assert_eq!(
            decode(&map, EV_ABS, ABS_HAT2Y, 1023),
            [Change::Axis(GamepadAxis::LeftTrigger, 1.0)]
        );
        assert_eq!(
            decode(&map, EV_KEY, BTN_TL2, 1),
            [Change::Button(GamepadButton::LeftTrigger, true)]
        );
    }

    #[test]
    fn reads_whole_state() {
        let map = xpad();
        let down = bits(&[0x131, 0x13b], KEY_CNT);
        let s = map.state(&down, |code| match code {
            ABS_X => -32768,
            ABS_RZ => 1023,
            ABS_HAT0Y => -1,
            _ => 0,
        });
        assert_eq!(
            s.pressed_buttons().collect::<Vec<_>>(),
            [
                GamepadButton::East,
                GamepadButton::RightTrigger,
                GamepadButton::Start,
                GamepadButton::DPadUp
            ]
        );
        assert_eq!(s.axis(GamepadAxis::LeftX), -1.0);
        assert_eq!(s.axis(GamepadAxis::RightTrigger), 1.0);
    }
}
