//! Platform-neutral accessibility tree.
//!
//! `stipple::platform` is dependency-free, so it can't name
//! `stipple::core::AccessNode`. `App` maps that semantic tree into
//! this mirror and hands it to a backend through
//! [`Window::set_accessibility_tree`](crate::platform::Window::set_accessibility_tree).
//! Each backend then exposes it to its OS accessibility API — `NSAccessibility`
//! on macOS, UI Automation on Windows, AT-SPI on Linux, UIAccessibility on iOS,
//! ARIA on the web — so a screen reader sees
//! the *whole* element hierarchy, not just the window root.
//!
//! This carries the same fields `stipple::core::AccessNode` does; keep
//! [`A11yRole`] in sync with `stipple::core::a11y::Role`. The desktop bridges
//! report the roles their APIs don't name yet (a combo box, a checkbox, a
//! toolbar…) as the closest one they do; the web's ARIA bridge names them all.

/// The semantic kind of an [`A11yNode`] — mirrors `stipple::core::a11y::Role`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum A11yRole {
    /// The top-level window (tree root).
    Window,
    /// A non-interactive container.
    Group,
    /// An activatable control.
    Button,
    /// An editable text field.
    TextField,
    /// A run of static text.
    Text,
    /// A list of selectable items.
    List,
    /// One item of a list.
    ListItem,
    /// A table / data grid.
    Table,
    /// One row of a table.
    Row,
    /// One cell of a table row.
    Cell,
    /// A table column header.
    ColumnHeader,
    /// A tree of expandable items.
    Tree,
    /// One item of a tree.
    TreeItem,
    /// A menu bar.
    MenuBar,
    /// An open menu.
    Menu,
    /// One item of a menu or menu bar.
    MenuItem,
    /// A button that opens a list to pick a value from (a select).
    ComboBox,
    /// A numeric field with increment and decrement steps.
    SpinButton,
    /// A checkbox ([`A11yNode::checked`] says whether it is checked).
    CheckBox,
    /// One option of a single-choice group ([`A11yNode::checked`] says
    /// whether it is the picked one).
    RadioButton,
    /// An on/off switch ([`A11yNode::checked`] says whether it is on).
    Switch,
    /// A draggable divider between two panes.
    Splitter,
    /// A control that sets a value along a track.
    Slider,
    /// How far a task has got (a read-only [`A11yNode::range`]).
    ProgressBar,
    /// A row of command buttons.
    Toolbar,
    /// A visual divider.
    Separator,
    /// A transient notification.
    Alert,
    /// A row of document tabs.
    TabList,
    /// One tab of a tab list ([`A11yNode::selected`] says whether it is the
    /// current one).
    Tab,
    /// A text field whose text is hidden.
    PasswordField,
    /// A modal dialog.
    Dialog,
    /// A scroll container's scroll bar ([`A11yNode::scroll_bar`] says which
    /// way, and where it is).
    ScrollBar,
}

/// Where a scroll container is scrolled — mirrors `stipple::core::AccessScroll`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct A11yScroll {
    /// How far it is scrolled from its start, `0..=max`, logical pixels.
    pub offset: f64,
    /// The largest offset (0 when its content fits).
    pub max: f64,
    /// The viewport's length along the axis it scrolls on.
    pub viewport: f64,
    /// Whether it scrolls sideways.
    pub horizontal: bool,
    /// A sideways strip laid out right to left (its start is on the right).
    pub rtl: bool,
}

/// A value along a range — mirrors `stipple::core::AccessRange`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct A11yRange {
    /// The current value, `min..=max`.
    pub value: f64,
    pub min: f64,
    pub max: f64,
    /// One increment (0: none given).
    pub step: f64,
    /// A slider's track runs right to left.
    pub rtl: bool,
}

/// A table's size — mirrors `stipple::core::AccessTable`: all its rows
/// (the header row and rows not built included) and columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct A11yTable {
    pub rows: usize,
    pub columns: usize,
    /// How many leading columns stay put while the rest scroll sideways.
    pub frozen: usize,
}

/// Where a table's row or cell is — mirrors `stipple::core::AccessCell`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct A11yCell {
    pub row: usize,
    /// 0 for a row.
    pub column: usize,
}

/// An item's place in its set, as declared — mirrors
/// `stipple::core::AccessPosition` (`index` from 0). What the bridges
/// report is [`A11yNode::set_position`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct A11yPosition {
    pub index: usize,
    pub count: usize,
}

