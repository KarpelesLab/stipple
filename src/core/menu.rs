//! Declarative menus: a [`MenuBar`] of [`Menu`]s of [`MenuItem`]s.
//!
//! An app declares its menu bar with [`App::menu`](crate::App::menu), as a
//! function of its state, so labels and enabled/checked states follow it;
//! context menus attach to an element with
//! [`Element::context_menu`](crate::core::Element::context_menu). Picking an
//! item runs its action against the state, the way
//! [`on_tap`](crate::core::Element::on_tap) handlers do.
//!
//! Each platform shows the menu bar its own way: a native `NSMenu` menu bar on
//! macOS, a native `HMENU` on Windows, and a bar drawn by Stipple at the top of
//! the window elsewhere. Shortcuts work everywhere. See `docs/menus.md`.
//!
//! ```
//! use stipple::core::{Menu, MenuBar, MenuItem, Shortcut};
//! struct Doc { saved: bool, wrap: bool }
//! fn menus(doc: &Doc) -> MenuBar<Doc> {
//!     MenuBar::new()
//!         .menu(
//!             Menu::new("&File")
//!                 .item(MenuItem::new("&Save", |d: &mut Doc| d.saved = true)
//!                     .shortcut(Shortcut::cmd('s'))
//!                     .enabled(!doc.saved))
//!                 .separator()
//!                 .submenu(Menu::new("&Recent").item(MenuItem::new("a.txt", |_: &mut Doc| {}))),
//!         )
//!         .menu(Menu::new("&View").item(
//!             MenuItem::new("&Wrap", |d: &mut Doc| d.wrap = !d.wrap).checked(doc.wrap),
//!         ))
//! }
//! let mut doc = Doc { saved: false, wrap: false };
//! let bar = menus(&doc);
//! assert!(bar.activate(1, &mut doc)); // ids count actions depth-first from 1
//! assert!(doc.saved);
//! ```

use std::cell::RefCell;
use std::rc::Rc;

use crate::platform::menu::{NativeMenu, NativeMenuBar, NativeMenuItem};
use crate::platform::{KeyCode, Modifiers};

pub use crate::platform::menu::{Shortcut, mnemonic};

/// A menu item's action, shared between the bar, the drawn menus and the
/// shortcut table.
pub(crate) type MenuAction<S> = Rc<RefCell<dyn FnMut(&mut S)>>;

/// One entry of a [`Menu`]: a command, a separator, or a submenu.
///
/// Labels may mark a mnemonic with `&` (`"&Open…"`): Alt plus that letter
/// picks the item from the keyboard on Windows and in the drawn menu bar.
pub struct MenuItem<S> {
    pub(crate) kind: ItemKind<S>,
}

pub(crate) enum ItemKind<S> {
    Action {
        label: String,
        action: MenuAction<S>,
        shortcut: Option<Shortcut>,
        enabled: bool,
        checked: Option<bool>,
    },
    Separator,
    Submenu(Menu<S>),
}

impl<S> MenuItem<S> {
    /// A command that runs `action` against the app state when picked.
    pub fn new(label: impl Into<String>, action: impl FnMut(&mut S) + 'static) -> Self {
        Self {
            kind: ItemKind::Action {
                label: label.into(),
                action: Rc::new(RefCell::new(action)),
                shortcut: None,
                enabled: true,
                checked: None,
            },
        }
    }

    /// A divider line between groups of items.
    pub fn separator() -> Self {
        Self {
            kind: ItemKind::Separator,
        }
    }

    /// An item that opens `menu` beside it.
    pub fn submenu(menu: Menu<S>) -> Self {
        Self {
            kind: ItemKind::Submenu(menu),
        }
    }

    /// Run this command with `shortcut` too, and show it beside the label. No
    /// effect on separators and submenus.
    pub fn shortcut(mut self, shortcut: Shortcut) -> Self {
        if let ItemKind::Action { shortcut: s, .. } = &mut self.kind {
            *s = Some(shortcut);
        }
        self
    }

    /// Grey this command out (`false`): it can't be picked and its shortcut
    /// does nothing. Commands are enabled by default.
    pub fn enabled(mut self, enabled: bool) -> Self {
        if let ItemKind::Action { enabled: e, .. } = &mut self.kind {
            *e = enabled;
        }
        self
    }

    /// Make this command checkable, showing a check mark when `checked`.
    pub fn checked(mut self, checked: bool) -> Self {
        if let ItemKind::Action { checked: c, .. } = &mut self.kind {
            *c = Some(checked);
        }
        self
    }

