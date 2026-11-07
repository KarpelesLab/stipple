//! Platforms whose native controls aren't drawn yet: Android and the web.

use super::{NativeControl, NativeDeco, SystemLook};
use crate::geometry::PhysicalSize;
use crate::render::Pixmap;

pub const AVAILABLE: bool = false;
pub const PRIMARY_LABEL_ON_ACCENT: bool = true;

pub fn system_look() -> SystemLook {
    SystemLook::default()
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