/// How a node's text is drawn — mirrors `stipple::core::AccessFont`.
#[derive(Clone, Debug, PartialEq)]
pub struct A11yFont {
    /// The family, when known.
    pub family: Option<String>,
    /// Logical pixels.
    pub size: f64,
    pub weight: u16,
    pub italic: bool,
}

/// A node in the platform-neutral accessibility tree.
#[derive(Clone, Debug, PartialEq)]
pub struct A11yNode {
    pub role: A11yRole,
    /// The accessible name (a control's label, or the text itself).
    pub name: String,
    /// Logical bounds `(x, y, width, height)` — used for frame reporting and
    /// hit-testing by backends whose AT exposes geometry.
    pub bounds: (f64, f64, f64, f64),
    /// Whether this node currently holds keyboard focus.
    pub focused: bool,
    /// Whether this item is selected (list items, rows, tree items).
    pub selected: bool,
    /// Whether this item is expanded, if it can expand.
    pub expanded: Option<bool>,
    /// Whether this checkbox or switch is checked, if it can be.
    pub checked: Option<bool>,
    /// For an editable text field, the caret as a byte index into `name`;
    /// `None` when the name isn't the field's text.
    pub caret: Option<usize>,
    /// For an editable text field, the selected byte range `[start, end)` of
    /// `name`, if any.
    pub text_selection: Option<(usize, usize)>,
    /// The id other nodes' relations name this one by.
    pub id: Option<String>,
    /// The [`id`](Self::id) of the node that labels this one.
    pub labelled_by: Option<String>,
    /// The [`id`](Self::id) of the node this one controls (a combo box's
    /// open list, a tab's panel); see [`relations`].
    pub controls: Option<String>,
    /// A tree item's depth, 1 for the top level.
    pub level: Option<usize>,
    /// An item's place in its set, when its widget declares it (a
    /// virtualized list's rows); see [`set_position`](Self::set_position).
    pub position: Option<A11yPosition>,
    /// A text field's hint, read while it's empty.
    pub placeholder: Option<String>,
    /// For a scroll container, where it is scrolled.
    pub scroll: Option<A11yScroll>,
    /// For a [`A11yRole::ScrollBar`], the scrolling it shows: its
    /// orientation, and its value (the offset, from 0 to the largest).
    pub scroll_bar: Option<A11yScroll>,
    /// For a slider, a progress bar or a spin button, its value along its
    /// range.
    pub range: Option<A11yRange>,
    /// For a table, its size.
    pub table: Option<A11yTable>,
    /// For a table's row, cell or column header, where it is.
    pub cell: Option<A11yCell>,
    /// For a node with text, how it's drawn.
    pub font: Option<A11yFont>,
    /// For rich text, where its runs drawn differently come from (see
    /// [`text_runs`](Self::text_runs), which builds them on demand).
    pub runs: A11yRuns,
    /// The live OS control that stands for this node (see
    /// [`NativeView`](crate::platform::native::NativeView)), by id: the
    /// bridge exposes that control here, as the OS's own accessibility
    /// does, instead of a node of its own.
    pub native_view: Option<u64>,
    pub children: Vec<A11yNode>,
}

// Not every bridge (nor every target) uses every helper; the tests do.
#[allow(dead_code)]
impl A11yNode {
    /// A node with no state and no children.
    pub fn new(role: A11yRole, name: impl Into<String>, bounds: (f64, f64, f64, f64)) -> Self {
        Self {
            role,
            name: name.into(),
            bounds,
            focused: false,
            selected: false,
            expanded: None,
            checked: None,
            caret: None,
            text_selection: None,
            id: None,
            labelled_by: None,
            controls: None,
            level: None,
            position: None,
            placeholder: None,
            scroll: None,
            scroll_bar: None,
            range: None,
            table: None,
            cell: None,
            font: None,
            runs: A11yRuns::default(),
            native_view: None,
            children: Vec::new(),
        }
    }

    /// Total node count (including this one), for tests/inspection.
    pub fn count(&self) -> usize {
        1 + self.children.iter().map(A11yNode::count).sum::<usize>()
    }

