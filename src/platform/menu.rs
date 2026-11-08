//! Platform-neutral menus: keyboard shortcuts and the menu bar description a
//! backend turns into a native one (an `NSMenu` on macOS, an `HMENU` on
//! Windows).
//!
//! The app declares its menus with `stipple::core::Menu`; the `App` converts
//! them into a [`NativeMenuBar`] (labels, shortcuts and states, with a numeric
//! id per item) and hands it to
//! [`Window::set_menu_bar`](crate::platform::Window::set_menu_bar). When the
//! user picks an item, the backend reports its id as
//! [`Event::MenuCommand`](crate::platform::Event::MenuCommand).

use crate::platform::event::{KeyCode, Modifiers};

/// A keyboard shortcut (an accelerator), such as Ctrl+S on Windows and Linux
/// and ⌘S on macOS.
///
/// [`command`](Shortcut::command) is the platform's command modifier: Command
/// on macOS, Ctrl elsewhere, so one declaration fits every platform.
///
/// ```
/// use stipple::platform::{KeyCode, Modifiers, Shortcut};
/// let save = Shortcut::cmd('s');
/// let ctrl = Modifiers { ctrl: true, ..Modifiers::default() };
/// assert!(save.matches_for(KeyCode::Char('s'), ctrl, false));
/// assert_eq!(save.label_for(false), "Ctrl+S");
/// assert_eq!(save.label_for(true), "\u{2318}S");
/// let redo = Shortcut::cmd('z').shift();
/// assert_eq!(redo.label_for(false), "Ctrl+Shift+Z");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Shortcut {
    /// The key. Letters are stored lower-case and match either case.
    pub key: KeyCode,
    /// The command modifier: Command (⌘) on macOS, Ctrl elsewhere.
    pub command: bool,
    pub shift: bool,
    /// Alt (Option ⌥ on macOS).
    pub alt: bool,
    /// The Control key itself. On macOS that's ⌃, distinct from ⌘; elsewhere
    /// it's the same key as [`command`](Shortcut::command).
    pub ctrl: bool,
}

impl Shortcut {
    /// A key with no modifiers (F5, Delete…).
    pub fn key(key: KeyCode) -> Self {
        let key = match key {
            KeyCode::Char(c) => KeyCode::Char(c.to_ascii_lowercase()),
            other => other,
        };
        Self {
            key,
            command: false,
            shift: false,
            alt: false,
            ctrl: false,
        }
    }

    /// The command modifier plus a character: Ctrl+`c`, or ⌘`c` on macOS.
    pub fn cmd(c: char) -> Self {
        Self {
            command: true,
            ..Self::key(KeyCode::Char(c))
        }
    }

    /// The command modifier plus a named key.
    pub fn cmd_key(key: KeyCode) -> Self {
        Self {
            command: true,
            ..Self::key(key)
        }
    }

    /// Also hold Shift.
    pub fn shift(mut self) -> Self {
        self.shift = true;
        self
    }

    /// Also hold Alt (Option on macOS).
    pub fn alt(mut self) -> Self {
        self.alt = true;
        self
    }

    /// Also hold Control (⌃ on macOS; the same as `command` elsewhere).
    pub fn ctrl(mut self) -> Self {
        self.ctrl = true;
        self
    }

    /// Whether pressing `key` with `modifiers` triggers this shortcut, under
    /// the conventions of the platform Stipple was built for.
    pub fn matches(&self, key: KeyCode, modifiers: Modifiers) -> bool {
        self.matches_for(key, modifiers, cfg!(target_os = "macos"))
    }

    /// [`matches`](Shortcut::matches) with the platform explicit: on macOS
    /// (`mac`) `command` is the Command key (reported as `meta`) and `ctrl` the
    /// Control key; elsewhere both mean Ctrl and the logo key must be up.
    pub fn matches_for(&self, key: KeyCode, modifiers: Modifiers, mac: bool) -> bool {
        let same_key = match (self.key, key) {
            (KeyCode::Char(a), KeyCode::Char(b)) => a.eq_ignore_ascii_case(&b),
            (a, b) => a == b,
        };
        let (command, ctrl) = if mac {
            (modifiers.meta, modifiers.ctrl)
        } else {
            let ctrl = modifiers.ctrl && !modifiers.meta;
            (ctrl, ctrl)
        };
        let wants_ctrl = if mac {
            self.ctrl
        } else {
            self.command || self.ctrl
        };
        let wants_command = if mac {
            self.command
        } else {
            self.command || self.ctrl
        };
        same_key
            && command == wants_command
            && ctrl == wants_ctrl
            && modifiers.shift == self.shift
            && modifiers.alt == self.alt
    }

