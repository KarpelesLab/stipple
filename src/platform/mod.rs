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
/// JNI access for Android APIs Stipple doesn't wrap: the `JavaVM`, the
/// activity, the application context, activities started for a result, the
/// main thread.
#[cfg(target_os = "android")]
pub mod android;
/// Files the app was packaged with: the APK's assets, the bundle's
/// resources, a directory next to the executable, the web server's.
mod assets;
/// The AT-SPI bridge X11 and Wayland windows serve their trees through.
#[cfg(target_os = "linux")]
pub(crate) mod atspi;
/// Keeping the screen awake on desktops.
pub(crate) mod awake;
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
/// A writer for the Dalvik executables the Android backend loads at run time
/// (and the one an APK carries, from [`android_receiver_dex`] on any host).
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
pub(crate) mod dex;
/// GPU frames handed to the compositor as `dma-buf`s (Wayland).
#[cfg(target_os = "linux")]
#[cfg_attr(not(any(feature = "gl", feature = "vk")), allow(dead_code))]
pub(crate) mod dmabuf;
/// The edit menu over a text field's selection on a touch screen.
mod edit_menu;
/// App extensions (`NSExtensionMain`) and app groups' shared containers.
mod extension;

/// A `classes.dex` for an Android APK holding the `org.stipple.android.Alarm`
/// broadcast receiver, which shows the app's
/// [scheduled notifications](schedule_notification) when its process isn't
/// running, and the `org.stipple.android.Tapped` activity, which reports a
/// tap on one while the app's activity runs: both load the native library
/// `lib` (the `android.app.lib_name` of the app's `NativeActivity`,
/// `stipple_android` for `libstipple_android.so`). Write it from a build
/// script (or a host tool) into the APK as `classes.dex`, set
/// `android:hasCode="true"`, and declare the receiver and the activity in the
/// manifest; see `docs/platforms/android.md`.
// Hidden: an Android packaging step, not an app API.
#[doc(hidden)]
pub fn android_receiver_dex(lib: &str) -> Vec<u8> {
    dex::receiver_dex(lib)
}
/// Native file dialogs (open/save/folder) backed by each OS's picker.
pub mod dialog;
mod error;
mod event;
// Images in another API's memory (dma-buf, IOSurface, DXGI shared handles,
// AHardwareBuffer), drawn without a copy; public as `render::external`.
pub(crate) mod external;
/// The display-driven frame clock that paces animations.
pub mod frame;
/// Gamepads: connection, buttons and axes on a standard layout, rumble.
pub(crate) mod gamepad;
/// URLs that open the app (custom schemes, universal links, App Links), for
/// `App::on_open_url`.
pub(crate) mod incoming_url;
/// Key and scroll translation tables shared by the native backends.
mod input;
/// Android's `InputConnection` model of a Stipple text field.
#[cfg(any(target_os = "android", test))]
pub(crate) mod input_connection;
/// The on-screen keyboard's hints for a text field, and their platform tables.
pub(crate) mod input_hints;
/// Raw key events: physical keys, key text, left/right modifiers, repeat.
mod key;
/// X11 keysym names and their Unicode characters (generated table).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod keysyms;
/// The user's preferred languages and formatting settings.
pub(crate) mod locale;
/// Keyboard shortcuts and the native menu bar description.
pub(crate) mod menu;
/// OS-drawn controls and system appearance for the native theme.
pub mod native;
/// Runtime permissions: the camera, the microphone, notifications, photos,
/// the location.
pub mod permission;
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
/// Orientations: which way the screen is turned, and which ways it may turn.
mod screen;
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
/// In-app purchases: StoreKit, Google Play Billing, a mock.
pub(crate) mod store;
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
/// The UI Automation bridge's COM-free logic (unit-tested everywhere).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) mod uia_logic;
/// Waking the event loop from timers and other threads.
pub(crate) mod wake;
mod window;

pub(crate) use access::{A11yNode, A11yRole};
pub use assets::{AssetError, AssetReader, Assets, assets, set_assets_dir};
pub use back::{BackEdge, BackGesture, back_enabled, set_back_enabled};
pub use bundle::{AppDirs, AppInfo, app_info, dirs, set_app_info};
pub use edit_menu::{EditAction, EditMenu};
pub use error::PlatformError;
pub use event::{
    ButtonState, DragOffer, DropData, Event, Ime, KeyCode, Lifecycle, Modifiers, PointerButton,
    SCROLL_LINE_PX, ScrollDelta, ScrollPhase, ScrollUnit, Touch, TouchPhase, WindowId,
    file_uri_to_path, parse_uri_list, path_to_file_uri, uri_list,
};
pub use extension::{extension_main, shared_dir};
pub(crate) use input_hints::InputHints;
pub use input_hints::{Autocapitalize, Autofill, EnterAction, InputKind};
pub use key::{
    KeyEvent, KeyLocation, ModifierSides, OptionAsMeta, PhysicalKey, consume_key, handle_back,
};
pub(crate) use locale::utc_offset;
pub use locale::{locale, preferred_languages, set_preferred_languages};
pub(crate) use menu::{MenuPopup, Shortcut};
pub(crate) use menu::{NativeMenuBar, NativeTray, TRAY_ID_BASE};
pub use menu::{mnemonic, mnemonic_at, mnemonic_range};
pub use pointer::{
    DoubleClick, double_click, pointer_modifiers, set_double_click, set_pointer_modifiers,
};
pub use saved_state::{save_state, take_restored_state};
pub use screen::{Orientation, Orientations};
pub use secure::{secure_input, set_secure_input};
pub use shell::{
    Haptic, Notification, NotificationTime, ScheduledNotification, Share, ShareFile, ShareOutcome,
    cancel_all_notifications, cancel_notification, haptic, notify, open_path, open_url,
    pending_notifications, schedule_notification, share, tray_shown,
};
pub use store::{
    MockOutcome, MockStore, Product, ProductId, ProductKind, PurchaseResult, Store, StoreError,
    StoreKind, Transaction, TransactionState, store,
};
#[cfg_attr(not(feature = "gpu"), allow(unused_imports))]
pub(crate) use window::GpuLayer;
pub use window::{
    CursorIcon, ImeState, UserAttention, WindowAttributes, WindowInfo, WindowRequest,
};
pub(crate) use window::{DragLift, Window};
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
