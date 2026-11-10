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

/// Attention requests (`xdg_activation_v1`) and the bell.
mod attention;
mod clipboard;
/// Server-side decorations, or a title bar and borders drawn by Stipple.
mod decor;
/// Raw key events and client-side key repeat.
mod keys;
mod pointer;
mod text_input;
/// Several windows: creating, destroying and configuring toplevels.
mod toplevel;

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use crate::geometry::{PhysicalSize, Point, ScaleFactor, Size};
use crate::platform::ControlFlow;
use crate::platform::error::PlatformError;
use crate::platform::event::{ButtonState, Event, KeyCode, Modifiers};
use crate::platform::window::{CursorIcon, Window, WindowAttributes, fit_icon};
use crate::render::{Pixmap, Surface};
use pointer::{fixed_to_f64, pointer_button};

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
const XDG_SURFACE_ACK_CONFIGURE: u16 = 4;
const XDG_TOPLEVEL_SET_TITLE: u16 = 2;
const XDG_TOPLEVEL_SET_MAX_SIZE: u16 = 7;
const XDG_TOPLEVEL_SET_MIN_SIZE: u16 = 8;
const XDG_TOPLEVEL_SET_MAXIMIZED: u16 = 9;
const XDG_TOPLEVEL_UNSET_MAXIMIZED: u16 = 10;
const XDG_TOPLEVEL_SET_FULLSCREEN: u16 = 11;
const XDG_TOPLEVEL_UNSET_FULLSCREEN: u16 = 12;
// cursor-shape-v1: `wp_cursor_shape_manager_v1.get_pointer` and
// `wp_cursor_shape_device_v1.set_shape`.
const CURSOR_SHAPE_GET_POINTER: u16 = 1;
const CURSOR_SHAPE_SET_SHAPE: u16 = 1;
// xdg-toplevel-icon-v1: the manager's `create_icon` / `set_icon` and the
// icon's `add_buffer`.
const TOPLEVEL_ICON_CREATE_ICON: u16 = 1;
const TOPLEVEL_ICON_SET_ICON: u16 = 2;
const TOPLEVEL_ICON_ADD_BUFFER: u16 = 2;

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
const WL_KEYBOARD_LEAVE: u16 = 2;
const WL_KEYBOARD_REPEAT_INFO: u16 = 5;
const WL_POINTER_ENTER: u16 = 0;
const WL_POINTER_MOTION: u16 = 2;
const WL_POINTER_BUTTON: u16 = 3;
const WL_POINTER_AXIS: u16 = 4;
/// Since `wl_seat` v5: a wheel's notch count, sent before its `axis` event.
const WL_POINTER_AXIS_DISCRETE: u16 = 8;
/// The highest `wl_seat` version we speak. v5 adds `axis_discrete`; v4's
/// `repeat_info` drives client-side key repeat; the other events (`frame`,
/// `axis_source`, `axis_stop`, `name`) are ignored. v8 would replace
/// `axis_discrete` with `axis_value120`.
const WL_SEAT_VERSION: u32 = 5;

// `wl_shm` pixel format: 32-bit little-endian XRGB → bytes `[B, G, R, X]`.
const WL_SHM_FORMAT_XRGB8888: u32 = 1;
// The same with premultiplied alpha → bytes `[B, G, R, A]`.
const WL_SHM_FORMAT_ARGB8888: u32 = 0;

/// The largest icon we hand the compositor (it scales down from there).
const MAX_ICON: u32 = 256;

/// The `wp_cursor_shape_device_v1.shape` value for each [`CursorIcon`].
fn cursor_shape(icon: CursorIcon) -> u32 {
    match icon {
        CursorIcon::Default => 1,
        CursorIcon::Pointer => 4,
        CursorIcon::Crosshair => 8,
        CursorIcon::Text => 9,
        CursorIcon::Move => 13,
        CursorIcon::NotAllowed => 15,
        CursorIcon::Grab => 16,
        CursorIcon::Grabbing => 17,
        CursorIcon::EwResize => 26,
        CursorIcon::NsResize => 27,
        CursorIcon::NeswResize => 28,
        CursorIcon::NwseResize => 29,
        // Hidden through `wl_pointer.set_cursor` instead (see `apply_cursor`).
        CursorIcon::Hidden => 1,
    }
}

/// An icon as a square `wl_shm` ARGB8888 image (premultiplied `[B, G, R, A]`),
/// centered on a transparent square if it isn't one: xdg-toplevel-icon needs
/// square buffers. Returns the side length and the bytes.
fn icon_argb(icon: &Pixmap) -> (u32, Vec<u8>) {
    let icon = fit_icon(icon, MAX_ICON);
    let size = icon.size();
    let side = size.width.max(size.height).max(1);
    let (ox, oy) = ((side - size.width) / 2, (side - size.height) / 2);
    let mut out = vec![0u8; (side * side * 4) as usize];
    for y in 0..size.height {
        for x in 0..size.width {
            let [r, g, b, a] = icon.pixel(x, y).unwrap_or_default();
            let pre = |c: u8| ((c as u32 * a as u32 + 127) / 255) as u8;
            let o = (((y + oy) * side + x + ox) * 4) as usize;
            out[o..o + 4].copy_from_slice(&[pre(b), pre(g), pre(r), a]);
        }
    }
    (side, out)
}

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
        56 => Some(KeyCode::Alt), // KEY_LEFTALT (right Alt is often AltGr)
        127 => Some(KeyCode::ContextMenu), // KEY_COMPOSE, the Menu key
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
/// Ctrl, Alt or the logo key is held (press and release), otherwise
/// [`Event::Text`] on press only. AltGr is another modifier, so it still types.
fn char_event(c: char, state: ButtonState, modifiers: Modifiers) -> Option<Event> {
    if modifiers.ctrl || modifiers.meta || modifiers.alt {
        return Some(Event::Key {
            code: KeyCode::Char(c),
            state,
            modifiers,
        });
    }
    (state == ButtonState::Pressed).then(|| Event::Text(c.to_string()))
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
/// A keysym name as the rest of the backend expects it. Keymaps written by
/// tools like `wtype` name keysyms by value (`0x61`, `0xff09`, `0x100263a`)
/// rather than by name; those become the name (`Tab`), the character (`a`) or
/// the `UXXXX` form, which [`keysym_name_to_event`] understands.
fn keysym_canonical_name(sym: &str) -> String {
    let Some(value) = sym
        .strip_prefix("0x")
        .and_then(|hex| u32::from_str_radix(hex, 16).ok())
    else {
        return sym.to_string();
    };
    let named = match value {
        0xff09 => "Tab",
        0xfe20 => "ISO_Left_Tab",
        0xff0d => "Return",
        0xff8d => "KP_Enter",
        0xff08 => "BackSpace",
        0xffff => "Delete",
        0xff51 => "Left",
        0xff52 => "Up",
        0xff53 => "Right",
        0xff54 => "Down",
        0xff50 => "Home",
        0xff57 => "End",
        0xff55 => "Prior",
        0xff56 => "Next",
        0xff63 => "Insert",
        0xff1b => "Escape",
        0x20 => "space",
        0xffbe..=0xffc9 => return format!("F{}", value - 0xffbe + 1),
        // Latin-1 keysyms are the code point; 0x01000000 + cp is Unicode.
        0x21..=0x7e | 0xa0..=0xff => {
            return char::from_u32(value).map_or_else(|| sym.to_string(), String::from);
        }
        0x0100_0000..=0x0110_ffff => return format!("U{:04X}", value - 0x0100_0000),
        _ => return sym.to_string(),
    };
    named.to_string()
}

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
                .map(keysym_canonical_name)
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
        "Alt_L" | "Alt_R" | "Meta_L" | "Meta_R" => KeyCode::Alt,
        "Menu" => KeyCode::ContextMenu,
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
    // Printable: a one-character keysym name is the character itself; other
    // names (`space`, `eacute`, `Cyrillic_a`, `UXXXX`, …) go through the
    // keysym table.
    let ch = crate::platform::compose::keysym_to_char(keysym_value(name)?);
    char_event(ch?, state, modifiers)
}

