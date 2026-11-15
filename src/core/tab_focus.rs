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
use crate::core::keys::FocusGroup;
use crate::core::runtime::{ActionId, FocusId, KeyInput, LayoutNode, find_focus, node_to_window};
use crate::geometry::{Point, Rect};

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
            | Role::RadioButton
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

/// The Tab order: every focusable in tree order, but one control only of a
/// [`FocusGroup::Select`] group (see [`Element::focus_group`]): the
/// `focused` one if it is in the group, else its checked one, else its
/// first. With a modal overlay up, `modal_layers` is how many of the root's
/// layers (not counting overlays leaving) it and the overlays above it take,
/// at the end: only those count, so Tab stays in the dialog.
///
/// [`Element::focus_group`]: crate::core::Element::focus_group
pub fn tab_order(
    tree: &LayoutNode,
    modal_layers: Option<usize>,
    focused: Option<FocusId>,
) -> Vec<FocusId> {
    let mut order = Vec::new();
    match modal_layers {
        Some(n) => {
            let layers: Vec<&LayoutNode> = tree.children.iter().filter(|l| !l.ghost).collect();
            for layer in &layers[layers.len().saturating_sub(n)..] {
                collect_stops(layer, focused, &mut order);
            }
        }
        None => collect_stops(tree, focused, &mut order),
    }
    order
}

/// The group `node` makes, if it is one (see [`Element::focus_group`]).
///
/// [`Element::focus_group`]: crate::core::Element::focus_group
fn group_of(node: &LayoutNode) -> Option<FocusGroup> {
    node.keyboard.as_ref().and_then(|k| k.focus_group)
}

/// The Tab stops under `node`, in tree order (see [`tab_order`]).
fn collect_stops(node: &LayoutNode, focused: Option<FocusId>, out: &mut Vec<FocusId>) {
    if group_of(node) == Some(FocusGroup::Select) {
        let mut members = Vec::new();
        crate::core::collect_focusables(node, &mut members);
        let checked = |id: &FocusId| {
            find_focus(node, *id)
                .and_then(|n| n.semantics.as_ref())
                .is_some_and(|s| s.checked == Some(true) || s.selected)
        };
        let stop = members
            .iter()
            .copied()
            .find(|f| Some(*f) == focused)
            .or_else(|| members.iter().copied().find(checked))
            .or_else(|| members.first().copied());
        out.extend(stop);
        return;
    }
    if let Some(id) = node.focus {
        out.push(id);
    }
    for child in &node.children {
        collect_stops(child, focused, out);
    }
}

/// Where the arrow key `input` moves focus from `focused`, a control inside
/// an element made a [`focus_group`](crate::core::Element::focus_group)
/// (the innermost, if they nest): the group's control it lands on and, in a
/// [`FocusGroup::Select`] group, the tap to run on it (as
/// [`tap_activation`] gives it). In a [`FocusGroup::Spatial`] group, the
/// nearest control the arrow points to (see [`spatial_pick`]). `None` when
/// `focused` isn't in a group or `input` isn't an arrow, Home or End.
pub fn arrow_step(
    tree: &LayoutNode,
    focused: FocusId,
    input: &KeyInput,
) -> Option<(FocusId, Option<(ActionId, Point)>)> {
    let group = group_around(tree, focused)?;
    if group_of(group) == Some(FocusGroup::Spatial) {
        let dir = Direction::of(input)?;
        return spatial_step(tree, group, focused, dir).map(|next| (next, None));
    }
    let mut members = Vec::new();
    crate::core::collect_focusables(group, &mut members);
    let n = members.len();
    let at = members.iter().position(|f| *f == focused)?;
    // Left and Right point to the start and the end of a mirrored row.
    let back = match input {
        KeyInput::Left => !group.rtl,
        KeyInput::Right => group.rtl,
        KeyInput::Up => true,
        KeyInput::Down => false,
        KeyInput::Home | KeyInput::End => false,
        _ => return None,
    };
    let to = match input {
        KeyInput::Home => 0,
        KeyInput::End => n - 1,
        _ if back => (at + n - 1) % n,
        _ => (at + 1) % n,
    };
    let next = members[to];
    let press = match group_of(group) {
        Some(FocusGroup::Select) => tap_activation(tree, next),
        _ => None,
    };
    Some((next, press))
}

/// The innermost [`focus_group`](crate::core::Element::focus_group) around
/// the node focused `id`, if it is in one.
fn group_around(tree: &LayoutNode, id: FocusId) -> Option<&LayoutNode> {
    /// The innermost group around the node focused `id` (`group`: the
    /// innermost around `node`).
    fn find<'a>(
        node: &'a LayoutNode,
        id: FocusId,
        group: Option<&'a LayoutNode>,
    ) -> Option<Option<&'a LayoutNode>> {
        if node.focus == Some(id) {
            return Some(group);
        }
        let group = if group_of(node).is_some() {
            Some(node)
        } else {
            group
        };
        node.children.iter().find_map(|c| find(c, id, group))
    }
    find(tree, id, None)?
}

