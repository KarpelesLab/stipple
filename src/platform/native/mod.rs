//! OS-drawn controls and system appearance, behind [`Theme::native`].
//!
//! On Windows, macOS and iOS the operating system's own UI framework paints a
//! control's chrome (bezel, check mark, switch, slider, text-field frame) into a
//! bitmap, which the scene then draws as an image. Stipple keeps layout, input,
//! the caret and the labels. Linux has no native framework, and Android's needs
//! the JVM, so there [`Theme::native`] stays self-drawn and only follows the
//! system's colors (on Android, its night mode and accent color).
//!
//! Bitmaps are cached by control, state and size, so a control only reaches the
//! OS when it actually changes.
//!
//! [`Theme::native`]: crate::style::Theme::native

use crate::geometry::PhysicalSize;
use crate::render::{Color, Pixmap};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

#[cfg(any(target_os = "macos", target_os = "ios"))]
#[allow(unsafe_code)]
pub(crate) mod objc;

// Kept by the backends that host OS controls (and the tests).
#[cfg_attr(
    not(any(target_os = "ios", target_os = "macos", target_os = "windows")),
    allow(dead_code)
)]
mod views;
pub use views::{LiveControl, NativeView, NativeViewEvent, live_controls, poke_control};
#[cfg(any(target_os = "ios", target_os = "macos", target_os = "windows"))]
pub(crate) use views::{ViewBackend, ViewSet};

/// The live controls of the windows that host them (see [`views`]).
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[path = "ios_views.rs"]
pub(crate) mod os_views;
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
#[path = "macos_views.rs"]
pub(crate) mod os_views;
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
#[path = "windows_views.rs"]
pub(crate) mod os_views;
#[cfg(not(any(target_os = "ios", target_os = "macos", target_os = "windows")))]
pub(crate) mod os_views {
    //! No window here hosts OS controls.
    pub(crate) fn live_controls() -> Vec<super::LiveControl> {
        Vec::new()
    }
    pub(crate) fn poke(_id: u64, _value: Option<f64>) -> bool {
        false
    }
    /// What a window here would show live: no window does, but the
    /// planning (and its tests) runs the same.
    pub(crate) fn live_kind(control: super::NativeControl) -> bool {
        !matches!(control, super::NativeControl::TextField)
    }
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
#[path = "windows.rs"]
mod os;
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
#[path = "macos.rs"]
mod os;
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[path = "ios.rs"]
mod os;
#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod os;
#[cfg(target_os = "android")]
#[path = "android.rs"]
mod os;
#[cfg(not(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "ios",
    target_os = "linux",
    target_os = "android"
)))]
#[path = "fallback.rs"]
mod os;

/// A control the OS can draw. Fractions are in thousandths, so values stay `Eq`
/// and `Hash` for the bitmap cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NativeControl {
    /// A push button; `primary` for the default (accented) one.
    Button { primary: bool },
    /// A check box.
    Checkbox { checked: bool },
    /// A radio button.
    Radio { selected: bool },
    /// An on/off switch.
    Switch { on: bool },
    /// A slider's track and thumb, `value` in thousandths.
    Slider { value: u16 },
    /// A progress bar, `value` in thousandths.
    Progress { value: u16 },
    /// A text field's frame.
    TextField,
}

/// The interaction state a control is drawn in. The app fills it in as the
/// pointer and focus move; [`Theme::native_deco`](crate::style::Theme::native_deco)
/// starts it at rest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct NativeState {
    /// The pointer is over the control.
    pub hovered: bool,
    /// The control is held down.
    pub pressed: bool,
    /// The control has keyboard focus.
    pub focused: bool,
}

/// How an element's chrome is drawn: which control, in which appearance and
/// state. Get one from [`Theme::native_deco`](crate::style::Theme::native_deco)
/// and attach it with [`Element::native`](crate::core::Element::native).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct NativeDeco {
    /// The control drawn.
    pub control: NativeControl,
    /// Drawn in the dark appearance.
    pub dark: bool,
    /// The interaction state drawn.
    pub state: NativeState,
}

/// The system's appearance settings. Anything the OS doesn't report keeps its
/// default: light mode, no accent.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct SystemLook {
    /// The system is in dark mode.
    pub dark: bool,
    /// The user's accent color, where the OS has one.
    pub accent: Option<Color>,
    /// Whether scroll bars overlay the content and show only while
    /// scrolling (`Some(true)`) or always show (`Some(false)`), where the OS
    /// has a setting for it (macOS's *Show scroll bars*).
    pub overlay_scrollers: Option<bool>,
}

/// Read the system's light/dark mode and accent color.
pub fn system_look() -> SystemLook {
    os::system_look()
}

