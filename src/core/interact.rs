//! Interaction states: hovered, pressed, focused, dragged, dragged over…, per
//! element, for views to read and to style from.
//!
//! An element is identified across builds by an [`ElementKey`]: the one given
//! with [`Element::key`](crate::core::Element::key), or else its position in
//! the tree (the path of child indices from the root, below the nearest keyed
//! ancestor). After each frame the app knows where every identified element
//! was laid out, and from the pointer, the press, keyboard focus and any drag
//! it works out each one's [`Interaction`]. The next build sees it two ways:
//!
//! - [`Cx::interaction`](crate::core::Cx::interaction) returns a keyed
//!   element's states, for the view to build from;
//! - per-state overrides ([`Element::hovered`](crate::core::Element::hovered),
//!   [`Element::drag_over`](crate::core::Element::drag_over),
//!   [`Element::when`](crate::core::Element::when)…) restyle an element while
//!   it is in a state. They are resolved when the built tree is lowered for
//!   layout ([`resolve`]), so the layout, the diff and damage see plain
//!   styles, and [`Transitions`] can animate between them.
//!
//! The app remembers which elements use which states ([`Watch`]) and rebuilds
//! after an event only when one of those changed.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use crate::core::element::{Element, ElementKind};
use crate::core::presence::PresenceSpec;
use crate::core::runtime::{ActionId, DragId, DragSourceId, DropId, FocusId, LayoutNode};
use crate::core::transition::{AnimProp, Transition, Transitions};
use crate::geometry::{Insets, Point, Transform};
use crate::platform::CursorIcon;
use crate::render::Color;

/// A set of interaction states. An element's states are what
/// [`Cx::interaction`](crate::core::Cx::interaction) returns; a set is also
/// what a per-state override waits for with
/// [`Element::when`](crate::core::Element::when) (all of its states at once).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Interaction(u16);

impl Interaction {
    /// No state.
    pub const NONE: Self = Self(0);
    /// The pointer is over the element (or over an element inside it), and
    /// nothing drawn above covers it there.
    pub const HOVERED: Self = Self(1);
    /// The primary button (or a finger) went down on the element and is still
    /// down, over it.
    pub const PRESSED: Self = Self(1 << 1);
    /// The element, or an element inside it, has keyboard focus.
    pub const FOCUSED: Self = Self(1 << 2);
    /// [`FOCUSED`](Self::FOCUSED), and focus got there from the keyboard (Tab),
    /// where a focus ring should show; a click focuses without it.
    pub const FOCUS_VISIBLE: Self = Self(1 << 3);
    /// The view marked the element [`disabled`](crate::core::Element::disabled).
    pub const DISABLED: Self = Self(1 << 4);
    /// The view marked the element [`selected`](crate::core::Element::selected).
    pub const SELECTED: Self = Self(1 << 5);
    /// The element (or one inside it) is being dragged: an in-app drag
    /// started on its [`drag_source`](crate::core::Element::drag_source), or a
    /// pointer drag on its [`on_drag`](crate::core::Element::on_drag) handle.
    pub const DRAGGED: Self = Self(1 << 6);
    /// A drag the element's drop target accepts is over it (or over a drop
    /// target inside it).
    pub const DRAG_OVER: Self = Self(1 << 7);
    /// A drag the element's drop target refuses is over it.
    pub const DRAG_REFUSED: Self = Self(1 << 8);

    /// The states the app follows from input (all but the view's own
    /// `DISABLED` and `SELECTED`).
    pub(crate) const DYNAMIC: Self = Self(
        Self::HOVERED.0
            | Self::PRESSED.0
            | Self::FOCUSED.0
            | Self::FOCUS_VISIBLE.0
            | Self::DRAGGED.0
            | Self::DRAG_OVER.0
            | Self::DRAG_REFUSED.0,
    );

    /// Whether every state in `other` is in `self`.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether any state in `other` is in `self`.
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// Whether the set is empty.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// See [`HOVERED`](Self::HOVERED).
    pub const fn hovered(self) -> bool {
        self.contains(Self::HOVERED)
    }