/// A direction focus moves in on screen (see [`FocusGroup::Spatial`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    /// The direction an arrow key points, `None` for other keys.
    pub fn of(input: &KeyInput) -> Option<Direction> {
        match input {
            KeyInput::Left => Some(Direction::Left),
            KeyInput::Right => Some(Direction::Right),
            KeyInput::Up => Some(Direction::Up),
            KeyInput::Down => Some(Direction::Down),
            _ => None,
        }
    }

    /// The arrow key pointing this way.
    pub fn key(self) -> KeyInput {
        match self {
            Direction::Left => KeyInput::Left,
            Direction::Right => KeyInput::Right,
            Direction::Up => KeyInput::Up,
            Direction::Down => KeyInput::Down,
        }
    }

    /// The other way.
    pub fn opposite(self) -> Direction {
        match self {
            Direction::Left => Direction::Right,
            Direction::Right => Direction::Left,
            Direction::Up => Direction::Down,
            Direction::Down => Direction::Up,
        }
    }

    /// Left or right.
    pub fn is_horizontal(self) -> bool {
        matches!(self, Direction::Left | Direction::Right)
    }
}

/// Where a candidate lies from the control focus leaves, going one way.
struct Placed {
    id: FocusId,
    /// Whether it is ahead that way at all.
    ahead: bool,
    /// The gap between the edges facing each other (0 when they overlap).
    gap: f64,
    /// How far its center is off the line through the focused one's.
    off: f64,
    /// Whether it shares the focused one's row (going left or right) or
    /// column (going up or down).
    in_beam: bool,
    /// How far its far edge is from the focused one's near edge.
    reach: f64,
}

fn place(from: Rect, (id, r): (FocusId, Rect), dir: Direction) -> Placed {
    const EPS: f64 = 0.5;
    let (fc, c) = (from.center(), r.center());
    let (ahead, gap, reach) = match dir {
        Direction::Right => (
            c.x > fc.x + EPS && r.max_x() > from.max_x() - EPS,
            r.min_x() - from.max_x(),
            r.max_x() - from.min_x(),
        ),
        Direction::Left => (
            c.x < fc.x - EPS && r.min_x() < from.min_x() + EPS,
            from.min_x() - r.max_x(),
            from.max_x() - r.min_x(),
        ),
        Direction::Down => (
            c.y > fc.y + EPS && r.max_y() > from.max_y() - EPS,
            r.min_y() - from.max_y(),
            r.max_y() - from.min_y(),
        ),
        Direction::Up => (
            c.y < fc.y - EPS && r.min_y() < from.min_y() + EPS,
            from.min_y() - r.max_y(),
            from.max_y() - r.min_y(),
        ),
    };
    let (off, in_beam) = if dir.is_horizontal() {
        (
            (c.y - fc.y).abs(),
            r.min_y() < from.max_y() && r.max_y() > from.min_y(),
        )
    } else {
        (
            (c.x - fc.x).abs(),
            r.min_x() < from.max_x() && r.max_x() > from.min_x(),
        )
    };
    Placed {
        id,
        ahead,
        gap: gap.max(0.0),
        off,
        in_beam,
        reach,
    }
}

/// Spatial navigation's geometry: of `candidates` (each control and where
/// it is on screen), the one focus moves to from the control at `from`
/// going `dir`.
///
/// Only controls ahead of `from` that way count. The nearest one sharing
/// its row (going left or right) or column (going up or down) wins, then
/// the best lined up of those as near, unless one out of line lies wholly
/// nearer than it starts: then the nearest out of line by
/// `13·gap² + offset²` (Android's weighing, which favors staying in line).
/// With nothing ahead and `wrap`, focus wraps to the farthest control the
/// other way in the same row or column.
pub fn spatial_pick(
    from: Rect,
    candidates: &[(FocusId, Rect)],
    dir: Direction,
    wrap: bool,
) -> Option<FocusId> {
    let by = |a: f64, b: f64| a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal);
    let placed: Vec<Placed> = candidates
        .iter()
        .filter(|(_, r)| *r != from)
        .map(|&c| place(from, c, dir))
        .filter(|p| p.ahead)
        .collect();
    let beam = placed
        .iter()
        .filter(|p| p.in_beam)
        .min_by(|a, b| by(a.gap, b.gap).then(by(a.off, b.off)));
    let score = |p: &Placed| 13.0 * p.gap * p.gap + p.off * p.off;
    let other = placed
        .iter()
        .filter(|p| !p.in_beam)
        .min_by(|a, b| by(score(a), score(b)));
    let extent = if dir.is_horizontal() {
        from.width()
    } else {
        from.height()
    };
    match (beam, other) {
        // Out of line, it only wins when it ends before the one in line
        // starts.
        (Some(b), Some(o)) if o.reach <= b.gap + extent => return Some(o.id),
        (Some(b), _) => return Some(b.id),
        (None, Some(o)) => return Some(o.id),
        (None, None) => {}
    }
    if !wrap {
        return None;
    }
    // Wrap: the farthest the other way, in line.
    candidates
        .iter()
        .filter(|(_, r)| *r != from)
        .map(|&c| place(from, c, dir.opposite()))
        .filter(|p| p.ahead && p.in_beam)
        .max_by(|a, b| by(a.reach, b.reach).then(by(b.off, a.off)))
        .map(|p| p.id)
}

