//! Enter, exit and layout animations: elements that fade, slide or scale in
//! when they appear, stay drawn as they fade out when they go away, and glide
//! to where the layout puts them instead of jumping there.
//!
//! An element declares them with
//! [`Element::enter`](crate::core::Element::enter),
//! [`exit`](crate::core::Element::exit) (or both at once with
//! [`appear`](crate::core::Element::appear)), and
//! [`animate_layout`](crate::core::Element::animate_layout) /
//! [`animate_position`](crate::core::Element::animate_position). They run on
//! the laid-out tree, after layout and scrolling, as a transform and an
//! opacity on top of the element's own:
//!
//! - **Enter:** an element seen for the first time (after the window's first
//!   frame) moves from its [`Effect`]'s look to its own, through a progress
//!   from 0 to 1.
//! - **Exit:** an element that was in the last frame and isn't in this one is
//!   kept, as it was drawn, as a *ghost*: a copy without handlers, keys or
//!   semantics, so it can't be hit, focused or read by a screen reader, put
//!   back under its old parent at its old index and place, and drawn with its
//!   effect as the progress runs back to 0. Then it's dropped. It takes no
//!   room in the layout: its siblings close up at once (or, with a layout
//!   animation, smoothly). An element that comes back while it's leaving
//!   turns round from where it is.
//! - **Layout:** each frame, an element's laid-out box is compared with the
//!   last one's (FLIP: first, last, invert, play). When it moved or resized,
//!   it is drawn where it was, through a translate and scale, and that
//!   transform animates back to none. Positions are measured from the nearest
//!   layout-animated ancestor (whose own motion carries its children), else
//!   from the nearest scroll container's content, else from the window, so
//!   scrolling doesn't animate anything and a resized window snaps.
//!
//! Everything moves on [`Animated`] values, so retargeting mid-flight keeps
//! the current value and speed, and reduced motion snaps it. [`Presence`]
//! reports whether anything still moves, so the app asks for frames only
//! meanwhile.

use std::collections::HashMap;
use std::time::Duration;

use crate::anim::{Animated, Easing};
use crate::core::interact::ElementKey;
use crate::core::runtime::{LayoutNode, NodeContent, ScrollId};
use crate::core::transition::Transition;
use crate::geometry::{Point, Rect, Size, Transform, Vec2};

/// How an element looks while it isn't there yet (or any more): what an
/// [`enter`](crate::core::Element::enter) animates from and an
/// [`exit`](crate::core::Element::exit) animates to, and how fast.
///
/// The presets cover the usual moves, and [`and`](Self::and) combines them:
///
/// ```
/// use stipple::prelude::*;
/// // Fade in while sliding 24 px up into place, on a spring.
/// let effect = Effect::fade()
///     .and(Effect::slide(0.0, 24.0))
///     .transition(Transition::spring(SpringParams::SMOOTH));
/// # let _ = effect;
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Effect {
    /// The group opacity, from 0 (invisible) to 1, multiplied into the
    /// element's own.
    pub opacity: f32,
    /// The transform, applied around the element's own (and its layout
    /// animation's).
    pub transform: Transform,
    /// How the element moves between this look and its own.
    pub transition: Transition,
}

impl Effect {
    /// How long the presets take: 200 ms.
    pub const DURATION: Duration = Duration::from_millis(200);

    /// The presets' transition: [`DURATION`](Self::DURATION), easing out.
    pub const TRANSITION: Transition = Transition::tween(Self::DURATION, Easing::EaseOut);

    /// An effect of `opacity` and `transform`, with the presets' transition.
    pub const fn custom(opacity: f32, transform: Transform) -> Self {
        Self {
            opacity,
            transform,
            transition: Self::TRANSITION,
        }
    }

    /// Invisible: fade in, or out.
    pub const fn fade() -> Self {
        Self::custom(0.0, Transform::IDENTITY)
    }

    /// Moved by `(dx, dy)` logical pixels: slide in from there, or out to
    /// there. Combine with [`fade`](Self::fade) for the usual look.
    pub fn slide(dx: f64, dy: f64) -> Self {
        Self::custom(1.0, Transform::IDENTITY.translate(dx, dy))
    }

    /// Scaled by `s` about the center: grow from it, or shrink to it.
    pub fn scale(s: f64) -> Self {
        Self::custom(1.0, Transform::IDENTITY.scale(s))
    }