/// The system's animation setting as a scale for durations: 0 when it asks
/// for reduced motion (or has animations off), 1 normally, and on Android
/// its animator duration scale. Read through
/// [`anim::motion_scale`](crate::anim::motion_scale), which the `App` keeps
/// up to date.
pub(crate) fn motion_scale() -> f64 {
    os::motion_scale()
}

/// Start following appearance changes where the OS doesn't announce them to the
/// window: on Linux, a thread listens for the desktop portal's `SettingChanged`
/// signal (the color scheme, the accent, and the animation setting). Elsewhere the backends hear about changes themselves, so this does
/// nothing. Safe to call more than once.
pub(crate) fn watch() {
    #[cfg(target_os = "linux")]
    os::watch();
}

/// The page's light/dark mode and accent, from `prefers-color-scheme` (see
/// `web/stipple.js`). Returns whether the look changed.
#[cfg(target_arch = "wasm32")]
pub(crate) fn set_web_look(look: SystemLook) -> bool {
    os::set_look(look)
}

/// The page's `prefers-reduced-motion` (see `web/stipple.js`). Returns whether
/// it changed.
#[cfg(target_arch = "wasm32")]
pub(crate) fn set_web_reduced_motion(reduced: bool) -> bool {
    os::set_reduced_motion(reduced)
}

static CHANGED: AtomicBool = AtomicBool::new(false);

/// Record that the system appearance changed and wake the event loop, which
/// then delivers [`Event::ThemeChanged`](crate::platform::Event::ThemeChanged).
/// Safe from any thread.
#[cfg_attr(
    not(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "ios",
        target_os = "linux",
        target_os = "android"
    )),
    allow(dead_code)
)]
pub(crate) fn note_changed() {
    CHANGED.store(true, Ordering::Release);
    crate::platform::wake::wake();
}

/// Whether the appearance changed since the last call.
pub(crate) fn take_changed() -> bool {
    CHANGED.swap(false, Ordering::AcqRel)
}

/// Forget every bitmap the OS drew, so controls are redrawn in the new look.
pub(crate) fn clear_cache() {
    if let Some(cache) = CACHE.get()
        && let Ok(mut cache) = cache.lock()
    {
        cache.clear();
    }
}

/// Whether this OS can draw `control` (Windows has no switch; iOS has no
/// checkbox or radio button). Unsupported controls stay self-drawn.
pub fn supports(control: NativeControl) -> bool {
    os::supports(control)
}

/// Whether this OS draws any controls at all.
pub fn available() -> bool {
    os::AVAILABLE
}

/// The size this OS gives `control`, in logical pixels, as `(width, height)`.
/// `None` leaves that dimension to layout.
pub fn size_hint(control: NativeControl) -> (Option<f64>, Option<f64>) {
    os::size_hint(control)
}

/// Whether the OS's own focus treatment replaces Stipple's focus ring.
pub fn draws_focus(control: NativeControl) -> bool {
    os::draws_focus(control)
}

/// Whether a label on this OS's primary button uses the on-accent color, rather
/// than the regular text color.
pub fn primary_label_is_on_accent() -> bool {
    os::PRIMARY_LABEL_ON_ACCENT
}

/// Have the OS draw `deco` at `size` physical pixels (`scale` physical pixels per
/// logical pixel). Returns straight-alpha RGBA, or `None` when it can't, in which
/// case the caller falls back to the element's self-drawn decoration.
pub(crate) fn render(deco: NativeDeco, size: PhysicalSize, scale: f64) -> Option<Arc<Pixmap>> {
    if size.width == 0 || size.height == 0 {
        return None;
    }
    // AppKit and UIKit views only work on the main thread: elsewhere (a test
    // thread rendering headless) they can stall in layout. Fall back to the
    // self-drawn look there, and don't cache that.
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    if !objc::on_main_thread() {
        return None;
    }
    // Thousandths of a pixel keep the key hashable; only exact scales repeat.
    let key = (deco, size.width, size.height, (scale * 1000.0) as u32);
    let cache = CACHE.get_or_init(Default::default);
    if let Some(hit) = cache.lock().ok()?.get(&key) {
        return hit.clone();
    }
    let image = os::render(deco, size, scale).map(Arc::new);
    let mut cache = cache.lock().ok()?;
    if cache.len() >= CACHE_LIMIT {
        cache.clear(); // sliders dragged at many values shouldn't grow forever
    }
    cache.insert(key, image.clone());
    image
}

type CacheKey = (NativeDeco, u32, u32, u32);
const CACHE_LIMIT: usize = 1024;
static CACHE: OnceLock<Mutex<HashMap<CacheKey, Option<Arc<Pixmap>>>>> = OnceLock::new();

/// Convert premultiplied RGBA to the straight alpha a [`Pixmap`] holds.
#[cfg(any(target_os = "macos", target_os = "ios"))]
fn unpremultiply(data: &mut [u8]) {
    for px in data.as_chunks_mut::<4>().0 {
        let a = px[3] as u32;
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}
