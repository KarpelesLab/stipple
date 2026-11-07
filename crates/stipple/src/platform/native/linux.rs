//! Linux has no native UI framework, so controls stay self-drawn. Appearance comes
//! from the XDG desktop portal's settings, which GNOME, KDE and others implement.

use super::{NativeControl, NativeDeco, SystemLook};
use crate::geometry::PhysicalSize;
use crate::platform::a11y::DBus;
use crate::render::{Color, Pixmap};

pub const AVAILABLE: bool = false;
pub const PRIMARY_LABEL_ON_ACCENT: bool = true;

pub fn system_look() -> SystemLook {
    let mut look = SystemLook::default();
    // GTK's override wins, and also covers desktops without a portal.
    if std::env::var("GTK_THEME").is_ok_and(|t| t.ends_with(":dark")) {
        look.dark = true;
    }
    let Ok(mut bus) = DBus::connect_session() else {
        return look;
    };
    // color-scheme: 0 = no preference, 1 = prefer dark, 2 = prefer light.
    if let Some(scheme) = bus.portal_setting_u32("org.freedesktop.appearance", "color-scheme") {
        look.dark = scheme == 1;
    }
    if let Some([r, g, b]) = bus.portal_setting_rgb("org.freedesktop.appearance", "accent-color")
        && [r, g, b].iter().all(|c| (0.0..=1.0).contains(c))
    {
        let c = |v: f64| (v * 255.0).round() as u8;
        look.accent = Some(Color::rgb(c(r), c(g), c(b)));
    }
    look
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