    /// Both effects at once: opacities multiply, offsets add, scales
    /// multiply and rotations add, about this effect's origin (or `other`'s,
    /// when this one neither scales nor turns). This effect's transition.
    pub fn and(self, other: Effect) -> Self {
        let (a, b) = (self.transform, other.transform);
        let turns = a.scale != Vec2::new(1.0, 1.0) || a.rotate != 0.0;
        Self {
            opacity: self.opacity * other.opacity,
            transform: Transform {
                translate: Vec2::new(
                    a.translate.dx + b.translate.dx,
                    a.translate.dy + b.translate.dy,
                ),
                scale: Vec2::new(a.scale.dx * b.scale.dx, a.scale.dy * b.scale.dy),
                rotate: a.rotate + b.rotate,
                origin: if turns { a.origin } else { b.origin },
            },
            transition: self.transition,
        }
    }

    /// Move with `transition` instead (any [`Transition`]: a tween, a spring).
    pub fn transition(mut self, transition: impl Into<Transition>) -> Self {
        self.transition = transition.into();
        self
    }

    /// The opacity and transform `progress` of the way (0: this effect, 1:
    /// the element's own look).
    fn at(&self, progress: f64) -> (f32, Transform) {
        let p = progress.clamp(0.0, 1.0);
        let opacity = self.opacity + (1.0 - self.opacity) * p as f32;
        let home = Transform {
            origin: self.transform.origin,
            ..Transform::IDENTITY
        };
        (opacity, Transform::lerp(self.transform, home, progress))
    }
}

impl Default for Effect {
    /// A [`fade`](Self::fade).
    fn default() -> Self {
        Self::fade()
    }
}

/// An element's enter, exit and layout animations, as declared.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PresenceSpec {
    pub enter: Option<Effect>,
    pub exit: Option<Effect>,
    /// How its laid-out box moves to a new place.
    pub layout: Option<Transition>,
    /// Whether size changes animate too (by scaling), or only position.
    pub layout_size: bool,
}

/// One element's motion.
#[derive(Clone, Debug)]
struct Entry {
    /// Its progress in (0 to 1) while entering.
    shown: Option<Animated<f64>>,
    /// The effect `shown` plays (its enter effect, or the exit effect it was
    /// leaving with when it came back).
    effect: Option<Effect>,
    /// Where it's drawn, relative to its anchor, and how big.
    pos: Option<Animated<Point>>,
    size: Option<Animated<Size>>,
    /// What it leaves with, from the last frame.
    exit: Option<Effect>,
    /// Its parent (when keyed), its index there, and its offset from the
    /// parent's content origin in the last frame: where a ghost goes.
    parent: Option<ElementKey>,
    index: usize,
    offset: Vec2,
    seen: bool,
}

/// An element that has gone, drawn while it leaves.
#[derive(Clone, Debug)]
struct Ghost {
    id: ElementKey,
    /// The node as it was last drawn, without its handlers.
    node: LayoutNode,
    parent: ElementKey,
    index: usize,
    offset: Vec2,
    /// Its progress, running down to 0.
    progress: Animated<f64>,
    effect: Effect,
}

/// The enter, exit and layout animations of one window's elements.
#[derive(Clone, Debug, Default)]
pub(crate) struct Presence {
    entries: HashMap<ElementKey, Entry>,
    ghosts: Vec<Ghost>,
    /// A frame was built (enter effects don't run on the first).
    built: bool,
    /// The last frame's time and window size.
    last: Option<Duration>,
    size: Option<Size>,
    /// This frame's time.
    now: Option<Duration>,
}

/// Where `node`'s children are laid out from, without its scrolling.
fn content_origin(node: &LayoutNode, offsets: &HashMap<ScrollId, f64>) -> Point {
    let dy = node
        .scroll
        .and_then(|id| offsets.get(&id).copied())
        .unwrap_or(0.0);
    Point::new(node.bounds.origin.x, node.bounds.origin.y - dy)
}

fn sub(a: Point, b: Point) -> Vec2 {
    Vec2::new(a.x - b.x, a.y - b.y)
}

fn add(a: Point, v: Vec2) -> Point {
    Point::new(a.x + v.dx, a.y + v.dy)
}

