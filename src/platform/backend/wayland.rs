//! A native Wayland backend implemented directly against the wire protocol.
//!
//! No `wayland-client`/`smithay` crate — just a `UnixStream` to the compositor
//! and the Wayland byte protocol (core `wl_*` plus `xdg-shell`), matching the
//! hand-authored X11 backend. It connects to `$WAYLAND_DISPLAY`, binds the
//! `wl_compositor` / `wl_shm` / `xdg_wm_base` globals, creates an `xdg_toplevel`
//! window, and presents the software [`Pixmap`] through a `wl_shm` buffer the
//! compositor maps directly.
//!
//! Scope: connection, the `xdg-shell` configure/ack handshake, shared-memory
//! presentation (window create + paint + close), and `wl_seat` input —
//! **keyboard** (decodes the compositor's `wl_keyboard.keymap`, received as an
//! fd via `recvmsg`/`SCM_RIGHTS`, by parsing the XKB_V1 text to map keycodes to
//! keysyms and to find the Alt/Super modifier bits for `wl_keyboard.modifiers`;
//! falls back to a layout-independent evdev table if none) and **pointer**
//! (motion, buttons and `axis` scrolling via `wl_pointer`, coordinates from
//! `wl_fixed`). The clipboard goes through `wl_data_device` (`wayland/clipboard.rs`).
//! The FFI footprint — `memfd_create`/`mmap`, `sendmsg`/`recvmsg` for fd
//! passing — is the reason for the module-level `allow(unsafe_code)`.
//!
//! **Verification:** the `Visual` workflow's headless-`sway` job screenshots the
//! render with `grim` and types into a field with `wtype` (whose virtual
//! keyboard exercises the keymap-decode path end to end); the evdev/xkb mappings
//! are unit-tested.

#![allow(unsafe_code)]

mod clipboard;

use std::collections::VecDeque;
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};

use crate::geometry::{PhysicalSize, Point, ScaleFactor};
use crate::platform::ControlFlow;
use crate::platform::error::PlatformError;
use crate::platform::event::{ButtonState, Event, KeyCode, Modifiers, ScrollDelta};
use crate::platform::window::{Window, WindowAttributes};
use crate::render::{Pixmap, Surface};

use core::ffi::{c_char, c_void};

// ---- libc FFI for the shared-memory buffer + fd passing ---------------------

unsafe extern "C" {
    fn memfd_create(name: *const c_char, flags: u32) -> i32;
    fn ftruncate(fd: i32, length: i64) -> i32;
    fn mmap(addr: *mut c_void, len: usize, prot: i32, flags: i32, fd: i32, off: i64)
    -> *mut c_void;
    fn munmap(addr: *mut c_void, len: usize) -> i32;
    fn close(fd: i32) -> i32;
    fn sendmsg(sockfd: i32, msg: *const Msghdr, flags: i32) -> isize;
    fn recvmsg(sockfd: i32, msg: *mut Msghdr, flags: i32) -> isize;
}
const PROT_READ: i32 = 1;
const PROT_WRITE: i32 = 2;
const MAP_SHARED: i32 = 1;
const MAP_PRIVATE: i32 = 2;
const SOL_SOCKET: i32 = 1;
const SCM_RIGHTS: i32 = 1;

// `sendmsg` scatter/gather + control-message structs (Linux x86_64 layout;
// `repr(C)` inserts the alignment padding glibc expects).
#[repr(C)]
struct Iovec {
    iov_base: *mut c_void,
    iov_len: usize,
}
#[repr(C)]
struct Msghdr {
    msg_name: *mut c_void,
    msg_namelen: u32,
    msg_iov: *mut Iovec,
    msg_iovlen: usize,
    msg_control: *mut c_void,
    msg_controllen: usize,
    msg_flags: i32,
}
#[repr(C)]
struct Cmsghdr {
    cmsg_len: usize,
    cmsg_level: i32,
    cmsg_type: i32,
}

/// Send `bytes` over `sock` with a single file descriptor attached as an
/// `SCM_RIGHTS` control message (one `sendmsg`).
fn sendmsg_fd(sock: i32, bytes: &[u8], fd: i32) -> io::Result<()> {
    // Control buffer: CMSG_SPACE(sizeof(int)) = align8(sizeof(cmsghdr)) + align8(4).
    let mut control = [0u8; 24];
    let cmsg = control.as_mut_ptr() as *mut Cmsghdr;
    unsafe {
        (*cmsg).cmsg_len = 20; // CMSG_LEN(4) = 16 + 4
        (*cmsg).cmsg_level = SOL_SOCKET;
        (*cmsg).cmsg_type = SCM_RIGHTS;
        // CMSG_DATA sits after the aligned header (offset 16).
        let data = control.as_mut_ptr().add(16) as *mut i32;
        data.write_unaligned(fd);
    }
    let mut iov = Iovec {
        iov_base: bytes.as_ptr() as *mut c_void,
        iov_len: bytes.len(),
    };
    let msg = Msghdr {
        msg_name: std::ptr::null_mut(),
        msg_namelen: 0,
        msg_iov: &mut iov,
        msg_iovlen: 1,
        msg_control: control.as_mut_ptr() as *mut c_void,
        msg_controllen: control.len(),
        msg_flags: 0,
    };
    let n = unsafe { sendmsg(sock, &msg, 0) };
    if n < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

// ---- Wayland object ids + opcodes -------------------------------------------

const WL_DISPLAY: u32 = 1;

// Request opcodes (client → server).
const WL_DISPLAY_GET_REGISTRY: u16 = 1;
const WL_DISPLAY_SYNC: u16 = 0;
const WL_REGISTRY_BIND: u16 = 0;
const WL_COMPOSITOR_CREATE_SURFACE: u16 = 0;
const WL_SHM_CREATE_POOL: u16 = 0;
const WL_SHM_POOL_CREATE_BUFFER: u16 = 0;
const WL_SURFACE_ATTACH: u16 = 1;
const WL_SURFACE_DAMAGE: u16 = 2;
const WL_SURFACE_COMMIT: u16 = 6;
const WL_SURFACE_SET_BUFFER_SCALE: u16 = 8;
const WL_SURFACE_ENTER: u16 = 0;
const WL_OUTPUT_SCALE: u16 = 3;
const WL_SEAT_GET_KEYBOARD: u16 = 1;
const XDG_WM_BASE_PONG: u16 = 3;
const XDG_WM_BASE_GET_XDG_SURFACE: u16 = 2;
const XDG_SURFACE_GET_TOPLEVEL: u16 = 1;
const XDG_SURFACE_ACK_CONFIGURE: u16 = 4;
const XDG_TOPLEVEL_SET_TITLE: u16 = 2;

// Event opcodes (server → client) for the objects we track.
const WL_DISPLAY_ERROR: u16 = 0;
const WL_REGISTRY_GLOBAL: u16 = 0;
const WL_CALLBACK_DONE: u16 = 0;
const XDG_WM_BASE_PING: u16 = 0;
const XDG_SURFACE_CONFIGURE: u16 = 0;
const XDG_TOPLEVEL_CONFIGURE: u16 = 0;
const XDG_TOPLEVEL_CLOSE: u16 = 1;
const WL_SEAT_CAPABILITIES: u16 = 0;
const WL_SEAT_CAP_POINTER: u32 = 1;
const WL_SEAT_CAP_KEYBOARD: u32 = 2;
const WL_SEAT_GET_POINTER: u16 = 0;
const WL_KEYBOARD_KEYMAP: u16 = 0;
const WL_KEYBOARD_KEY: u16 = 3;
const WL_KEYBOARD_MODIFIERS: u16 = 4;
const WL_POINTER_ENTER: u16 = 0;
const WL_POINTER_MOTION: u16 = 2;
const WL_POINTER_BUTTON: u16 = 3;
const WL_POINTER_AXIS: u16 = 4;
/// Since `wl_seat` v5: a wheel's notch count, sent before its `axis` event.
const WL_POINTER_AXIS_DISCRETE: u16 = 8;
/// The highest `wl_seat` version we speak. v5 adds `axis_discrete`; the other
/// events it adds (`frame`, `axis_source`, `axis_stop`, `repeat_info`, `name`)
/// are ignored. v8 would replace `axis_discrete` with `axis_value120`.
const WL_SEAT_VERSION: u32 = 5;

// `wl_shm` pixel format: 32-bit little-endian XRGB → bytes `[B, G, R, X]`.
const WL_SHM_FORMAT_XRGB8888: u32 = 1;

/// Whether a Wayland compositor is reachable (cheap check of `$WAYLAND_DISPLAY`).
pub fn available() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
}

