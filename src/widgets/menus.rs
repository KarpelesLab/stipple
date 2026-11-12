//! The menus Stipple draws itself: the in-window menu bar (where the platform
//! has no native one), and the dropdowns, submenus and context menus it opens,
//! as overlays.
//!
//! The open/highlight state lives in a [`MenuUi`] the app keeps per window,
//! shared with the tap handlers built here, so picking an item can both close
//! the menu and run the item's action against the app state. The app routes
//! keys to [`MenuUi::handle_key`] while a menu is open, and pointer moves to
//! [`MenuUi::hover`].

use std::cell::RefCell;
use std::rc::Rc;

use crate::core::menu::{ItemKind, Menu, MenuBar, MenuItem, mnemonic};
use crate::core::{
    ActionId, Align, Anchor, Axis, BoxStyle, ContextId, Cx, Element, OverlaySpec, Role,
};
use crate::geometry::{Insets, Point};
use crate::platform::menu::{mnemonic_at, mnemonic_range};
use crate::style::Theme;

/// What a drawn menu was opened from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum MenuRoot {
    /// The menu bar's menu at this index.
    Bar(usize),
    /// The context menu registered as `id`, opened at `at`.
    Context { id: ContextId, at: Point },
}

/// A key, as menu navigation sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MenuKey {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    Enter,
    Escape,
    Tab,
    /// A typed character (lower-case): a mnemonic, or Space to pick.
    Char(char),
}

/// What [`MenuUi::handle_key`] did with a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyResult {
    /// No menu is active; the key goes elsewhere.
    Ignored,
    /// The menus used the key.
    Handled,
    /// Pick the item whose row carries this tap action.
    Activate(ActionId),
}

/// The structure of a menu, enough to navigate it with the keyboard without
/// its actions.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Shape {
    items: Vec<ShapeItem>,
}

#[derive(Clone, Debug, PartialEq)]
struct ShapeItem {
    /// An enabled command or a submenu (not a separator or a disabled command).
    selectable: bool,
    submenu: Option<Shape>,
    /// The `&` mnemonic, lower-case.
    mnemonic: Option<char>,
    /// The first character of the label, lower-case.
    initial: Option<char>,
}

impl Shape {
    fn of<S>(menu: &Menu<S>) -> Shape {
        Shape {
            items: menu.items.iter().map(ShapeItem::of).collect(),
        }
    }

    /// The next selectable item from `from` in direction `step` (±1),
    /// wrapping; from nothing, the first (or last) one.
    fn step(&self, from: Option<usize>, step: isize) -> Option<usize> {
        let n = self.items.len() as isize;
        if n == 0 {
            return None;
        }
        let start = match from {
            Some(i) => i as isize,
            None if step > 0 => -1,
            None => n,
        };
        (1..=n)
            .map(|k| (start + step * k).rem_euclid(n) as usize)
            .find(|&i| self.items[i].selectable)
    }

    fn first(&self) -> Option<usize> {
        self.step(None, 1)
    }

    fn last(&self) -> Option<usize> {
        self.step(None, -1)
    }

    /// The selectable item whose mnemonic is `c`, else the first one whose
    /// label starts with it.
    fn find_key(&self, c: char) -> Option<usize> {
        let ok = |i: &usize| self.items[*i].selectable;
        (0..self.items.len())
            .filter(ok)
            .find(|&i| self.items[i].mnemonic == Some(c))
            .or_else(|| {
                (0..self.items.len())
                    .filter(ok)
                    .find(|&i| self.items[i].mnemonic.is_none() && self.items[i].initial == Some(c))
            })
    }
}

impl ShapeItem {
    fn of<S>(item: &MenuItem<S>) -> ShapeItem {
        let (text, key) = mnemonic(item.label());
        ShapeItem {
            selectable: item.is_enabled(),
            submenu: item.get_submenu().map(Shape::of),
            mnemonic: key,
            initial: text.chars().next().and_then(|c| c.to_lowercase().next()),
        }
    }
}

