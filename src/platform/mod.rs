//! Stipple's platform layer.
//!
//! This is the **only** crate with per-OS code. It owns native windowing,
//! input, IME, clipboard, DPI, vsync, and surface acquisition, and exposes
//! them through a small platform-neutral vocabulary: [`Event`], [`Window`],
//! [`WindowAttributes`], and a [`ControlFlow`]-driven loop. Everything above
//! this layer (`stipple::core`, widgets, …) is free of platform `cfg`s.
//!
//! Backends live under [`backend`] and are selected by `cfg`. The
//! [`backend::headless`] backend is always available and implements the full
//! contract without touching any OS API — it powers tests and the
//! golden-image conformance suite. The native **X11** backend is pure-socket
//! and safe; native backends that require OS FFI (e.g. Win32) opt into
//! `unsafe` per-module via `#[allow(unsafe_code)]`.

// Unsafe is denied crate-wide and re-allowed only on the FFI backend modules
// that genuinely need it (the X11 and headless backends stay safe).
#![deny(unsafe_code)]

/// Hand-written D-Bus / AT-SPI accessibility bridge (Linux).
#[cfg(target_os = "linux")]
pub mod a11y;
/// Platform-neutral accessibility tree handed to each backend's OS bridge.
pub mod access;
pub mod backend;
/// The system clipboard (plain text) on Windows, macOS, iOS and Android.
pub mod clipboard;
/// Dead keys and compose sequences from the libX11 Compose file (X11 and
/// Wayland, where composing is the client's job).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) mod compose;
/// A writer for the Dalvik executables the Android backend loads at run time.
#[cfg(any(target_os = "android", test))]
pub(crate) mod dex;
/// Native file dialogs (open/save/folder) backed by each OS's picker.
pub mod dialog;
mod error;
mod event;
/// The display-driven frame clock that paces animations.
pub mod frame;
/// Key and scroll translation tables shared by the native backends.
mod input;
/// Android's `InputConnection` model of a Stipple text field.
#[cfg(any(target_os = "android", test))]
pub(crate) mod input_connection;
/// Raw key events: physical keys, key text, left/right modifiers, repeat.
mod key;
/// X11 keysym names and their Unicode characters (generated table).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod keysyms;
/// Keyboard shortcuts and the native menu bar description.
pub mod menu;
/// OS-drawn controls and system appearance for the native theme.
pub mod native;
mod pointer;
/// seccomp-BPF syscall sandbox for the content process (the hardening layer of
/// the Stipple-as-compositor content path).
#[cfg(target_os = "linux")]
pub mod sandbox;
/// File-descriptor passing over a Unix socket (`SCM_RIGHTS`) — the transport for
/// DRI3/Present GPU buffers and the browser content-process IPC.
#[cfg(target_os = "linux")]
pub mod scm;
/// Secure keyboard entry (macOS) while a password field has focus.
mod secure;
/// Opening URLs and files in the default app, notifications, and the plumbing
/// behind tray icons.
pub mod shell;
/// `memfd`-backed shared-memory buffers — the CPU side of the content path (a
/// content process's pixels shared with the UI process, the dual of GPU dma-buf).
#[cfg(target_os = "linux")]
pub mod shm;
/// The text store behind the Windows Text Services Framework bridge, without
/// the COM (unit-tested everywhere).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) mod text_store;
/// A monotonic clock that also works on the web.
pub mod time;
/// Hand-written UI Automation provider (Windows accessibility bridge).
#[cfg(target_os = "windows")]
pub mod uia;
/// Waking the event loop from timers and other threads.
pub mod wake;
mod window;

pub use access::{A11yNode, A11yRole};
pub use error::PlatformError;
pub use event::{
    ButtonState, DragOffer, DropData, Event, Ime, KeyCode, Lifecycle, Modifiers, PointerButton,
    SCROLL_LINE_PX, ScrollDelta, ScrollPhase, ScrollUnit, Touch, TouchPhase, WindowId,
    file_uri_to_path, parse_uri_list, path_to_file_uri, uri_list,
};
pub use key::{
    KeyEvent, KeyLocation, ModifierSides, OptionAsMeta, PhysicalKey, consume_key, handle_back,
};
pub use menu::{
    MenuPopup, NativeMenu, NativeMenuBar, NativeMenuItem, NativeTray, Shortcut, TRAY_ID_BASE,
};
pub use pointer::{
    DoubleClick, double_click, pointer_modifiers, set_double_click, set_pointer_modifiers,
};
pub use secure::{secure_input, set_secure_input};
pub use shell::{Notification, notify, open_path, open_url};
pub use window::{
    CursorIcon, ImeState, UserAttention, Window, WindowAttributes, WindowInfo, WindowRequest,
};
#[cfg_attr(
    not(any(target_os = "windows", target_os = "macos")),
    allow(unused_imports)
)]
pub(crate) use window::{app_confirms_quit, set_app_confirms_quit};

/// What a window should do after the application handles an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlFlow {
    /// Continue running; wake again as soon as more events arrive.
    Poll,
    /// Continue running; sleep until the next event.
    Wait,
    /// Tear down the window / event loop.
    Exit,
}