    /// Give the text that names no font of its own `family` (the app's).
    pub fn fill_font_family(&mut self, family: &str) {
        if let Some(f) = &mut self.font
            && f.family.is_none()
        {
            f.family = Some(family.to_string());
        }
        for c in &mut self.children {
            c.fill_font_family(family);
        }
    }

    /// The table cell (or row, for `column` `None`) at `row` and
    /// `column` under this table node, with its path from it: the rows
    /// are found through any groups between (not inside a nested table).
    pub fn table_item(&self, row: usize, column: Option<usize>) -> Option<(Vec<usize>, &A11yNode)> {
        // The cell of `column` in the row `n` (at `path`), through the
        // groups that wrap its cells.
        fn cell_in<'a>(
            n: &'a A11yNode,
            column: usize,
            path: &[usize],
        ) -> Option<(Vec<usize>, &'a A11yNode)> {
            n.children.iter().enumerate().find_map(|(j, c)| {
                let mut p = path.to_vec();
                p.push(j);
                match c.role {
                    A11yRole::Cell | A11yRole::ColumnHeader => c
                        .cell
                        .is_some_and(|at| at.column == column)
                        .then_some((p, c)),
                    A11yRole::Group => cell_in(c, column, &p),
                    _ => None,
                }
            })
        }
        fn go<'a>(
            n: &'a A11yNode,
            row: usize,
            column: Option<usize>,
            path: &mut Vec<usize>,
        ) -> Option<(Vec<usize>, &'a A11yNode)> {
            for (i, c) in n.children.iter().enumerate() {
                path.push(i);
                let found = match c.role {
                    A11yRole::Row if c.cell.map(|p| p.row) == Some(row) => match column {
                        None => Some((path.clone(), c)),
                        Some(col) => cell_in(c, col, path),
                    },
                    A11yRole::Row | A11yRole::Table => None,
                    _ => go(c, row, column, path),
                };
                path.pop();
                if found.is_some() {
                    return found;
                }
            }
            None
        }
        go(self, row, column, &mut Vec::new())
    }

    /// The column headers under this table node (its header row's cells),
    /// by column, with their paths from it.
    pub fn column_headers(&self) -> Vec<(usize, Vec<usize>)> {
        fn go(n: &A11yNode, path: &mut Vec<usize>, out: &mut Vec<(usize, Vec<usize>)>) {
            for (i, c) in n.children.iter().enumerate() {
                path.push(i);
                match c.role {
                    A11yRole::ColumnHeader => {
                        if let Some(p) = c.cell {
                            out.push((p.column, path.clone()));
                        }
                    }
                    A11yRole::Table => {}
                    _ => go(c, path, out),
                }
                path.pop();
            }
        }
        let mut out = Vec::new();
        go(self, &mut Vec::new(), &mut out);
        out
    }

    /// The path to the nearest table above the node at `path` under this
    /// one (a prefix of `path`).
    pub fn table_above(&self, path: &[usize]) -> Option<Vec<usize>> {
        let mut at = self;
        let mut found = None;
        for (depth, &i) in path.iter().enumerate() {
            if at.role == A11yRole::Table {
                found = Some(path[..depth].to_vec());
            }
            at = at.children.get(i)?;
        }
        found
    }

    /// The hint a screen reader reads with this node, when it has one: a
    /// field's [`placeholder`](Self::placeholder), unless empty. Every bridge
    /// exposes it the platform's way (AT-SPI's description, UIA's
    /// `HelpText`, `AXPlaceholderValue`, iOS's `accessibilityHint`,
    /// Android's hint text, `aria-placeholder`).
    pub fn hint(&self) -> Option<&str> {
        self.placeholder.as_deref().filter(|p| !p.is_empty())
    }

    /// The path (child index at each level) to the first focused node, in
    /// depth-first order; `Some(vec![])` when it's this node.
    pub fn focused_path(&self) -> Option<Vec<usize>> {
        if self.focused {
            return Some(Vec::new());
        }
        self.children.iter().enumerate().find_map(|(i, c)| {
            c.focused_path().map(|mut p| {
                p.insert(0, i);
                p
            })
        })
    }

    /// The node at `path` (as [`focused_path`](Self::focused_path) gives it).
    pub fn at_path(&self, path: &[usize]) -> Option<&A11yNode> {
        path.iter().try_fold(self, |node, &i| node.children.get(i))
    }

    /// Where focus went, if it moved between the tree `old` (none before)
    /// and this one: the path to the newly focused node, when there is one
    /// and it isn't the node (same place, same role) focused before. A
    /// screen reader follows focus through the event a backend raises for
    /// it (UIA's focus-changed event); a tree rebuilt with focus where it was
    /// raises none.
    pub fn focus_moved_from(&self, old: Option<&A11yNode>) -> Option<Vec<usize>> {
        let path = self.focused_path()?;
        let role = self.at_path(&path).map(|n| n.role);
        let before = old.and_then(|o| {
            let p = o.focused_path()?;
            let r = o.at_path(&p).map(|n| n.role);
            Some((p, r))
        });
        match before {
            Some((p, r)) if p == path && r == role => None,
            _ => Some(path),
        }
    }

    /// The alerts (toasts) in this tree that weren't in the tree `old`, by
    /// name, in tree order: what a backend asks the screen reader to
    /// announce. None for the first tree (`old` is `None`), which the screen
    /// reader reads as a whole, nor for an alert with no name.
    pub fn alerts_new_since(&self, old: Option<&A11yNode>) -> Vec<String> {
        self.alert_paths_new_since(old)
            .into_iter()
            .map(|(_, name)| name)
            .collect()
    }

    /// [`alerts_new_since`](Self::alerts_new_since), with the path to each
    /// alert's node (child indices from this one).
    pub fn alert_paths_new_since(&self, old: Option<&A11yNode>) -> Vec<(Vec<usize>, String)> {
        fn alerts<'a>(
            n: &'a A11yNode,
            path: &mut Vec<usize>,
            out: &mut Vec<(Vec<usize>, &'a str)>,
        ) {
            if n.role == A11yRole::Alert && !n.name.is_empty() {
                out.push((path.clone(), &n.name));
            }
            for (i, c) in n.children.iter().enumerate() {
                path.push(i);
                alerts(c, path, out);
                path.pop();
            }
        }
        let Some(old) = old else {
            return Vec::new();
        };
        let mut before = Vec::new();
        alerts(old, &mut Vec::new(), &mut before);
        let mut now = Vec::new();
        alerts(self, &mut Vec::new(), &mut now);
        now.into_iter()
            .filter(|(_, name)| {
                // Each alert there before matches one now, by name.
                match before.iter().position(|(_, b)| b == name) {
                    Some(i) => {
                        before.swap_remove(i);
                        false
                    }
                    None => true,
                }
            })
            .map(|(path, name)| (path, name.to_owned()))
            .collect()
    }
}

