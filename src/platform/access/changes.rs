//! What changed between two trees, node by node at the same place (the same
//! child index at every level, as the bridges' element ids are): what UIA's
//! property-changed and structure-changed events, and NSAccessibility's
//! notifications, report. AT-SPI's bridge makes its own events from the same
//! walk (`atspi::events`), with the text inserted and deleted as well.

// Used by the bridges that report changes this way (UIA, macOS), and tests.
#![cfg_attr(not(any(target_os = "windows", target_os = "macos")), allow(dead_code))]

use super::{A11yNode, A11yRole};

/// One change, at the node `path` names (child indices from the root).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum A11yChange {
    /// Its name changed (for a field whose name is its text, its value too).
    Name(Vec<usize>),
    /// Its value changed: a field's text, a spin button's number.
    Value(Vec<usize>),
    /// A checkbox, radio button or switch was checked or unchecked.
    Checked(Vec<usize>),
    /// It was expanded or collapsed.
    Expanded(Vec<usize>),
    /// It was selected or deselected (an item, a row, a tab).
    Selected(Vec<usize>),
    /// A field's caret or text selection moved.
    TextSelection(Vec<usize>),
    /// It scrolled, or its content's length changed.
    Scrolled(Vec<usize>),
    /// Its children changed: some added or removed, or of other roles.
    Children(Vec<usize>),
}

/// Whether a node of `role` has a value: its name is its text.
fn has_value(n: &A11yNode) -> bool {
    matches!(
        n.role,
        A11yRole::TextField | A11yRole::SpinButton | A11yRole::PasswordField
    )
}

/// The changes going from `old` to `new`, in tree order (a node's own before
/// its descendants'). A node whose children changed (by number or role) is
/// reported once as [`A11yChange::Children`], and only the children both
/// trees have in common are compared further.
pub(crate) fn changes(old: &A11yNode, new: &A11yNode) -> Vec<A11yChange> {
    let mut out = Vec::new();
    walk(old, new, &mut Vec::new(), &mut out);
    out
}

fn walk(old: &A11yNode, new: &A11yNode, path: &mut Vec<usize>, out: &mut Vec<A11yChange>) {
    let here = || path.clone();
    if old.name != new.name {
        out.push(A11yChange::Name(here()));
        if has_value(new) {
            out.push(A11yChange::Value(here()));
        }
    }
    if old.checked != new.checked {
        out.push(A11yChange::Checked(here()));
    }
    if old.expanded != new.expanded {
        out.push(A11yChange::Expanded(here()));
    }
    if old.selected != new.selected {
        out.push(A11yChange::Selected(here()));
    }
    if (old.caret, old.text_selection) != (new.caret, new.text_selection) && has_value(new) {
        out.push(A11yChange::TextSelection(here()));
    }
    let moved = match (old.scroll, new.scroll) {
        (Some(a), Some(b)) => (a.offset - b.offset).abs() > 0.5 || (a.max - b.max).abs() > 0.5,
        (a, b) => a.is_some() != b.is_some(),
    };
    if moved {
        out.push(A11yChange::Scrolled(here()));
    }
    let reshaped = old.children.len() != new.children.len()
        || old
            .children
            .iter()
            .zip(&new.children)
            .any(|(a, b)| a.role != b.role);
    if reshaped {
        out.push(A11yChange::Children(here()));
    }
    for (i, (a, b)) in old.children.iter().zip(&new.children).enumerate() {
        if a.role == b.role {
            path.push(i);
            walk(a, b, path, out);
            path.pop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::access::A11yScroll;

    fn leaf(role: A11yRole, name: &str) -> A11yNode {
        A11yNode::new(role, name, (0.0, 0.0, 10.0, 10.0))
    }

    fn window(children: Vec<A11yNode>) -> A11yNode {
        A11yNode {
            children,
            ..leaf(A11yRole::Window, "")
        }
    }

    #[test]
    fn reports_each_kind_of_change_where_it_happened() {
        let field = |text: &str, caret| A11yNode {
            caret: Some(caret),
            ..leaf(A11yRole::TextField, text)
        };
        let check = |on| A11yNode {
            checked: Some(on),
            ..leaf(A11yRole::CheckBox, "Bold")
        };
        let list = |offset, selected| A11yNode {
            scroll: Some(A11yScroll {
                offset,
                max: 100.0,
                viewport: 50.0,
                horizontal: false,
                rtl: false,
            }),
            children: vec![A11yNode {
                selected,
                expanded: Some(selected),
                ..leaf(A11yRole::TreeItem, "src")
            }],
            ..leaf(A11yRole::Tree, "")
        };
        let old = window(vec![field("ab", 2), check(false), list(0.0, false)]);
        assert!(changes(&old, &old).is_empty());
        let new = window(vec![field("abc", 3), check(true), list(30.0, true)]);
        assert_eq!(
            changes(&old, &new),
            vec![
                A11yChange::Name(vec![0]),
                A11yChange::Value(vec![0]),
                A11yChange::TextSelection(vec![0]),
                A11yChange::Checked(vec![1]),
                A11yChange::Scrolled(vec![2]),
                A11yChange::Expanded(vec![2, 0]),
                A11yChange::Selected(vec![2, 0]),
            ]
        );
    }

    #[test]
    fn children_added_removed_or_replaced_reshape_the_parent() {
        let old = window(vec![leaf(A11yRole::Text, "a"), leaf(A11yRole::Button, "b")]);
        let fewer = window(vec![leaf(A11yRole::Text, "a")]);
        assert_eq!(changes(&old, &fewer), vec![A11yChange::Children(vec![])]);
        // Another role in a child's place: the parent, and not the child.
        let other = window(vec![leaf(A11yRole::Text, "a"), leaf(A11yRole::Alert, "c")]);
        assert_eq!(changes(&old, &other), vec![A11yChange::Children(vec![])]);
        // A label's new text is a name change, not a value.
        let renamed = window(vec![leaf(A11yRole::Text, "z"), leaf(A11yRole::Button, "b")]);
        assert_eq!(changes(&old, &renamed), vec![A11yChange::Name(vec![0])]);
    }
}