    /// The label as declared (with any `&`), or the submenu's title. Empty for
    /// a separator.
    pub fn label(&self) -> &str {
        match &self.kind {
            ItemKind::Action { label, .. } => label,
            ItemKind::Submenu(menu) => &menu.label,
            ItemKind::Separator => "",
        }
    }

    /// Whether this is a separator.
    pub fn is_separator(&self) -> bool {
        matches!(self.kind, ItemKind::Separator)
    }

    /// Whether this item can be picked: an enabled command, or a submenu.
    pub fn is_enabled(&self) -> bool {
        match &self.kind {
            ItemKind::Action { enabled, .. } => *enabled,
            ItemKind::Submenu(_) => true,
            ItemKind::Separator => false,
        }
    }

    /// The check state of a checkable command (`None` if it isn't one).
    pub fn is_checked(&self) -> Option<bool> {
        match &self.kind {
            ItemKind::Action { checked, .. } => *checked,
            _ => None,
        }
    }

    /// The command's shortcut, if any.
    pub fn get_shortcut(&self) -> Option<Shortcut> {
        match &self.kind {
            ItemKind::Action { shortcut, .. } => *shortcut,
            _ => None,
        }
    }

    /// The menu a submenu item opens.
    pub fn get_submenu(&self) -> Option<&Menu<S>> {
        match &self.kind {
            ItemKind::Submenu(menu) => Some(menu),
            _ => None,
        }
    }

    /// Run the command against `state` if it is enabled. Returns whether it ran.
    pub fn activate(&self, state: &mut S) -> bool {
        match &self.kind {
            ItemKind::Action {
                action,
                enabled: true,
                ..
            } => {
                (action.borrow_mut())(state);
                true
            }
            _ => false,
        }
    }

    /// The shared action of an enabled command.
    pub(crate) fn action(&self) -> Option<MenuAction<S>> {
        match &self.kind {
            ItemKind::Action {
                action,
                enabled: true,
                ..
            } => Some(action.clone()),
            _ => None,
        }
    }
}

impl<S> std::fmt::Debug for MenuItem<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.kind {
            ItemKind::Action {
                label,
                shortcut,
                enabled,
                checked,
                ..
            } => f
                .debug_struct("MenuItem")
                .field("label", label)
                .field("shortcut", shortcut)
                .field("enabled", enabled)
                .field("checked", checked)
                .finish_non_exhaustive(),
            ItemKind::Separator => f.write_str("Separator"),
            ItemKind::Submenu(menu) => f.debug_tuple("Submenu").field(menu).finish(),
        }
    }
}

/// A titled list of [`MenuItem`]s: one menu of a [`MenuBar`], a submenu, or a
/// context menu.
pub struct Menu<S> {
    pub(crate) label: String,
    pub(crate) items: Vec<MenuItem<S>>,
}

