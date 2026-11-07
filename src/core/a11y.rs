//! Accessibility tree: a semantic view of the UI for assistive technologies.
//!
//! [`accessibility_tree`] walks the retained [`LayoutNode`] tree and produces a
//! pruned [`AccessNode`] tree — the platform-neutral abstraction a future
//! AT-SPI (Linux), UI Automation (Windows), or `NSAccessibility` (macOS) backend
//! would expose to screen readers. Each node carries a [`Role`], an accessible
//! `name`, its `bounds`, and whether it currently holds focus.
//!
//! Roles are inferred from how an element behaves: a tap handler makes it a
//! [`Role::Button`], a keyboard/focus handle a [`Role::TextField`], bare text a
//! [`Role::Text`]; the rest are [`Role::Group`] containers (with the tree root
//! reported as the [`Role::Window`]). Interactive nodes absorb their text
//! descendants as their `name`, and purely decorative containers (no name, no
//! children) are dropped, so the tree stays small and meaningful.
//!
//! Widgets whose semantics can't be inferred (lists, tables, trees and their
//! rows and cells) declare them with [`Element::role`](crate::core::Element::role),
//! [`selected`](crate::core::Element::selected) and
//! [`expanded`](crate::core::Element::expanded); a declared role wins over the
//! inferred one.

use crate::core::runtime::{FocusId, LayoutNode, NodeContent, first_text};
use crate::geometry::Rect;

/// The semantic kind of an [`AccessNode`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The top-level window (tree root).
    Window,
    /// A non-interactive container.
    Group,
    /// An activatable control (has a tap handler).
    Button,
    /// An editable text field (focusable / keyboard target).
    TextField,
    /// A run of static text.
    Text,
    /// A list of selectable items.
    List,
    /// One item of a [`Role::List`].
    ListItem,
    /// A table / data grid: rows of cells under column headers.
    Table,
    /// One row of a [`Role::Table`], holding [`Role::Cell`]s (or headers).
    Row,
    /// One cell of a table row.
    Cell,
    /// A column header of a table (often clickable to sort).
    ColumnHeader,
    /// A tree of expandable items.
    Tree,
    /// One item of a [`Role::Tree`].
    TreeItem,
}

/// Semantics an element declares for assistive technologies, overriding the
/// inferred role. Set via [`Element::role`](crate::core::Element::role) and
/// friends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Semantics {
    pub role: Role,
    /// Whether this item is selected (list items, rows, tree items).
    pub selected: bool,
    /// For an expandable item, whether it is expanded; `None` if it can't
    /// expand.
    pub expanded: Option<bool>,
}

impl Semantics {
    /// Semantics with `role`, not selected, not expandable.
    pub fn new(role: Role) -> Self {
        Self {
            role,
            selected: false,
            expanded: None,
        }
    }
}

/// A node in the accessibility tree.
#[derive(Clone, Debug, PartialEq)]
pub struct AccessNode {
    pub role: Role,
    /// The accessible name (a control's label, or the text itself).
    pub name: String,
    pub bounds: Rect,
    /// Whether this node currently holds keyboard focus.
    pub focused: bool,
    /// Whether this item is selected (see [`Semantics::selected`]).
    pub selected: bool,
    /// Whether this item is expanded, if it can expand.
    pub expanded: Option<bool>,
    pub children: Vec<AccessNode>,
}

/// The accessible text at or under `node` (a control's label / a field's value).
fn text_of(node: &LayoutNode) -> String {
    let found = if node.semantics.is_some() {
        own_text(node)
    } else {
        first_text(node)
    };
    match found.map(|n| &n.content) {
        Some(NodeContent::Text { text, .. }) => text.clone(),
        _ => String::new(),
    }
}

/// The first text under `node`, skipping descendants that are controls of their
/// own (a tree item's disclosure toggle, a checkbox in a row), so an item is
/// named by its label rather than by a glyph.
fn own_text(node: &LayoutNode) -> Option<&LayoutNode> {
    if matches!(node.content, NodeContent::Text { .. }) {
        return Some(node);
    }
    node.children
        .iter()
        .filter(|c| c.action.is_none())
        .find_map(own_text)
}

/// Infer a node's role from its interaction handles and content.
fn role_of(node: &LayoutNode) -> Role {
    if let Some(sem) = node.semantics {
        sem.role
    } else if node.action.is_some() {
        Role::Button
    } else if node.focus.is_some() {
        Role::TextField
    } else if matches!(node.content, NodeContent::Text { .. }) {
        Role::Text
    } else {
        Role::Group
    }
}