/// The keysym value of a keysym name as [`XkbKeymap::sym`] gives it: a
/// one-character name is that character's keysym, anything else a name from
/// the keysym table (`dead_acute`, `eacute`, `UXXXX`, …).
fn keysym_value(name: &str) -> Option<u32> {
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if (c as u32) < 0x100 => Some(c as u32),
        (Some(c), None) => Some(0x0100_0000 + c as u32),
        _ => crate::platform::compose::keysym_from_name(name),
    }
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
    /// The `wp_cursor_shape_device_v1` for our pointer, when the compositor
    /// supports cursor-shape-v1.
    cursor_device: Option<u32>,
    /// The serial of the pointer's last `enter`, which `set_shape` must quote.
    enter_serial: Option<u32>,
    /// The cursor the app asked for, re-applied on each `enter`.
    cursor: CursorIcon,
    /// The seat's `zwp_text_input_v3`, when the compositor offers it.
    text_input: text_input::TextInput,
    /// The seat's `wl_pointer`, once it has one (to hide the cursor).
    pointer: Option<u32>,
    /// The decorations of the window in use (see `toplevel::use_decor`),
    /// and the other windows'.
    decor: decor::Decor,
    decors: Vec<decor::Decor>,
    /// Activation (attention) requests and the bell.
    attention: attention::Attention,
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
            cursor_device: None,
            enter_serial: None,
            cursor: CursorIcon::Default,
            pointer: None,
            text_input: text_input::TextInput::default(),
            decor: decor::Decor::default(),
            decors: Vec::new(),
            attention: attention::Attention::default(),
        })
    }

    /// Show the current cursor shape, if the compositor supports
    /// cursor-shape-v1 and the pointer has entered our surface.
    fn apply_cursor(&mut self) {
        if let (Some(device), Some(serial)) = (self.cursor_device, self.enter_serial) {
            let mut a = Vec::new();
            arg_u32(&mut a, serial);
            if self.cursor == CursorIcon::Hidden
                && self.decor.cursor().is_none()
                && let Some(pointer) = self.pointer
            {
                // `wl_pointer.set_cursor` with no surface hides it (and a
                // shape shows it again). Without cursor-shape-v1 there'd be
                // no shape to bring it back, so it isn't hidden then.
                arg_u32(&mut a, 0); // surface
                arg_u32(&mut a, 0); // hotspot x
                arg_u32(&mut a, 0); // hotspot y
                let _ = self.send(pointer, pointer::WL_POINTER_SET_CURSOR, &a);
            } else {
                // Over the decorations Stipple draws, their cursor (a resize
                // arrow over the margin).
                let shape = self.decor.cursor().unwrap_or(self.cursor);
                arg_u32(&mut a, cursor_shape(shape));
                let _ = self.send(device, CURSOR_SHAPE_SET_SHAPE, &a);
            }
            let _ = self.stream.flush();
        }
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

/// Deliver drag-and-drop messages from the data device to the app, answering
/// the drag source after each enter and move with what the app said through
/// `set_drop_accepted`.
fn deliver_dnd(
    shared: &Shared,
    win: &WaylandWindow,
    messages: Vec<clipboard::DndMessage>,
    handler: &mut dyn FnMut(Event, &dyn Window) -> ControlFlow,
) -> ControlFlow {
    use clipboard::DndMessage;
    for message in messages {
        let flow = match message {
            DndMessage::Enter { x, y, offer } => {
                let position = Point::new(x, y);
                win.drop_accepted.set(false);
                dbg(format_args!("dnd: enter at {position:?} {offer:?}"));
                let flow = handler(Event::DragEnter { position, offer }, win);
                clipboard::answer_drag(&mut shared.lock().unwrap(), win.drop_accepted.get());
                flow
            }
            DndMessage::Motion { x, y } => {
                let position = Point::new(x, y);
                let was = win.drop_accepted.get();
                let flow = handler(Event::DragOver { position }, win);
                if win.drop_accepted.get() != was {
                    clipboard::answer_drag(&mut shared.lock().unwrap(), win.drop_accepted.get());
                }
                flow
            }
            DndMessage::Leave => {
                dbg(format_args!("dnd: leave"));
                handler(Event::DragLeave, win)
            }
            DndMessage::Drop { x, y } => {
                let data =
                    clipboard::take_drop(&mut shared.lock().unwrap(), win.drop_accepted.get());
                dbg(format_args!("dnd: drop {data:?}"));
                match data {
                    Some(data) => handler(
                        Event::Drop {
                            position: Point::new(x, y),
                            data,
                        },
                        win,
                    ),
                    None => handler(Event::DragLeave, win),
                }
            }
        };
        if flow == ControlFlow::Exit {
            return flow;
        }
    }
    ControlFlow::Wait
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
    format: u32,
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
/// `size` over it in `format`. Sends `create_pool` (with the fd) and
/// `create_buffer`.
fn make_shm(
    conn: &mut WlConn,
    shm: u32,
    size: PhysicalSize,
    format: u32,
) -> Result<ShmBuffer, PlatformError> {
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
    arg_u32(&mut a, format);
    conn.send(pool, WL_SHM_POOL_CREATE_BUFFER, &a).map_err(os)?;

    Ok(ShmBuffer {
        fd,
        ptr: addr as *mut u8,
        capacity: bytes,
        buffer,
        size,
        format,
    })
}

// ---- Window + Surface -------------------------------------------------------

type Shared = Arc<Mutex<WlConn>>;

struct WaylandWindow {
    conn: Shared,
    surface: u32,
    xdg_surface: u32,
    shm: u32,
    toplevel: u32,
    /// The `xdg_toplevel_icon_manager_v1`, when the compositor has one.
    icon_manager: Option<u32>,
    /// What new windows are made from, and where they wait for the loop.
    globals: toplevel::Globals,
    reg: Rc<RefCell<toplevel::Reg>>,
    /// The buffer size of the content, and the content's size in surface
    /// units (logical pixels), which the scale multiplies.
    size: Cell<PhysicalSize>,
    logical: Cell<PhysicalSize>,
    /// The window size the last `xdg_toplevel.configure` asked for (title
    /// bar included), applied with its `xdg_surface.configure`; and the
    /// maximized and fullscreen states, as asked and as applied.
    pending_size: Cell<Option<PhysicalSize>>,
    pending_state: Cell<(bool, bool)>,
    state: Cell<(bool, bool)>,
    /// Buffer pixels per surface unit (the scale of the output the window is
    /// on, which `output` names), shared with its surface.
    scale: Rc<Cell<u32>>,
    output: Cell<Option<u32>>,
    /// The icon's pixels, which must stay mapped while the compositor uses them.
    icon: RefCell<Option<ShmBuffer>>,
    /// Whether the app takes the drag over the surface (see
    /// `set_drop_accepted`).
    drop_accepted: std::cell::Cell<bool>,
}

impl WaylandWindow {
    /// The window for `objects`, `logical` surface units at `scale`.
    fn new(
        conn: Shared,
        objects: toplevel::Objects,
        globals: toplevel::Globals,
        reg: Rc<RefCell<toplevel::Reg>>,
        logical: PhysicalSize,
        scale: u32,
        icon: Option<ShmBuffer>,
    ) -> Self {
        Self {
            conn,
            surface: objects.surface,
            xdg_surface: objects.xdg_surface,
            shm: globals.shm,
            toplevel: objects.toplevel,
            icon_manager: globals.icon_manager,
            globals,
            reg,
            size: Cell::new(PhysicalSize::new(
                logical.width * scale,
                logical.height * scale,
            )),
            logical: Cell::new(logical),
            pending_size: Cell::new(None),
            pending_state: Cell::new((false, false)),
            state: Cell::new((false, false)),
            scale: Rc::new(Cell::new(scale)),
            output: Cell::new(None),
            icon: RefCell::new(icon),
            drop_accepted: Cell::new(false),
        }
    }

    fn objects(&self) -> toplevel::Objects {
        toplevel::Objects {
            surface: self.surface,
            xdg_surface: self.xdg_surface,
            toplevel: self.toplevel,
        }
    }

    /// Take a new content size (logical) at the current scale.
    fn set_logical(&self, logical: PhysicalSize) {
        let scale = self.scale.get();
        self.logical.set(logical);
        self.size.set(PhysicalSize::new(
            logical.width * scale,
            logical.height * scale,
        ));
    }

    /// Send a toplevel request, then commit so it applies (xdg-shell state is
    /// double-buffered).
    fn toplevel_request(&self, opcode: u16, args: &[u8]) {
        let mut conn = self.conn.lock().unwrap();
        let _ = conn.send(self.toplevel, opcode, args);
        let _ = conn.send(self.surface, WL_SURFACE_COMMIT, &[]);
        let _ = conn.stream.flush();
    }
}

/// The arguments of `xdg_toplevel.set_min_size` / `set_max_size`: a size in
/// surface coordinates (logical pixels), 0 for "no limit".
fn size_args(size: Option<Size>) -> Vec<u8> {
    let (w, h) = size.map_or((0, 0), |s| {
        (
            s.width.round().max(1.0) as u32,
            s.height.round().max(1.0) as u32,
        )
    });
    let mut a = Vec::new();
    arg_u32(&mut a, w);
    arg_u32(&mut a, h);
    a
}

/// Set (or clear) the toplevel's icon through xdg-toplevel-icon-v1, returning
/// the buffer holding its pixels.
fn set_toplevel_icon(
    conn: &mut WlConn,
    shm: u32,
    manager: u32,
    toplevel: u32,
    icon: Option<&Pixmap>,
) -> Option<ShmBuffer> {
    let mut a = Vec::new();
    let Some(icon) = icon else {
        arg_u32(&mut a, toplevel);
        arg_u32(&mut a, 0); // null icon: back to the default
        let _ = conn.send(manager, TOPLEVEL_ICON_SET_ICON, &a);
        return None;
    };
    let (side, bytes) = icon_argb(icon);
    let mut buf = make_shm(
        conn,
        shm,
        PhysicalSize::new(side, side),
        WL_SHM_FORMAT_ARGB8888,
    )
    .ok()?;
    buf.bytes()[..bytes.len()].copy_from_slice(&bytes);
    let id = conn.new_id();
    arg_u32(&mut a, id);
    conn.send(manager, TOPLEVEL_ICON_CREATE_ICON, &a).ok()?;
    let mut a = Vec::new();
    arg_u32(&mut a, buf.buffer);
    arg_u32(&mut a, 1); // scale
    conn.send(id, TOPLEVEL_ICON_ADD_BUFFER, &a).ok()?;
    let mut a = Vec::new();
    arg_u32(&mut a, toplevel);
    arg_u32(&mut a, id);
    conn.send(manager, TOPLEVEL_ICON_SET_ICON, &a).ok()?;
    Some(buf)
}

impl std::fmt::Debug for WaylandWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WaylandWindow")
            .field("surface", &self.surface)
            .field("size", &self.size.get())
            .finish()
    }
}