fn dbg(args: std::fmt::Arguments<'_>) {
    if std::env::var_os("STIPPLE_WAYLAND_DEBUG").is_some() {
        eprintln!("stipple wayland: {args}");
    }
}

fn os(e: io::Error) -> PlatformError {
    PlatformError::Os(e.to_string())
}

/// Translate a Linux evdev key code + press/release into a Stipple event.
///
/// Editing/navigation keys have layout-independent evdev codes, so they map
/// directly. Printable keys use a best-effort US-QWERTY table for [`Event::Text`]
/// (full layout handling needs the compositor's xkb keymap, preferred when it
/// was received); Shift upper-cases letters, and with Ctrl/logo held a printable
/// key becomes a [`KeyCode::Char`] shortcut, as on X11.
fn evdev_to_event(code: u32, pressed: bool, modifiers: Modifiers) -> Option<Event> {
    let state = if pressed {
        ButtonState::Pressed
    } else {
        ButtonState::Released
    };
    // Editing / navigation keys (Linux input-event-codes.h).
    let key = match code {
        15 => Some(KeyCode::Tab),
        28 | 96 => Some(KeyCode::Enter), // Enter, KP-Enter
        14 => Some(KeyCode::Backspace),
        111 => Some(KeyCode::Delete),
        105 => Some(KeyCode::ArrowLeft),
        106 => Some(KeyCode::ArrowRight),
        103 => Some(KeyCode::ArrowUp),
        108 => Some(KeyCode::ArrowDown),
        102 => Some(KeyCode::Home),
        107 => Some(KeyCode::End),
        104 => Some(KeyCode::PageUp),
        109 => Some(KeyCode::PageDown),
        110 => Some(KeyCode::Insert),
        1 => Some(KeyCode::Escape),
        59..=68 => Some(FUNCTION_KEYS[(code - 59) as usize]), // F1 … F10
        87 => Some(KeyCode::F11),
        88 => Some(KeyCode::F12),
        _ => None,
    };
    if let Some(code) = key {
        return Some(Event::Key {
            code,
            state,
            modifiers,
        });
    }
    // Printable keys (US QWERTY).
    let ch = match code {
        16..=25 => Some(b"qwertyuiop"[(code - 16) as usize]),
        30..=38 => Some(b"asdfghjkl"[(code - 30) as usize]),
        44..=50 => Some(b"zxcvbnm"[(code - 44) as usize]),
        2..=10 => Some(b"123456789"[(code - 2) as usize]),
        11 => Some(b'0'),
        57 => Some(b' '),
        _ => None,
    };
    let c = ch? as char;
    let c = if modifiers.shift {
        c.to_ascii_uppercase()
    } else {
        c
    };
    char_event(c, state, modifiers)
}