mod actions;
mod changes;
mod position;
mod relations;
mod reveal;
mod runs;
#[allow(unused_imports)]
pub(crate) use actions::{editable, number_text};
#[allow(unused_imports)]
pub(crate) use changes::{A11yChange, changes};
#[allow(unused_imports)]
pub(crate) use position::SetPosition;
#[allow(unused_imports)]
pub(crate) use relations::Relations;
pub use runs::{A11yRuns, A11yTextRun, RunSource};

/// The ARIA role the web's bridge gives `role`; empty for none (static text,
/// and the window root, which the page's overlay itself stands for).
pub(crate) fn aria_role(role: A11yRole) -> &'static str {
    match role {
        A11yRole::Window | A11yRole::Text => "",
        A11yRole::Group => "group",
        A11yRole::Button => "button",
        A11yRole::TextField => "textbox",
        A11yRole::List => "listbox",
        A11yRole::ListItem => "option",
        A11yRole::Table => "grid",
        A11yRole::Row => "row",
        A11yRole::Cell => "gridcell",
        A11yRole::ColumnHeader => "columnheader",
        A11yRole::Tree => "tree",
        A11yRole::TreeItem => "treeitem",
        A11yRole::MenuBar => "menubar",
        A11yRole::Menu => "menu",
        A11yRole::MenuItem => "menuitem",
        A11yRole::ComboBox => "combobox",
        A11yRole::SpinButton => "spinbutton",
        A11yRole::CheckBox => "checkbox",
        A11yRole::RadioButton => "radio",
        A11yRole::Switch => "switch",
        A11yRole::Slider => "slider",
        A11yRole::ProgressBar => "progressbar",
        A11yRole::Splitter | A11yRole::Separator => "separator",
        A11yRole::Toolbar => "toolbar",
        A11yRole::Alert => "alert",
        A11yRole::TabList => "tablist",
        A11yRole::Tab => "tab",
        // ARIA has no password role; the overlay's textbox stands for it.
        A11yRole::PasswordField => "textbox",
        A11yRole::Dialog => "alertdialog",
        A11yRole::ScrollBar => "scrollbar",
    }
}

