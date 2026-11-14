//! Keyboard focus for the controls a tap activates: buttons, checkboxes,
//! switches, tabs, selects, list and tree items, table rows, cells and column
//! headers.
//!
//! Once the tree is laid out, [`promote_tap_focus`] gives a node with a tap
//! handler ([`Element::on_tap`](crate::core::Element::on_tap)) a focus made
//! from it ([`FocusId::tap`]) when the node's role (declared, or
//! [`Role::Button`] as a tap handler implies) is one of those controls, it
//! has no keyboard handler of its own, and it isn't inside
//! another focusable element (a password field's reveal toggle stays part of
//! the field; a tab's close button part of the tab). Such a node then joins
//! the Tab order, in tree order, gets the focus ring and is reported focused
//! to screen readers, and Enter or Space presses it ([`tap_activation`]).
//!
//! A click doesn't focus it ([`focus_at`](crate::core::focus_at) passes it
//! over, as AppKit does for buttons and the web's `:focus-visible` hides the
//! ring after a click), so pointer users never see a stray ring: clicking a
//! button moves focus away from a field just as before.
//!
//! Menu items aren't Tab stops: a menu takes the arrow keys, Enter and Escape
//! itself (see `docs/keyboard.md`).

use crate::core::a11y::Role;
use crate::core::runtime::{ActionId, FocusId, KeyInput, LayoutNode, node_to_window};
use crate::geometry::Point;

/// How far a button's focus ring stands off its edge, in logical pixels.
pub const RING_OFFSET: f64 = 2.0;

/// Whether a tappable node is one of the controls that take keyboard focus.
fn takes_tab(node: &LayoutNode) -> bool {
    let role = node
        .semantics
        .as_ref()
        .and_then(|s| s.role)
        .unwrap_or(Role::Button);
    matches!(
        role,
        Role::Button
            | Role::CheckBox
            | Role::Switch
            | Role::Tab
            | Role::ComboBox
            | Role::ListItem
            | Role::TreeItem
            | Role::Row
            | Role::Cell
            | Role::ColumnHeader
    )
}

/// Give the controls a tap activates in `tree` keyboard focus, so Tab
/// reaches them (see the module docs). The tap handlers in `skip` (the
/// overlays' press-outside catchers) don't take it.
pub fn promote_tap_focus(tree: &mut LayoutNode, skip: &[ActionId]) {
    fn walk(node: &mut LayoutNode, inside: bool, skip: &[ActionId]) {
        if !inside
            && !node.ghost
            && node.focus.is_none()
            && let Some(action) = node.action
            && !skip.contains(&action)
            && takes_tab(node)
        {
            node.focus = Some(FocusId::tap(action));
        }
        let inside = inside || node.focus.is_some();
        for child in &mut node.children {
            walk(child, inside, skip);
        }
    }
    walk(tree, false, skip);
}

/// Whether `node`'s focus is a button's (see [`FocusId::tap`]), not a
/// keyboard handler's.
pub fn is_tap_focus(node: &LayoutNode) -> bool {
    node.focus.is_some_and(FocusId::is_tap)
}

/// Whether `input` presses a focused button: Enter, or Space (which the
/// backends deliver as typed text).
pub fn activates(input: &KeyInput) -> bool {
    match input {
        KeyInput::Enter => true,
        KeyInput::Text(t) => t == " ",
        _ => false,
    }
}

/// The tap a key press on the focused element runs when that focus is a
/// button's (see [`is_tap_focus`]): its tap handler, and the middle of the
/// node in window coordinates (where the tap is said to be).
pub fn tap_activation(tree: &LayoutNode, focused: FocusId) -> Option<(ActionId, Point)> {
    let (node, to_window, _) = node_to_window(tree, &|n| n.focus == Some(focused))?;
    if !is_tap_focus(node) {
        return None;
    }
    Some((node.action?, to_window.apply(node.bounds.center())))
}

/// The Tab order: every focusable in tree order. With a modal overlay up,
/// `modal_layers` is how many of the root's layers (not counting overlays
/// leaving) it and the overlays above it take, at the end: only those count,
/// so Tab stays in the dialog.
pub fn tab_order(tree: &LayoutNode, modal_layers: Option<usize>) -> Vec<FocusId> {
    let mut order = Vec::new();
    match modal_layers {
        Some(n) => {
            let layers: Vec<&LayoutNode> = tree.children.iter().filter(|l| !l.ghost).collect();
            for layer in &layers[layers.len().saturating_sub(n)..] {
                crate::core::collect_focusables(layer, &mut order);
            }
        }
        None => crate::core::collect_focusables(tree, &mut order),
    }
    order
}

