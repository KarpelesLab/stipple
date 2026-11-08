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
/// The system clipboard (plain text) on Windows, macOS and iOS.
pub mod clipboard;
/// Dead keys and compose sequences from the libX11 Compose file (X11 and
/// Wayland, where composing is the client's job).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) mod compose;
/// Native file dialogs (open/save/folder) backed by each OS's picker.
pub mod dialog;
mod error;
mod event;
/// The display-driven frame clock that paces animations.
pub mod frame;
/// Key and scroll translation tables shared by the native backends.
mod input;
/// X11 keysym names and their Unicode characters (generated table).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod keysyms;
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
/// `memfd`-backed shared-memory buffers — the CPU side of the content path (a
/// content process's pixels shared with the UI process, the dual of GPU dma-buf).
#[cfg(target_os = "linux")]
pub mod shm;
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
    ButtonState, Event, Ime, KeyCode, Modifiers, PointerButton, ScrollDelta, WindowId,
};
pub use pointer::{pointer_modifiers, set_pointer_modifiers};
pub use window::{CursorIcon, ImeState, Window, WindowAttributes, WindowRequest};

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