/// F1 … F12, indexed from F1.
const FUNCTION_KEYS: [KeyCode; 12] = [
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

/// The event for a printable character: a [`KeyCode::Char`] shortcut while
/// Ctrl or the logo key is held (press and release), otherwise [`Event::Text`]
/// on press only.
fn char_event(c: char, state: ButtonState, modifiers: Modifiers) -> Option<Event> {
    if modifiers.ctrl || modifiers.meta {
        return Some(Event::Key {
            code: KeyCode::Char(c),
            state,
            modifiers,
        });
    }
    (state == ButtonState::Pressed).then(|| Event::Text(c.to_string()))
}

/// Convert a `wl_fixed` (signed 24.8 fixed-point) to logical pixels.
fn fixed_to_f64(raw: u32) -> f64 {
    (raw as i32) as f64 / 256.0
}

/// The scroll delta for a `wl_pointer.axis` event: `axis` 0 is vertical, 1
/// horizontal, and `value` (a `wl_fixed`) is in surface pixels, positive down
/// or right. A wheel also sends `axis_discrete` with its notch count first;
/// then the delta is one line per notch, as on the other backends, rather than
/// the compositor's per-notch distance (often 10 or 15 px).
fn axis_delta(axis: u32, value: u32, discrete: i32) -> Option<ScrollDelta> {
    let px = if discrete != 0 {
        f64::from(discrete) * crate::platform::input::LINE_PX
    } else {
        fixed_to_f64(value)
    };
    match axis {
        0 => Some(ScrollDelta { dx: 0.0, dy: px }),
        1 => Some(ScrollDelta { dx: px, dy: 0.0 }),
        _ => None,
    }
}

/// Translate an evdev pointer button code into a Stipple [`PointerButton`].
fn pointer_button(code: u32) -> Option<crate::platform::event::PointerButton> {
    use crate::platform::event::PointerButton;
    match code {
        0x110 => Some(PointerButton::Left),
        0x111 => Some(PointerButton::Right),
        0x112 => Some(PointerButton::Middle),
        _ => None,
    }
}

// ---- xkb keymap (text format) -----------------------------------------------

/// `wl_keyboard.modifiers` bits for xkb's fixed real modifiers (Shift, Lock,
/// Control); Alt and the logo key are whichever of Mod1…Mod5 the keymap binds
/// them to.
const XKB_SHIFT: u32 = 1 << 0;
const XKB_LOCK: u32 = 1 << 1;
const XKB_CONTROL: u32 = 1 << 2;
/// The usual bindings, used when the keymap doesn't say: Alt on Mod1, the logo
/// key on Mod4.
const XKB_MOD1: u32 = 1 << 3;
const XKB_MOD4: u32 = 1 << 6;

/// What we read from the compositor's XKB_V1 keymap.
#[derive(Debug)]
struct XkbKeymap {
    /// xkb keycode (evdev + 8) → keysym names at shift levels 1 and 2.
    keys: std::collections::HashMap<u32, (String, Option<String>)>,
    /// The modifier bits that mean Alt and the logo (Super) key.
    alt_mask: u32,
    meta_mask: u32,
}

impl XkbKeymap {
    /// The keysym name for `keycode` under the given modifier state: the
    /// level-2 symbol when Shift (or Caps Lock, for letters) selects it, else
    /// level 1.
    fn sym(&self, keycode: u32, mask: u32) -> Option<&str> {
        let (base, shifted) = self.keys.get(&keycode)?;
        let letter = base.chars().count() == 1 && base.chars().all(char::is_alphabetic);
        let shift = mask & XKB_SHIFT != 0;
        let caps = mask & XKB_LOCK != 0 && letter;
        match shifted {
            // Named keys (Tab → ISO_Left_Tab, …) stay themselves: Shift is
            // reported through the modifiers instead.
            Some(s) if shift != caps && keysym_name_to_key(base).is_none() => Some(s),
            _ => Some(base),
        }
    }
}

/// Translate a `wl_keyboard.modifiers` state (depressed | latched | locked)
/// into [`Modifiers`].
fn wl_modifiers(mask: u32, keymap: Option<&XkbKeymap>) -> Modifiers {
    let (alt, meta) = keymap.map_or((XKB_MOD1, XKB_MOD4), |k| (k.alt_mask, k.meta_mask));
    Modifiers {
        shift: mask & XKB_SHIFT != 0,
        ctrl: mask & XKB_CONTROL != 0,
        alt: mask & alt != 0,
        meta: mask & meta != 0,
    }
}

/// The substring of `s` between the first `a` and the next `b`.
fn between(s: &str, a: char, b: char) -> Option<&str> {
    let start = s.find(a)? + 1;
    let end = s[start..].find(b)? + start;
    Some(&s[start..end])
}

/// The symbol list in a `key` line: the `[ a, A ]` after `{` or after
/// `symbols[Group1]=`, never an `actions[…]` list. `None` if the line has none.
fn symbol_list(t: &str) -> Option<Vec<&str>> {
    let from = match t.find("symbols[") {
        Some(i) => i + t[i..].find(']')? + 1,
        None => 0,
    };
    let lb = t[from..].find('[')? + from;
    // `actions[` / `vmods[`-style subscripts aren't symbol lists.
    if from == 0
        && t[..lb]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
    {
        return None;
    }
    let inner = &t[lb + 1..];
    let inner = &inner[..inner.find(']')?];
    Some(inner.split(',').map(str::trim).collect())
}

/// Parse an XKB_V1 keymap. We read keycode definitions `<NAME> = N;`, per-key
/// symbols (`key <NAME> { [ sym1, sym2 ] };`, or the multi-line form with a
/// `symbols[Group1]= [ … ]` line), and `modifier_map ModN { … };` to learn
/// which modifier bits Alt and Super use.
fn parse_xkb(text: &str) -> XkbKeymap {
    use std::collections::HashMap;
    let mut codes: HashMap<&str, u32> = HashMap::new();
    let mut syms: HashMap<&str, (String, Option<String>)> = HashMap::new();
    // (modifier bit, members: `<KEY>` names or keysym names)
    let mut mod_maps: Vec<(u32, Vec<&str>)> = Vec::new();
    // A multi-line `key <NAME> {` block whose symbols are still to come.
    let mut open_key: Option<&str> = None;
    let mut record = |name, list: Vec<&str>| {
        let level = |i: usize| {
            list.get(i)
                .copied()
                .filter(|s| !s.is_empty() && *s != "NoSymbol")
                .map(str::to_string)
        };
        if let Some(base) = level(0) {
            syms.insert(name, (base, level(1)));
        }
    };
    for line in text.lines() {
        let t = line.trim();
        // `key <NAME>`, with any run of spaces (xkbcomp pads short names).
        let key_line = t
            .strip_prefix("key ")
            .and_then(|r| r.trim_start().strip_prefix('<'));
        if let Some(rest) = key_line {
            // key <NAME> { [ sym1, sym2 ] };
            open_key = None;
            if let Some(end) = rest.find('>') {
                let name = &rest[..end];
                match symbol_list(t) {
                    Some(list) => record(name, list),
                    None if !t.ends_with("};") => open_key = Some(name),
                    None => {}
                }
            }
        } else if let Some(name) = open_key {
            if t.starts_with('}') {
                open_key = None;
            } else if (t.starts_with("symbols[") || t.starts_with('['))
                && let Some(list) = symbol_list(t)
            {
                // Only the first group's symbols.
                record(name, list);
                open_key = None;
            }
        } else if let Some(rest) = t.strip_prefix("modifier_map ") {
            // modifier_map Mod1 { <LALT>, <META> };
            let modifier = rest.split_whitespace().next().unwrap_or("");
            let bit = match modifier {
                "Mod1" => 3,
                "Mod2" => 4,
                "Mod3" => 5,
                "Mod4" => 6,
                "Mod5" => 7,
                _ => continue, // Shift/Lock/Control have fixed bits
            };
            if let Some(members) = between(t, '{', '}') {
                mod_maps.push((1 << bit, members.split(',').map(str::trim).collect()));
            }
        } else if t.starts_with('<') && t.contains('=') {
            // <NAME> = N;
            if let Some(name) = between(t, '<', '>')
                && let Some(eq) = t.find('=')
            {
                let num: String = t[eq + 1..].chars().filter(|c| c.is_ascii_digit()).collect();
                if let Ok(code) = num.parse::<u32>() {
                    codes.insert(name, code);
                }
            }
        }
    }
    // Which modifier bits hold Alt/Meta and Super keys.
    let (mut alt_mask, mut meta_mask) = (0, 0);
    for (bit, members) in &mod_maps {
        for member in members {
            let sym = match member.strip_prefix('<').and_then(|m| m.strip_suffix('>')) {
                Some(key) => syms.get(key).map_or("", |(base, _)| base.as_str()),
                None => member,
            };
            if sym.starts_with("Alt_") || sym.starts_with("Meta_") {
                alt_mask |= bit;
            } else if sym.starts_with("Super_") {
                meta_mask |= bit;
            }
        }
    }
    let keys = codes
        .into_iter()
        .filter_map(|(name, code)| syms.get(name).map(|s| (code, s.clone())))
        .collect();
    XkbKeymap {
        keys,
        alt_mask: if alt_mask == 0 { XKB_MOD1 } else { alt_mask },
        meta_mask: if meta_mask == 0 { XKB_MOD4 } else { meta_mask },
    }
}

/// The named key an xkb keysym name stands for, if it isn't printable.
fn keysym_name_to_key(name: &str) -> Option<KeyCode> {
    Some(match name {
        "Tab" | "ISO_Left_Tab" => KeyCode::Tab,
        "Return" | "KP_Enter" => KeyCode::Enter,
        "BackSpace" => KeyCode::Backspace,
        "Delete" => KeyCode::Delete,
        "Left" => KeyCode::ArrowLeft,
        "Right" => KeyCode::ArrowRight,
        "Up" => KeyCode::ArrowUp,
        "Down" => KeyCode::ArrowDown,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        "Prior" | "Page_Up" => KeyCode::PageUp,
        "Next" | "Page_Down" => KeyCode::PageDown,
        "Insert" => KeyCode::Insert,
        "Escape" => KeyCode::Escape,
        "F1" => KeyCode::F1,
        "F2" => KeyCode::F2,
        "F3" => KeyCode::F3,
        "F4" => KeyCode::F4,
        "F5" => KeyCode::F5,
        "F6" => KeyCode::F6,
        "F7" => KeyCode::F7,
        "F8" => KeyCode::F8,
        "F9" => KeyCode::F9,
        "F10" => KeyCode::F10,
        "F11" => KeyCode::F11,
        "F12" => KeyCode::F12,
        _ => return None,
    })
}

/// Translate an xkb keysym name into a Stipple event (editing/navigation keys →
/// [`Event::Key`], printable keysyms → [`Event::Text`] on press, or a
/// [`KeyCode::Char`] shortcut with Ctrl/logo held).
fn keysym_name_to_event(name: &str, pressed: bool, modifiers: Modifiers) -> Option<Event> {
    let state = if pressed {
        ButtonState::Pressed
    } else {
        ButtonState::Released
    };
    if let Some(code) = keysym_name_to_key(name) {
        return Some(Event::Key {
            code,
            state,
            modifiers,
        });
    }
    // Printable: a one-character keysym name is the character itself, `UXXXX`
    // a Unicode code point; otherwise the common named ASCII punctuation.
    let ch = if name.chars().count() == 1 {
        name.chars().next()
    } else if let Some(hex) = name.strip_prefix('U')
        && hex.len() >= 4
        && let Ok(cp) = u32::from_str_radix(hex, 16)
    {
        char::from_u32(cp)
    } else {
        match name {
            "space" => Some(' '),
            "exclam" => Some('!'),
            "quotedbl" => Some('"'),
            "numbersign" => Some('#'),
            "dollar" => Some('$'),
            "percent" => Some('%'),
            "ampersand" => Some('&'),
            "apostrophe" => Some('\''),
            "parenleft" => Some('('),
            "parenright" => Some(')'),
            "asterisk" => Some('*'),
            "plus" => Some('+'),
            "comma" => Some(','),
            "minus" => Some('-'),
            "period" => Some('.'),
            "slash" => Some('/'),
            "colon" => Some(':'),
            "semicolon" => Some(';'),
            "less" => Some('<'),
            "equal" => Some('='),
            "greater" => Some('>'),
            "question" => Some('?'),
            "at" => Some('@'),
            "bracketleft" => Some('['),
            "backslash" => Some('\\'),
            "bracketright" => Some(']'),
            "asciicircum" => Some('^'),
            "underscore" => Some('_'),
            "grave" => Some('`'),
            "braceleft" => Some('{'),
            "bar" => Some('|'),
            "braceright" => Some('}'),
            "asciitilde" => Some('~'),
            _ => None,
        }
    };
    char_event(ch?, state, modifiers)
}

/// Map a keymap `fd` of `size` bytes (the `wl_keyboard.keymap` payload) and
/// parse it. Closes the fd; returns `None` on failure.
fn read_keymap(fd: i32, size: usize) -> Option<XkbKeymap> {
    if size == 0 {
        unsafe { close(fd) };
        return None;
    }
    let addr = unsafe { mmap(std::ptr::null_mut(), size, PROT_READ, MAP_PRIVATE, fd, 0) };
    unsafe { close(fd) }; // the mapping outlives the fd
    if addr as isize == -1 {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(addr as *const u8, size) };
    let text = String::from_utf8_lossy(bytes).into_owned();
    let map = parse_xkb(&text);
    unsafe { munmap(addr, size) };
    Some(map)
}

// ---- Wire connection --------------------------------------------------------

/// The compositor connection plus the client-side object-id allocator.
struct WlConn {
    stream: UnixStream,
    next_id: u32,
    /// File descriptors received with messages (the `wl_keyboard.keymap` fd, a
    /// `wl_data_source.send` fd), in arrival order, taken by the handler of the
    /// message that carries each.
    fds: VecDeque<i32>,
    /// Messages read while a paste waited on its pipe, for the event loop.
    queue: VecDeque<(u32, u16, Vec<u8>)>,
    clip: clipboard::Clipboard,
    /// Buffer pixels per surface unit: the scale of the output we're on.
    buffer_scale: u32,
}

impl WlConn {
    /// Connect to the compositor socket named by `$WAYLAND_DISPLAY` (relative
    /// names resolve under `$XDG_RUNTIME_DIR`).
    fn connect() -> Result<Self, PlatformError> {
        let disp = std::env::var_os("WAYLAND_DISPLAY")
            .ok_or_else(|| PlatformError::Os("WAYLAND_DISPLAY not set".into()))?;
        let path = std::path::PathBuf::from(&disp);
        let full = if path.is_absolute() {
            path
        } else {
            let dir = std::env::var_os("XDG_RUNTIME_DIR")
                .ok_or_else(|| PlatformError::Os("XDG_RUNTIME_DIR not set".into()))?;
            std::path::Path::new(&dir).join(path)
        };
        let stream = UnixStream::connect(&full).map_err(os)?;
        Ok(Self {
            stream,
            next_id: 2,
            fds: VecDeque::new(),
            queue: VecDeque::new(),
            clip: clipboard::Clipboard::default(),
            buffer_scale: 1,
        })
    }

    /// Allocate the next client object id.
    fn new_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Send a request `(object, opcode, args)`; `args` is the already-encoded
    /// argument block (a multiple of 4 bytes).
    fn send(&mut self, object: u32, opcode: u16, args: &[u8]) -> io::Result<()> {
        let size = (8 + args.len()) as u32;
        let mut msg = Vec::with_capacity(size as usize);
        msg.extend_from_slice(&object.to_le_bytes());
        msg.extend_from_slice(&((size << 16) | opcode as u32).to_le_bytes());
        msg.extend_from_slice(args);
        self.stream.write_all(&msg)
    }

    /// Send a request carrying a file descriptor via an `SCM_RIGHTS` control
    /// message (used by `wl_shm.create_pool`).
    fn send_with_fd(&mut self, object: u32, opcode: u16, args: &[u8], fd: i32) -> io::Result<()> {
        let size = (8 + args.len()) as u32;
        let mut msg = Vec::with_capacity(size as usize);
        msg.extend_from_slice(&object.to_le_bytes());
        msg.extend_from_slice(&((size << 16) | opcode as u32).to_le_bytes());
        msg.extend_from_slice(args);
        sendmsg_fd(self.stream.as_raw_fd(), &msg, fd)
    }

    /// Fill `buf` exactly via `recvmsg`, queueing any `SCM_RIGHTS` file
    /// descriptors that arrive with it in `self.fds`. Used instead of
    /// `read_exact` so the `wl_keyboard.keymap` fd isn't silently dropped.
    fn recv_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        let sock = self.stream.as_raw_fd();
        let mut got = 0;
        while got < buf.len() {
            let mut iov = Iovec {
                iov_base: buf[got..].as_mut_ptr() as *mut c_void,
                iov_len: buf.len() - got,
            };
            let mut control = [0u8; 32];
            let mut msg = Msghdr {
                msg_name: std::ptr::null_mut(),
                msg_namelen: 0,
                msg_iov: &mut iov,
                msg_iovlen: 1,
                msg_control: control.as_mut_ptr() as *mut c_void,
                msg_controllen: control.len(),
                msg_flags: 0,
            };
            let n = unsafe { recvmsg(sock, &mut msg, 0) };
            if n <= 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "wayland socket closed",
                ));
            }
            got += n as usize;
            // Collect the SCM_RIGHTS fds if the kernel attached any.
            if msg.msg_controllen >= core::mem::size_of::<Cmsghdr>() {
                let cmsg = control.as_ptr() as *const Cmsghdr;
                let (len, level, ty) =
                    unsafe { ((*cmsg).cmsg_len, (*cmsg).cmsg_level, (*cmsg).cmsg_type) };
                if level == SOL_SOCKET && ty == SCM_RIGHTS {
                    let count = (len.min(msg.msg_controllen).saturating_sub(16)) / 4;
                    for i in 0..count {
                        let fd = unsafe {
                            (control.as_ptr().add(16 + i * 4) as *const i32).read_unaligned()
                        };
                        self.fds.push_back(fd);
                    }
                }
            }
        }
        Ok(())
    }

    /// Read exactly one message: returns `(object, opcode, args)`. Any attached
    /// fd lands in `self.fds`.
    fn recv(&mut self) -> io::Result<(u32, u16, Vec<u8>)> {
        let mut header = [0u8; 8];
        self.recv_exact(&mut header)?;
        let object = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let word = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
        let size = (word >> 16) as usize;
        let opcode = (word & 0xffff) as u16;
        let mut args = vec![0u8; size.saturating_sub(8)];
        self.recv_exact(&mut args)?;
        Ok((object, opcode, args))
    }
}

