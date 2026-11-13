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
    /// An on/off switch ([`A11yNode::checked`] says whether it is on).
    Switch,
    /// A draggable divider between two panes.
    Splitter,
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
    pub children: Vec<A11yNode>,
}

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
            children: Vec::new(),
        }
    }

    /// Total node count (including this one), for tests/inspection.
    pub fn count(&self) -> usize {
        1 + self.children.iter().map(A11yNode::count).sum::<usize>()
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
}

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
        A11yRole::Switch => "switch",
        A11yRole::Splitter | A11yRole::Separator => "separator",
        A11yRole::Toolbar => "toolbar",
        A11yRole::Alert => "alert",
        A11yRole::TabList => "tablist",
        A11yRole::Tab => "tab",
        // ARIA has no password role; the overlay's textbox stands for it.
        A11yRole::PasswordField => "textbox",
        A11yRole::Dialog => "alertdialog",
    }
}

/// The tree as the JSON `web/stipple.js` turns into its ARIA overlay: per
/// node `{"r": role, "n": name, "b": [x, y, w, h], "f": focused, "s":
/// selected, "e": expanded, "c": checked, "k": [children]}`, with `r` an ARIA
/// role (see [`aria_role`]; an unnamed group gets none), the booleans as 0/1,
/// and `s`, `e`, `c` and `k` left out where they don't apply. Bounds are in
/// whole CSS pixels.
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
    fn node_json(out: &mut String, n: &A11yNode) {
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
        if let Some(c) = n.checked {
            let _ = write!(out, ",\"c\":{}", u8::from(c));
        }
        if !n.children.is_empty() {
            out.push_str(",\"k\":[");
            for (i, c) in n.children.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                node_json(out, c);
            }
            out.push(']');
        }
        out.push('}');
    }
    let mut out = String::new();
    node_json(&mut out, node);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(role: A11yRole, name: &str) -> A11yNode {
        A11yNode::new(role, name, (0.0, 0.0, 0.0, 0.0))
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
    fn aria_json_names_roles_and_state() {
        let root = A11yNode {
            children: vec![
                leaf(A11yRole::Text, "Say \"hi\"\\\n"),
                A11yNode {
                    focused: true,
                    ..A11yNode::new(A11yRole::TextField, "abc", (1.4, 2.6, 30.0, 10.0))
                },
                A11yNode {
                    checked: Some(true),
                    ..leaf(A11yRole::CheckBox, "Bold")
                },
                A11yNode {
                    selected: true,
                    expanded: Some(false),
                    ..leaf(A11yRole::TreeItem, "src")
                },
                leaf(A11yRole::Group, ""),
            ],
            ..A11yNode::new(A11yRole::Window, "", (0.0, 0.0, 100.0, 50.0))
        };
        assert_eq!(
            aria_json(&root),
            concat!(
                r#"{"r":"","n":"","b":[0,0,100,50],"f":0,"k":["#,
                r#"{"r":"","n":"Say \"hi\"\\\u000a","b":[0,0,0,0],"f":0},"#,
                r#"{"r":"textbox","n":"abc","b":[1,3,30,10],"f":1},"#,
                r#"{"r":"checkbox","n":"Bold","b":[0,0,0,0],"f":0,"c":1},"#,
                r#"{"r":"treeitem","n":"src","b":[0,0,0,0],"f":0,"s":1,"e":0},"#,
                r#"{"r":"","n":"","b":[0,0,0,0],"f":0}]}"#,
            )
        );
    }
}