/// Where the node focused `id` is drawn, in window coordinates (the box
/// around it, through any transform).
fn drawn_rect(tree: &LayoutNode, id: FocusId) -> Option<Rect> {
    let (node, to_window, _) = node_to_window(tree, &|n| n.focus == Some(id))?;
    Some(if to_window.is_identity() {
        node.bounds
    } else {
        to_window.map_rect(node.bounds)
    })
}

/// The control in spatial `group` focus moves to from `focused` going `dir`
/// (see [`spatial_pick`]).
fn spatial_step(
    tree: &LayoutNode,
    group: &LayoutNode,
    focused: FocusId,
    dir: Direction,
) -> Option<FocusId> {
    let mut members = Vec::new();
    crate::core::collect_focusables(group, &mut members);
    let from = drawn_rect(tree, focused)?;
    let candidates: Vec<(FocusId, Rect)> = members
        .into_iter()
        .filter(|id| *id != focused)
        .filter_map(|id| Some((id, drawn_rect(tree, id)?)))
        .collect();
    spatial_pick(from, &candidates, dir, true)
}

/// Whether the key `input` on the control focused `focused` moves focus
/// spatially instead of going to the control: the control sits in a
/// [`FocusGroup::Spatial`] group (the innermost around it) and `input` is
/// an arrow that it doesn't keep. A button keeps none; a control with a key
/// handler of its own (a slider, a field) keeps Left and Right.
pub fn spatial_takes(tree: &LayoutNode, focused: FocusId, input: &KeyInput) -> bool {
    let Some(dir) = Direction::of(input) else {
        return false;
    };
    if group_around(tree, focused).and_then(group_of) != Some(FocusGroup::Spatial) {
        return false;
    }
    !dir.is_horizontal() || find_focus(tree, focused).is_some_and(is_tap_focus)
}

/// The control a [`FocusGroup::Spatial`] group gives focus to when focus
/// isn't in the window's active layers (`modal_layers`, as for
/// [`tab_order`]): nothing has focus, or a modal dialog opened over the
/// focused control. The first control of the first spatial group there, in
/// tree order; `None` when focus is already there, or there is no such
/// group.
pub fn spatial_entry(
    tree: &LayoutNode,
    modal_layers: Option<usize>,
    focused: Option<FocusId>,
) -> Option<FocusId> {
    fn first(node: &LayoutNode) -> Option<FocusId> {
        if node.ghost {
            return None;
        }
        if group_of(node) == Some(FocusGroup::Spatial) {
            let mut members = Vec::new();
            crate::core::collect_focusables(node, &mut members);
            if let Some(f) = members.first() {
                return Some(*f);
            }
        }
        node.children.iter().find_map(first)
    }
    let layers: Vec<&LayoutNode> = match modal_layers {
        Some(n) => {
            let all: Vec<&LayoutNode> = tree.children.iter().filter(|l| !l.ghost).collect();
            all[all.len().saturating_sub(n)..].to_vec()
        }
        None => vec![tree],
    };
    if let Some(f) = focused
        && layers.iter().any(|l| find_focus(l, f).is_some())
    {
        return None;
    }
    layers.into_iter().find_map(first)
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
        let order = tab_order(&tree, None, None);
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
    fn a_radio_group_is_one_tab_stop_and_its_arrows_press() {
        use crate::core::FocusGroup;
        let tree = laid(|cx| {
            let radio = |cx: &mut Cx<()>, on: bool| {
                tile()
                    .on_tap(cx, |_| {})
                    .role(Role::RadioButton)
                    .checked(on)
            };
            let group = Element::stack(
                Axis::Vertical,
                vec![radio(cx, false), radio(cx, true), radio(cx, false)],
            )
            .focus_group(FocusGroup::Select);
            Element::stack(Axis::Vertical, vec![tile().on_tap(cx, |_| {}), group])
        });
        let ids: Vec<FocusId> = tree.children[1]
            .children
            .iter()
            .map(|n| n.focus.unwrap())
            .collect();
        let button = tree.children[0].focus.unwrap();
        // The checked radio stands for the group, or the focused one.
        assert_eq!(tab_order(&tree, None, None), [button, ids[1]]);
        assert_eq!(tab_order(&tree, None, Some(ids[2])), [button, ids[2]]);
        // Down goes to the next and presses it; Up from the first wraps.
        let (next, press) = arrow_step(&tree, ids[1], &KeyInput::Down).unwrap();
        assert_eq!(next, ids[2]);
        assert_eq!(press.map(|p| p.0), tree.children[1].children[2].action);
        assert_eq!(arrow_step(&tree, ids[0], &KeyInput::Up).unwrap().0, ids[2]);
        assert_eq!(
            arrow_step(&tree, ids[2], &KeyInput::Home).unwrap().0,
            ids[0]
        );
        assert!(arrow_step(&tree, ids[0], &KeyInput::Enter).is_none());
        assert!(
            arrow_step(&tree, button, &KeyInput::Down).is_none(),
            "not in a group"
        );
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