    /// See [`PRESSED`](Self::PRESSED).
    pub const fn pressed(self) -> bool {
        self.contains(Self::PRESSED)
    }

    /// See [`FOCUSED`](Self::FOCUSED).
    pub const fn focused(self) -> bool {
        self.contains(Self::FOCUSED)
    }

    /// See [`FOCUS_VISIBLE`](Self::FOCUS_VISIBLE).
    pub const fn focus_visible(self) -> bool {
        self.contains(Self::FOCUS_VISIBLE)
    }

    /// See [`DISABLED`](Self::DISABLED).
    pub const fn disabled(self) -> bool {
        self.contains(Self::DISABLED)
    }

    /// See [`SELECTED`](Self::SELECTED).
    pub const fn selected(self) -> bool {
        self.contains(Self::SELECTED)
    }

    /// See [`DRAGGED`](Self::DRAGGED).
    pub const fn dragged(self) -> bool {
        self.contains(Self::DRAGGED)
    }

    /// A drag is over the element, accepted or not.
    pub const fn drag_hover(self) -> bool {
        self.intersects(Self(Self::DRAG_OVER.0 | Self::DRAG_REFUSED.0))
    }

    /// Whether the drag over the element is one it accepts
    /// ([`DRAG_OVER`](Self::DRAG_OVER)).
    pub const fn drag_accepted(self) -> bool {
        self.contains(Self::DRAG_OVER)
    }
}

impl std::ops::BitOr for Interaction {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for Interaction {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl std::ops::BitAnd for Interaction {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl std::fmt::Debug for Interaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const NAMES: [&str; 9] = [
            "HOVERED",
            "PRESSED",
            "FOCUSED",
            "FOCUS_VISIBLE",
            "DISABLED",
            "SELECTED",
            "DRAGGED",
            "DRAG_OVER",
            "DRAG_REFUSED",
        ];
        let names: Vec<&str> = NAMES
            .iter()
            .enumerate()
            .filter(|(i, _)| self.0 & (1 << i) != 0)
            .map(|(_, n)| *n)
            .collect();
        write!(f, "Interaction({})", names.join(" | "))
    }
}

/// An element's identity across builds: from its
/// [`key`](crate::core::Element::key), or its place in the tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ElementKey(pub(crate) u64);

impl ElementKey {
    /// The identity of an element keyed with `key`. Equal keys are the same
    /// element wherever they are in the window, so give each keyed element its
    /// own (a string, a number, an item id, or a tuple of them).
    pub fn new(key: impl Hash) -> Self {
        let mut h = std::hash::DefaultHasher::new();
        0x6b65_795fu32.hash(&mut h);
        key.hash(&mut h);
        Self(h.finish())
    }

    /// The `index`th child of `parent`, for elements without a key.
    pub(crate) fn child(parent: ElementKey, index: usize) -> Self {
        let mut h = std::hash::DefaultHasher::new();
        parent.0.hash(&mut h);
        index.hash(&mut h);
        Self(h.finish())
    }

    /// The root of layer `layer` (the main tree is 0, overlays count up).
    pub(crate) fn root(layer: usize) -> Self {
        Self::child(Self(0x726f_6f74), layer)
    }
}

/// A per-state style override: the style properties a closure given to
/// [`Element::when`](crate::core::Element::when) changed, to apply while the
/// state holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct StylePatch {
    fill: Option<Option<Color>>,
    radius: Option<f64>,
    border: Option<Option<(Color, f64)>>,
    padding: Option<Insets>,
    width: Option<Option<f64>>,
    height: Option<Option<f64>>,
    text_color: Option<Color>,
    cursor: Option<Option<CursorIcon>>,
    opacity: Option<f32>,
    transform: Option<Option<Transform>>,
}

fn text_color(e: &Element) -> Option<Color> {
    match &e.kind {
        ElementKind::Text { color, .. } => Some(*color),
        _ => None,
    }
}