impl<S> Menu<S> {
    /// An empty menu titled `label` (which may mark a `&` mnemonic). A context
    /// menu's title isn't shown.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            items: Vec::new(),
        }
    }

    /// Append `item`.
    pub fn item(mut self, item: MenuItem<S>) -> Self {
        self.items.push(item);
        self
    }

    /// Append a separator.
    pub fn separator(self) -> Self {
        self.item(MenuItem::separator())
    }

    /// Append a submenu.
    pub fn submenu(self, menu: Menu<S>) -> Self {
        self.item(MenuItem::submenu(menu))
    }

    /// The title as declared (with any `&`).
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The items, in order.
    pub fn items(&self) -> &[MenuItem<S>] {
        &self.items
    }

    /// The item at `path`: an index into this menu, then into each submenu.
    pub fn item_at(&self, path: &[usize]) -> Option<&MenuItem<S>> {
        let (&first, rest) = path.split_first()?;
        let item = self.items.get(first)?;
        if rest.is_empty() {
            Some(item)
        } else {
            item.get_submenu()?.item_at(rest)
        }
    }

    /// The first enabled command in this menu or its submenus whose shortcut
    /// `key` with `modifiers` triggers.
    pub fn find_shortcut(&self, key: KeyCode, modifiers: Modifiers) -> Option<&MenuItem<S>> {
        self.items.iter().find_map(|item| match &item.kind {
            ItemKind::Action {
                shortcut: Some(s),
                enabled: true,
                ..
            } if s.matches(key, modifiers) => Some(item),
            ItemKind::Submenu(menu) => menu.find_shortcut(key, modifiers),
            _ => None,
        })
    }

    /// Every command of this menu and its submenus with its id, depth-first
    /// from 1: the ids [`to_native`](Menu::to_native) gives them, as a native
    /// context menu reports its pick.
    pub fn commands(&self) -> Vec<(u32, &MenuItem<S>)> {
        let mut out = Vec::new();
        self.number(&mut 1, &mut out);
        out
    }

    /// The command with id `id` (see [`commands`](Menu::commands)).
    pub fn command(&self, id: u32) -> Option<&MenuItem<S>> {
        self.commands()
            .into_iter()
            .find_map(|(i, item)| (i == id).then_some(item))
    }

    /// The platform-neutral description of this menu, for a native context
    /// menu, its commands numbered as in [`commands`](Menu::commands).
    pub fn to_native(&self) -> NativeMenu {
        self.native_from(&mut 1)
    }

    /// Visit every command depth-first, numbering them from `*next`.
    fn number<'a>(&'a self, next: &mut u32, out: &mut Vec<(u32, &'a MenuItem<S>)>) {
        for item in &self.items {
            match &item.kind {
                ItemKind::Action { .. } => {
                    out.push((*next, item));
                    *next += 1;
                }
                ItemKind::Submenu(menu) => menu.number(next, out),
                ItemKind::Separator => {}
            }
        }
    }

    fn native_from(&self, next: &mut u32) -> NativeMenu {
        let items = self
            .items
            .iter()
            .map(|item| match &item.kind {
                ItemKind::Action {
                    label,
                    shortcut,
                    enabled,
                    checked,
                    ..
                } => {
                    let id = *next;
                    *next += 1;
                    NativeMenuItem::Action {
                        id,
                        label: label.clone(),
                        shortcut: *shortcut,
                        enabled: *enabled,
                        checked: *checked,
                    }
                }
                ItemKind::Separator => NativeMenuItem::Separator,
                ItemKind::Submenu(menu) => NativeMenuItem::Submenu(menu.native_from(next)),
            })
            .collect();
        NativeMenu {
            label: self.label.clone(),
            items,
        }
    }
}

impl<S> std::fmt::Debug for Menu<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Menu")
            .field("label", &self.label)
            .field("items", &self.items)
            .finish()
    }
}

/// The application's menus, left to right. Declare it with
/// [`App::menu`](crate::App::menu).
pub struct MenuBar<S> {
    pub(crate) menus: Vec<Menu<S>>,
}

impl<S> Default for MenuBar<S> {
    fn default() -> Self {
        Self { menus: Vec::new() }
    }
}

impl<S> MenuBar<S> {
    /// An empty menu bar.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append `menu`.
    pub fn menu(mut self, menu: Menu<S>) -> Self {
        self.menus.push(menu);
        self
    }

    /// The menus, left to right.
    pub fn menus(&self) -> &[Menu<S>] {
        &self.menus
    }

    /// Every command with its id: depth-first in declaration order, from 1.
    /// These are the ids a native menu bar reports.
    pub fn commands(&self) -> Vec<(u32, &MenuItem<S>)> {
        let mut out = Vec::new();
        let mut next = 1;
        for menu in &self.menus {
            menu.number(&mut next, &mut out);
        }
        out
    }

    /// The command with id `id` (see [`commands`](MenuBar::commands)).
    pub fn command(&self, id: u32) -> Option<&MenuItem<S>> {
        self.commands()
            .into_iter()
            .find_map(|(i, item)| (i == id).then_some(item))
    }

    /// Run command `id` against `state` if it exists and is enabled. Returns
    /// whether it ran.
    pub fn activate(&self, id: u32, state: &mut S) -> bool {
        self.command(id).is_some_and(|item| item.activate(state))
    }

    /// The first enabled command whose shortcut `key` with `modifiers`
    /// triggers.
    pub fn find_shortcut(&self, key: KeyCode, modifiers: Modifiers) -> Option<&MenuItem<S>> {
        self.menus
            .iter()
            .find_map(|m| m.find_shortcut(key, modifiers))
    }

    /// The platform-neutral description handed to a native menu bar, with
    /// `app_name` for the items macOS adds.
    pub fn to_native(&self, app_name: &str) -> NativeMenuBar {
        let mut next = 1;
        NativeMenuBar {
            app_name: app_name.to_string(),
            menus: self
                .menus
                .iter()
                .map(|m| m.native_from(&mut next))
                .collect(),
        }
    }
}

impl<S> std::fmt::Debug for MenuBar<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MenuBar")
            .field("menus", &self.menus)
            .finish()
    }
}