/// `outer` applied after `inner` (`None`: none), for a node laid out at
/// `bounds`, as one transform about `inner`'s origin. Exact unless `outer`
/// scales unevenly and `inner` rotates (the scale is then applied as if
/// before the rotation).
pub(crate) fn compose(
    outer: Transform,
    inner: Option<Transform>,
    bounds: Rect,
) -> Option<Transform> {
    if outer.is_identity() {
        return inner;
    }
    let Some(inner) = inner else {
        return Some(outer);
    };
    // `inner` maps p to o + t + RS(p - o); `outer` then maps that. Its fixed
    // point o lands at outer(o + t), so that is the combined offset.
    let o = inner.origin_in(bounds);
    let lands = outer.to_affine(bounds).apply(add(o, inner.translate));
    Some(Transform {
        translate: sub(lands, o),
        scale: Vec2::new(
            outer.scale.dx * inner.scale.dx,
            outer.scale.dy * inner.scale.dy,
        ),
        rotate: outer.rotate + inner.rotate,
        origin: inner.origin,
    })
}

/// Draw `node` with `opacity` and `outer` on top of its own.
fn apply(node: &mut LayoutNode, opacity: f32, outer: Transform) {
    node.opacity = (node.opacity * opacity).clamp(0.0, 1.0);
    node.transform = compose(outer, node.transform, node.bounds).filter(|t| !t.is_identity());
}

/// `node` and its subtree shifted by `by`.
fn shift(node: &mut LayoutNode, by: Vec2) {
    node.bounds = node.bounds.translate(by);
    for c in &mut node.children {
        shift(c, by);
    }
}

/// A drawn-only copy of `node`: no handlers, identity or semantics.
fn strip(node: &mut LayoutNode) {
    node.ghost = true;
    node.action = None;
    node.focus = None;
    node.drag = None;
    node.context = None;
    node.drop = None;
    node.drag_source = None;
    node.pointer = None;
    node.caret = None;
    node.selection = None;
    node.preedit = None;
    node.keyboard = None;
    node.text_pos = None;
    node.scroll = None;
    node.cursor = None;
    node.semantics = None;
    node.key = None;
    if matches!(node.content, NodeContent::Viewport(_)) {
        node.content = NodeContent::None;
    }
    for c in &mut node.children {
        strip(c);
    }
}

fn find(node: &LayoutNode, id: ElementKey) -> Option<&LayoutNode> {
    if node.key == Some(id) {
        return Some(node);
    }
    node.children.iter().find_map(|c| find(c, id))
}

fn find_mut(node: &mut LayoutNode, id: ElementKey) -> Option<&mut LayoutNode> {
    if node.key == Some(id) {
        return Some(node);
    }
    node.children.iter_mut().find_map(|c| find_mut(c, id))
}

/// What the walk over the tree knows about a node's surroundings.
#[derive(Clone, Copy)]
struct Around {
    /// The parent's identity and content origin, when it has one.
    parent: Option<(ElementKey, Point)>,
    index: usize,
    /// What layout positions are measured from.
    anchor: Point,
}

impl Presence {
    /// Run this frame's animations on `tree` (laid out and scrolled, with
    /// `offsets`), whose elements declared `specs`, at `now` (`None`: no
    /// clock, so nothing animates). `old` is the last frame's tree, where
    /// elements that left are found; `size` is the window's.
    pub(crate) fn update(
        &mut self,
        tree: &mut LayoutNode,
        specs: &HashMap<ElementKey, PresenceSpec>,
        old: Option<&LayoutNode>,
        offsets: &HashMap<ScrollId, f64>,
        now: Option<Duration>,
        size: Size,
    ) {
        let first = !self.built;
        let resized = self.size.is_some_and(|s| s != size);
        self.built = true;
        self.size = Some(size);
        self.now = now;
        if specs.is_empty() && self.entries.is_empty() && self.ghosts.is_empty() {
            self.last = now;
            return;
        }
        // A change counts from one frame before this one (but not before the
        // last frame), so it already shows on the frame that sees it.
        let head = now.map(|now| {
            let frame = now.saturating_sub(crate::platform::frame::interval());
            self.last.map_or(frame, |last| frame.max(last.min(now)))
        });
        for e in self.entries.values_mut() {
            e.seen = false;
        }
        let around = Around {
            parent: None,
            index: 0,
            anchor: Point::new(0.0, 0.0),
        };
        let ctx = Ctx {
            specs,
            offsets,
            now,
            head,
            first,
            resized,
        };
        self.walk(tree, &ctx, around);

        // The elements that left: ghosts, if they leave with an effect.
        let gone: Vec<ElementKey> = self
            .entries
            .iter()
            .filter(|(_, e)| !e.seen)
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            let Some(e) = self.entries.remove(&id) else {
                continue;
            };
            let (Some(effect), Some(parent), Some(now), Some(head), Some(old)) =
                (e.exit, e.parent, now, head, old)
            else {
                continue;
            };
            let Some(drawn) = find(old, id) else {
                continue;
            };
            let from = e.shown.as_ref().map_or(1.0, |s| s.value(now));
            let mut progress = Animated::new(from);
            progress.animate_to(0.0, effect.transition.0, head);
            if !progress.is_animating(now) {
                continue; // Reduced motion: gone at once.
            }
            let mut node = drawn.clone();
            strip(&mut node);
            self.ghosts.push(Ghost {
                id,
                node,
                parent,
                index: e.index,
                offset: e.offset,
                progress,
                effect,
            });
        }