impl StylePatch {
    /// What changed from `before` to `after`.
    pub(crate) fn diff(before: &Element, after: &Element) -> Self {
        fn changed<T: PartialEq + Copy>(a: T, b: T) -> Option<T> {
            (a != b).then_some(b)
        }
        Self {
            fill: changed(before.decoration.fill, after.decoration.fill),
            radius: changed(before.decoration.radius, after.decoration.radius),
            border: changed(before.decoration.border, after.decoration.border),
            padding: changed(before.layout.padding, after.layout.padding),
            width: changed(before.layout.size.width, after.layout.size.width),
            height: changed(before.layout.size.height, after.layout.size.height),
            text_color: changed(text_color(before), text_color(after)).flatten(),
            cursor: changed(before.cursor, after.cursor),
            opacity: changed(before.opacity, after.opacity),
            transform: changed(before.transform, after.transform),
        }
    }

    pub(crate) fn apply(&self, e: &mut Element) {
        if let Some(v) = self.fill {
            e.decoration.fill = v;
        }
        if let Some(v) = self.radius {
            e.decoration.radius = v;
        }
        if let Some(v) = self.border {
            e.decoration.border = v;
        }
        if let Some(v) = self.padding {
            e.layout.padding = v;
        }
        if let Some(v) = self.width {
            e.layout.size.width = v;
        }
        if let Some(v) = self.height {
            e.layout.size.height = v;
        }
        if let (Some(v), ElementKind::Text { color, .. }) = (self.text_color, &mut e.kind) {
            *color = v;
        }
        if let Some(v) = self.cursor {
            e.cursor = v;
        }
        if let Some(v) = self.opacity {
            e.opacity = v;
        }
        if let Some(v) = self.transform {
            e.transform = v;
        }
    }
}

/// The interaction-state parts of an [`Element`]: its key, whether it is
/// disabled, its per-state overrides and its transitions. Set through the
/// element's modifiers ([`key`](crate::core::Element::key),
/// [`hovered`](crate::core::Element::hovered),
/// [`transition`](crate::core::Element::transition)…).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reactive {
    pub(crate) key: Option<ElementKey>,
    /// The identity it was given when lowered (its key, or its tree path).
    pub(crate) id: Option<ElementKey>,
    pub(crate) disabled: bool,
    pub(crate) overrides: Vec<(Interaction, StylePatch)>,
    pub(crate) transitions: Vec<(Option<AnimProp>, Transition)>,
    /// Its enter, exit and layout animations.
    pub(crate) presence: Option<Box<PresenceSpec>>,
}

/// What the app knows about input, to work out elements' states.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Inputs {
    /// The pointer, while it is in the window.
    pub pointer: Option<Point>,
    /// Where the primary button went down, while it is down.
    pub press: Option<Point>,
    pub focused: Option<FocusId>,
    /// Focus last moved by the keyboard.
    pub focus_visible: bool,
    /// The drag source of the in-app drag in progress.
    pub drag_source: Option<DragSourceId>,
    /// The element a pointer drag latched on.
    pub drag: Option<DragId>,
    /// The drop target under a drag, and whether it accepts it.
    pub drop_over: Option<(DropId, bool)>,
}