/// The open/highlight state of one window's drawn menus, plus what the last
/// build laid out (to route pointer moves and keys back to items).
#[derive(Debug, Default)]
pub(crate) struct MenuUi {
    /// The open menu's origin, or `None` when no menu is open.
    pub root: Option<MenuRoot>,
    /// The highlighted item at each open level: `levels[0]` in the root menu,
    /// `levels[1]` in the submenu open from it, and so on.
    pub levels: Vec<Option<usize>>,
    /// With no menu open, the menu bar title the keyboard is on (after Alt or
    /// F10).
    pub bar_focus: Option<usize>,
    /// Highlight the first item once the root menu's items are known (it was
    /// opened from the keyboard).
    first_pending: bool,
    /// Underline the mnemonics (keyboard cues): the keyboard is driving the
    /// menus, or Alt is held. Windows' convention: menus opened with the mouse
    /// don't show them.
    pub cues: bool,
    /// The bar titles' tap actions and keys, from the last build.
    titles: Vec<(ActionId, Option<char>, Option<char>)>,
    /// The open menus' rows: each row's tap action and its path.
    rows: Vec<(ActionId, Vec<usize>)>,
    /// The open root menu's structure.
    shape: Option<Shape>,
    /// When each open level's menu opened (with what it was opened from: the
    /// root, and the path of items to it), for its entrance.
    opened: Vec<(MenuRoot, Vec<usize>, std::time::Duration)>,
}

impl MenuUi {
    /// Whether a menu is open or the keyboard is on the menu bar.
    pub fn is_active(&self) -> bool {
        self.root.is_some() || self.bar_focus.is_some()
    }

    /// Close every menu and leave the menu bar.
    pub fn close(&mut self) {
        self.root = None;
        self.levels.clear();
        self.opened.clear();
        self.bar_focus = None;
        self.first_pending = false;
        self.cues = false;
    }

    /// Open the menu bar's menu `index`, highlighting its first item if
    /// `keyboard`.
    pub fn open_bar(&mut self, index: usize, keyboard: bool) {
        self.root = Some(MenuRoot::Bar(index));
        self.levels = vec![None];
        self.bar_focus = None;
        self.first_pending = keyboard;
        self.cues = keyboard;
    }

    /// Open the context menu `id` at `at`, highlighting its first item if
    /// `keyboard`.
    pub fn open_context(&mut self, id: ContextId, at: Point, keyboard: bool) {
        self.root = Some(MenuRoot::Context { id, at });
        self.levels = vec![None];
        self.bar_focus = None;
        self.first_pending = keyboard;
        self.cues = keyboard;
    }

    /// A click on bar title `index`: open its menu, or close it if it's open.
    pub fn toggle_bar(&mut self, index: usize) {
        if self.root == Some(MenuRoot::Bar(index)) {
            self.close();
        } else {
            self.open_bar(index, false);
        }
    }

    /// Put the keyboard on the menu bar's first title (F10, or Alt released on
    /// its own). Returns whether there is a menu bar.
    pub fn focus_bar(&mut self) -> bool {
        if self.titles.is_empty() {
            return false;
        }
        self.close();
        self.bar_focus = Some(0);
        self.cues = true;
        true
    }

    /// Open the bar menu whose title has mnemonic (or, failing that, initial)
    /// `c`, for Alt+letter. Returns whether one matched.
    pub fn open_by_mnemonic(&mut self, c: char) -> bool {
        match self.title_for(c) {
            Some(i) => {
                self.open_bar(i, true);
                true
            }
            None => false,
        }
    }

    fn title_for(&self, c: char) -> Option<usize> {
        let c = c.to_lowercase().next()?;
        self.titles.iter().position(|t| t.1 == Some(c)).or_else(|| {
            self.titles
                .iter()
                .position(|t| t.1.is_none() && t.2 == Some(c))
        })
    }