    /// How the shortcut reads in a menu on this platform: `Ctrl+Shift+S`, or
    /// `⇧⌘S` on macOS.
    pub fn label(&self) -> String {
        self.label_for(cfg!(target_os = "macos"))
    }

    /// [`label`](Shortcut::label) with the platform explicit.
    pub fn label_for(&self, mac: bool) -> String {
        let mut out = String::new();
        if mac {
            // Apple's order: Control, Option, Shift, Command.
            if self.ctrl {
                out.push('\u{2303}');
            }
            if self.alt {
                out.push('\u{2325}');
            }
            if self.shift {
                out.push('\u{21E7}');
            }
            if self.command {
                out.push('\u{2318}');
            }
            out.push_str(&key_name(self.key, true));
        } else {
            if self.command || self.ctrl {
                out.push_str("Ctrl+");
            }
            if self.alt {
                out.push_str("Alt+");
            }
            if self.shift {
                out.push_str("Shift+");
            }
            out.push_str(&key_name(self.key, false));
        }
        out
    }
}

/// A key's name in a shortcut label.
fn key_name(key: KeyCode, mac: bool) -> String {
    let named = |pc: &str, apple: &str| if mac { apple } else { pc }.to_string();
    match key {
        KeyCode::Char(' ') | KeyCode::Space => named("Space", "Space"),
        KeyCode::Char(c) => c.to_uppercase().collect(),
        KeyCode::Enter => named("Enter", "\u{21A9}"),
        KeyCode::Escape => named("Esc", "\u{238B}"),
        KeyCode::Backspace => named("Backspace", "\u{232B}"),
        KeyCode::Delete => named("Del", "\u{2326}"),
        KeyCode::Tab => named("Tab", "\u{21E5}"),
        KeyCode::Insert => named("Ins", "Ins"),
        KeyCode::Home => named("Home", "\u{2196}"),
        KeyCode::End => named("End", "\u{2198}"),
        KeyCode::PageUp => named("PgUp", "\u{21DE}"),
        KeyCode::PageDown => named("PgDn", "\u{21DF}"),
        KeyCode::ArrowLeft => named("Left", "\u{2190}"),
        KeyCode::ArrowRight => named("Right", "\u{2192}"),
        KeyCode::ArrowUp => named("Up", "\u{2191}"),
        KeyCode::ArrowDown => named("Down", "\u{2193}"),
        KeyCode::F1 => "F1".into(),
        KeyCode::F2 => "F2".into(),
        KeyCode::F3 => "F3".into(),
        KeyCode::F4 => "F4".into(),
        KeyCode::F5 => "F5".into(),
        KeyCode::F6 => "F6".into(),
        KeyCode::F7 => "F7".into(),
        KeyCode::F8 => "F8".into(),
        KeyCode::F9 => "F9".into(),
        KeyCode::F10 => "F10".into(),
        KeyCode::F11 => "F11".into(),
        KeyCode::F12 => "F12".into(),
        KeyCode::Alt => named("Alt", "\u{2325}"),
        KeyCode::ContextMenu => "Menu".into(),
        KeyCode::Unidentified(_) => "?".into(),
    }
}

/// Split a menu label into the text to show and its mnemonic: the character
/// after a single `&` (`"&File"` shows "File" with mnemonic `f`; `&&` is a
/// literal `&`). The mnemonic is lower-case.
///
/// ```
/// use stipple::platform::menu::mnemonic;
/// assert_eq!(mnemonic("&File"), ("File".to_string(), Some('f')));
/// assert_eq!(mnemonic("Save &As…"), ("Save As…".to_string(), Some('a')));
/// assert_eq!(mnemonic("Fish && Chips"), ("Fish & Chips".to_string(), None));
/// ```
pub fn mnemonic(label: &str) -> (String, Option<char>) {
    let (text, at) = mnemonic_at(label);
    let key = at.and_then(|i| text[i..].chars().next()?.to_lowercase().next());
    (text, key)
}

/// [`mnemonic`], with where the mnemonic sits: the byte range of its
/// character in the text to show, for underlining it.
///
/// ```
/// use stipple::platform::menu::mnemonic_at;
/// assert_eq!(mnemonic_at("Save &As…"), ("Save As…".to_string(), Some(5)));
/// assert_eq!(mnemonic_at("A && B"), ("A & B".to_string(), None));
/// ```
pub fn mnemonic_at(label: &str) -> (String, Option<usize>) {
    let mut text = String::with_capacity(label.len());
    let mut at = None;
    let mut chars = label.chars();
    while let Some(c) = chars.next() {
        if c != '&' {
            text.push(c);
            continue;
        }
        match chars.next() {
            Some('&') => text.push('&'),
            Some(next) => {
                if at.is_none() {
                    at = Some(text.len());
                }
                text.push(next);
            }
            None => {}
        }
    }
    (text, at)
}