// ---- Argument encoding helpers ----------------------------------------------

/// Append a 32-bit argument (int / uint / object / new_id).
fn arg_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

/// Append a string argument: length (incl NUL) + bytes + NUL, padded to 4.
fn arg_str(buf: &mut Vec<u8>, s: &str) {
    let len = s.len() + 1; // include the trailing NUL
    arg_u32(buf, len as u32);
    buf.extend_from_slice(s.as_bytes());
    buf.push(0);
    while !buf.len().is_multiple_of(4) {
        buf.push(0);
    }
}

/// Read a length-prefixed string from an event arg block at `off`, returning the
/// text (without the trailing NUL) and the next offset (the string bytes are
/// padded to a 4-byte boundary).
fn read_str(args: &[u8], off: usize) -> Option<(String, usize)> {
    let len = u32::from_le_bytes([
        *args.get(off)?,
        *args.get(off + 1)?,
        *args.get(off + 2)?,
        *args.get(off + 3)?,
    ]) as usize;
    let start = off + 4;
    let bytes = args.get(start..start + len)?;
    let text = String::from_utf8_lossy(&bytes[..len.saturating_sub(1)]).into_owned();
    let next = start + len.div_ceil(4) * 4;
    Some((text, next))
}

/// Convert a straight-RGBA8 pixmap into `wl_shm` XRGB8888 bytes (`[B, G, R, 0]`).
fn rgba_to_xrgb(rgba: &[u8], dst: &mut [u8]) {
    for (s, d) in rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(dst.as_chunks_mut::<4>().0.iter_mut())
    {
        d[0] = s[2];
        d[1] = s[1];
        d[2] = s[0];
        d[3] = 0;
    }
}

// ---- Shared-memory buffer ---------------------------------------------------

/// A `memfd`-backed region mapped into this process and shared with the
/// compositor as a `wl_shm` pool.
struct ShmBuffer {
    fd: i32,
    ptr: *mut u8,
    capacity: usize,
    buffer: u32,
    size: PhysicalSize,
}

impl ShmBuffer {
    fn bytes(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.capacity) }
    }
}

impl Drop for ShmBuffer {
    fn drop(&mut self) {
        unsafe {
            munmap(self.ptr as *mut c_void, self.capacity);
            close(self.fd);
        }
    }
}

/// Create a `memfd` of `bytes`, map it, and make a `wl_shm` pool + buffer of
/// `size` over it (XRGB8888). Sends `create_pool` (with the fd) and
/// `create_buffer`.
fn make_shm(conn: &mut WlConn, shm: u32, size: PhysicalSize) -> Result<ShmBuffer, PlatformError> {
    let stride = size.width as usize * 4;
    let bytes = stride * size.height as usize;
    let name = c"stipple-wl";
    let fd = unsafe { memfd_create(name.as_ptr(), 0) };
    if fd < 0 {
        return Err(PlatformError::Os("memfd_create failed".into()));
    }
    if unsafe { ftruncate(fd, bytes as i64) } < 0 {
        unsafe { close(fd) };
        return Err(PlatformError::Os("ftruncate failed".into()));
    }
    let addr = unsafe {
        mmap(
            std::ptr::null_mut(),
            bytes,
            PROT_READ | PROT_WRITE,
            MAP_SHARED,
            fd,
            0,
        )
    };
    if addr as isize == -1 {
        unsafe { close(fd) };
        return Err(PlatformError::Os("mmap failed".into()));
    }

    let pool = conn.new_id();
    let mut a = Vec::new();
    arg_u32(&mut a, pool);
    arg_u32(&mut a, bytes as u32);
    conn.send_with_fd(shm, WL_SHM_CREATE_POOL, &a, fd)
        .map_err(os)?;

    let buffer = conn.new_id();
    let mut a = Vec::new();
    arg_u32(&mut a, buffer);
    arg_u32(&mut a, 0); // offset
    arg_u32(&mut a, size.width);
    arg_u32(&mut a, size.height);
    arg_u32(&mut a, stride as u32);
    arg_u32(&mut a, WL_SHM_FORMAT_XRGB8888);
    conn.send(pool, WL_SHM_POOL_CREATE_BUFFER, &a).map_err(os)?;

    Ok(ShmBuffer {
        fd,
        ptr: addr as *mut u8,
        capacity: bytes,
        buffer,
        size,
    })
}