impl Window for WaylandWindow {
    fn id(&self) -> crate::platform::WindowId {
        crate::platform::WindowId(self.surface as u64)
    }
    fn set_tray(&self, tray: Option<&crate::platform::NativeTray>) -> bool {
        crate::platform::shell::sni::set_tray(tray)
    }
    fn inner_size(&self) -> PhysicalSize {
        self.size.get()
    }
    fn scale_factor(&self) -> ScaleFactor {
        ScaleFactor::new(self.scale.get() as f64)
    }
    fn request_redraw(&self) {}
    fn set_title(&self, title: &str) {
        // The title bar Stipple draws (if it does) shows it from the next
        // frame on.
        {
            let mut conn = self.conn.lock().unwrap();
            toplevel::use_decor(&mut conn, self.toplevel);
            conn.decor.title = title.to_string();
        }
        let mut a = Vec::new();
        arg_str(&mut a, title);
        self.toplevel_request(XDG_TOPLEVEL_SET_TITLE, &a);
        crate::platform::window::debug_window(format_args!("title {:#x} {title:?}", self.surface));
    }
    fn open_window(&self, attrs: WindowAttributes) -> Option<crate::platform::WindowId> {
        // Another toplevel on the same connection, adopted by the loop once
        // the handler returns. Wayland leaves focus to the compositor.
        let (objects, icon) = {
            let mut conn = self.conn.lock().unwrap();
            toplevel::create(&mut conn, self.globals, &attrs).ok()?
        };
        let logical = ScaleFactor::IDENTITY.to_physical(attrs.logical_size);
        let win = WaylandWindow::new(
            self.conn.clone(),
            objects,
            self.globals,
            self.reg.clone(),
            PhysicalSize::new(logical.width.max(1), logical.height.max(1)),
            self.scale.get(),
            icon,
        );
        let id = win.id();
        self.reg.borrow_mut().opened.push(win);
        Some(id)
    }
    fn close_window(&self) {
        self.reg.borrow_mut().closed.push(self.surface);
    }
    fn set_visible(&self, visible: bool) {
        toplevel::set_visible(&mut self.conn.lock().unwrap(), self.surface, visible);
    }
    fn info(&self) -> crate::platform::WindowInfo {
        let mut info = crate::platform::WindowInfo::from_size(self.size.get(), self.scale_factor());
        (info.maximized, info.fullscreen) = self.state.get();
        info
    }
    fn set_inner_size(&self, size: Size) {
        self.reg.borrow_mut().resized.push((self.surface, size));
    }
    fn set_minimized(&self, minimized: bool) {
        // There's no request to bring a window back: the user does.
        if minimized {
            self.toplevel_request(toplevel::XDG_TOPLEVEL_SET_MINIMIZED, &[]);
        }
    }
    fn request_attention(&self, kind: Option<crate::platform::UserAttention>) {
        // Nothing to withdraw: the compositor drops it on activation.
        if kind.is_some() {
            let mut conn = self.conn.lock().unwrap();
            attention::request(&mut conn, self.surface);
            let _ = conn.stream.flush();
        }
    }
    fn beep(&self) {
        let mut conn = self.conn.lock().unwrap();
        attention::ring(&mut conn, self.surface);
        let _ = conn.stream.flush();
    }
    fn clipboard(&self) -> Option<String> {
        clipboard::text(&mut self.conn.lock().unwrap())
    }
    fn set_drop_accepted(&self, accepted: bool) {
        self.drop_accepted.set(accepted);
    }
    fn start_drag(&self, data: &crate::platform::DropData) -> bool {
        clipboard::drag_out::start(&mut self.conn.lock().unwrap(), self.surface, data)
    }
    fn set_clipboard(&self, text: &str) {
        let content = crate::platform::clipboard::ClipboardContent::text(text);
        clipboard::set(&mut self.conn.lock().unwrap(), &content);
    }
    fn read_clipboard(
        &self,
        kind: crate::platform::clipboard::ClipboardKind,
        formats: crate::platform::clipboard::ClipboardFormats,
    ) -> crate::platform::clipboard::ClipboardRead {
        use crate::platform::clipboard::{ClipboardKind, ClipboardRead};
        let mut conn = self.conn.lock().unwrap();
        ClipboardRead::Ready(match kind {
            ClipboardKind::Clipboard => clipboard::read(&mut conn, formats),
            ClipboardKind::Primary => clipboard::primary::read(&mut conn, formats),
        })
    }
    fn write_clipboard(
        &self,
        kind: crate::platform::clipboard::ClipboardKind,
        content: &crate::platform::clipboard::ClipboardContent,
    ) {
        use crate::platform::clipboard::ClipboardKind;
        let mut conn = self.conn.lock().unwrap();
        match kind {
            ClipboardKind::Clipboard => clipboard::set(&mut conn, content),
            ClipboardKind::Primary => clipboard::primary::set(&mut conn, content),
        }
    }
    fn has_primary_selection(&self) -> bool {
        self.conn.lock().unwrap().clip.primary.available()
    }
    fn set_ime(&self, state: Option<&crate::platform::ImeState>) {
        text_input::set_state(&mut self.conn.lock().unwrap(), state);
    }
    fn set_cursor(&self, cursor: CursorIcon) {
        let mut conn = self.conn.lock().unwrap();
        conn.cursor = cursor;
        conn.apply_cursor();
    }
    fn set_min_size(&self, size: Option<Size>) {
        // The compositor's limits are the window's, title bar included.
        let (min, _) = {
            let mut conn = self.conn.lock().unwrap();
            toplevel::use_decor(&mut conn, self.toplevel);
            conn.decor.set_limits(Some(size), None)
        };
        self.toplevel_request(XDG_TOPLEVEL_SET_MIN_SIZE, &size_args(min));
    }
    fn set_max_size(&self, size: Option<Size>) {
        let (_, max) = {
            let mut conn = self.conn.lock().unwrap();
            toplevel::use_decor(&mut conn, self.toplevel);
            conn.decor.set_limits(None, Some(size))
        };
        self.toplevel_request(XDG_TOPLEVEL_SET_MAX_SIZE, &size_args(max));
    }
    fn set_maximized(&self, maximized: bool) {
        let opcode = if maximized {
            XDG_TOPLEVEL_SET_MAXIMIZED
        } else {
            XDG_TOPLEVEL_UNSET_MAXIMIZED
        };
        self.toplevel_request(opcode, &[]);
    }
    fn set_fullscreen(&self, fullscreen: bool) {
        if fullscreen {
            // A null output lets the compositor pick.
            self.toplevel_request(XDG_TOPLEVEL_SET_FULLSCREEN, &0u32.to_le_bytes());
        } else {
            self.toplevel_request(XDG_TOPLEVEL_UNSET_FULLSCREEN, &[]);
        }
    }
    fn set_icon(&self, icon: Option<&Pixmap>) {
        let Some(manager) = self.icon_manager else {
            return;
        };
        let mut conn = self.conn.lock().unwrap();
        let buf = set_toplevel_icon(&mut conn, self.shm, manager, self.toplevel, icon);
        // The icon is double-buffered toplevel state: commit to apply it.
        let _ = conn.send(self.surface, WL_SURFACE_COMMIT, &[]);
        let _ = conn.stream.flush();
        *self.icon.borrow_mut() = buf;
    }
    fn create_surface(&self) -> Box<dyn Surface> {
        Box::new(WaylandSurface {
            conn: self.conn.clone(),
            surface: self.surface,
            toplevel: self.toplevel,
            shm: self.shm,
            size: self.size.get(),
            scale: self.scale.clone(),
            buffer: None,
        })
    }
}