/// Each identified element's states in the laid-out `tree`, given `inputs`
/// (`DISABLED` and `SELECTED` aside: the view declares those).
pub(crate) fn collect(tree: &LayoutNode, inputs: &Inputs) -> HashMap<ElementKey, Interaction> {
    /// `pointer` and `press` are in the parent's coordinates, and `None` off
    /// the path to the top-most element under them.
    fn walk(
        node: &LayoutNode,
        inputs: &Inputs,
        pointer: Option<Point>,
        press: Option<Point>,
        out: &mut HashMap<ElementKey, Interaction>,
    ) -> Interaction {
        // A transformed node is under the pointer where it's drawn.
        let local = |p: Option<Point>| {
            p.and_then(|p| node.to_local(p))
                .filter(|p| node.bounds.contains(*p))
        };
        let (pointer, press) = (local(pointer), local(press));
        let (hovered, pressed) = (pointer.is_some(), press.is_some());
        // The pointer is over the top-most child under it (the last drawn).
        let top = |p: Option<Point>| {
            p.and_then(|p| {
                node.children
                    .iter()
                    .rposition(|c| !c.ghost && c.to_local(p).is_some_and(|q| c.bounds.contains(q)))
            })
        };
        let (hover_child, press_child) = (top(pointer), top(press));
        let mut within = Interaction::NONE;
        if node.focus.is_some() && node.focus == inputs.focused {
            within |= Interaction::FOCUSED;
            if inputs.focus_visible {
                within |= Interaction::FOCUS_VISIBLE;
            }
        }
        if (node.drag_source.is_some() && node.drag_source == inputs.drag_source)
            || (node.drag.is_some() && node.drag == inputs.drag)
        {
            within |= Interaction::DRAGGED;
        }
        if let (Some(id), Some((over, accepted))) = (node.drop, inputs.drop_over)
            && id == over
        {
            within |= if accepted {
                Interaction::DRAG_OVER
            } else {
                Interaction::DRAG_REFUSED
            };
        }
        for (i, child) in node.children.iter().enumerate() {
            if child.ghost {
                continue; // Drawn while it leaves, and nothing else.
            }
            within |= walk(
                child,
                inputs,
                pointer.filter(|_| hover_child == Some(i)),
                press.filter(|_| press_child == Some(i)),
                out,
            );
        }
        if let Some(key) = node.key {
            let mut states = within;
            if hovered {
                states |= Interaction::HOVERED;
                if pressed {
                    states |= Interaction::PRESSED;
                }
            }
            *out.entry(key).or_default() |= states;
        }
        within
    }
    let mut out = HashMap::new();
    walk(tree, inputs, inputs.pointer, inputs.press, &mut out);
    out
}

/// The elements whose look (or whose view) depends on states, with the states
/// they depend on and what those were when last built.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Watch {
    entries: Vec<(ElementKey, Interaction, Interaction)>,
}

