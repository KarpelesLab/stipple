//! Android's controls live in its Java framework, so they stay self-drawn here.
//! The look follows the configuration's night mode and, on Android 12 and
//! later, the wallpaper's accent color (see the Android backend).

use super::{NativeControl, NativeDeco, SystemLook};
use crate::geometry::PhysicalSize;
use crate::render::Pixmap;

pub const AVAILABLE: bool = false;
pub const PRIMARY_LABEL_ON_ACCENT: bool = true;

pub fn system_look() -> SystemLook {
    crate::platform::backend::android::system_look()
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