struct WaylandSurface {
    conn: Shared,
    surface: u32,
    /// Its window's toplevel, whose decorations frame it.
    toplevel: u32,
    shm: u32,
    size: PhysicalSize,
    /// Its window's buffer scale.
    scale: Rc<Cell<u32>>,
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
        let scale = self.scale.get().max(1);
        toplevel::use_decor(&mut conn, self.toplevel);
        // Decorations Stipple draws go around the content, in an ARGB buffer
        // (the resize margin is transparent).
        let content = size;
        let framed = conn.decor.frame(pixmap, scale);
        let (size, format) = match &framed {
            Some((size, _)) => (*size, WL_SHM_FORMAT_ARGB8888),
            None => (size, WL_SHM_FORMAT_XRGB8888),
        };
        // (Re)create the shm buffer if missing or the size changed.
        if self.buffer.as_ref().map(|b| (b.size, b.format)) != Some((size, format)) {
            self.buffer = make_shm(&mut conn, self.shm, size, format).ok();
        }
        let Some(buf) = self.buffer.as_mut() else {
            return;
        };
        let stride = size.width as usize * 4;
        let bytes = stride * size.height as usize;
        let buffer = buf.buffer;
        match &framed {
            Some((_, argb)) => buf.bytes()[..bytes].copy_from_slice(argb),
            None => rgba_to_xrgb(pixmap.as_bytes(), &mut buf.bytes()[..bytes]),
        }
        // Which part is the window proper (all of it, without decorations).
        let logical = PhysicalSize::new(content.width / scale, content.height / scale);
        decor::send_geometry(&mut conn, logical);