    /// The bar titles' tap actions, left to right.
    pub fn title_actions(&self) -> impl Iterator<Item = ActionId> + '_ {
        self.titles.iter().map(|t| t.0)
    }

    /// The structure of the menu open at `level`.
    fn level_shape(&self, level: usize) -> Option<&Shape> {
        let mut shape = self.shape.as_ref()?;
        for l in 0..level {
            let i = (*self.levels.get(l)?)?;
            shape = shape.items.get(i)?.submenu.as_ref()?;
        }
        Some(shape)
    }

    /// The pointer moved onto the element with tap action `id`: if it is a row
    /// of an open menu, highlight it (opening its submenu). Returns whether
    /// anything changed.
    pub fn hover(&mut self, id: ActionId) -> bool {
        let Some(path) = self.rows.iter().find(|r| r.0 == id).map(|r| r.1.clone()) else {
            return false;
        };
        let mut levels: Vec<Option<usize>> = path.iter().map(|&i| Some(i)).collect();
        let last = path.len() - 1;
        let is_submenu = self
            .level_shape(last)
            .and_then(|s| s.items.get(path[last]))
            .is_some_and(|it| it.submenu.is_some() && it.selectable);
        if is_submenu {
            levels.push(None);
        }
        let changed = levels != self.levels;
        self.levels = levels;
        self.first_pending = false;
        changed
    }

    /// Handle a key while a menu is open or the keyboard is on the menu bar.
    pub fn handle_key(&mut self, key: MenuKey) -> KeyResult {
        if self.is_active() {
            // The keyboard is driving the menus: show their mnemonics.
            self.cues = true;
        }
        let Some(root) = self.root else {
            let Some(i) = self.bar_focus else {
                return KeyResult::Ignored;
            };
            let n = self.titles.len();
            if n == 0 {
                self.close();
                return KeyResult::Ignored;
            }
            match key {
                MenuKey::Left => self.bar_focus = Some((i + n - 1) % n),
                MenuKey::Right => self.bar_focus = Some((i + 1) % n),
                MenuKey::Home => self.bar_focus = Some(0),
                MenuKey::End => self.bar_focus = Some(n - 1),
                MenuKey::Down | MenuKey::Up | MenuKey::Enter => self.open_bar(i, true),
                MenuKey::Char(' ') => self.open_bar(i, true),
                MenuKey::Escape | MenuKey::Tab => self.close(),
                MenuKey::Char(c) => {
                    self.open_by_mnemonic(c);
                }
            }
            return KeyResult::Handled;
        };
        let titles = self.titles.len();
        let depth = self.levels.len().max(1);
        self.levels.resize(depth, None);
        let Some(shape) = self.level_shape(depth - 1).cloned() else {
            self.close();
            return KeyResult::Handled;
        };
        let cur = self.levels[depth - 1];
        let next_bar = |b: usize, step: usize| (b + step) % titles.max(1);
        match key {
            MenuKey::Down => self.levels[depth - 1] = shape.step(cur, 1),
            MenuKey::Up => self.levels[depth - 1] = shape.step(cur, -1),
            MenuKey::Home => self.levels[depth - 1] = shape.first(),
            MenuKey::End => self.levels[depth - 1] = shape.last(),
            MenuKey::Right => {
                let sub = cur.and_then(|i| {
                    let item = &shape.items[i];
                    item.submenu.as_ref().filter(|_| item.selectable)
                });
                match (sub, root) {
                    (Some(sub), _) => self.levels.push(sub.first()),
                    (None, MenuRoot::Bar(b)) => self.open_bar(next_bar(b, 1), true),
                    _ => {}
                }
            }
            MenuKey::Left => match root {
                _ if depth > 1 => {
                    self.levels.pop();
                }
                MenuRoot::Bar(b) => self.open_bar(next_bar(b, titles.max(1) - 1), true),
                MenuRoot::Context { .. } => {}
            },
            MenuKey::Enter | MenuKey::Char(' ') => {
                if let Some(i) = cur {
                    return self.choose(&shape, depth - 1, i);
                }
            }
            MenuKey::Escape => match root {
                _ if depth > 1 => {
                    self.levels.pop();
                }
                MenuRoot::Bar(b) => {
                    self.close();
                    self.bar_focus = Some(b);
                    self.cues = true;
                }
                MenuRoot::Context { .. } => self.close(),
            },
            MenuKey::Tab => self.close(),
            MenuKey::Char(c) => {
                if let Some(i) = shape.find_key(c) {
                    self.levels[depth - 1] = Some(i);
                    return self.choose(&shape, depth - 1, i);
                }
            }
        }
        KeyResult::Handled
    }

    /// Pick item `i` of the menu open at `level`: open a submenu, or activate
    /// a command through its row's tap action.
    fn choose(&mut self, shape: &Shape, level: usize, i: usize) -> KeyResult {
        let Some(item) = shape.items.get(i).filter(|it| it.selectable) else {
            return KeyResult::Handled;
        };
        if let Some(sub) = &item.submenu {
            self.levels.truncate(level + 1);
            self.levels.push(sub.first());
            return KeyResult::Handled;
        }
        let path: Option<Vec<usize>> = self.levels[..=level].iter().copied().collect();
        match path.and_then(|p| self.rows.iter().find(|r| r.1 == p)) {
            Some(row) => KeyResult::Activate(row.0),
            None => KeyResult::Handled,
        }
    }
}