        // Draw the ghosts under their parents, where they were.
        if let Some(now) = now {
            self.ghosts.retain(|g| g.progress.is_animating(now));
            self.ghosts.sort_by_key(|g| g.index);
            let mut lost = Vec::new();
            for (i, g) in self.ghosts.iter().enumerate() {
                let Some(parent) = find_mut(tree, g.parent) else {
                    lost.push(i);
                    continue;
                };
                let mut node = g.node.clone();
                let at = add(content_origin(parent, offsets), g.offset);
                let by = sub(at, node.bounds.origin);
                shift(&mut node, by);
                let (opacity, t) = g.effect.at(g.progress.value(now));
                apply(&mut node, opacity, t);
                let index = g.index.min(parent.children.len());
                parent.children.insert(index, node);
            }
            for i in lost.into_iter().rev() {
                self.ghosts.remove(i);
            }
        } else {
            self.ghosts.clear();
        }
        self.last = now;
    }

    fn walk(&mut self, node: &mut LayoutNode, ctx: &Ctx<'_>, around: Around) {
        let mut anchor = around.anchor;
        if let Some(id) = node.key
            && let Some(spec) = ctx.specs.get(&id)
        {
            if spec.layout.is_some() {
                // Its children are measured from where it's laid out; its
                // own motion carries them.
                anchor = node.bounds.origin;
            }
            self.animate(node, id, spec, ctx, around);
        }
        if node.scroll.is_some() {
            anchor = content_origin(node, ctx.offsets);
        }
        let parent = node.key.map(|k| (k, content_origin(node, ctx.offsets)));
        for (index, child) in node.children.iter_mut().enumerate() {
            let around = Around {
                parent,
                index,
                anchor,
            };
            self.walk(child, ctx, around);
        }
    }

    /// Animate element `id` at `node`.
    fn animate(
        &mut self,
        node: &mut LayoutNode,
        id: ElementKey,
        spec: &PresenceSpec,
        ctx: &Ctx<'_>,
        around: Around,
    ) {
        let returning = self.ghosts.iter().position(|g| g.id == id);
        let ghost = returning.map(|i| self.ghosts.remove(i));
        let fresh = !self.entries.contains_key(&id);
        let entry = self.entries.entry(id).or_insert_with(|| Entry {
            shown: None,
            effect: None,
            pos: None,
            size: None,
            exit: None,
            parent: None,
            index: 0,
            offset: Vec2::new(0.0, 0.0),
            seen: false,
        });
        entry.seen = true;
        entry.exit = spec.exit;
        entry.parent = around.parent.map(|(k, _)| k);
        entry.index = around.index;
        if let Some((_, origin)) = around.parent {
            entry.offset = sub(node.bounds.origin, origin);
        }
        let (Some(now), Some(head)) = (ctx.now, ctx.head) else {
            entry.shown = None;
            entry.pos = None;
            entry.size = None;
            return;
        };

        // Entering: from its effect (or, coming back, from where it was).
        if fresh {
            if let Some(g) = ghost {
                entry.shown = Some(g.progress);
                entry.effect = Some(spec.enter.unwrap_or(g.effect));
            } else if let (Some(effect), false) = (spec.enter, ctx.first) {
                entry.shown = Some(Animated::new(0.0));
                entry.effect = Some(effect);
            }
            if let (Some(shown), Some(effect)) = (entry.shown.as_mut(), entry.effect) {
                shown.animate_to(1.0, effect.transition.0, head);
            }
        }
        let mut opacity = 1.0;
        let mut effect_t = Transform::IDENTITY;
        if let (Some(shown), Some(effect)) = (entry.shown.as_ref(), entry.effect) {
            if shown.is_animating(now) {
                (opacity, effect_t) = effect.at(shown.value(now));
            } else {
                entry.shown = None;
            }
        }

        // Moving: from where it was drawn to where it's laid out.
        let mut flip = Transform::IDENTITY;
        match spec.layout {
            Some(t) => {
                let laid = node.bounds;
                let rel = Point::new(
                    laid.origin.x - around.anchor.x,
                    laid.origin.y - around.anchor.y,
                );
                let snap = fresh || ctx.resized;
                let pos = match entry.pos.as_mut() {
                    Some(p) if !snap => {
                        p.animate_to(rel, t.0, head);
                        p.value(now)
                    }
                    _ => {
                        entry.pos = Some(Animated::new(rel));
                        rel
                    }
                };
                let size = match (spec.layout_size, entry.size.as_mut()) {
                    (false, _) => {
                        entry.size = None;
                        laid.size
                    }
                    (true, Some(s)) if !snap => {
                        s.animate_to(laid.size, t.0, head);
                        s.value(now)
                    }
                    (true, _) => {
                        entry.size = Some(Animated::new(laid.size));
                        laid.size
                    }
                };
                let drawn = add(around.anchor, Vec2::new(pos.x, pos.y));
                let ratio = |d: f64, l: f64| if l > 0.0 { d / l } else { 1.0 };
                flip = Transform {
                    translate: sub(drawn, laid.origin),
                    scale: Vec2::new(
                        ratio(size.width, laid.size.width),
                        ratio(size.height, laid.size.height),
                    ),
                    rotate: 0.0,
                    origin: Point::new(0.0, 0.0),
                };
            }
            None => {
                entry.pos = None;
                entry.size = None;
            }
        }

        if opacity != 1.0 || !effect_t.is_identity() || !flip.is_identity() {
            apply(node, opacity, effect_t);
            apply(node, 1.0, flip);
        }
    }

    /// The parents of the elements leaving or that may leave, which must
    /// keep an identity on the laid-out tree for their ghosts to be found by.
    pub(crate) fn anchors(&self) -> std::collections::HashSet<ElementKey> {
        let may_leave = self
            .entries
            .values()
            .filter(|e| e.exit.is_some())
            .filter_map(|e| e.parent);
        self.ghosts
            .iter()
            .map(|g| g.parent)
            .chain(may_leave)
            .collect()
    }

    /// Whether anything still moves (as of the last frame).
    pub(crate) fn animating(&self) -> bool {
        let Some(now) = self.now else {
            return false;
        };
        !self.ghosts.is_empty()
            || self.entries.values().any(|e| {
                e.shown.as_ref().is_some_and(|a| a.is_animating(now))
                    || e.pos.as_ref().is_some_and(|a| a.is_animating(now))
                    || e.size.as_ref().is_some_and(|a| a.is_animating(now))
            })
    }
}