/// The tree as the JSON `web/stipple.js` turns into its ARIA overlay: per
/// node `{"r": role, "n": name, "b": [x, y, w, h], "f": focused, "s":
/// selected, "e": expanded, "c": checked, "sc": [offset, max, sideways,
/// right to left], "sb": [value, max, sideways], "rv": [value, min, max],
/// "rt": value text, "tb": [rows, columns], "ce": [row, column], "lb":
/// labelled-by id (resolved by the caption's "i"), "k": [children]}`, with `r` an ARIA role (see
/// [`aria_role`]; an unnamed group gets none), the booleans as 0/1, and `s`,
/// `e`, `c`, `sc` (a scroll container's, in whole CSS pixels), `sb` (a
/// scroll bar's), `rv` and `rt` (a range's), `tb` (a table's), `ce` (a row's
/// or cell's, from 0), `id` (only on a node another labels itself by), `lb`
/// and `k` left out where they don't apply. Bounds are in whole CSS pixels.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn aria_json(node: &A11yNode) -> String {
    use std::fmt::Write;
    fn string(out: &mut String, s: &str) {
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                c if (c as u32) < 0x20 => {
                    let _ = write!(out, "\\u{:04x}", c as u32);
                }
                c => out.push(c),
            }
        }
        out.push('"');
    }
    fn node_json(out: &mut String, root: &A11yNode, path: &mut Vec<usize>, n: &A11yNode) {
        let role = match aria_role(n.role) {
            "group" if n.name.is_empty() => "",
            role => role,
        };
        out.push_str("{\"r\":");
        string(out, role);
        out.push_str(",\"n\":");
        string(out, &n.name);
        let (x, y, w, h) = n.bounds;
        let _ = write!(
            out,
            ",\"b\":[{},{},{},{}],\"f\":{}",
            x.round(),
            y.round(),
            w.round(),
            h.round(),
            u8::from(n.focused)
        );
        if matches!(
            n.role,
            A11yRole::ListItem | A11yRole::Row | A11yRole::Cell | A11yRole::TreeItem
        ) {
            let _ = write!(out, ",\"s\":{}", u8::from(n.selected));
        }
        if let Some(e) = n.expanded {
            let _ = write!(out, ",\"e\":{}", u8::from(e));
        }
        if let Some(p) = n.hint() {
            out.push_str(",\"p\":");
            string(out, p);
        }
        if let Some(c) = n.checked {
            let _ = write!(out, ",\"c\":{}", u8::from(c));
        }
        // What `aria-controls` names (by the target's id, its "i") and a
        // tree item's `aria-level`.
        if let Some(id) = &n.id {
            out.push_str(",\"i\":");
            string(out, id);
        }
        if let Some(id) = &n.controls {
            out.push_str(",\"ct\":");
            string(out, id);
        }
        if let (A11yRole::TreeItem, Some(level)) = (n.role, n.level) {
            let _ = write!(out, ",\"lv\":{level}");
        }
        // An item's place in its set (`aria-posinset`, `aria-setsize`).
        if let Some(p) = root.set_position(path) {
            let _ = write!(out, ",\"ps\":[{},{}]", p.position, p.size);
        }
        if let Some(s) = n.scroll {
            let _ = write!(
                out,
                ",\"sc\":[{},{},{},{}]",
                s.offset.round(),
                s.max.round(),
                u8::from(s.horizontal),
                u8::from(s.rtl)
            );
        }
        if let Some(s) = n.scroll_bar {
            let _ = write!(
                out,
                ",\"sb\":[{},{},{}]",
                s.offset.round(),
                s.max.round(),
                u8::from(s.horizontal)
            );
        }
        if let Some(r) = n.range {
            let _ = write!(out, ",\"rv\":[{},{},{}],\"rt\":", r.value, r.min, r.max);
            string(out, &r.text());
        }
        if let Some(t) = n.table {
            let _ = write!(out, ",\"tb\":[{},{}]", t.rows, t.columns);
        }
        if let Some(c) = n.cell {
            let _ = write!(out, ",\"ce\":[{},{}]", c.row, c.column);
        }
        if let Some(lb) = &n.labelled_by {
            out.push_str(",\"lb\":");
            string(out, lb);
        }
        if !n.children.is_empty() {
            out.push_str(",\"k\":[");
            for (i, c) in n.children.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                path.push(i);
                node_json(out, root, path, c);
                path.pop();
            }
            out.push(']');
        }
        out.push('}');
    }
    let mut out = String::new();
    node_json(&mut out, node, &mut Vec::new(), node);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(role: A11yRole, name: &str) -> A11yNode {
        A11yNode::new(role, name, (0.0, 0.0, 0.0, 0.0))
    }

    #[test]
    fn new_alerts_are_the_ones_to_announce() {
        let window = |children| A11yNode {
            children,
            ..leaf(A11yRole::Window, "App")
        };
        let saved = || leaf(A11yRole::Alert, "Saved");
        let first = window(vec![leaf(A11yRole::Button, "OK"), saved()]);
        assert!(first.alerts_new_since(None).is_empty(), "the first tree");
        assert!(first.alerts_new_since(Some(&first)).is_empty());
        let more = window(vec![
            saved(),
            leaf(A11yRole::Alert, "Deleted"),
            saved(),
            leaf(A11yRole::Alert, ""),
        ]);
        assert_eq!(more.alerts_new_since(Some(&first)), ["Deleted", "Saved"]);
        assert_eq!(
            more.alert_paths_new_since(Some(&first)),
            [
                (vec![1], "Deleted".to_string()),
                (vec![2], "Saved".to_string())
            ]
        );
        assert!(first.alerts_new_since(Some(&more)).is_empty(), "gone");
    }

    #[test]
    fn count_includes_all_descendants() {
        let root = A11yNode {
            children: vec![
                leaf(A11yRole::Text, "Title"),
                A11yNode {
                    children: vec![leaf(A11yRole::Button, "OK")],
                    ..leaf(A11yRole::Group, "")
                },
            ],
            ..A11yNode::new(A11yRole::Window, "", (0.0, 0.0, 100.0, 100.0))
        };
        // root + Title + Group + OK = 4
        assert_eq!(root.count(), 4);
    }

    #[test]
    fn focus_moves_only_when_another_node_takes_it() {
        let tree = |focus: Option<usize>, label: &str| A11yNode {
            children: vec![
                A11yNode {
                    focused: focus == Some(0),
                    ..leaf(A11yRole::TextField, "")
                },
                leaf(A11yRole::Text, label),
                A11yNode {
                    children: vec![A11yNode {
                        focused: focus == Some(1),
                        ..leaf(A11yRole::Button, "Row 0")
                    }],
                    ..leaf(A11yRole::Group, "")
                },
            ],
            ..leaf(A11yRole::Window, "")
        };
        let none = tree(None, "Rows tapped: 0");
        let field = tree(Some(0), "Rows tapped: 0");
        let button = tree(Some(1), "Rows tapped: 0");
        assert_eq!(none.focused_path(), None);
        assert_eq!(button.focused_path(), Some(vec![2, 0]));
        assert_eq!(
            button.at_path(&[2, 0]).map(|n| n.name.as_str()),
            Some("Row 0")
        );
        assert!(button.at_path(&[2, 1]).is_none());
        // Nothing focused: nothing to report.
        assert_eq!(none.focus_moved_from(Some(&field)), None);
        // The first tree, focus arriving, focus moving on.
        assert_eq!(field.focus_moved_from(None), Some(vec![0]));
        assert_eq!(field.focus_moved_from(Some(&none)), Some(vec![0]));
        assert_eq!(button.focus_moved_from(Some(&field)), Some(vec![2, 0]));
        // A tree rebuilt around the same focus (another label) raises none.
        let relabelled = tree(Some(1), "Rows tapped: 1");
        assert_eq!(relabelled.focus_moved_from(Some(&button)), None);
        // Another kind of node in the focused node's place is a new focus.
        let mut swapped = button.clone();
        swapped.children[2].children[0].role = A11yRole::CheckBox;
        assert_eq!(swapped.focus_moved_from(Some(&button)), Some(vec![2, 0]));
    }

    #[test]
    fn a_placeholder_is_the_hint_unless_empty() {
        let field = |p: Option<&str>| A11yNode {
            placeholder: p.map(String::from),
            ..leaf(A11yRole::TextField, "")
        };
        assert_eq!(field(Some("Your name")).hint(), Some("Your name"));
        assert_eq!(field(Some("")).hint(), None);
        assert_eq!(field(None).hint(), None);
    }

    #[test]
    fn aria_json_carries_ranges_tables_and_labels() {
        let root = A11yNode {
            children: vec![
                A11yNode {
                    id: Some("vol".into()),
                    ..leaf(A11yRole::Text, "Volume")
                },
                A11yNode {
                    id: Some("unused".into()),
                    labelled_by: Some("vol".into()),
                    range: Some(A11yRange {
                        value: 0.5,
                        min: 0.0,
                        max: 1.0,
                        step: 0.05,
                        rtl: false,
                    }),
                    ..leaf(A11yRole::Slider, "")
                },
                A11yNode {
                    table: Some(A11yTable {
                        rows: 9,
                        columns: 2,
                        frozen: 0,
                    }),
                    children: vec![A11yNode {
                        cell: Some(A11yCell { row: 3, column: 1 }),
                        ..leaf(A11yRole::Cell, "x")
                    }],
                    ..leaf(A11yRole::Table, "")
                },
            ],
            ..A11yNode::new(A11yRole::Window, "", (0.0, 0.0, 0.0, 0.0))
        };
        assert_eq!(
            aria_json(&root),
            concat!(
                r#"{"r":"","n":"","b":[0,0,0,0],"f":0,"k":["#,
                r#"{"r":"","n":"Volume","b":[0,0,0,0],"f":0,"i":"vol"},"#,
                r#"{"r":"slider","n":"","b":[0,0,0,0],"f":0,"i":"unused","rv":[0.5,0,1],"rt":"50%","lb":"vol"},"#,
                r#"{"r":"grid","n":"","b":[0,0,0,0],"f":0,"tb":[9,2],"k":["#,
                r#"{"r":"gridcell","n":"x","b":[0,0,0,0],"f":0,"s":0,"ce":[3,1]}]}]}"#,
            )
        );
    }

    #[test]
    fn aria_json_names_roles_and_state() {
        let root = A11yNode {
            children: vec![
                leaf(A11yRole::Text, "Say \"hi\"\\\n"),
                A11yNode {
                    focused: true,
                    placeholder: Some("Name".into()),
                    ..A11yNode::new(A11yRole::TextField, "abc", (1.4, 2.6, 30.0, 10.0))
                },
                A11yNode {
                    checked: Some(true),
                    ..leaf(A11yRole::CheckBox, "Bold")
                },
                A11yNode {
                    selected: true,
                    expanded: Some(false),
                    level: Some(2),
                    ..leaf(A11yRole::TreeItem, "src")
                },
                A11yNode {
                    controls: Some("menu".into()),
                    ..leaf(A11yRole::ComboBox, "Pick")
                },
                A11yNode {
                    id: Some("menu".into()),
                    ..leaf(A11yRole::List, "")
                },
                A11yNode {
                    scroll: Some(A11yScroll {
                        offset: 12.4,
                        max: 300.0,
                        viewport: 50.0,
                        horizontal: false,
                        rtl: false,
                    }),
                    ..leaf(A11yRole::Group, "")
                },
            ],
            ..A11yNode::new(A11yRole::Window, "", (0.0, 0.0, 100.0, 50.0))
        };
        assert_eq!(
            aria_json(&root),
            concat!(
                r#"{"r":"","n":"","b":[0,0,100,50],"f":0,"k":["#,
                r#"{"r":"","n":"Say \"hi\"\\\u000a","b":[0,0,0,0],"f":0},"#,
                r#"{"r":"textbox","n":"abc","b":[1,3,30,10],"f":1,"p":"Name"},"#,
                r#"{"r":"checkbox","n":"Bold","b":[0,0,0,0],"f":0,"c":1},"#,
                r#"{"r":"treeitem","n":"src","b":[0,0,0,0],"f":0,"s":1,"e":0,"lv":2,"ps":[1,1]},"#,
                r#"{"r":"combobox","n":"Pick","b":[0,0,0,0],"f":0,"ct":"menu"},"#,
                r#"{"r":"listbox","n":"","b":[0,0,0,0],"f":0,"i":"menu"},"#,
                r#"{"r":"","n":"","b":[0,0,0,0],"f":0,"sc":[12,300,0,0]}]}"#,
            )
        );
    }
}