/// Put an in-window menu bar for `bar` above `content`, and declare the open
/// bar menu (if any) as overlays.
pub(crate) fn with_bar<S: 'static>(
    cx: &mut Cx<S>,
    theme: &Theme,
    bar: &MenuBar<S>,
    ui: &Rc<RefCell<MenuUi>>,
    content: Element,
) -> Element {
    let p = &theme.palette;
    let (root, focus, cues) = {
        let u = ui.borrow();
        (u.root, u.bar_focus, u.cues)
    };
    let mut titles = Vec::with_capacity(bar.menus.len());
    let mut keys = Vec::with_capacity(bar.menus.len());
    for (i, menu) in bar.menus.iter().enumerate() {
        let (text, key) = mnemonic(&menu.label);
        let initial = text.chars().next().and_then(|c| c.to_lowercase().next());
        let open = root == Some(MenuRoot::Bar(i));
        let ui2 = ui.clone();
        let mut title = Element::stack(
            Axis::Horizontal,
            vec![label_text(&menu.label, cues, theme.typography.body, p.text)],
        )
        .padding(Insets::symmetric(theme.spacing.sm + 2.0, theme.spacing.xs))
        .radius(theme.radius / 2.0)
        .align(Align::Start, Align::Center)
        .role(Role::MenuItem)
        .a11y_label(text)
        .expanded(open);
        if open || focus == Some(i) {
            title = title.fill(p.selection);
        }
        let title = title.on_tap(cx, move |_: &mut S| ui2.borrow_mut().toggle_bar(i));
        keys.push((title.action.expect("on_tap sets it"), key, initial));
        titles.push(title);
    }
    ui.borrow_mut().titles = keys;

    match root {
        Some(MenuRoot::Bar(i)) => match bar.menus.get(i) {
            Some(menu) => {
                let anchor = Anchor::Below(ui.borrow().titles[i].0);
                popups(cx, theme, menu, anchor, ui);
            }
            None => ui.borrow_mut().close(),
        },
        None if focus.is_some_and(|i| i >= bar.menus.len()) => ui.borrow_mut().close(),
        _ => {}
    }

    let strip = Element::stack(Axis::Horizontal, titles)
        .gap(2.0)
        .padding(Insets::symmetric(theme.spacing.xs, 2.0))
        .align(Align::Start, Align::Center);
    let line = Element::boxed(BoxStyle {
        fill: Some(p.border),
        ..BoxStyle::default()
    })
    .height(1.0);
    let menubar = Element::stack(Axis::Vertical, vec![strip, line])
        .fill(p.surface)
        .align(Align::Start, Align::Stretch)
        .role(Role::MenuBar);
    Element::stack(Axis::Vertical, vec![menubar, content.grow(1.0)])
        .align(Align::Start, Align::Stretch)
}

/// Declare the open context menu (if any) as overlays, from the context menus
/// the view registered this frame.
pub(crate) fn context<S: 'static>(
    cx: &mut Cx<S>,
    theme: &Theme,
    menus: &[(ContextId, Menu<S>)],
    ui: &Rc<RefCell<MenuUi>>,
) {
    let root = ui.borrow().root;
    if let Some(MenuRoot::Context { id, at }) = root {
        match menus.iter().find(|(m, _)| *m == id) {
            Some((_, menu)) => popups(cx, theme, menu, Anchor::Popup(at), ui),
            // The element went away.
            None => ui.borrow_mut().close(),
        }
    }
}