/// The focusable after `current` in `order` (before it, `back`), wrapping;
/// the first (last) when nothing in `order` has focus.
pub fn step(order: &[FocusId], current: Option<FocusId>, back: bool) -> Option<FocusId> {
    let n = order.len();
    if n == 0 {
        return None;
    }
    let at = current.and_then(|c| order.iter().position(|f| *f == c));
    Some(match (at, back) {
        (Some(i), false) => order[(i + 1) % n],
        (Some(i), true) => order[(i + n - 1) % n],
        (None, false) => order[0],
        (None, true) => order[n - 1],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Cx, Element, layout};
    use crate::geometry::Rect;
    use crate::layout::Axis;
    use crate::style::Theme;

    fn tile() -> Element {
        Element::stack(Axis::Vertical, vec![])
            .width(50.0)
            .height(20.0)
    }

    fn laid(build: impl FnOnce(&mut Cx<()>) -> Element) -> LayoutNode {
        let theme = Theme::light();
        let mut cx = Cx::new(&theme);
        let el = build(&mut cx);
        let mut tree = layout(&el, Rect::from_xywh(0.0, 0.0, 200.0, 200.0), None);
        promote_tap_focus(&mut tree, &[]);
        tree
    }

    #[test]
    fn buttons_and_toggles_take_focus_but_not_groups_menu_items_or_parts_of_a_field() {
        let tree = laid(|cx| {
            Element::stack(
                Axis::Vertical,
                vec![
                    tile().on_tap(cx, |_| {}),
                    tile().on_tap(cx, |_| {}).role(Role::CheckBox),
                    tile().on_tap(cx, |_| {}).role(Role::MenuItem),
                    tile().on_tap(cx, |_| {}).role(Role::Group),
                    Element::stack(Axis::Horizontal, vec![tile().on_tap(cx, |_| {})])
                        .on_key(cx, |_, _| {}),
                    tile(),
                ],
            )
        });
        let c = &tree.children;
        assert!(is_tap_focus(&c[0]), "a button");
        assert!(is_tap_focus(&c[1]), "a checkbox");
        assert!(c[2].focus.is_none(), "a menu item");
        assert!(c[3].focus.is_none(), "a declared group");
        assert!(c[4].focus.is_some() && !is_tap_focus(&c[4]), "a field");
        assert!(
            c[4].children[0].focus.is_none(),
            "a toggle inside the field"
        );
        assert!(c[5].focus.is_none());
        let order = tab_order(&tree, None);
        assert_eq!(order.len(), 3);
        assert_eq!(order[0], c[0].focus.unwrap());
        assert_eq!(order[2], c[4].focus.unwrap());
    }

    #[test]
    fn a_tappable_with_a_key_handler_keeps_it() {
        let tree = laid(|cx| tile().on_key(cx, |_, _| {}).on_tap(cx, |_| {}));
        assert!(tree.focus.is_some() && !is_tap_focus(&tree));
        let id = tree.focus.unwrap();
        assert_eq!(
            tap_activation(&tree, id),
            None,
            "its keys go to its handler"
        );
    }

    #[test]
    fn enter_and_space_press_it_in_its_middle() {
        let tree =
            laid(|cx| Element::stack(Axis::Vertical, vec![tile(), tile().on_tap(cx, |_| {})]));
        let b = &tree.children[1];
        let (action, at) = tap_activation(&tree, b.focus.unwrap()).unwrap();
        assert_eq!(Some(action), b.action);
        assert_eq!(at, b.bounds.center());
        assert!(activates(&KeyInput::Enter));
        assert!(activates(&KeyInput::Text(" ".into())));
        assert!(!activates(&KeyInput::Text("a".into())));
        assert!(!activates(&KeyInput::Escape));
    }

    #[test]
    fn steps_wrap_both_ways() {
        let order = [FocusId(3), FocusId(5), FocusId(9)];
        assert_eq!(step(&order, None, false), Some(FocusId(3)));
        assert_eq!(step(&order, None, true), Some(FocusId(9)));
        assert_eq!(step(&order, Some(FocusId(9)), false), Some(FocusId(3)));
        assert_eq!(step(&order, Some(FocusId(3)), true), Some(FocusId(9)));
        assert_eq!(step(&order, Some(FocusId(5)), true), Some(FocusId(3)));
        assert_eq!(step(&order, Some(FocusId(7)), false), Some(FocusId(3)));
        assert_eq!(step(&[], None, false), None);
    }
}