// ---- Window + Surface -------------------------------------------------------

type Shared = Arc<Mutex<WlConn>>;

struct WaylandWindow {
    conn: Shared,
    surface: u32,
    shm: u32,
    size: PhysicalSize,
}

impl std::fmt::Debug for WaylandWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WaylandWindow")
            .field("surface", &self.surface)
            .field("size", &self.size)
            .finish()
    }
}

impl Window for WaylandWindow {
    fn id(&self) -> crate::platform::WindowId {
        crate::platform::WindowId(self.surface as u64)
    }
    fn inner_size(&self) -> PhysicalSize {
        self.size
    }
    fn scale_factor(&self) -> ScaleFactor {
        ScaleFactor::new(self.conn.lock().unwrap().buffer_scale as f64)
    }
    fn request_redraw(&self) {}
    fn set_title(&self, _title: &str) {}
    fn clipboard(&self) -> Option<String> {
        clipboard::text(&mut self.conn.lock().unwrap())
    }
    fn set_clipboard(&self, text: &str) {
        clipboard::set_text(&mut self.conn.lock().unwrap(), text);
    }
    fn create_surface(&self) -> Box<dyn Surface> {
        Box::new(WaylandSurface {
            conn: self.conn.clone(),
            surface: self.surface,
            shm: self.shm,
            size: self.size,
            buffer: None,
        })
    }
}

struct WaylandSurface {
    conn: Shared,
    surface: u32,
    shm: u32,
    size: PhysicalSize,
    buffer: Option<ShmBuffer>,
}

impl std::fmt::Debug for WaylandSurface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WaylandSurface")
            .field("surface", &self.surface)
            .field("size", &self.size)
            .finish()
    }
}

impl Surface for WaylandSurface {
    fn resize(&mut self, size: PhysicalSize) {
        if size != self.size {
            self.size = size;
            self.buffer = None; // reallocate at the new size on next present
        }
    }
    fn size(&self) -> PhysicalSize {
        self.size
    }
    fn present(&mut self, pixmap: &Pixmap, _damage: &[crate::geometry::Rect]) {
        let size = pixmap.size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        let mut conn = self.conn.lock().unwrap();
        // (Re)create the shm buffer if missing or the size changed.
        if self.buffer.as_ref().map(|b| b.size) != Some(size) {
            self.buffer = make_shm(&mut conn, self.shm, size).ok();
        }
        let Some(buf) = self.buffer.as_mut() else {
            return;
        };
        let stride = size.width as usize * 4;
        let bytes = stride * size.height as usize;
        let buffer = buf.buffer;
        rgba_to_xrgb(pixmap.as_bytes(), &mut buf.bytes()[..bytes]);

        let surface = self.surface;
        // attach(buffer, 0, 0); damage(0, 0, w, h); commit.
        let scale = conn.buffer_scale.max(1);
        let mut a = Vec::new();
        arg_u32(&mut a, scale);
        let _ = conn.send(surface, WL_SURFACE_SET_BUFFER_SCALE, &a);
        let mut a = Vec::new();
        arg_u32(&mut a, buffer);
        arg_u32(&mut a, 0);
        arg_u32(&mut a, 0);
        let _ = conn.send(surface, WL_SURFACE_ATTACH, &a);
        // Damage is in surface units, which the scale divides the buffer into.
        let mut a = Vec::new();
        arg_u32(&mut a, 0);
        arg_u32(&mut a, 0);
        arg_u32(&mut a, size.width / scale);
        arg_u32(&mut a, size.height / scale);
        let _ = conn.send(surface, WL_SURFACE_DAMAGE, &a);
        let _ = conn.send(surface, WL_SURFACE_COMMIT, &[]);
        let _ = conn.stream.flush();
        dbg(format_args!("present {}x{}", size.width, size.height));
    }
}

// ---- Run loop ---------------------------------------------------------------