/// Declare an overlay for each open level of `menu`: the root at `anchor`
/// (with a press-outside catcher that closes the menus), each submenu beside
/// its item.
fn popups<S: 'static>(
    cx: &mut Cx<S>,
    theme: &Theme,
    menu: &Menu<S>,
    anchor: Anchor,
    ui: &Rc<RefCell<MenuUi>>,
) {
    let levels = {
        let mut u = ui.borrow_mut();
        let shape = Shape::of(menu);
        if u.levels.is_empty() {
            u.levels.push(None);
        }
        if std::mem::take(&mut u.first_pending) && u.levels[0].is_none() {
            u.levels[0] = shape.first();
        }
        u.shape = Some(shape);
        u.rows.clear();
        u.levels.clone()
    };
    let dismiss = {
        let ui = ui.clone();
        cx.register(move |_: &mut S| ui.borrow_mut().close())
    };
    let mut current = menu;
    let mut path: Vec<usize> = Vec::new();
    let mut anchor = anchor;
    let root = ui.borrow().root;
    let now = cx.frame_time();
    // Grow from the corner nearest the anchor: the start edge's top.
    let origin = (if theme.is_rtl() { 1.0 } else { 0.0 }, 0.0);
    for (level, highlight) in levels.iter().enumerate() {
        let (panel, row_ids) = panel(cx, theme, current, &path, *highlight, ui);
        // A level opens when what it's opened from changes.
        let opened = root.map(|root| {
            let mut u = ui.borrow_mut();
            u.opened.truncate(level + 1);
            match u.opened.get(level) {
                Some((r, p, at)) if *r == root && *p == path => *at,
                _ => {
                    let at = super::motion::start_of(now);
                    u.opened.truncate(level);
                    u.opened.push((root, path.clone(), at));
                    at
                }
            }
        });
        let t = super::motion::progress(cx, theme.motion.popup, opened);
        let panel = super::motion::popup(panel, &theme.motion, t, origin)
            .exit(super::motion::popup_exit(&theme.motion, origin));
        cx.overlay(OverlaySpec {
            content: panel,
            anchor,
            modal: false,
            dismiss: (level == 0).then_some(dismiss),
        });
        // Go on to the submenu open from the highlighted item, if any.
        if level + 1 >= levels.len() {
            break;
        }
        let next = highlight.and_then(|i| {
            let sub = current.items.get(i)?.get_submenu()?;
            Some((i, sub, row_ids.get(i).copied().flatten()?))
        });
        match next {
            Some((i, sub, row)) => {
                current = sub;
                path.push(i);
                anchor = Anchor::RightOf(row);
            }
            None => {
                ui.borrow_mut().levels.truncate(level + 1);
                break;
            }
        }
    }
}

/// A menu label's text, its `&` mnemonic underlined when `cues` are on.
fn label_text(label: &str, cues: bool, size: f64, color: crate::render::Color) -> Element {
    let (text, at) = mnemonic_at(label);
    let underline = if cues {
        mnemonic_range(&text, at)
    } else {
        None
    };
    Element::text(text, size, color).underline(underline)
}