        let surface = self.surface;
        // attach(buffer, 0, 0); damage(0, 0, w, h); commit.
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
        request_frame_callback(&mut conn, surface);
        let _ = conn.send(surface, WL_SURFACE_COMMIT, &[]);
        let _ = conn.stream.flush();
        dbg(format_args!(
            "present {}x{}{}",
            size.width,
            size.height,
            if framed.is_some() { " decorated" } else { "" }
        ));
    }
}

/// `wl_surface.frame`: a `wl_callback` that fires when the compositor wants the
/// next frame.
const WL_SURFACE_FRAME: u16 = 3;

/// The outstanding frame callback's object id, or 0 for none.
static FRAME_CALLBACK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// If an animation wants a frame, ask the compositor for a frame callback with
/// the commit that's about to go out, so the next frame is paced by its repaint
/// (and stops while the window is hidden). Call just before `commit`.
fn request_frame_callback(conn: &mut WlConn, surface: u32) {
    use std::sync::atomic::Ordering;
    if !crate::platform::frame::wanted() {
        return;
    }
    if FRAME_CALLBACK.load(Ordering::Acquire) == 0 {
        let callback = conn.new_id();
        let mut a = Vec::new();
        arg_u32(&mut a, callback);
        if conn.send(surface, WL_SURFACE_FRAME, &a).is_err() {
            return;
        }
        FRAME_CALLBACK.store(callback, Ordering::Release);
    }
    crate::platform::frame::claim();
}

