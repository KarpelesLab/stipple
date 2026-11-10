//! The accessibility tree as iOS's VoiceOver gets it, kept free of UIKit so it
//! is unit-tested on every host: one item per node of the tree (the root, the
//! view itself, aside), in reading order, each with what its
//! `UIAccessibilityElement` says, and which notification an update needs.

use crate::platform::access::{A11yNode, A11yRole};

/// `UIAccessibilityTraitButton`, `…StaticText`, `…Header` and `…Selected`,
/// as bits the backend maps to UIKit's constants.
pub(crate) const BUTTON: u8 = 1;
pub(crate) const STATIC_TEXT: u8 = 2;
pub(crate) const HEADER: u8 = 4;
pub(crate) const SELECTED: u8 = 8;

/// One node, as its accessibility element shows it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Item {
    pub(crate) role: A11yRole,
    /// Whether VoiceOver stops on it: containers (a group, a list, a
    /// toolbar…) and separators don't, their children do.
    pub(crate) is_element: bool,
    pub(crate) label: String,
    pub(crate) value: String,
    pub(crate) traits: u8,
    /// `(x, y, width, height)` in the view's coordinates (logical pixels).
    pub(crate) frame: (f64, f64, f64, f64),
}

/// The roles whose children VoiceOver reads instead of them (as
/// `core::a11y`'s containers).
fn is_container(role: A11yRole) -> bool {
    use A11yRole::*;
    matches!(
        role,
        Window
            | Group
            | List
            | Table
            | Row
            | Tree
            | Toolbar
            | Alert
            | MenuBar
            | Menu
            | TabList
            | Dialog
    )
}

fn item(node: &A11yNode) -> Item {
    use A11yRole::*;
    let role = node.role;
    // A text field's name is its text: that's the element's value.
    let field = matches!(role, TextField | SpinButton | PasswordField);
    let (label, value) = if field {
        (String::new(), node.name.clone())
    } else {
        let value = match (role, node.checked, node.expanded) {
            (Switch, Some(on), _) => if on { "on" } else { "off" }.to_string(),
            (_, Some(checked), _) => if checked { "checked" } else { "unchecked" }.to_string(),
            (_, None, Some(open)) => if open { "expanded" } else { "collapsed" }.to_string(),
            _ => String::new(),
        };
        (node.name.clone(), value)
    };
    let mut traits = match role {
        Button | MenuItem | ComboBox | CheckBox | Switch | Tab => BUTTON,
        Text => STATIC_TEXT,
        ColumnHeader => HEADER,
        _ => 0,
    };
    if node.selected && matches!(role, ListItem | Row | Cell | TreeItem | Tab) {
        traits |= SELECTED;
    }
    let is_element = match role {
        Separator => false,
        _ if is_container(role) => node.children.is_empty() && !node.name.is_empty(),
        Text => !node.name.is_empty(),
        _ => true,
    };
    Item {
        role,
        is_element,
        label,
        value,
        traits,
        frame: node.bounds,
    }
}

/// Every node under `root` (not `root`, which the view stands for), depth
/// first in reading order.
pub(crate) fn flatten(root: &A11yNode) -> Vec<Item> {
    fn walk(node: &A11yNode, out: &mut Vec<Item>) {
        for child in &node.children {
            out.push(item(child));
            walk(child, out);
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out
}

/// What VoiceOver must hear of an update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Change {
    /// Nothing, or only frames moved (scrolling): the elements carry the new
    /// frames when asked.
    None,
    /// Elements came, went or changed: `UIAccessibilityLayoutChangedNotification`.
    Layout,
    /// A different screen: the first tree, or a dialog opened or closed.
    /// `UIAccessibilityScreenChangedNotification`.
    Screen,
}

pub(crate) fn change(old: &[Item], new: &[Item]) -> Change {
    let dialogs = |items: &[Item]| {
        items
            .iter()
            .filter(|i| matches!(i.role, A11yRole::Dialog | A11yRole::Alert))
            .count()
    };
    if old.is_empty() && !new.is_empty() || dialogs(old) != dialogs(new) {
        return Change::Screen;
    }
    let same = old.len() == new.len()
        && old.iter().zip(new).all(|(a, b)| {
            a.role == b.role
                && a.is_element == b.is_element
                && a.label == b.label
                && a.value == b.value
                && a.traits == b.traits
        });
    if same { Change::None } else { Change::Layout }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(role: A11yRole, name: &str) -> A11yNode {
        A11yNode::new(role, name, (1.0, 2.0, 30.0, 10.0))
    }

    fn window(children: Vec<A11yNode>) -> A11yNode {
        A11yNode {
            children,
            ..node(A11yRole::Window, "App")
        }
    }

    #[test]
    fn one_item_per_node_with_label_value_and_traits() {
        let tree = window(vec![
            node(A11yRole::TextField, "hello"),
            A11yNode {
                children: vec![
                    A11yNode {
                        selected: true,
                        ..node(A11yRole::ListItem, "One")
                    },
                    node(A11yRole::Button, "Two"),
                ],
                ..node(A11yRole::List, "")
            },
            A11yNode {
                checked: Some(true),
                ..node(A11yRole::Switch, "Wi-Fi")
            },
            node(A11yRole::Separator, ""),
        ]);
        let items = flatten(&tree);
        let summary: Vec<_> = items
            .iter()
            .map(|i| (i.is_element, i.label.as_str(), i.value.as_str(), i.traits))
            .collect();
        assert_eq!(
            summary,
            [
                (true, "", "hello", 0),
                (false, "", "", 0),
                (true, "One", "", SELECTED),
                (true, "Two", "", BUTTON),
                (true, "Wi-Fi", "on", BUTTON),
                (false, "", "", 0),
            ]
        );
        assert_eq!(items[0].frame, (1.0, 2.0, 30.0, 10.0));
    }

    #[test]
    fn updates_post_the_notification_they_need() {
        let first = flatten(&window(vec![node(A11yRole::Button, "OK")]));
        assert_eq!(change(&[], &first), Change::Screen);
        assert_eq!(change(&first, &first), Change::None);
        // Scrolling moves frames only.
        let moved = flatten(&window(vec![A11yNode::new(
            A11yRole::Button,
            "OK",
            (5.0, 50.0, 30.0, 10.0),
        )]));
        assert_eq!(change(&first, &moved), Change::None);
        let renamed = flatten(&window(vec![node(A11yRole::Button, "Done")]));
        assert_eq!(change(&first, &renamed), Change::Layout);
        let dialog = flatten(&window(vec![
            node(A11yRole::Button, "OK"),
            A11yNode {
                children: vec![node(A11yRole::Button, "Yes")],
                ..node(A11yRole::Dialog, "Sure?")
            },
        ]));
        assert_eq!(change(&first, &dialog), Change::Screen);
    }
}