/// The app's tray icon: a status item in macOS's menu bar, a notification-area
/// icon on Windows, a `StatusNotifierItem` on Linux. Declare it with
/// [`App::tray`](crate::App::tray).
///
/// ```
/// use stipple::core::{Menu, MenuItem, Tray};
/// struct State { muted: bool, shown: bool }
/// fn tray(s: &State) -> Tray<State> {
///     Tray::new("My App")
///         .menu(
///             Menu::new("")
///                 .item(MenuItem::new("&Mute", |s: &mut State| s.muted = !s.muted).checked(s.muted))
///                 .separator()
///                 .item(MenuItem::new("&Quit", |_: &mut State| {})),
///         )
///         .on_activate(|s: &mut State| s.shown = true)
/// }
/// let mut s = State { muted: false, shown: false };
/// let t = tray(&s);
/// assert!(t.activate_command(stipple::platform::TRAY_ID_BASE, &mut s));
/// assert!(s.muted);
/// ```
pub struct Tray<S> {
    tooltip: String,
    icon: Option<crate::render::Pixmap>,
    menu: Option<Menu<S>>,
    on_activate: Option<MenuAction<S>>,
}

impl<S> Tray<S> {
    /// A tray icon with `tooltip`, shown on hover (and, on macOS, as the
    /// item's title when it has no icon).
    pub fn new(tooltip: impl Into<String>) -> Self {
        Self {
            tooltip: tooltip.into(),
            icon: None,
            menu: None,
            on_activate: None,
        }
    }

    /// The icon (straight RGBA, ideally square; it's scaled down to fit).
    pub fn icon(mut self, icon: crate::render::Pixmap) -> Self {
        self.icon = Some(icon);
        self
    }

    /// The menu a right-click opens (on macOS, any click). Its title isn't
    /// shown, and shortcuts don't apply.
    pub fn menu(mut self, menu: Menu<S>) -> Self {
        self.menu = Some(menu);
        self
    }

    /// Run `action` when the icon is clicked (a left click on Windows, the
    /// host's "activate" on Linux; on macOS only when there's no menu).
    pub fn on_activate(mut self, action: impl FnMut(&mut S) + 'static) -> Self {
        self.on_activate = Some(Rc::new(RefCell::new(action)));
        self
    }

    /// The description handed to the platform: command ids start at
    /// [`TRAY_ID_BASE`](crate::platform::TRAY_ID_BASE), shortcuts dropped.
    pub fn to_native(&self) -> crate::platform::NativeTray {
        fn strip(items: &mut [NativeMenuItem]) {
            for item in items {
                match item {
                    NativeMenuItem::Action { shortcut, .. } => *shortcut = None,
                    NativeMenuItem::Submenu(m) => strip(&mut m.items),
                    NativeMenuItem::Separator => {}
                }
            }
        }
        let mut items = match &self.menu {
            Some(menu) => {
                let mut next = crate::platform::TRAY_ID_BASE;
                menu.native_from(&mut next).items
            }
            None => Vec::new(),
        };
        strip(&mut items);
        crate::platform::NativeTray {
            tooltip: self.tooltip.clone(),
            icon: self.icon.clone(),
            menu: items,
        }
    }

    /// Run the menu command with native id `id` (from
    /// [`TRAY_ID_BASE`](crate::platform::TRAY_ID_BASE)) if it exists and is
    /// enabled. Returns whether it ran.
    pub fn activate_command(&self, id: u32, state: &mut S) -> bool {
        let Some(menu) = &self.menu else {
            return false;
        };
        let mut out = Vec::new();
        let mut next = crate::platform::TRAY_ID_BASE;
        menu.number(&mut next, &mut out);
        out.into_iter()
            .find(|(i, _)| *i == id)
            .is_some_and(|(_, item)| item.activate(state))
    }

    /// Run the click action, if there is one. Returns whether it ran.
    pub fn activate(&self, state: &mut S) -> bool {
        match &self.on_activate {
            Some(action) => {
                (action.borrow_mut())(state);
                true
            }
            None => false,
        }
    }
}