/// The byte range `[start, end)` of the mnemonic character [`mnemonic_at`]
/// found, for [`Element::underline`](crate::core::Element::underline).
pub fn mnemonic_range(text: &str, at: Option<usize>) -> Option<(usize, usize)> {
    let i = at?;
    let c = text.get(i..)?.chars().next()?;
    Some((i, i + c.len_utf8()))
}

/// One entry of a [`NativeMenu`].
#[derive(Clone, Debug, PartialEq)]
pub enum NativeMenuItem {
    /// A command. `label` may carry a `&` mnemonic (see [`mnemonic`]).
    Action {
        /// Reported back as [`Event::MenuCommand`](crate::platform::Event::MenuCommand).
        /// Ids start at 1 and stay below `0x8000`.
        id: u32,
        label: String,
        shortcut: Option<Shortcut>,
        enabled: bool,
        /// `Some` for a checkable item, with its state.
        checked: Option<bool>,
    },
    /// A divider line.
    Separator,
    /// A nested menu.
    Submenu(NativeMenu),
}

/// A menu: a title and its items.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeMenu {
    pub label: String,
    pub items: Vec<NativeMenuItem>,
}

/// A whole menu bar, for [`Window::set_menu_bar`](crate::platform::Window::set_menu_bar).
#[derive(Clone, Debug, PartialEq)]
pub struct NativeMenuBar {
    /// The application's name, for the standard items macOS adds (About,
    /// Hide, Quit).
    pub app_name: String,
    pub menus: Vec<NativeMenu>,
}

/// What came of showing a native context menu with
/// [`Window::popup_menu`](crate::platform::Window::popup_menu).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuPopup {
    /// The platform has no native popup menus: draw one instead.
    Unsupported,
    /// The menu closed without a pick.
    Dismissed,
    /// The command with this id (as in [`NativeMenuItem::Action`]) was picked.
    Picked(u32),
}

/// The first id of a tray menu's commands (see [`NativeTray`]). Menu bar ids
/// stay below it, so one [`Event::MenuCommand`](crate::platform::Event::MenuCommand)
/// stream serves both.
pub const TRAY_ID_BASE: u32 = 0x8000;

/// A tray icon (a status item on macOS, a notification-area icon on Windows, a
/// `StatusNotifierItem` on Linux), for
/// [`Window::set_tray`](crate::platform::Window::set_tray).
#[derive(Clone, Debug, PartialEq)]
pub struct NativeTray {
    /// Shown on hover (and as the item's title where a host has no icon).
    pub tooltip: String,
    /// The icon, straight RGBA. `None` shows a plain placeholder.
    pub icon: Option<crate::render::Pixmap>,
    /// The menu shown on a right-click (on macOS, any click). Command ids
    /// start at [`TRAY_ID_BASE`]; shortcuts are ignored.
    pub menu: Vec<NativeMenuItem>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(shift: bool, ctrl: bool, alt: bool, meta: bool) -> Modifiers {
        Modifiers {
            shift,
            ctrl,
            alt,
            meta,
        }
    }

    #[test]
    fn command_is_ctrl_off_mac_and_cmd_on_mac() {
        let s = Shortcut::cmd('S');
        assert_eq!(s.key, KeyCode::Char('s'));
        let c = KeyCode::Char('s');
        assert!(s.matches_for(c, mods(false, true, false, false), false));
        assert!(!s.matches_for(c, mods(false, false, false, true), false));
        assert!(s.matches_for(c, mods(false, false, false, true), true));
        assert!(!s.matches_for(c, mods(false, true, false, false), true));
        // Extra modifiers don't match.
        assert!(!s.matches_for(c, mods(true, true, false, false), false));
        // Shift may deliver the upper-case keysym.
        let redo = Shortcut::cmd('z').shift();
        assert!(redo.matches_for(KeyCode::Char('Z'), mods(true, true, false, false), false));
        // A bare key.
        let f5 = Shortcut::key(KeyCode::F5);
        assert!(f5.matches_for(KeyCode::F5, Modifiers::default(), false));
        assert!(!f5.matches_for(KeyCode::F5, mods(false, true, false, false), false));
    }

    #[test]
    fn labels_follow_the_platform() {
        let s = Shortcut::cmd('q').alt();
        assert_eq!(s.label_for(false), "Ctrl+Alt+Q");
        assert_eq!(s.label_for(true), "\u{2325}\u{2318}Q");
        assert_eq!(Shortcut::key(KeyCode::Delete).label_for(false), "Del");
        assert_eq!(Shortcut::key(KeyCode::F2).label_for(true), "F2");
    }
}