/// One open menu: a panel of rows. Returns it and each item's row tap action
/// (`None` for separators and disabled commands).
fn panel<S: 'static>(
    cx: &mut Cx<S>,
    theme: &Theme,
    menu: &Menu<S>,
    path: &[usize],
    highlight: Option<usize>,
    ui: &Rc<RefCell<MenuUi>>,
) -> (Element, Vec<Option<ActionId>>) {
    let p = &theme.palette;
    let cues = ui.borrow().cues;
    let row_height = (theme.typography.body * 1.4 + 2.0 * theme.spacing.xs).round();
    let mut rows = Vec::with_capacity(menu.items.len());
    let mut ids = Vec::with_capacity(menu.items.len());
    for (i, item) in menu.items.iter().enumerate() {
        if item.is_separator() {
            let line = Element::boxed(BoxStyle {
                fill: Some(p.border),
                ..BoxStyle::default()
            })
            .height(1.0);
            rows.push(
                Element::stack(Axis::Vertical, vec![line])
                    .padding(Insets::symmetric(theme.spacing.xs, theme.spacing.xs))
                    .align(Align::Start, Align::Stretch)
                    .role(Role::Separator),
            );
            ids.push(None);
            continue;
        }
        let (text, _) = mnemonic(item.label());
        let enabled = item.is_enabled();
        let ink = if enabled { p.text } else { p.text_muted };
        let mark = match item.is_checked() {
            Some(true) => "\u{2713}",
            _ => "",
        };
        let mut cells = vec![
            Element::text(mark, theme.typography.body, p.primary).width(18.0),
            label_text(item.label(), cues, theme.typography.body, ink).grow(1.0),
        ];
        if let Some(s) = item.get_shortcut() {
            // Keep shortcuts clear of the labels.
            let text = Element::text(s.label(), theme.typography.body, p.text_muted);
            cells.push(
                Element::stack(Axis::Horizontal, vec![text]).padding(Insets::new(
                    0.0,
                    0.0,
                    0.0,
                    theme.spacing.lg,
                )),
            );
        }
        let submenu = item.get_submenu().is_some();
        cells.push(
            Element::text(
                match (submenu, theme.is_rtl()) {
                    (false, _) => "",
                    // Submenus open leftwards in a mirrored UI.
                    (true, true) => "\u{25C2}",
                    (true, false) => "\u{25B8}",
                },
                theme.typography.body,
                p.text_muted,
            )
            .width(12.0),
        );
        let mut row = Element::stack(Axis::Horizontal, cells)
            .padding(Insets::symmetric(theme.spacing.xs, 0.0))
            .gap(theme.spacing.sm)
            .align(Align::Start, Align::Center)
            .height(row_height)
            .radius(theme.radius / 2.0)
            .role(Role::MenuItem)
            .a11y_label(text);
        if submenu {
            row = row.expanded(highlight == Some(i));
        }
        if highlight == Some(i) && enabled {
            row = row.fill(p.selection);
        }
        let mut item_path = path.to_vec();
        item_path.push(i);
        let id = match &item.kind {
            ItemKind::Submenu(_) => {
                let ui = ui.clone();
                let open: Vec<usize> = item_path.clone();
                Some(cx.register(move |_: &mut S| {
                    let mut u = ui.borrow_mut();
                    u.levels = open.iter().map(|&i| Some(i)).collect();
                    u.levels.push(None);
                }))
            }
            ItemKind::Action { .. } => item.action().map(|action| {
                let ui = ui.clone();
                cx.register(move |s: &mut S| {
                    ui.borrow_mut().close();
                    (action.borrow_mut())(s);
                })
            }),
            ItemKind::Separator => None,
        };
        if let Some(id) = id {
            row.action = Some(id);
            ui.borrow_mut().rows.push((id, item_path));
        }
        ids.push(id);
        rows.push(row);
    }
    let (title, _) = mnemonic(&menu.label);
    let mut el = Element::stack(Axis::Vertical, rows)
        .fill(p.surface)
        .border(p.border, 1.0)
        .radius(theme.radius)
        .padding(Insets::uniform(theme.spacing.xs))
        .align(Align::Start, Align::Stretch)
        .min_width(160.0)
        .role(Role::Menu);
    if !title.is_empty() {
        el = el.a11y_label(title);
    }
    (el, ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape() -> Shape {
        let menu: Menu<()> = Menu::new("File")
            .item(MenuItem::new("&New", |_| {}))
            .separator()
            .item(MenuItem::new("Gone", |_| {}).enabled(false))
            .submenu(Menu::new("&Recent").item(MenuItem::new("a", |_| {})))
            .item(MenuItem::new("Quit", |_| {}));
        Shape::of(&menu)
    }

    #[test]
    fn stepping_skips_separators_and_disabled_items_and_wraps() {
        let s = shape();
        assert_eq!(s.first(), Some(0));
        assert_eq!(s.step(Some(0), 1), Some(3));
        assert_eq!(s.step(Some(3), 1), Some(4));
        assert_eq!(s.step(Some(4), 1), Some(0), "wraps");
        assert_eq!(s.step(Some(0), -1), Some(4), "wraps backwards");
        assert_eq!(s.last(), Some(4));
        assert_eq!(s.find_key('r'), Some(3));
        assert_eq!(s.find_key('q'), Some(4), "initial without a mnemonic");
        assert_eq!(s.find_key('g'), None, "disabled");
    }

    #[test]
    fn keyboard_walks_into_submenus_and_back() {
        let mut ui = MenuUi {
            shape: Some(shape()),
            root: Some(MenuRoot::Context {
                id: ContextId(0),
                at: Point::new(0.0, 0.0),
            }),
            levels: vec![Some(0)],
            ..MenuUi::default()
        };
        ui.handle_key(MenuKey::Down);
        assert_eq!(ui.levels, vec![Some(3)]);
        ui.handle_key(MenuKey::Right);
        assert_eq!(ui.levels, vec![Some(3), Some(0)], "opened the submenu");
        ui.handle_key(MenuKey::Left);
        assert_eq!(ui.levels, vec![Some(3)]);
        ui.handle_key(MenuKey::Escape);
        assert!(!ui.is_active());
    }

    #[test]
    fn the_bar_moves_between_titles() {
        let mut ui = MenuUi {
            titles: vec![
                (ActionId(0), Some('f'), Some('f')),
                (ActionId(1), Some('e'), Some('e')),
            ],
            ..MenuUi::default()
        };
        assert!(ui.focus_bar());
        ui.handle_key(MenuKey::Right);
        assert_eq!(ui.bar_focus, Some(1));
        ui.handle_key(MenuKey::Right);
        assert_eq!(ui.bar_focus, Some(0), "wraps");
        ui.handle_key(MenuKey::Down);
        assert_eq!(ui.root, Some(MenuRoot::Bar(0)));
        assert!(ui.first_pending);
        assert_eq!(ui.handle_key(MenuKey::Char('e')), KeyResult::Handled);
        assert!(ui.open_by_mnemonic('E'));
        assert_eq!(ui.root, Some(MenuRoot::Bar(1)));
    }
}