impl Watch {
    pub(crate) fn add(&mut self, id: ElementKey, mask: Interaction, states: Interaction) {
        let mask = mask & Interaction::DYNAMIC;
        if !mask.is_empty() {
            self.entries.push((id, mask, states & mask));
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether any watched state differs in `now`.
    pub(crate) fn changed(&self, now: &HashMap<ElementKey, Interaction>) -> bool {
        self.entries
            .iter()
            .any(|&(id, mask, was)| now.get(&id).copied().unwrap_or_default() & mask != was)
    }
}

/// The lowering pass's inputs and what it found.
pub(crate) struct Resolver<'a> {
    pub states: &'a HashMap<ElementKey, Interaction>,
    pub transitions: &'a mut Transitions,
    pub watch: Watch,
    /// Tappable elements that style their own hover or press look, and drop
    /// targets their own drag-over look (the app doesn't add its highlight).
    pub styled_actions: Vec<ActionId>,
    pub styled_drops: Vec<DropId>,
    /// The elements with enter, exit or layout animations, by identity.
    pub presence: HashMap<ElementKey, PresenceSpec>,
    /// Elements to give an identity on the laid-out tree even without
    /// states: the parents of elements leaving.
    pub anchors: std::collections::HashSet<ElementKey>,
}

impl Resolver<'_> {
    /// Give `el` (at tree position `id`) and everything inside it their
    /// identities, apply the overrides of the states they're in, and animate
    /// what changed.
    pub(crate) fn resolve(&mut self, el: &mut Element, id: ElementKey) {
        let id = el.reactive.as_ref().and_then(|r| r.key).unwrap_or(id);
        if let Some(mut re) = el.reactive.take() {
            re.id = Some(id);
            let mut states = self.states.get(&id).copied().unwrap_or_default();
            if re.disabled {
                states |= Interaction::DISABLED;
            }
            if el.semantics.as_ref().is_some_and(|s| s.selected) {
                states |= Interaction::SELECTED;
            }
            let mask = re
                .overrides
                .iter()
                .fold(Interaction::NONE, |m, (when, _)| m | *when);
            self.watch.add(id, mask, states);
            if mask.intersects(Interaction::HOVERED | Interaction::PRESSED)
                && let Some(a) = el.action
            {
                self.styled_actions.push(a);
            }
            if mask.intersects(Interaction::DRAG_OVER | Interaction::DRAG_REFUSED)
                && let Some(d) = el.drop
            {
                self.styled_drops.push(d);
            }
            for (when, patch) in &re.overrides {
                if states.contains(*when) {
                    patch.apply(el);
                }
            }
            if !re.transitions.is_empty() {
                self.transitions.animate(id, el, &re.transitions);
            }
            if let Some(spec) = &re.presence {
                self.presence.insert(id, (**spec).clone());
            }
            el.reactive = Some(re);
        }
        if let Some(children) = el.children_mut() {
            let mut exits = false;
            for (i, child) in children.iter_mut().enumerate() {
                self.resolve(child, ElementKey::child(id, i));
                exits |= child
                    .reactive
                    .as_ref()
                    .is_some_and(|r| r.presence.as_ref().is_some_and(|p| p.exit.is_some()));
            }
            // A child that leaves is drawn under its parent while it does:
            // the parent needs an identity to be found by.
            if exits || self.anchors.contains(&id) {
                el.reactive.get_or_insert_with(Default::default).id = Some(id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::layout;
    use crate::geometry::Rect;
    use crate::layout::Axis;

    fn tree(el: &mut Element, states: &HashMap<ElementKey, Interaction>) -> (LayoutNode, Watch) {
        let mut transitions = Transitions::default();
        let mut r = Resolver {
            states,
            transitions: &mut transitions,
            watch: Watch::default(),
            styled_actions: Vec::new(),
            styled_drops: Vec::new(),
            presence: HashMap::new(),
            anchors: Default::default(),
        };
        r.resolve(el, ElementKey::root(0));
        let watch = std::mem::take(&mut r.watch);
        (
            layout(el, Rect::from_xywh(0.0, 0.0, 200.0, 100.0), None),
            watch,
        )
    }

    fn card(key: &str) -> Element {
        Element::stack(Axis::Vertical, vec![])
            .width(50.0)
            .height(50.0)
            .fill(Color::rgb(0, 0, 0))
            .hovered(|e| e.fill(Color::rgb(255, 0, 0)))
            .key(key)
    }

    #[test]
    fn hover_follows_the_pointer_and_restyles() {
        let view = || Element::stack(Axis::Horizontal, vec![card("a"), card("b")]);
        let (laid, watch) = tree(&mut view(), &HashMap::new());
        let (a, b) = (ElementKey::new("a"), ElementKey::new("b"));
        let inputs = Inputs {
            pointer: Some(Point::new(60.0, 10.0)),
            ..Inputs::default()
        };
        let states = collect(&laid, &inputs);
        assert!(states[&b].hovered() && !states[&a].hovered());
        assert!(watch.changed(&states), "b's look depends on it");
        let (laid, watch) = tree(&mut view(), &states);
        assert_eq!(
            laid.children[1].decoration.fill,
            Some(Color::rgb(255, 0, 0))
        );
        assert_eq!(laid.children[0].decoration.fill, Some(Color::rgb(0, 0, 0)));
        assert!(!watch.changed(&collect(&laid, &inputs)), "stable");
    }

    #[test]
    fn unwatched_states_do_not_rebuild() {
        // A card styled only on drag-over doesn't care about hover.
        let el = || {
            Element::stack(Axis::Vertical, vec![])
                .width(50.0)
                .height(50.0)
                .drag_over(|e| e.fill(Color::rgb(0, 0, 255)))
        };
        let (laid, watch) = tree(&mut el(), &HashMap::new());
        let hover = Inputs {
            pointer: Some(Point::new(10.0, 10.0)),
            ..Inputs::default()
        };
        assert!(collect(&laid, &hover).values().any(|s| s.hovered()));
        assert!(!watch.changed(&collect(&laid, &hover)));
    }

    #[test]
    fn untouched_elements_keep_their_position_identity() {
        let view = |n: usize| {
            let mut kids: Vec<Element> = (0..n)
                .map(|_| Element::stack(Axis::Vertical, vec![]).width(10.0))
                .collect();
            kids.push(Element::stack(Axis::Vertical, vec![]).pressed(|e| e.radius(4.0)));
            Element::stack(Axis::Horizontal, kids)
        };
        let id = |el: &Element, i: usize| match &el.kind {
            ElementKind::Stack { children, .. } => children[i].reactive.as_ref().unwrap().id,
            _ => None,
        };
        let (mut a, mut b) = (view(1), view(1));
        tree(&mut a, &HashMap::new());
        tree(&mut b, &HashMap::new());
        assert_eq!(id(&a, 1), id(&b, 1));
        let mut c = view(2);
        tree(&mut c, &HashMap::new());
        assert_ne!(id(&a, 1), id(&c, 2), "a new position is a new element");
    }

    #[test]
    fn press_focus_and_drag_states() {
        let el = Element::stack(
            Axis::Vertical,
            vec![
                Element::stack(Axis::Vertical, vec![])
                    .width(40.0)
                    .height(40.0)
                    .key("inner"),
            ],
        )
        .key("outer");
        let mut laid = layout(&el, Rect::from_xywh(0.0, 0.0, 100.0, 100.0), None);
        laid.key = Some(ElementKey::new("outer"));
        laid.children[0].key = Some(ElementKey::new("inner"));
        laid.children[0].focus = Some(FocusId(0));
        laid.children[0].drop = Some(DropId(3));
        let inputs = Inputs {
            pointer: Some(Point::new(10.0, 10.0)),
            press: Some(Point::new(5.0, 5.0)),
            focused: Some(FocusId(0)),
            focus_visible: true,
            drop_over: Some((DropId(3), false)),
            ..Inputs::default()
        };
        let s = collect(&laid, &inputs);
        let inner = s[&ElementKey::new("inner")];
        assert!(inner.pressed() && inner.focus_visible() && inner.drag_hover());
        assert!(!inner.drag_accepted() && inner.contains(Interaction::DRAG_REFUSED));
        let outer = s[&ElementKey::new("outer")];
        assert!(outer.focused(), "focus within counts");
        // The pointer moved off the pressed element: not pressed any more.
        let off = Inputs {
            pointer: Some(Point::new(80.0, 80.0)),
            ..inputs
        };
        assert!(!collect(&laid, &off)[&ElementKey::new("inner")].pressed());
    }

    #[test]
    fn overrides_compose_in_order_and_need_all_their_states() {
        let mut el = Element::text("x", 12.0, Color::rgb(0, 0, 0))
            .hovered(|e| e.padding(Insets::uniform(2.0)))
            .when(Interaction::HOVERED | Interaction::PRESSED, |e| {
                e.padding(Insets::uniform(4.0))
            })
            .disabled(true)
            .when(Interaction::DISABLED, |e| {
                Element::text("ignored", 1.0, Color::rgb(9, 9, 9)).padding(e.layout.padding)
            })
            .key("t");
        let states = HashMap::from([(ElementKey::new("t"), Interaction::HOVERED)]);
        let (laid, _) = tree(&mut el, &states);
        assert_eq!(laid.decoration, Default::default());
        let crate::core::NodeContent::Text { color, size, .. } = laid.content else {
            panic!("text");
        };
        assert_eq!(color, Color::rgb(9, 9, 9), "the disabled color");
        assert_eq!(size, 12.0, "an override changes style, not content");
        assert_eq!(
            el.layout.padding,
            Insets::uniform(2.0),
            "hovered, not pressed"
        );
    }
}
