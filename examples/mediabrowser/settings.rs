//! What the app remembers between runs: the window's size and whether it was
//! maximized, the theme, the language, the view, the split positions and the
//! folder last opened. A `key=value` file in the app's config directory
//! (`platform::dirs()`); nothing on the web.

use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeChoice {
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    List,
    Grid,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub width: f64,
    pub height: f64,
    pub maximized: bool,
    pub theme: ThemeChoice,
    /// A tag of `i18n::LANGUAGES`, or `None` for the system's.
    pub language: Option<String>,
    pub mode: Mode,
    pub outer: f64,
    pub inner: f64,
    pub root: Option<PathBuf>,
    /// Where they're saved; `None` keeps them in memory only.
    pub file: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            width: 1040.0,
            height: 680.0,
            maximized: false,
            theme: ThemeChoice::System,
            language: None,
            mode: Mode::List,
            outer: 0.24,
            inner: 0.58,
            root: None,
            file: None,
        }
    }
}

impl Settings {
    /// The settings file in the app's config directory, if there is one.
    pub fn default_file() -> Option<PathBuf> {
        if cfg!(target_arch = "wasm32") {
            return None;
        }
        stipple::platform::dirs()
            .ok()
            .map(|d| d.config.join("mediabrowser.conf"))
    }

    /// Read `file` (defaults for what's missing or unreadable).
    pub fn load(file: Option<PathBuf>) -> Settings {
        let mut s = Settings {
            file: file.clone(),
            ..Settings::default()
        };
        let Some(text) = file.and_then(|f| std::fs::read_to_string(f).ok()) else {
            return s;
        };
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let num = |v: &str, lo: f64, hi: f64, def: f64| {
                v.parse::<f64>()
                    .ok()
                    .filter(|n| n.is_finite())
                    .map_or(def, |n| n.clamp(lo, hi))
            };
            match key.trim() {
                "width" => s.width = num(value, 480.0, 8000.0, s.width),
                "height" => s.height = num(value, 360.0, 8000.0, s.height),
                "maximized" => s.maximized = value == "true",
                "theme" => {
                    s.theme = match value {
                        "light" => ThemeChoice::Light,
                        "dark" => ThemeChoice::Dark,
                        _ => ThemeChoice::System,
                    }
                }
                "language" => s.language = (!value.is_empty()).then(|| value.to_string()),
                "mode" => {
                    s.mode = if value == "grid" {
                        Mode::Grid
                    } else {
                        Mode::List
                    }
                }
                "outer" => s.outer = num(value, 0.1, 0.6, s.outer),
                "inner" => s.inner = num(value, 0.2, 0.9, s.inner),
                "root" => s.root = (!value.is_empty()).then(|| PathBuf::from(value)),
                _ => {}
            }
        }
        s
    }

    fn text(&self) -> String {
        let theme = match self.theme {
            ThemeChoice::System => "system",
            ThemeChoice::Light => "light",
            ThemeChoice::Dark => "dark",
        };
        let mode = match self.mode {
            Mode::List => "list",
            Mode::Grid => "grid",
        };
        format!(
            "width={}\nheight={}\nmaximized={}\ntheme={theme}\nlanguage={}\nmode={mode}\nouter={:.3}\ninner={:.3}\nroot={}\n",
            self.width.round(),
            self.height.round(),
            self.maximized,
            self.language.as_deref().unwrap_or(""),
            self.outer,
            self.inner,
            self.root
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        )
    }

    /// Write them, if they have a file and changed since they were read.
    pub fn save(&self) {
        if let Some(file) = &self.file {
            let text = self.text();
            if std::fs::read_to_string(file).ok().as_deref() != Some(text.as_str()) {
                let _ = std::fs::write(file, text);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_through_their_file() {
        let dir =
            std::env::temp_dir().join(format!("mediabrowser-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("mediabrowser.conf");
        let s = Settings {
            width: 900.0,
            height: 600.0,
            maximized: true,
            theme: ThemeChoice::Dark,
            language: Some("ar".into()),
            mode: Mode::Grid,
            outer: 0.3,
            inner: 0.5,
            root: Some(dir.clone()),
            file: Some(file.clone()),
        };
        s.save();
        assert_eq!(Settings::load(Some(file)), s);
        let _ = std::fs::remove_dir_all(dir);
    }
}