/// Whether `(object, opcode)` is the outstanding frame callback firing; if so,
/// deliver the frame.
fn frame_callback_done(object: u32, opcode: u16) -> bool {
    use std::sync::atomic::Ordering;
    let ours = object != 0
        && opcode == WL_CALLBACK_DONE
        && FRAME_CALLBACK
            .compare_exchange(object, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
    if ours {
        crate::platform::frame::tick();
    }
    ours
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
    let mut cursor_shape_name = None;
    let mut icon_manager_name = None;
    let mut text_input_name = None;
    let mut decoration_name = None;
    let mut activation_name = None;
    let mut bell_name = None;
    let mut primary_name = None;
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
                    "wp_cursor_shape_manager_v1" => cursor_shape_name = Some(name),
                    "xdg_toplevel_icon_manager_v1" => icon_manager_name = Some(name),
                    text_input::MANAGER => text_input_name = Some(name),
                    decor::MANAGER => decoration_name = Some(name),
                    attention::ACTIVATION => activation_name = Some(name),
                    attention::SYSTEM_BELL => bell_name = Some(name),
                    clipboard::primary::MANAGER => primary_name = Some(name),
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

    // Optional seat. The keyboard is created lazily from its `capabilities`
    // event — calling get_keyboard on a seat that never had the keyboard
    // capability (e.g. a headless compositor with no devices) is a protocol
    // error, so we wait until one is advertised.
    let seat = seat_name.map(|name| bind(&mut conn, name, "wl_seat", seat_version));
    // Optional protocols: cursor shapes, and the toplevel icon.
    let cursor_shape_manager =
        cursor_shape_name.map(|name| bind(&mut conn, name, "wp_cursor_shape_manager_v1", 1));
    let icon_manager =
        icon_manager_name.map(|name| bind(&mut conn, name, "xdg_toplevel_icon_manager_v1", 1));
    dbg(format_args!("seat={seat:?} v{seat_version}"));
    // The clipboard goes through the seat's data device.
    if let (Some(seat), Some((name, version))) = (seat, data_manager) {
        let manager = bind(&mut conn, name, "wl_data_device_manager", version);
        clipboard::setup(&mut conn, manager, version, seat);
    }
    // ...and the primary selection through its own device.
    if let (Some(seat), Some(name)) = (seat, primary_name) {
        let manager = bind(&mut conn, name, clipboard::primary::MANAGER, 1);
        clipboard::primary::setup(&mut conn, manager, seat);
    }
    // Input methods go through the seat's text input.
    if let (Some(seat), Some(name)) = (seat, text_input_name) {
        let manager = bind(&mut conn, name, text_input::MANAGER, 1);
        text_input::setup(&mut conn, manager, seat);
    }
    let decoration_manager = decoration_name.map(|name| bind(&mut conn, name, decor::MANAGER, 1));
    let activation = activation_name.map(|name| bind(&mut conn, name, attention::ACTIVATION, 1));
    let bell = bell_name.map(|name| bind(&mut conn, name, attention::SYSTEM_BELL, 1));
    attention::setup(&mut conn, activation, bell);

    // The first window; `open_window` makes more the same way.
    let globals = toplevel::Globals {
        compositor,
        wm_base,
        shm,
        icon_manager,
        decoration_manager,
        seat,
    };
    let (objects, icon) = toplevel::create(&mut conn, globals, &attrs).map_err(os)?;
    dbg(format_args!(
        "window: surface={} xdg={} toplevel={} size={}x{}",
        objects.surface, objects.xdg_surface, objects.toplevel, size.width, size.height
    ));

    let shared: Shared = Arc::new(Mutex::new(conn));
    let reg: Rc<RefCell<toplevel::Reg>> = Default::default();
    let mut windows = vec![WaylandWindow::new(
        shared.clone(),
        objects,
        globals,
        reg.clone(),
        size,
        1,
        icon,
    )];
    // The windows the pointer, the keyboard and a drag are over (surfaces).
    let mut pointer_focus = objects.surface;
    let mut keyboard_focus = objects.surface;
    let mut drag_focus = objects.surface;

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
    // Dead keys and compose sequences, raw keys and key repeat: the client's
    // job on Wayland.
    let mut keys = keys::Keys::default();
    // Wheel notches and the scroll source, for the `axis` events that follow.
    let mut axes = pointer::Axes::default();
    // Between messages, wait on the connection and on wakeups (timers, other
    // threads) together.
    crate::platform::wake::install_unix();
    let fd = shared.lock().unwrap().stream.as_raw_fd();
    // Unfocused until the compositor activates the toplevel.
    if handler(Event::FocusChanged(false), &windows[0]) == ControlFlow::Exit {
        return Ok(());
    }
    loop {
        // Windows opened, closed or resized by the last handler call.
        if settle(&shared, &reg, &mut windows, &mut handler) == ControlFlow::Exit
            || windows.is_empty()
        {
            break;
        }
        // Messages a paste read while it waited come first.
        let queued = shared.lock().unwrap().queue.pop_front();
        if queued.is_none()
            && !crate::platform::wake::wait_readable_until(fd, keys.repeat.deadline())
        {
            // A held key repeats.
            let now = std::time::Instant::now();
            if let Some(key) = keys.repeat.due(now) {
                dbg(format_args!("repeat key {key}"));
                let flow = keys.key(
                    keymap.as_ref(),
                    mods_mask,
                    (key, keys::Stroke::Repeat),
                    now,
                    &mut |e| handler(e, find(&windows, keyboard_focus)),
                );
                if flow == ControlFlow::Exit {
                    break;
                }
            }
            // Every window hears the wake, to bring itself up to date.
            if crate::platform::wake::take(std::time::Instant::now())
                && windows
                    .iter()
                    .any(|w| handler(Event::Wake, w) == ControlFlow::Exit)
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
            // A drag entering names the surface it's over.
            if message.1 == WL_DATA_DEVICE_ENTER
                && conn.clip.device != 0
                && message.0 == conn.clip.device
                && let Some(s) = message.2.get(4..8)
            {
                drag_focus = u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
            }
            if clipboard::handle(&mut conn, message.0, message.1, &message.2) {
                let dnd = std::mem::take(&mut conn.clip.dnd);
                drop(conn);
                let win = find(&windows, drag_focus);
                if deliver_dnd(&shared, win, dnd, &mut handler) == ControlFlow::Exit {
                    break;
                }
                continue;
            }
            message
        };
        let first_word = args
            .get(0..4)
            .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        let ours = |surface: u32| windows.iter().any(|w| w.surface == surface);
        // Input-method updates for the focused text field.
        let ime_surface = if ours(first_word) { first_word } else { 0 };
        let ime = text_input::handle(
            &mut shared.lock().unwrap(),
            object,
            opcode,
            &args,
            ime_surface,
        );
        if let Some(events) = ime {
            let win = find(&windows, keyboard_focus);
            if events
                .into_iter()
                .any(|ime| handler(Event::Ime(ime), win) == ControlFlow::Exit)
            {
                break;
            }
            continue;
        }
        // The compositor is ready for the next animation frame: it arrives as
        // a wake once this loop goes back to waiting.
        if frame_callback_done(object, opcode) {
            continue;
        }
        // Decoration modes and activation tokens.
        {
            let mut conn = shared.lock().unwrap();
            if (toplevel::use_decor_of(&mut conn, object)
                && decor::handle(&mut conn, object, opcode, &args))
                || attention::handle(&mut conn, object, opcode, &args)
            {
                let _ = conn.stream.flush();
                continue;
            }
        }
        // Moving to another output, or an output's scale changing, means rendering
        // at a new resolution.
        let entered = opcode == WL_SURFACE_ENTER && ours(object);
        let rescaled = opcode == WL_OUTPUT_SCALE && outputs.iter().any(|(id, _)| *id == object);
        if entered || rescaled {
            if entered {
                find(&windows, object).output.set(Some(first_word));
            } else if let Some(entry) = outputs.iter_mut().find(|(id, _)| *id == object) {
                entry.1 = first_word.max(1);
            }
            for win in &windows {
                let scale = outputs
                    .iter()
                    .find(|(id, _)| Some(*id) == win.output.get())
                    .map_or(1, |(_, scale)| *scale);
                if win.scale.get() == scale {
                    continue;
                }
                dbg(format_args!("buffer scale {scale}"));
                shared.lock().unwrap().buffer_scale = scale;
                win.scale.set(scale);
                win.set_logical(win.logical.get());
                if handler(Event::Resized(win.size.get()), win) == ControlFlow::Exit
                    || handler(Event::RedrawRequested, win) == ControlFlow::Exit
                {
                    return Ok(());
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
                keys.reset();
                dbg(format_args!("keyboard dropped (caps={caps})"));
            }
            if pointer.is_none() && caps & WL_SEAT_CAP_POINTER != 0 {
                let ptr = conn.new_id();
                let mut a = Vec::new();
                arg_u32(&mut a, ptr);
                if conn.send(seat.unwrap(), WL_SEAT_GET_POINTER, &a).is_ok() {
                    pointer = Some(ptr);
                    conn.pointer = Some(ptr);
                    dbg(format_args!("pointer={ptr} (caps={caps})"));
                    if let Some(manager) = cursor_shape_manager {
                        let device = conn.new_id();
                        let mut a = Vec::new();
                        arg_u32(&mut a, device);
                        arg_u32(&mut a, ptr);
                        if conn.send(manager, CURSOR_SHAPE_GET_POINTER, &a).is_ok() {
                            conn.cursor_device = Some(device);
                        }
                    }
                }
            }
        } else if Some(object) == pointer
            && (opcode == WL_POINTER_ENTER || opcode == WL_POINTER_MOTION)
        {
            // enter: serial, surface, x, y. motion: time, x, y. Both end in two
            // wl_fixed coordinates.
            let n = args.len();
            if opcode == WL_POINTER_ENTER {
                if let Some(s) = args.get(4..8) {
                    pointer_focus = u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
                }
                // The compositor resets the cursor on enter; set ours again.
                let serial = u32::from_le_bytes([args[0], args[1], args[2], args[3]]);
                let mut conn = shared.lock().unwrap();
                toplevel::use_decor(&mut conn, find(&windows, pointer_focus).toplevel);
                conn.enter_serial = Some(serial);
                conn.apply_cursor();
            }
            let x = u32::from_le_bytes([args[n - 8], args[n - 7], args[n - 6], args[n - 5]]);
            let y = u32::from_le_bytes([args[n - 4], args[n - 3], args[n - 2], args[n - 1]]);
            pointer_pos = Point::new(fixed_to_f64(x), fixed_to_f64(y));
            crate::platform::set_pointer_modifiers(wl_modifiers(mods_mask, keymap.as_ref()));
            // Over decorations Stipple draws, the pointer is theirs.
            let win = find(&windows, pointer_focus);
            let motion = {
                let mut conn = shared.lock().unwrap();
                toplevel::use_decor(&mut conn, win.toplevel);
                let motion = conn.decor.motion(pointer_pos, win.logical.get());
                conn.apply_cursor();
                motion
            };
            let mut events = Vec::new();
            if motion.left_content {
                events.push(Event::PointerLeft);
            }
            if let Some(position) = motion.forward {
                events.push(Event::PointerMoved { position });
            }
            if motion.redraw {
                events.push(Event::RedrawRequested);
            }
            if events
                .into_iter()
                .any(|event| handler(event, win) == ControlFlow::Exit)
            {
                break;
            }
        } else if Some(object) == pointer && opcode == WL_POINTER_BUTTON {
            // serial, time, button(evdev), state(0 released / 1 pressed).
            let serial = u32::from_le_bytes([args[0], args[1], args[2], args[3]]);
            let button = u32::from_le_bytes([args[8], args[9], args[10], args[11]]);
            let pressed = u32::from_le_bytes([args[12], args[13], args[14], args[15]]) == 1;
            {
                let mut conn = shared.lock().unwrap();
                conn.clip.serial = serial;
                // A drag out of the app cites the press that began it; a
                // new press means the last one is over.
                conn.clip.press_serial = pressed.then_some(serial);
                if pressed {
                    clipboard::drag_out::end(&mut conn);
                }
            }
            // A press on the decorations moves, resizes or clicks their
            // buttons; one on the content is the app's.
            let win = find(&windows, pointer_focus);
            let press = {
                let mut conn = shared.lock().unwrap();
                toplevel::use_decor(&mut conn, win.toplevel);
                let logical = win.logical.get();
                let press = decor::button(&mut conn, pointer_pos, logical, button, pressed, serial);
                let _ = conn.stream.flush();
                press
            };
            let position = match press {
                decor::Press::Forward(position) => position,
                decor::Press::Close => {
                    if handler(Event::CloseRequested, win) == ControlFlow::Exit {
                        break;
                    }
                    continue;
                }
                decor::Press::Redraw => {
                    if handler(Event::RedrawRequested, win) == ControlFlow::Exit {
                        break;
                    }
                    continue;
                }
                decor::Press::Done => continue,
            };
            let state = if pressed {
                ButtonState::Pressed
            } else {
                ButtonState::Released
            };
            crate::platform::set_pointer_modifiers(wl_modifiers(mods_mask, keymap.as_ref()));
            if handler(
                Event::PointerButton {
                    button: pointer_button(button),
                    state,
                    position,
                },
                win,
            ) == ControlFlow::Exit
            {
                break;
            }
        } else if Some(object) == pointer && opcode == pointer::WL_POINTER_LEAVE {
            dbg(format_args!("pointer left"));
            let win = find(&windows, pointer_focus);
            let redraw = {
                let mut conn = shared.lock().unwrap();
                toplevel::use_decor(&mut conn, win.toplevel);
                conn.decor.leave()
            };
            if handler(Event::PointerLeft, win) == ControlFlow::Exit
                || (redraw && handler(Event::RedrawRequested, win) == ControlFlow::Exit)
            {
                break;
            }
        } else if Some(object) == pointer && opcode == WL_POINTER_AXIS_DISCRETE {
            // axis(4), discrete(4: signed notches).
            let axis = u32::from_le_bytes([args[0], args[1], args[2], args[3]]);
            let steps = i32::from_le_bytes([args[4], args[5], args[6], args[7]]);
            axes.discrete(axis, steps);
        } else if Some(object) == pointer && opcode == pointer::WL_POINTER_AXIS_SOURCE {
            axes.source(u32::from_le_bytes([args[0], args[1], args[2], args[3]]));
        } else if Some(object) == pointer && opcode == pointer::WL_POINTER_FRAME {
            axes.frame();
        } else if Some(object) == pointer
            && (opcode == WL_POINTER_AXIS || opcode == pointer::WL_POINTER_AXIS_STOP)
        {
            // axis: time(4), axis(4: 0 vertical, 1 horizontal), value(wl_fixed).
            // axis_stop: time(4), axis(4).
            let delta = if opcode == WL_POINTER_AXIS {
                let axis = u32::from_le_bytes([args[4], args[5], args[6], args[7]]);
                let value = u32::from_le_bytes([args[8], args[9], args[10], args[11]]);
                axes.axis(axis, value)
            } else {
                axes.stop()
            };
            crate::platform::set_pointer_modifiers(wl_modifiers(mods_mask, keymap.as_ref()));
            // The wheel over the decorations Stipple draws isn't the app's.
            let win = find(&windows, pointer_focus);
            let on_decor = {
                let mut conn = shared.lock().unwrap();
                toplevel::use_decor(&mut conn, win.toplevel);
                conn.decor.pointer_on_decorations()
            };
            if let Some(delta) = delta.filter(|_| !on_decor)
                && handler(Event::Scroll { delta }, win) == ControlFlow::Exit
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
        } else if let Some(win) = windows
            .iter()
            .find(|w| w.xdg_surface == object && opcode == XDG_SURFACE_CONFIGURE)
        {
            {
                let mut conn = shared.lock().unwrap();
                let mut a = Vec::new();
                arg_u32(&mut a, first_word);
                let _ = conn.send(win.xdg_surface, XDG_SURFACE_ACK_CONFIGURE, &a);
            }
            // Apply the size from the toplevel.configure this one completes:
            // the window's, of which the content is what our title bar (if
            // any) leaves.
            let content = win.pending_size.take().map(|window| {
                let mut conn = shared.lock().unwrap();
                toplevel::use_decor(&mut conn, win.toplevel);
                conn.decor.content_size(window)
            });
            if let Some(size) = content.filter(|s| *s != win.logical.get()) {
                win.set_logical(size);
                if handler(Event::Resized(win.size.get()), win) == ControlFlow::Exit {
                    break;
                }
            }
            let state = win.pending_state.get();
            if state != win.state.get() {
                win.state.set(state);
                crate::platform::window::debug_window(format_args!(
                    "state {:#x} maximized={} minimized=false fullscreen={}",
                    win.surface, state.0, state.1
                ));
                if handler(Event::WindowChanged, win) == ControlFlow::Exit {
                    break;
                }
            }
            // Paint the frame for this configure.
            if handler(Event::RedrawRequested, win) == ControlFlow::Exit {
                break;
            }
        } else if let Some(win) = windows
            .iter()
            .find(|w| w.toplevel == object && opcode == XDG_TOPLEVEL_CONFIGURE)
        {
            // width, height, states[]: a 0 size means "client picks".
            let configure = toplevel::parse_configure(&args);
            dbg(format_args!("toplevel configure {configure:?}"));
            win.pending_state
                .set((configure.maximized, configure.fullscreen));
            let states = decor::configure_states(&args);
            let focus = {
                let mut conn = shared.lock().unwrap();
                toplevel::use_decor(&mut conn, win.toplevel);
                // Whether the window is active (`activated`) is its focus.
                let focus = conn.decor.set_states(&states);
                win.pending_size.set(Some(configure.size.unwrap_or_else(|| {
                    // The client picks: keep the content's size (the title
                    // bar may have come or gone).
                    let title = conn.decor.title_height() as u32;
                    let logical = win.logical.get();
                    PhysicalSize::new(logical.width, logical.height + title)
                })));
                focus
            };
            if let Some(focused) = focus {
                dbg(format_args!("focus {focused}"));
                // A held key stops repeating when the window loses focus.
                if !focused {
                    keys.reset();
                }
                if handler(Event::FocusChanged(focused), win) == ControlFlow::Exit {
                    break;
                }
            }
        } else if let Some(win) = windows
            .iter()
            .find(|w| w.toplevel == object && opcode == XDG_TOPLEVEL_CLOSE)
        {
            // The app may keep the window (and ask the user first).
            if handler(Event::CloseRequested, win) == ControlFlow::Exit {
                break;
            }
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
            dbg(format_args!(
                "raw key {key} pressed={pressed} sym={:?}",
                keymap.as_ref().and_then(|m| m.sym(key + 8, mods_mask))
            ));
            // Prefer the compositor's keymap (xkb keycode = evdev + 8); fall back
            // to the layout-independent evdev table when none was parsed. The
            // key goes out raw first, then through compose or as itself.
            let now = std::time::Instant::now();
            let stroke = if pressed {
                keys::Stroke::Press
            } else {
                keys::Stroke::Release
            };
            let flow = keys.key(keymap.as_ref(), mods_mask, (key, stroke), now, &mut |e| {
                dbg(format_args!("key {key} pressed={pressed} -> {e:?}"));
                handler(e, find(&windows, keyboard_focus))
            });
            if flow == ControlFlow::Exit {
                break;
            }
        } else if Some(object) == keyboard && opcode == WL_KEYBOARD_MODIFIERS {
            // serial, mods_depressed, mods_latched, mods_locked, group.
            let word =
                |i: usize| u32::from_le_bytes([args[i], args[i + 1], args[i + 2], args[i + 3]]);
            mods_mask = word(4) | word(8) | word(12);
            dbg(format_args!("modifiers {mods_mask:#x}"));
        } else if Some(object) == keyboard && opcode == WL_KEYBOARD_LEAVE {
            // Focus left the surface: keys held now are released elsewhere.
            keys.reset();
            dbg(format_args!("keyboard leave"));
        } else if Some(object) == keyboard && opcode == WL_KEYBOARD_REPEAT_INFO {
            // rate (keys per second, 0 = off), delay (ms).
            let word =
                |i: usize| i32::from_le_bytes([args[i], args[i + 1], args[i + 2], args[i + 3]]);
            keys.repeat.set_info(word(0), word(4));
            dbg(format_args!(
                "repeat info: {}/s after {} ms",
                word(0),
                word(4)
            ));
        } else if Some(object) == keyboard {
            // enter (1): serial, surface, keys. Keys go to that window; the
            // window's focus is its toplevel's `activated` state, which a seat
            // without a keyboard has too.
            if opcode == WL_KEYBOARD_ENTER
                && let Some(s) = args.get(4..8)
            {
                keyboard_focus = u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
            }
            dbg(format_args!("keyboard event {opcode}"));
        } else if object == WL_DISPLAY && opcode == WL_DISPLAY_ERROR {
            return Err(PlatformError::Os("wl_display error".into()));
        }
    }
    Ok(())
}

/// `wl_keyboard.enter` and `wl_data_device.enter`.
const WL_KEYBOARD_ENTER: u16 = 1;
const WL_DATA_DEVICE_ENTER: u16 = 1;

/// The window with this surface, or else the first.
fn find(windows: &[WaylandWindow], surface: u32) -> &WaylandWindow {
    windows
        .iter()
        .find(|w| w.surface == surface)
        .unwrap_or(&windows[0])
}

/// Take up what the handler asked of the windows: adopt the ones it opened
/// (unfocused until the compositor activates them), destroy the ones it
/// closed, and resize the ones it resized (where the compositor leaves the
/// size to the window).
fn settle<H>(
    shared: &Shared,
    reg: &Rc<RefCell<toplevel::Reg>>,
    windows: &mut Vec<WaylandWindow>,
    handler: &mut H,
) -> ControlFlow
where
    H: FnMut(Event, &dyn Window) -> ControlFlow,
{
    let (opened, closed, resized) = {
        let mut r = reg.borrow_mut();
        (
            std::mem::take(&mut r.opened),
            std::mem::take(&mut r.closed),
            std::mem::take(&mut r.resized),
        )
    };
    for win in opened {
        windows.push(win);
        let win = windows.last().expect("just pushed");
        if handler(Event::FocusChanged(false), win) == ControlFlow::Exit {
            return ControlFlow::Exit;
        }
    }
    for surface in closed {
        if let Some(pos) = windows.iter().position(|w| w.surface == surface) {
            let win = windows.remove(pos);
            toplevel::destroy(&mut shared.lock().unwrap(), win.objects());
        }
    }
    for (surface, size) in resized {
        let Some(win) = windows.iter().find(|w| w.surface == surface) else {
            continue;
        };
        if win.state.get() != (false, false) {
            continue; // maximized or fullscreen: the compositor's size holds
        }
        let logical = PhysicalSize::new(
            size.width.round().max(1.0) as u32,
            size.height.round().max(1.0) as u32,
        );
        win.set_logical(logical);
        if handler(Event::Resized(win.size.get()), win) == ControlFlow::Exit
            || handler(Event::RedrawRequested, win) == ControlFlow::Exit
        {
            return ControlFlow::Exit;
        }
    }
    ControlFlow::Wait
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
    fn icon_is_square_premultiplied_argb() {
        let mut icon = Pixmap::new(PhysicalSize::new(2, 1));
        icon.as_bytes_mut()
            .copy_from_slice(&[255, 0, 0, 255, 0, 255, 0, 128]);
        let (side, bytes) = icon_argb(&icon);
        assert_eq!(side, 2);
        // The 2x1 icon fills row 0 of the 2x2 square (premultiplied BGRA);
        // row 1 is transparent padding.
        assert_eq!(&bytes[..8], &[0, 0, 255, 255, 0, 128, 0, 128]);
        assert_eq!(&bytes[8..], &[0; 8]);
    }

    #[test]
    fn cursor_shapes_and_size_args() {
        assert_eq!(cursor_shape(CursorIcon::Text), 9);
        assert_eq!(cursor_shape(CursorIcon::NwseResize), 29);
        assert_eq!(size_args(None), vec![0; 8]);
        assert_eq!(
            size_args(Some(Size::new(320.0, 240.0))),
            [320u32.to_le_bytes(), 240u32.to_le_bytes()].concat()
        );
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
    fn keysyms_given_by_value_become_names() {
        assert_eq!(keysym_canonical_name("0xff09"), "Tab");
        assert_eq!(keysym_canonical_name("0x61"), "a");
        assert_eq!(keysym_canonical_name("0x20"), "space");
        assert_eq!(keysym_canonical_name("0xffbf"), "F2");
        assert_eq!(keysym_canonical_name("0x100263a"), "U263A");
        assert_eq!(keysym_canonical_name("Return"), "Return");
        let ev = keysym_name_to_event(
            &keysym_canonical_name("0x63"),
            true,
            Modifiers {
                ctrl: true,
                ..Default::default()
            },
        );
        assert!(ev.is_some(), "Ctrl+c from a wtype keymap");
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
        // Named Latin-1 and other legacy keysyms type their character too.
        assert_eq!(
            keysym_name_to_event("eacute", true, Modifiers::default()),
            Some(Event::Text("é".into()))
        );
        assert_eq!(
            keysym_name_to_event("Cyrillic_a", true, Modifiers::default()),
            Some(Event::Text("а".into()))
        );
        // Dead keys type nothing by themselves: compose handles them.
        assert_eq!(
            keysym_name_to_event("dead_acute", true, Modifiers::default()),
            None
        );
        // The values the compose table matches on.
        assert_eq!(keysym_value("dead_acute"), Some(0xfe51));
        assert_eq!(keysym_value("é"), Some(0xe9));
        assert_eq!(keysym_value("€"), Some(0x0100_20ac));
        assert_eq!(keysym_value("Multi_key"), Some(0xff20));
    }
}