/// Connect to the compositor, create a window, and drive `handler` over its
/// event stream until it returns [`ControlFlow::Exit`] or the toplevel closes.
pub fn run<H>(attrs: WindowAttributes, mut handler: H) -> Result<(), PlatformError>
where
    H: FnMut(Event, &dyn Window) -> ControlFlow,
{
    let mut conn = WlConn::connect()?;
    let size = ScaleFactor::IDENTITY.to_physical(attrs.logical_size);
    let size = PhysicalSize::new(size.width.max(1), size.height.max(1));

    // get_registry, then sync so the compositor flushes all globals before the
    // sync callback's `done` event.
    let registry = conn.new_id();
    let mut a = Vec::new();
    arg_u32(&mut a, registry);
    conn.send(WL_DISPLAY, WL_DISPLAY_GET_REGISTRY, &a)
        .map_err(os)?;
    let sync_cb = conn.new_id();
    let mut a = Vec::new();
    arg_u32(&mut a, sync_cb);
    conn.send(WL_DISPLAY, WL_DISPLAY_SYNC, &a).map_err(os)?;

    // Collect globals until the sync callback fires.
    let mut compositor_name = None;
    let mut shm_name = None;
    let mut wm_base_name = None;
    let mut seat_name = None;
    let mut seat_version = 1;
    let mut data_manager = None;
    let mut output_names = Vec::new();
    loop {
        let (object, opcode, args) = conn.recv().map_err(os)?;
        if object == registry && opcode == WL_REGISTRY_GLOBAL {
            let name = u32::from_le_bytes([args[0], args[1], args[2], args[3]]);
            if let Some((iface, next)) = read_str(&args, 4) {
                let version = args
                    .get(next..next + 4)
                    .map_or(1, |v| u32::from_le_bytes([v[0], v[1], v[2], v[3]]));
                match iface.as_str() {
                    "wl_compositor" => compositor_name = Some(name),
                    "wl_shm" => shm_name = Some(name),
                    "xdg_wm_base" => wm_base_name = Some(name),
                    "wl_seat" => {
                        seat_name = Some(name);
                        seat_version = version.clamp(1, WL_SEAT_VERSION);
                    }
                    "wl_output" => output_names.push(name),
                    "wl_data_device_manager" => {
                        data_manager = Some((name, version.clamp(1, clipboard::MANAGER_VERSION)));
                    }
                    _ => {}
                }
            }
        } else if object == sync_cb && opcode == WL_CALLBACK_DONE {
            break;
        } else if object == WL_DISPLAY && opcode == WL_DISPLAY_ERROR {
            return Err(PlatformError::Os("wl_display error during setup".into()));
        }
    }

    let bind = |conn: &mut WlConn, name: u32, iface: &str, version: u32| -> u32 {
        let id = conn.new_id();
        let mut a = Vec::new();
        arg_u32(&mut a, name);
        arg_str(&mut a, iface);
        arg_u32(&mut a, version);
        arg_u32(&mut a, id);
        let _ = conn.send(registry, WL_REGISTRY_BIND, &a);
        id
    };
    let compositor = bind(
        &mut conn,
        compositor_name.ok_or_else(|| PlatformError::Os("no wl_compositor".into()))?,
        "wl_compositor",
        4,
    );
    let shm = bind(
        &mut conn,
        shm_name.ok_or_else(|| PlatformError::Os("no wl_shm".into()))?,
        "wl_shm",
        1,
    );
    let wm_base = bind(
        &mut conn,
        wm_base_name.ok_or_else(|| PlatformError::Os("no xdg_wm_base".into()))?,
        "xdg_wm_base",
        1,
    );

    // Outputs report their scale; the surface's `enter` says which one we're on.
    let mut outputs: Vec<(u32, u32)> = output_names
        .into_iter()
        .map(|name| (bind(&mut conn, name, "wl_output", 2), 1))
        .collect();
    let mut current_output: Option<u32> = None;

    // Optional seat. The keyboard is created lazily from its `capabilities`
    // event — calling get_keyboard on a seat that never had the keyboard
    // capability (e.g. a headless compositor with no devices) is a protocol
    // error, so we wait until one is advertised.
    let seat = seat_name.map(|name| bind(&mut conn, name, "wl_seat", seat_version));
    dbg(format_args!("seat={seat:?} v{seat_version}"));
    // The clipboard goes through the seat's data device.
    if let (Some(seat), Some((name, version))) = (seat, data_manager) {
        let manager = bind(&mut conn, name, "wl_data_device_manager", version);
        clipboard::setup(&mut conn, manager, seat);
    }

    // Surface → xdg_surface → xdg_toplevel.
    let surface = conn.new_id();
    let mut a = Vec::new();
    arg_u32(&mut a, surface);
    conn.send(compositor, WL_COMPOSITOR_CREATE_SURFACE, &a)
        .map_err(os)?;

    let xdg_surface = conn.new_id();
    let mut a = Vec::new();
    arg_u32(&mut a, xdg_surface);
    arg_u32(&mut a, surface);
    conn.send(wm_base, XDG_WM_BASE_GET_XDG_SURFACE, &a)
        .map_err(os)?;

    let toplevel = conn.new_id();
    let mut a = Vec::new();
    arg_u32(&mut a, toplevel);
    conn.send(xdg_surface, XDG_SURFACE_GET_TOPLEVEL, &a)
        .map_err(os)?;

    let mut a = Vec::new();
    arg_str(&mut a, &attrs.title);
    conn.send(toplevel, XDG_TOPLEVEL_SET_TITLE, &a)
        .map_err(os)?;

    // Initial commit: the compositor responds with an xdg_surface.configure.
    conn.send(surface, WL_SURFACE_COMMIT, &[]).map_err(os)?;
    conn.stream.flush().map_err(os)?;
    dbg(format_args!(
        "window: surface={surface} xdg={xdg_surface} toplevel={toplevel} size={}x{}",
        size.width, size.height
    ));

    let shared: Shared = Arc::new(Mutex::new(conn));
    let mut win = WaylandWindow {
        conn: shared.clone(),
        surface,
        shm,
        size,
    };

    // Event loop. The keyboard/pointer are created when the seat advertises
    // them; `pointer_pos` tracks the latest cursor position for button events,
    // and `keymap` decodes keycodes to characters once the compositor sends it.
    let mut keyboard: Option<u32> = None;
    // Keyboards we dropped: a keymap still in flight for one carries an fd that
    // must be taken off the queue (and closed), or the next keymap would get it.
    let mut old_keyboards: Vec<u32> = Vec::new();
    let mut pointer: Option<u32> = None;
    let mut pointer_pos = Point::new(0.0, 0.0);
    let mut keymap: Option<XkbKeymap> = None;
    // The keyboard's modifier state (depressed | latched | locked xkb bits).
    let mut mods_mask = 0u32;
    // Wheel notches from `axis_discrete`, per axis, for the `axis` that follows.
    let mut discrete = [0i32; 2];
    // Toplevel sizes are in surface units; the buffer is that times the scale.
    let mut logical = size;
    let mut pending_size: Option<PhysicalSize> = None;
    // Between messages, wait on the connection and on wakeups (timers, other
    // threads) together.
    crate::platform::wake::install_unix();
    let fd = shared.lock().unwrap().stream.as_raw_fd();
    loop {
        // Messages a paste read while it waited come first.
        let queued = shared.lock().unwrap().queue.pop_front();
        if queued.is_none() && !crate::platform::wake::wait_readable(fd) {
            if crate::platform::wake::take(std::time::Instant::now())
                && handler(Event::Wake, &win) == ControlFlow::Exit
            {
                break;
            }
            continue;
        }
        let (object, opcode, args) = {
            let mut conn = shared.lock().unwrap();
            let message = match queued {
                Some(message) => message,
                None => conn.recv().map_err(os)?,
            };
            if clipboard::handle(&mut conn, message.0, message.1, &message.2) {
                continue;
            }
            message
        };
        // Moving to another output, or an output's scale changing, means rendering
        // at a new resolution.
        let moved = object == surface && opcode == WL_SURFACE_ENTER;
        let rescaled = opcode == WL_OUTPUT_SCALE && outputs.iter().any(|(id, _)| *id == object);
        if moved || rescaled {
            let value = u32::from_le_bytes([args[0], args[1], args[2], args[3]]);
            if moved {
                current_output = Some(value);
            } else if let Some(entry) = outputs.iter_mut().find(|(id, _)| *id == object) {
                entry.1 = value.max(1);
            }
            let scale = outputs
                .iter()
                .find(|(id, _)| Some(*id) == current_output)
                .map_or(1, |(_, scale)| *scale);
            let changed = {
                let mut conn = shared.lock().unwrap();
                let changed = conn.buffer_scale != scale;
                conn.buffer_scale = scale;
                changed
            };
            if changed {
                dbg(format_args!("buffer scale {scale}"));
                win.size = PhysicalSize::new(logical.width * scale, logical.height * scale);
                if handler(Event::Resized(win.size), &win) == ControlFlow::Exit
                    || handler(Event::RedrawRequested, &win) == ControlFlow::Exit
                {
                    break;
                }
            }
            continue;
        }
        if Some(object) == seat && opcode == WL_SEAT_CAPABILITIES {
            let caps = u32::from_le_bytes([args[0], args[1], args[2], args[3]]);
            dbg(format_args!("seat capabilities: {caps}"));
            let mut conn = shared.lock().unwrap();
            if caps & WL_SEAT_CAP_KEYBOARD != 0 {
                if keyboard.is_none() {
                    let kbd = conn.new_id();
                    let mut a = Vec::new();
                    arg_u32(&mut a, kbd);
                    if conn.send(seat.unwrap(), WL_SEAT_GET_KEYBOARD, &a).is_ok() {
                        keyboard = Some(kbd);
                        dbg(format_args!("keyboard={kbd} (caps={caps})"));
                    }
                }
            } else if let Some(old) = keyboard.take() {
                old_keyboards.push(old);
                // The keyboard went away (e.g. a virtual keyboard was destroyed).
                // The compositor makes the wl_keyboard inert on capability loss,
                // so we simply drop our reference (and its keymap) and acquire a
                // fresh one if the capability returns — sending an explicit
                // release races the compositor's own teardown and can error.
                keymap = None;
                mods_mask = 0;
                dbg(format_args!("keyboard dropped (caps={caps})"));
            }
            if pointer.is_none() && caps & WL_SEAT_CAP_POINTER != 0 {
                let ptr = conn.new_id();
                let mut a = Vec::new();
                arg_u32(&mut a, ptr);
                if conn.send(seat.unwrap(), WL_SEAT_GET_POINTER, &a).is_ok() {
                    pointer = Some(ptr);
                    dbg(format_args!("pointer={ptr} (caps={caps})"));
                }
            }
        } else if Some(object) == pointer
            && (opcode == WL_POINTER_ENTER || opcode == WL_POINTER_MOTION)
        {
            // enter: serial, surface, x, y. motion: time, x, y. Both end in two
            // wl_fixed coordinates.
            let n = args.len();
            let x = u32::from_le_bytes([args[n - 8], args[n - 7], args[n - 6], args[n - 5]]);
            let y = u32::from_le_bytes([args[n - 4], args[n - 3], args[n - 2], args[n - 1]]);
            pointer_pos = Point::new(fixed_to_f64(x), fixed_to_f64(y));
            if handler(
                Event::PointerMoved {
                    position: pointer_pos,
                },
                &win,
            ) == ControlFlow::Exit
            {
                break;
            }
        } else if Some(object) == pointer && opcode == WL_POINTER_BUTTON {
            // serial, time, button(evdev), state(0 released / 1 pressed).
            shared.lock().unwrap().clip.serial =
                u32::from_le_bytes([args[0], args[1], args[2], args[3]]);
            let button = u32::from_le_bytes([args[8], args[9], args[10], args[11]]);
            let pressed = u32::from_le_bytes([args[12], args[13], args[14], args[15]]) == 1;
            if let Some(button) = pointer_button(button) {
                let state = if pressed {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                };
                if handler(
                    Event::PointerButton {
                        button,
                        state,
                        position: pointer_pos,
                    },
                    &win,
                ) == ControlFlow::Exit
                {
                    break;
                }
            }
        } else if Some(object) == pointer && opcode == WL_POINTER_AXIS_DISCRETE {
            // axis(4), discrete(4: signed notches).
            let axis = u32::from_le_bytes([args[0], args[1], args[2], args[3]]);
            let steps = i32::from_le_bytes([args[4], args[5], args[6], args[7]]);
            if let Some(slot) = discrete.get_mut(axis as usize) {
                *slot = steps;
            }
        } else if Some(object) == pointer && opcode == WL_POINTER_AXIS {
            // time(4), axis(4: 0 vertical, 1 horizontal), value(wl_fixed).
            let axis = u32::from_le_bytes([args[4], args[5], args[6], args[7]]);
            let value = u32::from_le_bytes([args[8], args[9], args[10], args[11]]);
            let steps = discrete.get_mut(axis as usize).map_or(0, std::mem::take);
            if let Some(delta) = axis_delta(axis, value, steps)
                && handler(Event::Scroll { delta }, &win) == ControlFlow::Exit
            {
                break;
            }
        } else if object == wm_base && opcode == XDG_WM_BASE_PING {
            let serial = u32::from_le_bytes([args[0], args[1], args[2], args[3]]);
            let mut a = Vec::new();
            arg_u32(&mut a, serial);
            let mut conn = shared.lock().unwrap();
            let _ = conn.send(wm_base, XDG_WM_BASE_PONG, &a);
            let _ = conn.stream.flush();
        } else if object == xdg_surface && opcode == XDG_SURFACE_CONFIGURE {
            let serial = u32::from_le_bytes([args[0], args[1], args[2], args[3]]);
            {
                let mut conn = shared.lock().unwrap();
                let mut a = Vec::new();
                arg_u32(&mut a, serial);
                let _ = conn.send(xdg_surface, XDG_SURFACE_ACK_CONFIGURE, &a);
            }
            // Apply the size from the toplevel.configure this one completes.
            if let Some(size) = pending_size.take().filter(|s| *s != logical) {
                logical = size;
                let scale = shared.lock().unwrap().buffer_scale;
                win.size = PhysicalSize::new(size.width * scale, size.height * scale);
                if handler(Event::Resized(win.size), &win) == ControlFlow::Exit {
                    break;
                }
            }
            // Paint the frame for this configure.
            if handler(Event::RedrawRequested, &win) == ControlFlow::Exit {
                break;
            }
        } else if object == toplevel && opcode == XDG_TOPLEVEL_CONFIGURE {
            // width, height, states[]: a 0 size means "client picks".
            let w = i32::from_le_bytes([args[0], args[1], args[2], args[3]]);
            let h = i32::from_le_bytes([args[4], args[5], args[6], args[7]]);
            dbg(format_args!("toplevel configure {w}x{h}"));
            if w > 0 && h > 0 {
                pending_size = Some(PhysicalSize::new(w as u32, h as u32));
            }
        } else if object == toplevel && opcode == XDG_TOPLEVEL_CLOSE {
            handler(Event::CloseRequested, &win);
            break;
        } else if old_keyboards.contains(&object) {
            if opcode == WL_KEYBOARD_KEYMAP
                && let Some(fd) = shared.lock().unwrap().fds.pop_front()
            {
                unsafe { close(fd) };
            }
        } else if Some(object) == keyboard && opcode == WL_KEYBOARD_KEYMAP {
            // format(4), [fd via ancillary], size(4). Format 1 = XKB_V1 text.
            let format = u32::from_le_bytes([args[0], args[1], args[2], args[3]]);
            let size = u32::from_le_bytes([args[4], args[5], args[6], args[7]]) as usize;
            let fd = shared.lock().unwrap().fds.pop_front();
            if let (1, Some(fd)) = (format, fd) {
                keymap = read_keymap(fd, size);
                dbg(format_args!(
                    "keymap parsed: {} keys",
                    keymap.as_ref().map(|m| m.keys.len()).unwrap_or(0)
                ));
            } else if let Some(fd) = fd {
                unsafe { close(fd) };
            }
        } else if Some(object) == keyboard && opcode == WL_KEYBOARD_KEY {
            // serial(4), time(4), key(4 evdev), state(4: 1 = pressed). A copy
            // cites the serial of the key that made it.
            shared.lock().unwrap().clip.serial =
                u32::from_le_bytes([args[0], args[1], args[2], args[3]]);
            let key = u32::from_le_bytes([args[8], args[9], args[10], args[11]]);
            let pressed = u32::from_le_bytes([args[12], args[13], args[14], args[15]]) == 1;
            // Prefer the compositor's keymap (xkb keycode = evdev + 8); fall back
            // to the layout-independent evdev table when none was parsed.
            let modifiers = wl_modifiers(mods_mask, keymap.as_ref());
            let ev = match keymap.as_ref().and_then(|m| m.sym(key + 8, mods_mask)) {
                Some(sym) => keysym_name_to_event(sym, pressed, modifiers),
                None => evdev_to_event(key, pressed, modifiers),
            };
            if let Some(ev) = ev {
                dbg(format_args!("key {key} pressed={pressed} -> {ev:?}"));
                if handler(ev, &win) == ControlFlow::Exit {
                    break;
                }
            }
        } else if Some(object) == keyboard && opcode == WL_KEYBOARD_MODIFIERS {
            // serial, mods_depressed, mods_latched, mods_locked, group.
            let word =
                |i: usize| u32::from_le_bytes([args[i], args[i + 1], args[i + 2], args[i + 3]]);
            mods_mask = word(4) | word(8) | word(12);
            dbg(format_args!("modifiers {mods_mask:#x}"));
        } else if object == WL_DISPLAY && opcode == WL_DISPLAY_ERROR {
            return Err(PlatformError::Os("wl_display error".into()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decode a message header `[object][size<<16|opcode]` as the wire encodes it.
    fn decode_header(msg: &[u8]) -> (u32, u32, u16) {
        let object = u32::from_le_bytes([msg[0], msg[1], msg[2], msg[3]]);
        let word = u32::from_le_bytes([msg[4], msg[5], msg[6], msg[7]]);
        (object, word >> 16, (word & 0xffff) as u16)
    }

    #[test]
    fn message_header_packs_size_and_opcode() {
        // get_registry(new_id=2): one u32 arg → size 12, opcode 1 on object 1.
        let mut msg = Vec::new();
        msg.extend_from_slice(&WL_DISPLAY.to_le_bytes());
        let mut args = Vec::new();
        arg_u32(&mut args, 2);
        let size = (8 + args.len()) as u32;
        msg.extend_from_slice(&((size << 16) | WL_DISPLAY_GET_REGISTRY as u32).to_le_bytes());
        msg.extend_from_slice(&args);
        let (object, sz, opcode) = decode_header(&msg);
        assert_eq!((object, sz, opcode), (1, 12, 1));
        assert_eq!(msg.len(), 12);
    }

    #[test]
    fn string_arg_pads_to_four_bytes() {
        let mut buf = Vec::new();
        arg_str(&mut buf, "wl_shm"); // len 6 + NUL = 7 → padded to 8 bytes of text
        // 4 (length) + 8 (padded "wl_shm\0") = 12 bytes, length field = 7.
        assert_eq!(buf.len(), 12);
        assert_eq!(u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]), 7);
        assert!(buf.len().is_multiple_of(4));
        // Round-trips through read_str, leaving the offset at the padded end.
        let (s, next) = read_str(&buf, 0).unwrap();
        assert_eq!(s, "wl_shm");
        assert_eq!(next, 12);
    }

    #[test]
    fn read_str_parses_a_registry_global() {
        // name(uint) + interface(string) + version(uint), as wl_registry.global.
        let mut args = Vec::new();
        arg_u32(&mut args, 42);
        arg_str(&mut args, "xdg_wm_base");
        arg_u32(&mut args, 3);
        let name = u32::from_le_bytes([args[0], args[1], args[2], args[3]]);
        let (iface, next) = read_str(&args, 4).unwrap();
        let version =
            u32::from_le_bytes([args[next], args[next + 1], args[next + 2], args[next + 3]]);
        assert_eq!((name, iface.as_str(), version), (42, "xdg_wm_base", 3));
    }

    #[test]
    fn xrgb_swizzle_drops_alpha() {
        let mut dst = [0u8; 8];
        rgba_to_xrgb(&[10, 20, 30, 255, 1, 2, 3, 128], &mut dst);
        assert_eq!(dst, [30, 20, 10, 0, 3, 2, 1, 0]);
    }

    #[test]
    fn evdev_maps_editing_keys_and_us_text() {
        let none = Modifiers::default();
        let evdev_to_event = |code, pressed| evdev_to_event(code, pressed, none);
        // Layout-independent editing/navigation keys.
        assert!(matches!(
            evdev_to_event(15, true),
            Some(Event::Key {
                code: KeyCode::Tab,
                state: ButtonState::Pressed,
                ..
            })
        ));
        assert!(matches!(
            evdev_to_event(105, true),
            Some(Event::Key {
                code: KeyCode::ArrowLeft,
                ..
            })
        ));
        assert!(matches!(
            evdev_to_event(28, false),
            Some(Event::Key {
                code: KeyCode::Enter,
                state: ButtonState::Released,
                ..
            })
        ));
        // US-QWERTY text on press; nothing on release.
        assert_eq!(evdev_to_event(30, true), Some(Event::Text("a".into()))); // KEY_A
        assert_eq!(evdev_to_event(57, true), Some(Event::Text(" ".into()))); // KEY_SPACE
        assert_eq!(evdev_to_event(30, false), None);
        assert_eq!(evdev_to_event(0, true), None);
    }

    #[test]
    fn evdev_maps_new_keys_and_modifiers() {
        let key = |code| match evdev_to_event(code, true, Modifiers::default()) {
            Some(Event::Key { code, .. }) => Some(code),
            _ => None,
        };
        assert_eq!(key(104), Some(KeyCode::PageUp));
        assert_eq!(key(109), Some(KeyCode::PageDown));
        assert_eq!(key(110), Some(KeyCode::Insert));
        assert_eq!(key(59), Some(KeyCode::F1));
        assert_eq!(key(68), Some(KeyCode::F10));
        assert_eq!(key(87), Some(KeyCode::F11));
        assert_eq!(key(88), Some(KeyCode::F12));
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        assert_eq!(
            evdev_to_event(30, true, shift),
            Some(Event::Text("A".into()))
        );
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        assert_eq!(
            evdev_to_event(46, true, ctrl), // KEY_C
            Some(Event::Key {
                code: KeyCode::Char('c'),
                state: ButtonState::Pressed,
                modifiers: ctrl,
            })
        );
        // Shift-selection needs the modifiers on named keys too.
        assert_eq!(
            evdev_to_event(105, true, shift),
            Some(Event::Key {
                code: KeyCode::ArrowLeft,
                state: ButtonState::Pressed,
                modifiers: shift,
            })
        );
    }

    #[test]
    fn parse_xkb_maps_keycodes_to_keysyms() {
        let keymap = "\
xkb_keymap {
xkb_keycodes \"x\" {
    <AC01> = 38;
    <SPCE> = 65;
    <TAB>  = 23;
};
xkb_symbols \"x\" {
    key <AC01> { [ a, A ] };
    key <SPCE> { [ space ] };
    key <TAB>  { [ Tab ] };
};
};";
        let map = parse_xkb(keymap);
        assert_eq!(map.sym(38, 0), Some("a"));
        assert_eq!(map.sym(38, XKB_SHIFT), Some("A"));
        assert_eq!(map.sym(65, 0), Some("space"));
        assert_eq!(map.sym(23, 0), Some("Tab"));
        // No modifier_map: Alt and the logo key default to Mod1 and Mod4.
        assert_eq!((map.alt_mask, map.meta_mask), (XKB_MOD1, XKB_MOD4));
        let none = Modifiers::default();
        let keysym_name_to_event = |name, pressed| keysym_name_to_event(name, pressed, none);
        // keysym names decode to events.
        assert_eq!(
            keysym_name_to_event("a", true),
            Some(Event::Text("a".into()))
        );
        assert_eq!(
            keysym_name_to_event("space", true),
            Some(Event::Text(" ".into()))
        );
        assert!(matches!(
            keysym_name_to_event("Tab", true),
            Some(Event::Key {
                code: KeyCode::Tab,
                ..
            })
        ));
        assert_eq!(keysym_name_to_event("a", false), None); // text on press only
    }

    #[test]
    fn parse_xkb_reads_levels_blocks_and_modifier_maps() {
        // The shape xkbcommon serializes: multi-line blocks for keys with an
        // explicit type, and modifier_map lines naming keys.
        let keymap = "\
xkb_keymap {
xkb_keycodes \"x\" {
    <TAB>  = 23;
    <AC01> = 38;
    <AE01> = 10;
    <LALT> = 64;
    <LWIN> = 133;
    <PGUP> = 112;
    <FK05> = 71;
};
xkb_symbols \"x\" {
    key <TAB>  {
        type= \"TWO_LEVEL\",
        symbols[Group1]= [             Tab,    ISO_Left_Tab ]
    };
    key  <AC01> {	[               a,               A ] };
    key <AE01> {	[               1,          exclam ] };
    key <LALT> {	[           Alt_L,          Meta_L ] };
    key <LWIN> {
        actions[Group1]= [ SetMods(modifiers=Mod4) ],
        symbols[Group1]= [ Super_L ]
    };
    key <PGUP> {	[           Prior ] };
    key <FK05> {	[              F5,              F5 ] };
    modifier_map Mod1 { <LALT> };
    modifier_map Mod4 { <LWIN> };
};
};";
        let map = parse_xkb(keymap);
        // Shift picks level 2 for printable keys, but Tab stays Tab.
        assert_eq!(map.sym(23, XKB_SHIFT), Some("Tab"));
        assert_eq!(map.sym(10, XKB_SHIFT), Some("exclam"));
        assert_eq!(map.sym(10, XKB_LOCK), Some("1")); // Caps Lock: letters only
        assert_eq!(map.sym(38, XKB_LOCK), Some("A"));
        assert_eq!(map.sym(38, XKB_LOCK | XKB_SHIFT), Some("a"));
        assert_eq!(map.sym(133, 0), Some("Super_L")); // not the actions list
        assert_eq!(map.sym(112, 0), Some("Prior"));
        assert_eq!((map.alt_mask, map.meta_mask), (XKB_MOD1, XKB_MOD4));

        // A keymap that puts the logo key on Mod3.
        let moved = keymap.replace("modifier_map Mod4", "modifier_map Mod3");
        let moved = parse_xkb(&moved);
        assert_eq!(moved.meta_mask, 1 << 5);
        assert!(wl_modifiers(1 << 5, Some(&moved)).meta);
        assert!(!wl_modifiers(XKB_MOD4, Some(&moved)).meta);

        // wl_keyboard.modifiers bits → Modifiers.
        assert_eq!(
            wl_modifiers(XKB_SHIFT | XKB_CONTROL | XKB_MOD1 | XKB_MOD4, Some(&map)),
            Modifiers {
                shift: true,
                ctrl: true,
                alt: true,
                meta: true,
            }
        );
        assert_eq!(
            wl_modifiers(XKB_LOCK | (1 << 4), None),
            Modifiers::default()
        );

        // Named keys, and Ctrl+printable as a shortcut.
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        assert!(matches!(
            keysym_name_to_event("Prior", true, ctrl),
            Some(Event::Key {
                code: KeyCode::PageUp,
                modifiers,
                ..
            }) if modifiers == ctrl
        ));
        assert!(matches!(
            keysym_name_to_event("F5", true, Modifiers::default()),
            Some(Event::Key {
                code: KeyCode::F5,
                ..
            })
        ));
        assert_eq!(
            keysym_name_to_event("v", true, ctrl),
            Some(Event::Key {
                code: KeyCode::Char('v'),
                state: ButtonState::Pressed,
                modifiers: ctrl,
            })
        );
        assert_eq!(
            keysym_name_to_event("exclam", true, Modifiers::default()),
            Some(Event::Text("!".into()))
        );
        assert_eq!(
            keysym_name_to_event("U20AC", true, Modifiers::default()),
            Some(Event::Text("\u{20ac}".into()))
        );
    }

    #[test]
    fn pointer_axis_deltas() {
        // A touchpad: no discrete steps, the value is in pixels (+ = down).
        assert_eq!(
            axis_delta(0, (7 * 256) as u32, 0),
            Some(ScrollDelta { dx: 0.0, dy: 7.0 })
        );
        // A wheel notch up: one line, whatever distance the compositor sent.
        assert_eq!(
            axis_delta(0, (-15i32 * 256) as u32, -1),
            Some(ScrollDelta { dx: 0.0, dy: -40.0 })
        );
        // Horizontal.
        assert_eq!(
            axis_delta(1, (15 * 256) as u32, 1),
            Some(ScrollDelta { dx: 40.0, dy: 0.0 })
        );
        assert_eq!(axis_delta(2, 0, 0), None);
    }

    #[test]
    fn fixed_point_and_pointer_buttons() {
        use crate::platform::event::PointerButton;
        // wl_fixed is signed 24.8: 256 → 1.0, 128 → 0.5, negatives wrap.
        assert_eq!(fixed_to_f64(256), 1.0);
        assert_eq!(fixed_to_f64(128), 0.5);
        assert_eq!(fixed_to_f64((-256i32) as u32), -1.0);
        assert_eq!(pointer_button(0x110), Some(PointerButton::Left));
        assert_eq!(pointer_button(0x111), Some(PointerButton::Right));
        assert_eq!(pointer_button(0x999), None);
    }
}
