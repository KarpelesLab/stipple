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
/// The desktop's animation setting, as a duration scale: GNOME's
/// `enable-animations` (0 when off) or KDE's `AnimationDurationFactor`, both
/// read through the portal; without a portal, GTK's own `settings.ini`.
pub fn motion_scale() -> f64 {
    if let Ok(mut bus) = DBus::connect_session() {
        if let Some(on) =
            bus.portal_setting_bool("org.gnome.desktop.interface", "enable-animations")
        {
            return if on { 1.0 } else { 0.0 };
        }
        if let Some(factor) =
            bus.portal_setting_f64("org.kde.kdeglobals.KDE", "AnimationDurationFactor")
        {
            return factor;
        }
    }
    gtk_settings_animations().map_or(1.0, |on| if on { 1.0 } else { 0.0 })
}

/// `gtk-enable-animations` from the user's GTK 4 or GTK 3 `settings.ini`.
fn gtk_settings_animations() -> Option<bool> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".config")))?;
    ["gtk-4.0", "gtk-3.0"].iter().find_map(|dir| {
        let text = std::fs::read_to_string(config.join(dir).join("settings.ini")).ok()?;
        parse_gtk_enable_animations(&text)
    })
}

/// The `gtk-enable-animations` key of a GTK `settings.ini`.
fn parse_gtk_enable_animations(text: &str) -> Option<bool> {
    text.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key.trim() == "gtk-enable-animations").then_some(())?;
        match value.trim().to_ascii_lowercase().as_str() {
            "0" | "false" | "no" => Some(false),
            "1" | "true" | "yes" => Some(true),
            _ => None,
        }
    })
}

/// Listen for the portal's `SettingChanged` on a thread of its own, with its own
/// bus connection, and flag appearance changes for the event loop.
pub fn watch() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let spawned = std::thread::Builder::new()
            .name("stipple-appearance".into())
            .spawn(|| {
                let Ok(mut bus) = DBus::connect_session() else {
                    return;
                };
                let _ = bus.watch_portal_settings(|namespace, key| {
                    let look = namespace == "org.freedesktop.appearance"
                        && matches!(key, "color-scheme" | "accent-color");
                    let motion = (namespace, key)
                        == ("org.gnome.desktop.interface", "enable-animations")
                        || (namespace, key)
                            == ("org.kde.kdeglobals.KDE", "AnimationDurationFactor");
                    if look || motion {
                        super::note_changed();
                    }
                });
            });
        drop(spawned);
    });
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

#[cfg(test)]
mod tests {
    use super::parse_gtk_enable_animations as parse;

    #[test]
    fn reads_gtk_enable_animations() {
        assert_eq!(
            parse("[Settings]\ngtk-enable-animations = 0\n"),
            Some(false)
        );
        assert_eq!(parse("[Settings]\ngtk-enable-animations=true"), Some(true));
        assert_eq!(parse("[Settings]\ngtk-theme-name=Adwaita"), None);
    }
}