/// The frame's inputs, for the walk.
struct Ctx<'a> {
    specs: &'a HashMap<ElementKey, PresenceSpec>,
    offsets: &'a HashMap<ScrollId, f64>,
    now: Option<Duration>,
    head: Option<Duration>,
    first: bool,
    resized: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::set_reduced_motion;
    use crate::core::Element;
    use crate::core::interact::{Inputs, Resolver, collect};
    use crate::core::transition::Transitions;
    use crate::layout::Axis;

    const MS: Duration = Duration::from_millis(1);
    const T0: Duration = Duration::from_secs(1);
    const WINDOW: Size = Size::new(200.0, 200.0);

    fn frame() -> Duration {
        crate::platform::frame::interval()
    }

    fn linear() -> Transition {
        Transition::tween(Duration::from_millis(100), Easing::Linear)
    }

    /// The App's frame pipeline, at fixed times.
    #[derive(Default)]
    struct Frames {
        transitions: Transitions,
        presence: Presence,
        tree: Option<LayoutNode>,
        offsets: HashMap<ScrollId, f64>,
    }

    impl Frames {
        fn build(&mut self, mut el: Element, now: Duration) -> &LayoutNode {
            self.build_in(&mut el, now, WINDOW)
        }

        fn build_in(&mut self, el: &mut Element, now: Duration, size: Size) -> &LayoutNode {
            self.transitions.begin(Some(now));
            let states = HashMap::new();
            let mut r = Resolver {
                states: &states,
                transitions: &mut self.transitions,
                watch: Default::default(),
                styled_actions: Vec::new(),
                styled_drops: Vec::new(),
                presence: HashMap::new(),
                anchors: self.presence.anchors(),
            };
            r.resolve(el, ElementKey::root(0));
            let specs = std::mem::take(&mut r.presence);
            self.transitions.end();
            let win = Rect::from_xywh(0.0, 0.0, size.width, size.height);
            let mut tree = crate::core::layout(el, win, None);
            crate::core::apply_scroll(&mut tree, &mut self.offsets);
            let old = self.tree.as_ref();
            self.presence
                .update(&mut tree, &specs, old, &self.offsets, Some(now), size);
            self.tree = Some(tree);
            self.tree.as_ref().unwrap()
        }

        fn animating(&self) -> bool {
            self.presence.animating()
        }
    }

    /// A 20 px tall row keyed `k`.
    fn item(k: &str) -> Element {
        Element::stack(Axis::Vertical, vec![])
            .width(100.0)
            .height(20.0)
            .key(k.to_string())
    }

    fn list(items: Vec<Element>) -> Element {
        Element::stack(Axis::Vertical, items)
    }

    /// Where `node`'s top-left corner is drawn.
    fn drawn(node: &LayoutNode) -> Point {
        node.affine()
            .map_or(node.bounds.origin, |m| m.apply(node.bounds.origin))
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn an_element_fades_in_when_it_appears_but_not_on_the_first_frame() {
        let fade = || item("c").enter(Effect::fade().transition(linear()));
        let mut f = Frames::default();
        let first = f.build(list(vec![item("a"), fade()]), T0);
        assert_eq!(first.children[1].opacity, 1.0, "the first frame shows it");
        assert!(!f.animating());

        let mut f = Frames::default();
        f.build(list(vec![item("a")]), T0);
        let t1 = T0 + 500 * MS;
        let view = || list(vec![item("a"), fade()]);
        // On its own frame it's already one frame in.
        let o = f.build(view(), t1).children[1].opacity as f64;
        assert!((o - frame().as_secs_f64() / 0.1).abs() < 1e-4, "{o}");
        assert!(f.animating());
        let mid = f.build(view(), t1 + 50 * MS - frame()).children[1].opacity;
        assert!((mid as f64 - 0.5).abs() < 1e-4, "half-way: {mid}");
        let end = f.build(view(), t1 + 100 * MS - frame());
        assert_eq!(end.children[1].opacity, 1.0);
        assert!(!f.animating(), "in: no more frames");
    }

    #[test]
    fn slide_and_scale_compose_with_the_elements_own_transform() {
        let effect = Effect::slide(0.0, 40.0)
            .and(Effect::scale(0.5))
            .transition(linear());
        let own = Transform::IDENTITY.translate(5.0, 0.0);
        let view = |on: bool| {
            let mut kids = vec![item("a")];
            if on {
                kids.push(item("b").transform(own).enter(effect));
            }
            list(kids)
        };
        let mut f = Frames::default();
        f.build(view(false), T0);
        f.build(view(true), T0 + frame());
        let mid = f.build(view(true), T0 + 50 * MS).children[1].clone();
        let t = mid.transform.expect("moving");
        // Half-way: 20 px down, at 75 % size, still 5 px right.
        assert!(close(t.scale.dx, 0.75) && close(t.scale.dy, 0.75), "{t:?}");
        // The center: scaled about itself, so only the offsets move it (the
        // element's own 5 px, scaled with it).
        let b = mid.bounds;
        let center = Point::new(b.min_x() + b.width() / 2.0, b.min_y() + b.height() / 2.0);
        let at = mid.affine().unwrap().apply(center);
        let (dx, dy) = (at.x - center.x, at.y - center.y);
        assert!(close(dx, 3.75) && close(dy, 20.0), "{dx} {dy}");
    }

    #[test]
    fn a_leaving_element_is_drawn_as_a_ghost_until_its_exit_ends() {
        let gone = || item("b").exit(Effect::fade().transition(linear()));
        let mut f = Frames::default();
        f.build(list(vec![item("a"), gone(), item("c")]), T0);
        let t1 = T0 + 500 * MS;
        let after = || list(vec![item("a"), item("c")]);
        let tree = f.build(after(), t1).clone();
        assert_eq!(tree.children.len(), 3, "a, the ghost of b, c");
        let ghost = &tree.children[1];
        assert!(ghost.ghost && ghost.key.is_none());
        // Where it was, over c, which has closed up beneath it.
        assert_eq!(ghost.bounds.origin.y, 20.0);
        assert_eq!(tree.children[2].bounds.origin.y, 20.0);
        assert!(ghost.opacity < 1.0 && ghost.opacity > 0.8);
        assert!(f.animating(), "leaving: frames");
        // Not hovered, hit or read: the pointer over it finds c.
        let inputs = Inputs {
            pointer: Some(Point::new(10.0, 25.0)),
            ..Inputs::default()
        };
        let states = collect(&tree, &inputs);
        assert!(states[&ElementKey::new("c".to_string())].hovered());
        let a11y = crate::core::a11y::accessibility_tree(&tree, None);
        assert_eq!(a11y.count(), 1, "nothing to read: {a11y:?}");

        let mid = f.build(after(), t1 + 50 * MS - frame()).children[1].opacity;
        assert!((mid as f64 - 0.5).abs() < 1e-4, "{mid}");
        let end = f.build(after(), t1 + 100 * MS - frame());
        assert_eq!(end.children.len(), 2, "gone");
        assert!(!f.animating(), "no more frames");
    }

    #[test]
    fn an_element_that_comes_back_while_leaving_turns_round() {
        let b = || item("b").appear(Effect::fade().transition(linear()));
        let mut f = Frames::default();
        f.build(list(vec![item("a"), b()]), T0);
        let t1 = T0 + 500 * MS;
        f.build(list(vec![item("a")]), t1);
        let half = f.build(list(vec![item("a")]), t1 + 50 * MS - frame());
        assert!((half.children[1].opacity as f64 - 0.5).abs() < 1e-4);
        // Back: no ghost, and it comes in from half-way.
        let t2 = t1 + 50 * MS;
        let back = f.build(list(vec![item("a"), b()]), t2);
        assert_eq!(back.children.len(), 2);
        assert!(!back.children[1].ghost);
        let o = back.children[1].opacity as f64;
        // (Easing back in from its speed out: a little further first.)
        assert!(o > 0.35 && o < 0.6, "{o}");
        let end = f.build(list(vec![item("a"), b()]), t2 + 100 * MS);
        assert_eq!(end.children[1].opacity, 1.0);
        assert!(!f.animating());
    }

    #[test]
    fn a_reorder_moves_items_from_where_they_were() {
        let it = |k: &str| item(k).animate_position(linear());
        let mut f = Frames::default();
        f.build(list(vec![it("a"), it("b")]), T0);
        assert!(!f.animating(), "first sight: in place");
        let t1 = T0 + 500 * MS;
        let swapped = || list(vec![it("b"), it("a")]);
        f.build(swapped(), t1);
        assert!(f.animating());
        let mid = f.build(swapped(), t1 + 50 * MS - frame()).clone();
        // b goes from 20 up to 0, a from 0 down to 20: both half-way.
        assert!(close(drawn(&mid.children[0]).y, 10.0), "{mid:?}");
        assert!(close(drawn(&mid.children[1]).y, 10.0));
        assert_eq!(mid.children[1].bounds.origin.y, 20.0, "laid out there");
        let end = f.build(swapped(), t1 + 100 * MS - frame());
        assert_eq!(end.children[0].transform, None);
        assert_eq!(end.children[1].transform, None);
        assert!(!f.animating());
    }

    #[test]
    fn a_reorder_mid_flight_heads_back_from_where_it_is() {
        let it = |k: &str| item(k).animate_position(linear());
        let mut f = Frames::default();
        f.build(list(vec![it("a"), it("b")]), T0);
        let t1 = T0 + 500 * MS;
        f.build(list(vec![it("b"), it("a")]), t1);
        let t2 = t1 + 50 * MS - frame();
        let mid = f.build(list(vec![it("b"), it("a")]), t2);
        let was = drawn(&mid.children[1]).y;
        assert!(close(was, 10.0));
        // Swapped back a frame later: a is near 10 still, not at 0 or 20.
        let back = f.build(list(vec![it("a"), it("b")]), t2 + frame());
        let now = drawn(&back.children[0]).y;
        assert!((now - was).abs() < 5.0, "{was} -> {now}");
        assert!(f.animating());
        let end = f.build(list(vec![it("a"), it("b")]), t2 + 300 * MS);
        assert_eq!(drawn(&end.children[0]).y, 0.0);
        assert!(!f.animating());
    }

    #[test]
    fn a_size_change_scales_from_the_old_size() {
        let it = |h: f64| {
            Element::stack(Axis::Vertical, vec![])
                .width(100.0)
                .height(h)
                .key("p")
                .animate_layout(linear())
        };
        let mut f = Frames::default();
        f.build(list(vec![it(20.0)]), T0);
        let t1 = T0 + 500 * MS;
        f.build(list(vec![it(60.0)]), t1);
        let mid = f.build(list(vec![it(60.0)]), t1 + 50 * MS - frame());
        let t = mid.children[0].transform.expect("scaling");
        assert!(close(t.scale.dy, 40.0 / 60.0), "{t:?}");
        assert!(close(drawn(&mid.children[0]).y, 0.0), "anchored at the top");
    }

    #[test]
    fn scrolling_and_resizing_do_not_animate() {
        let it = |k: &str| item(k).animate_position(linear());
        let mut f = Frames::default();
        let view = || {
            let mut el = list((0..20).map(|i| it(&format!("{i}"))).collect()).height(100.0);
            el.scroll = Some(ScrollId(1));
            list(vec![el])
        };
        f.build(view(), T0);
        f.offsets.insert(ScrollId(1), 50.0);
        let t = f.build(view(), T0 + 100 * MS).clone();
        assert!(!f.animating(), "scrolled, not moved");
        assert!(t.children[0].children.iter().all(|c| c.transform.is_none()));
        // A centered item moves when the window resizes: it snaps.
        let centered = || {
            list(vec![it("x").width(20.0)])
                .align(crate::core::Align::Center, crate::core::Align::Center)
        };
        let mut f = Frames::default();
        f.build(centered(), T0);
        let mut el = centered();
        f.build_in(&mut el, T0 + 100 * MS, Size::new(300.0, 300.0));
        assert!(!f.animating(), "resized: snapped");
    }

    #[test]
    fn reduced_motion_snaps_everything() {
        set_reduced_motion(Some(true));
        let it = |k: &str| {
            item(k)
                .appear(Effect::fade().transition(linear()))
                .animate_position(linear())
        };
        let mut f = Frames::default();
        f.build(list(vec![it("a"), it("b")]), T0);
        let t1 = T0 + 500 * MS;
        let swapped = f.build(list(vec![it("c"), it("b"), it("a")]), t1).clone();
        assert!(swapped.children.iter().all(|c| c.opacity == 1.0));
        assert!(swapped.children.iter().all(|c| c.transform.is_none()));
        let gone = f.build(list(vec![it("c")]), t1 + 10 * MS);
        assert_eq!(gone.children.len(), 1, "no ghosts");
        assert!(!f.animating());
        set_reduced_motion(None);
    }

    #[test]
    fn composing_matches_applying_one_after_the_other() {
        let b = Rect::from_xywh(10.0, 20.0, 40.0, 30.0);
        let inner = Transform::IDENTITY
            .scale_xy(1.5, 0.5)
            .translate(3.0, -2.0)
            .origin(0.2, 0.7);
        let outer = Transform::IDENTITY
            .scale(2.0)
            .rotate_degrees(30.0)
            .translate(-4.0, 6.0);
        let both = compose(outer, Some(inner), b).unwrap();
        let (i, o, c) = (inner.to_affine(b), outer.to_affine(b), both.to_affine(b));
        for p in [Point::new(0.0, 0.0), Point::new(13.0, -7.0), b.origin] {
            let want = o.apply(i.apply(p));
            let got = c.apply(p);
            assert!(
                close(want.x, got.x) && close(want.y, got.y),
                "{want:?} {got:?}"
            );
        }
    }
}