fn build(node: &LayoutNode, focused: Option<FocusId>, is_root: bool) -> AccessNode {
    let role = if is_root { Role::Window } else { role_of(node) };
    // Interactive/text roles take their name from their text and absorb their
    // descendants; containers recurse and expose their meaningful children.
    let (name, children) = match role {
        Role::Button
        | Role::TextField
        | Role::Text
        | Role::ListItem
        | Role::Cell
        | Role::ColumnHeader
        | Role::TreeItem => (text_of(node), Vec::new()),
        Role::Window | Role::Group | Role::List | Role::Table | Role::Row | Role::Tree => (
            String::new(),
            node.children
                .iter()
                .map(|c| build(c, focused, false))
                .filter(|a| !a.is_prunable())
                .collect(),
        ),
    };
    AccessNode {
        role,
        name,
        bounds: node.bounds,
        focused: node.focus.is_some() && node.focus == focused,
        selected: node.semantics.is_some_and(|s| s.selected),
        expanded: node.semantics.and_then(|s| s.expanded),
        children,
    }
}

impl AccessNode {
    /// A container with no name and no children carries no semantics, so it can
    /// be dropped from the tree.
    fn is_prunable(&self) -> bool {
        self.role == Role::Group && self.name.is_empty() && self.children.is_empty()
    }

    /// Total node count (including this one), for tests/inspection.
    pub fn count(&self) -> usize {
        1 + self.children.iter().map(AccessNode::count).sum::<usize>()
    }

    /// Depth-first iterator visiting this node and all descendants.
    pub fn descendants(&self) -> Vec<&AccessNode> {
        let mut out = vec![self];
        for c in &self.children {
            out.extend(c.descendants());
        }
        out
    }
}

/// Build the accessibility tree for a laid-out UI, marking the node that holds
/// `focused` (if any). The root is reported as a [`Role::Window`].
pub fn accessibility_tree(root: &LayoutNode, focused: Option<FocusId>) -> AccessNode {
    build(root, focused, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Align, Axis, BoxStyle, Cx, Element, layout};
    use crate::render::Color;
    use crate::style::Theme;

    /// Lay a small interactive view out and return its accessibility tree.
    fn tree(focused: Option<FocusId>) -> AccessNode {
        let theme = Theme::light();
        let mut cx = Cx::<()>::new(&theme);
        let field = Element::stack(
            Axis::Horizontal,
            vec![Element::text("hello", 14.0, Color::BLACK)],
        )
        .on_key(&mut cx, |_: &mut (), _| {});
        let button = Element::stack(
            Axis::Horizontal,
            vec![Element::text("OK", 14.0, Color::BLACK)],
        )
        .on_tap(&mut cx, |_: &mut ()| {});
        let root = Element::stack(
            Axis::Vertical,
            vec![Element::text("Title", 18.0, Color::BLACK), field, button],
        )
        .align(Align::Start, Align::Stretch)
        .fill(Color::WHITE)
        .padding(crate::geometry::Insets::uniform(4.0));
        let laid = layout(&root, Rect::from_xywh(0.0, 0.0, 200.0, 120.0), None);
        accessibility_tree(&laid, focused)
    }

    #[test]
    fn derives_roles_and_names() {
        let a = tree(None);
        assert_eq!(a.role, Role::Window);
        // The root exposes: a Title (Text), the field (TextField), the button.
        let roles: Vec<Role> = a.children.iter().map(|c| c.role).collect();
        assert_eq!(roles, vec![Role::Text, Role::TextField, Role::Button]);
        assert_eq!(a.children[0].name, "Title");
        assert_eq!(a.children[1].name, "hello"); // field value
        assert_eq!(a.children[2].name, "OK"); // button label
        // Interactive nodes absorbed their text children.
        assert!(a.children[1].children.is_empty());
        assert!(a.children[2].children.is_empty());
    }

    #[test]
    fn marks_the_focused_field() {
        // Focus the text field (the only focusable, id 0).
        let a = tree(Some(FocusId(0)));
        let field = a
            .descendants()
            .into_iter()
            .find(|n| n.role == Role::TextField)
            .unwrap();
        assert!(field.focused);
        // Nothing else is focused.
        assert_eq!(a.descendants().iter().filter(|n| n.focused).count(), 1);
    }

    #[test]
    fn prunes_decorative_containers() {
        // A panel wrapping an empty decorative box plus a label.
        let root = Element::stack(
            Axis::Vertical,
            vec![
                Element::boxed(BoxStyle::default()).width(10.0).height(10.0), // decorative
                Element::text("Label", 14.0, Color::BLACK),
            ],
        );
        let laid = layout(&root, Rect::from_xywh(0.0, 0.0, 100.0, 100.0), None);
        let a = accessibility_tree(&laid, None);
        // Only the text survives; the empty box is pruned.
        assert_eq!(a.children.len(), 1);
        assert_eq!(a.children[0].role, Role::Text);
    }
}
