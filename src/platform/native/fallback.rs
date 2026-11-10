//! Platforms whose native controls aren't drawn yet: the web.
//!
//! The web's look is the page's: `web/stipple.js` reads
//! `prefers-color-scheme` (and the CSS `AccentColor`, where the browser
//! exposes it) and hands it over through `stipple_color_scheme`, which lands
//! in [`set_look`]. Its `prefers-reduced-motion` arrives the same way, through
//! `stipple_reduced_motion` and [`set_reduced_motion`].

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use super::{NativeControl, NativeDeco, SystemLook};
use crate::geometry::PhysicalSize;
use crate::render::{Color, Pixmap};

pub const AVAILABLE: bool = false;
pub const PRIMARY_LABEL_ON_ACCENT: bool = true;

static DARK: AtomicBool = AtomicBool::new(false);
/// The accent as `0x01RRGGBB`, or 0 for none.
static ACCENT: AtomicU32 = AtomicU32::new(0);
static REDUCED_MOTION: AtomicBool = AtomicBool::new(false);

pub fn motion_scale() -> f64 {
    if REDUCED_MOTION.load(Ordering::Relaxed) {
        0.0
    } else {
        1.0
    }
}

/// Record the page's `prefers-reduced-motion`. Returns whether it changed.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn set_reduced_motion(reduced: bool) -> bool {
    REDUCED_MOTION.swap(reduced, Ordering::Relaxed) != reduced
}

pub fn system_look() -> SystemLook {
    let accent = ACCENT.load(Ordering::Relaxed);
    SystemLook {
        dark: DARK.load(Ordering::Relaxed),
        accent: (accent != 0)
            .then(|| Color::rgb((accent >> 16) as u8, (accent >> 8) as u8, accent as u8)),
    }
}

/// Record the page's look. Returns whether it changed.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn set_look(look: SystemLook) -> bool {
    let accent = look.accent.map_or(0, |c| {
        0x0100_0000 | (u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b)
    });
    let old_dark = DARK.swap(look.dark, Ordering::Relaxed);
    let old_accent = ACCENT.swap(accent, Ordering::Relaxed);
    old_dark != look.dark || old_accent != accent
}
pub fn supports(_control: NativeControl) -> bool {
    false
}
pub fn size_hint(_control: NativeControl) -> (Option<f64>, Option<f64>) {
    (None, None)
}
pub fn draws_focus(_control: NativeControl) -> bool {
    false
}
pub fn render(_deco: NativeDeco, _size: PhysicalSize, _scale: f64) -> Option<Pixmap> {
    None
}
