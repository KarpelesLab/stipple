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
pub enum NativeControl {
    Button { primary: bool },
    Checkbox { checked: bool },
    Radio { selected: bool },
    Switch { on: bool },
    Slider { value: u16 },
    Progress { value: u16 },
    TextField,
}

/// The interaction state a control is drawn in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct NativeState {
    pub hovered: bool,
    pub pressed: bool,
    pub focused: bool,
}

/// How an element's chrome is drawn: which control, in which appearance and state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeDeco {
    pub control: NativeControl,
    pub dark: bool,
    pub state: NativeState,
}

/// The system's appearance settings. Anything the OS doesn't report keeps its
/// default: light mode, no accent.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SystemLook {
    pub dark: bool,
    pub accent: Option<Color>,
}

/// Read the system's light/dark mode and accent color.
pub fn system_look() -> SystemLook {
    os::system_look()
}

/// Start following appearance changes where the OS doesn't announce them to the
/// window: on Linux, a thread listens for the desktop portal's `SettingChanged`
/// signal. Elsewhere the backends hear about changes themselves, so this does
/// nothing. Safe to call more than once.
pub fn watch() {
    #[cfg(target_os = "linux")]
    os::watch();
}

/// The page's light/dark mode and accent, from `prefers-color-scheme` (see
/// `web/stipple.js`). Returns whether the look changed.
#[cfg(target_arch = "wasm32")]
pub(crate) fn set_web_look(look: SystemLook) -> bool {
    os::set_look(look)
}

static CHANGED: AtomicBool = AtomicBool::new(false);

/// Record that the system appearance changed and wake the event loop, which
/// then delivers [`Event::ThemeChanged`](crate::platform::Event::ThemeChanged).
/// Safe from any thread.
#[cfg_attr(
    not(any(
        target_os = "windows",
        target_os = "macos",
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
pub fn clear_cache() {
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
pub fn render(deco: NativeDeco, size: PhysicalSize, scale: f64) -> Option<Arc<Pixmap>> {
    if size.width == 0 || size.height == 0 {
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