impl<S> std::fmt::Debug for Tray<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tray")
            .field("tooltip", &self.tooltip)
            .field("icon", &self.icon.as_ref().map(|i| i.size()))
            .field("menu", &self.menu)
            .field("on_activate", &self.on_activate.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_numbers_from_the_tray_base_without_shortcuts() {
        let base = crate::platform::TRAY_ID_BASE;
        let tray = Tray::new("T")
            .menu(
                Menu::new("")
                    .item(
                        MenuItem::new("a", |s: &mut Vec<&str>| s.push("a"))
                            .shortcut(Shortcut::cmd('a')),
                    )
                    .submenu(
                        Menu::new("More").item(MenuItem::new("b", |s: &mut Vec<&str>| s.push("b"))),
                    ),
            )
            .on_activate(|s: &mut Vec<&str>| s.push("click"));
        let native = tray.to_native();
        assert_eq!(native.tooltip, "T");
        match &native.menu[0] {
            NativeMenuItem::Action { id, shortcut, .. } => {
                assert_eq!((*id, *shortcut), (base, None))
            }
            other => panic!("{other:?}"),
        }
        match &native.menu[1] {
            NativeMenuItem::Submenu(m) => match &m.items[0] {
                NativeMenuItem::Action { id, .. } => assert_eq!(*id, base + 1),
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
        let mut log = Vec::new();
        assert!(tray.activate_command(base + 1, &mut log));
        assert!(!tray.activate_command(1, &mut log));
        assert!(tray.activate(&mut log));
        assert_eq!(log, vec!["b", "click"]);
    }

    fn bar() -> MenuBar<Vec<&'static str>> {
        MenuBar::new()
            .menu(
                Menu::new("&File")
                    .item(
                        MenuItem::new("&New", |s: &mut Vec<_>| s.push("new"))
                            .shortcut(Shortcut::cmd('n')),
                    )
                    .separator()
                    .submenu(
                        Menu::new("&Recent")
                            .item(MenuItem::new("one", |s: &mut Vec<_>| s.push("one")))
                            .item(
                                MenuItem::new("two", |s: &mut Vec<_>| s.push("two")).enabled(false),
                            ),
                    )
                    .item(MenuItem::new("&Quit", |s: &mut Vec<_>| s.push("quit"))),
            )
            .menu(
                Menu::new("&View").item(
                    MenuItem::new("&Wrap", |s: &mut Vec<_>| s.push("wrap"))
                        .checked(true)
                        .shortcut(Shortcut::key(KeyCode::F5)),
                ),
            )
    }

    #[test]
    fn ids_number_commands_depth_first() {
        let b = bar();
        let labels: Vec<(u32, &str)> = b
            .commands()
            .iter()
            .map(|(i, it)| (*i, it.label()))
            .collect();
        assert_eq!(
            labels,
            vec![
                (1, "&New"),
                (2, "one"),
                (3, "two"),
                (4, "&Quit"),
                (5, "&Wrap")
            ]
        );
        let native = b.to_native("Demo");
        assert_eq!(native.menus.len(), 2);
        match &native.menus[0].items[2] {
            NativeMenuItem::Submenu(m) => match &m.items[1] {
                NativeMenuItem::Action { id, enabled, .. } => {
                    assert_eq!((*id, *enabled), (3, false))
                }
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
        match &native.menus[1].items[0] {
            NativeMenuItem::Action { id, checked, .. } => {
                assert_eq!((*id, *checked), (5, Some(true)))
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_menu_numbers_its_own_commands() {
        let b = bar();
        let file = &b.menus()[0];
        let native = file.to_native();
        assert_eq!(native.label, "&File");
        match &native.items[2] {
            NativeMenuItem::Submenu(m) => match &m.items[0] {
                NativeMenuItem::Action { id, .. } => assert_eq!(*id, 2),
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
        assert_eq!(file.command(2).map(|i| i.label()), Some("one"));
        assert_eq!(file.command(4).map(|i| i.label()), Some("&Quit"));
        assert!(file.command(5).is_none());
    }

    #[test]
    fn activation_skips_disabled_commands() {
        let b = bar();
        let mut log = Vec::new();
        assert!(b.activate(2, &mut log));
        assert!(!b.activate(3, &mut log), "disabled");
        assert!(!b.activate(99, &mut log));
        assert_eq!(log, vec!["one"]);
        assert_eq!(b.menus()[0].item_at(&[2, 0]).unwrap().label(), "one");
    }

    #[test]
    fn shortcuts_find_enabled_commands() {
        let b = bar();
        let mut log = Vec::new();
        let ctrl = Modifiers {
            ctrl: !cfg!(target_os = "macos"),
            meta: cfg!(target_os = "macos"),
            ..Modifiers::default()
        };
        b.find_shortcut(KeyCode::Char('n'), ctrl)
            .unwrap()
            .activate(&mut log);
        b.find_shortcut(KeyCode::F5, Modifiers::default())
            .unwrap()
            .activate(&mut log);
        assert!(
            b.find_shortcut(KeyCode::Char('n'), Modifiers::default())
                .is_none()
        );
        assert_eq!(log, vec!["new", "wrap"]);
    }
}
