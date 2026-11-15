//! Stipple's platform layer.
//!
//! This is the **only** crate with per-OS code. It owns native windowing,
//! input, IME, clipboard, DPI, vsync, and surface acquisition, and exposes
//! them through a small platform-neutral vocabulary: [`Event`],
//! [`WindowAttributes`], [`WindowRequest`], [`WindowInfo`], and services such as
//! [`clipboard`], [`dialog`] and [`notify`] (gamepads are
//! [`crate::gamepad`]). Everything above
//! this layer (`stipple::core`, widgets, …) is free of platform `cfg`s.
//!
//! Backends live under [`backend`] and are selected by `cfg`. A headless
//! backend is always available and implements the full contract without
//! touching any OS API — it powers tests and the golden-image conformance
//! suite. The native **X11** backend is pure-socket
//! and safe; native backends that require OS FFI (e.g. Win32) opt into
//! `unsafe` per-module via `#[allow(unsafe_code)]`.

// Unsafe is denied crate-wide and re-allowed only on the FFI backend modules
// that genuinely need it (the X11 and headless backends stay safe).
#![deny(unsafe_code)]

/// Hand-written D-Bus / AT-SPI accessibility bridge (Linux).
// Public only for the repository's `a11ydemo` and `filedialog` harness
// examples, which drive the D-Bus client and the mock file portal directly.
#[cfg(target_os = "linux")]
#[doc(hidden)]
pub mod a11y;
/// Platform-neutral accessibility tree handed to each backend's OS bridge.
pub(crate) mod access;
/// Files the app was packaged with: the APK's assets, the bundle's
/// resources, a directory next to the executable, the web server's.
mod assets;
/// The AT-SPI bridge X11 and Wayland windows serve their trees through.
#[cfg(target_os = "linux")]
pub(crate) mod atspi;
/// Predictive back: whether the app would go back, and the gesture's progress.
pub(crate) mod back;
pub mod backend;
/// The app's directories (data, config, cache, temp) and bundle information
/// (identifier, name, version, build number).
mod bundle;
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
/// Gamepads: connection, buttons and axes on a standard layout, rumble.
pub(crate) mod gamepad;
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
pub(crate) mod menu;
/// OS-drawn controls and system appearance for the native theme.
pub mod native;
mod pointer;
/// seccomp-BPF syscall sandbox for the content process (the hardening layer of
/// the Stipple-as-compositor content path).
// Public only for the repository's `contentproc` harness example.
#[cfg(target_os = "linux")]
#[doc(hidden)]
pub mod sandbox;
/// The app's state, kept across process death (Android's saved instance state).
mod saved_state;
/// File-descriptor passing over a Unix socket (`SCM_RIGHTS`) — the transport for
/// DRI3/Present GPU buffers and the browser content-process IPC.
// Public only for the repository's `contentproc` harness example.
#[cfg(target_os = "linux")]
#[doc(hidden)]
pub mod scm;
/// Secure keyboard entry (macOS) while a password field has focus.
mod secure;
/// Opening URLs and files in the default app, notifications, and the plumbing
/// behind tray icons.
pub(crate) mod shell;
/// `memfd`-backed shared-memory buffers — the CPU side of the content path (a
/// content process's pixels shared with the UI process, the dual of GPU dma-buf).
// Public only for the repository's `contentproc` harness example.
#[cfg(target_os = "linux")]
#[doc(hidden)]
pub mod shm;
/// The text store behind the Windows Text Services Framework bridge, without
/// the COM (unit-tested everywhere).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) mod text_store;
/// A monotonic clock that also works on the web.
pub mod time;
/// Hand-written UI Automation provider (Windows accessibility bridge).
// Public only for the repository's `uiademo` harness example, which runs its
// self-test.
#[cfg(target_os = "windows")]
#[doc(hidden)]
pub mod uia;
/// Waking the event loop from timers and other threads.
pub(crate) mod wake;
mod window;

pub(crate) use access::{A11yNode, A11yRole};
pub use assets::{AssetError, AssetReader, Assets, assets, set_assets_dir};
pub use back::{BackEdge, BackGesture, back_enabled, set_back_enabled};
pub use bundle::{AppDirs, AppInfo, app_info, dirs, set_app_info};
pub use error::PlatformError;
pub use event::{
    ButtonState, DragOffer, DropData, Event, Ime, KeyCode, Lifecycle, Modifiers, PointerButton,
    SCROLL_LINE_PX, ScrollDelta, ScrollPhase, ScrollUnit, Touch, TouchPhase, WindowId,
    file_uri_to_path, parse_uri_list, path_to_file_uri, uri_list,
};
pub use key::{
    KeyEvent, KeyLocation, ModifierSides, OptionAsMeta, PhysicalKey, consume_key, handle_back,
};
pub(crate) use menu::{MenuPopup, Shortcut};
pub(crate) use menu::{NativeMenuBar, NativeTray, TRAY_ID_BASE};
pub use menu::{mnemonic, mnemonic_at, mnemonic_range};
pub use pointer::{
    DoubleClick, double_click, pointer_modifiers, set_double_click, set_pointer_modifiers,
};
pub use saved_state::{save_state, take_restored_state};
pub use secure::{secure_input, set_secure_input};
pub use shell::{Notification, notify, open_path, open_url, tray_shown};
pub(crate) use window::Window;
pub use window::{
    CursorIcon, ImeState, UserAttention, WindowAttributes, WindowInfo, WindowRequest,
};
#[cfg_attr(
    not(any(target_os = "windows", target_os = "macos")),
    allow(unused_imports)
)]
pub(crate) use window::{app_confirms_quit, set_app_confirms_quit};

/// What a window should do after the application handles an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ControlFlow {
    /// Continue running; wake again as soon as more events arrive.
    // Part of the backend contract; nothing asks for it today.
    #[allow(dead_code)]
    Poll,
    /// Continue running; sleep until the next event.
    Wait,
    /// Tear down the window / event loop.
    Exit,
}
