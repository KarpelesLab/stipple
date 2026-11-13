//! Raw pointer and wheel events for custom elements: every button, the
//! modifiers held, click counts, moves, the pointer leaving, and wheel and
//! trackpad deltas with their units and phase, and two-finger pinches on
//! touch screens. See
//! [`Element::on_pointer`](crate::core::Element::on_pointer),
//! [`Element::on_wheel`](crate::core::Element::on_wheel) and
//! [`Element::on_pinch`](crate::core::Element::on_pinch), and
//! `docs/pointer-events.md`.

use crate::core::runtime::LayoutNode;
use crate::geometry::{Point, Size, Vec2};
use crate::platform::{Modifiers, PointerButton, ScrollPhase, ScrollUnit};

/// An opaque handle to an element with pointer or wheel handlers (see
/// [`Element::on_pointer`](crate::core::Element::on_pointer)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PointerId(pub(crate) u32);

/// What a [`PointerEvent`] reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PointerEventKind {
    /// A button went down over the element. The element then captures the
    /// pointer: it gets every move and release until the last button is up,
    /// even outside it (and outside the window).
    Down,
    /// A button came up.
    Up,
    /// The pointer moved over the element, or anywhere while it holds the
    /// capture.
    Move,
    /// The pointer left the element: it moved onto something else, or out
    /// of the window. Not sent while the element holds the capture; it comes
    /// with the last button's release instead, if the pointer is elsewhere
    /// by then.
    Leave,
}

/// A set of pointer buttons, such as the ones held down.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PointerButtons(u32);

impl PointerButtons {
    /// No buttons.
    pub const NONE: PointerButtons = PointerButtons(0);

    fn bit(button: PointerButton) -> u32 {
        match button {
            PointerButton::Left => 1,
            PointerButton::Right => 2,
            PointerButton::Middle => 4,
            PointerButton::Back => 8,
            PointerButton::Forward => 16,
            PointerButton::Other(n) => 1 << (5 + u32::from(n) % 27),
        }
    }

    /// Whether `button` is in the set.
    pub fn contains(self, button: PointerButton) -> bool {
        self.0 & Self::bit(button) != 0
    }

    /// Whether the set is empty.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The set with `button` added.
    pub fn with(self, button: PointerButton) -> Self {
        Self(self.0 | Self::bit(button))
    }

    /// The set without `button`.
    pub fn without(self, button: PointerButton) -> Self {
        Self(self.0 & !Self::bit(button))
    }
}

/// A pointer event delivered to an element's
/// [`on_pointer`](crate::core::Element::on_pointer) handler.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointerEvent {
    /// What happened.
    pub kind: PointerEventKind,
    /// The button that went down or up; `None` for moves and leaves.
    pub button: Option<PointerButton>,
    /// The buttons held after this event (so a release no longer counts its
    /// own button).
    pub buttons_held: PointerButtons,
    /// The pointer in the element's own coordinates (logical pixels from its
    /// top-left corner), through any transform on the way to it. Outside the
    /// element's size while it holds the capture and the pointer is
    /// elsewhere.
    pub position: Point,
    /// The pointer in window coordinates (logical pixels).
    pub window_position: Point,
    /// The element's laid-out size, to tell a position against (a terminal
    /// turning it into a cell).
    pub size: Size,
    /// The modifier keys held.
    pub modifiers: Modifiers,
    /// For a press, 1 for a single click, 2 for a double click, 3 for a
    /// triple; then it starts over. Presses count together when they are of
    /// the same button, close in time and place (the platform's double-click
    /// time and distance, see [`double_click`](crate::platform::double_click)).
    /// A release has its press's count; moves and leaves have 0.
    pub click_count: u32,
}

impl PointerEvent {
    /// Whether the pointer is over the element (not when it holds the capture
    /// and the pointer is elsewhere, nor for a leave).
    pub fn is_inside(&self) -> bool {
        self.kind != PointerEventKind::Leave
            && self.position.x >= 0.0
            && self.position.y >= 0.0
            && self.position.x < self.size.width
            && self.position.y < self.size.height
    }
}

/// A wheel or trackpad scroll over an element with an
/// [`on_wheel`](crate::core::Element::on_wheel) handler (which then takes it
/// instead of the scroll container it is in).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WheelEvent {
    /// The horizontal delta: positive scrolls right (towards the end of the
    /// content), in `unit`s.
    pub dx: f64,
    /// The vertical delta: positive scrolls down (towards the end of the
    /// content), in `unit`s.
    pub dy: f64,
    /// Lines (wheel notches: scroll that many lines) or logical pixels
    /// (trackpads: accumulate them).
    pub unit: ScrollUnit,
    /// Where a trackpad gesture is, where the platform says.
    pub phase: ScrollPhase,
    /// The pointer in the element's coordinates (see
    /// [`PointerEvent::position`]).
    pub position: Point,
    /// The pointer in window coordinates.
    pub window_position: Point,
    /// The modifier keys held.
    pub modifiers: Modifiers,
}

/// Where a [`PinchEvent`] is in its gesture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PinchPhase {
    /// The second finger landed: the gesture starts (scale 1, no rotation or
    /// translation yet).
    Began,
    /// A finger moved.
    Changed,
    /// A finger lifted: the gesture ends with these values.
    Ended,
    /// The system took the touches away (an incoming call, a system
    /// gesture): undo what the gesture did, if that matters.
    Cancelled,
}

/// A two-finger gesture on a touch screen over an element with an
/// [`on_pinch`](crate::core::Element::on_pinch) handler: pinching (scale),
/// panning with both fingers (translation) and turning (rotation), all at
/// once and each measured from where the gesture began, so a handler applies
/// them to the value it had at [`Began`](PinchPhase::Began).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PinchEvent {
    /// Where the gesture is.
    pub phase: PinchPhase,
    /// The distance between the fingers over the distance when it began:
    /// above 1 spreading apart (zoom in), below 1 pinching together.
    pub scale: f64,
    /// How far the line between the fingers turned since it began, in
    /// radians, clockwise on screen.
    pub rotation: f64,
    /// How far the midpoint between the fingers moved since it began
    /// (logical pixels): a two-finger pan.
    pub translation: Vec2,
    /// The midpoint between the fingers in the element's coordinates (see
    /// [`PointerEvent::position`]): the point to zoom about.
    pub centroid: Point,
    /// The midpoint in window coordinates.
    pub window_centroid: Point,
    /// The element's laid-out size.
    pub size: Size,
}

type PointerFn<S> = Box<dyn FnMut(&PointerEvent, &mut S)>;
type WheelFn<S> = Box<dyn FnMut(&WheelEvent, &mut S)>;
type PinchFn<S> = Box<dyn FnMut(&PinchEvent, &mut S)>;

/// An element's pointer, wheel and pinch handlers.
pub(crate) struct Slot<S> {
    pointer: Option<PointerFn<S>>,
    wheel: Option<WheelFn<S>>,
    pinch: Option<PinchFn<S>>,
}

/// The pointer and wheel handlers registered while building a frame, by
/// [`PointerId`].
pub(crate) struct PointerHandlers<S> {
    slots: Vec<Slot<S>>,
}

impl<S> Default for PointerHandlers<S> {
    fn default() -> Self {
        Self { slots: Vec::new() }
    }
}

impl<S> PointerHandlers<S> {
    /// The slot `id` names, or a new one.
    fn slot(&mut self, id: Option<PointerId>) -> PointerId {
        match id {
            Some(id) if (id.0 as usize) < self.slots.len() => id,
            _ => {
                self.slots.push(Slot {
                    pointer: None,
                    wheel: None,
                    pinch: None,
                });
                PointerId(self.slots.len() as u32 - 1)
            }
        }
    }

    /// The handlers, by id (for a memoized subtree to carry its own over).
    pub(crate) fn into_slots(self) -> Vec<Slot<S>> {
        self.slots
    }

    /// Add `slot` under the next id.
    pub(crate) fn push_slot(&mut self, slot: Slot<S>) {
        self.slots.push(slot);
    }

    /// Set the pointer handler of `id` (or of a new id), returning the id.
    pub(crate) fn set_pointer(
        &mut self,
        id: Option<PointerId>,
        handler: impl FnMut(&PointerEvent, &mut S) + 'static,
    ) -> PointerId {
        let id = self.slot(id);
        self.slots[id.0 as usize].pointer = Some(Box::new(handler));
        id
    }

    /// Set the wheel handler of `id` (or of a new id), returning the id.
    pub(crate) fn set_wheel(
        &mut self,
        id: Option<PointerId>,
        handler: impl FnMut(&WheelEvent, &mut S) + 'static,
    ) -> PointerId {
        let id = self.slot(id);
        self.slots[id.0 as usize].wheel = Some(Box::new(handler));
        id
    }

    /// Set the pinch handler of `id` (or of a new id), returning the id.
    pub(crate) fn set_pinch(
        &mut self,
        id: Option<PointerId>,
        handler: impl FnMut(&PinchEvent, &mut S) + 'static,
    ) -> PointerId {
        let id = self.slot(id);
        self.slots[id.0 as usize].pinch = Some(Box::new(handler));
        id
    }

    /// Whether `id` has a pointer handler.
    pub(crate) fn has_pointer(&self, id: PointerId) -> bool {
        self.slots
            .get(id.0 as usize)
            .is_some_and(|s| s.pointer.is_some())
    }

    /// Whether `id` has a wheel handler.
    pub(crate) fn has_wheel(&self, id: PointerId) -> bool {
        self.slots
            .get(id.0 as usize)
            .is_some_and(|s| s.wheel.is_some())
    }

    /// Whether `id` has a pinch handler.
    pub(crate) fn has_pinch(&self, id: PointerId) -> bool {
        self.slots
            .get(id.0 as usize)
            .is_some_and(|s| s.pinch.is_some())
    }

    /// Run `id`'s pointer handler. Returns whether there was one.
    pub(crate) fn dispatch_pointer(
        &mut self,
        id: PointerId,
        ev: &PointerEvent,
        state: &mut S,
    ) -> bool {
        match self
            .slots
            .get_mut(id.0 as usize)
            .and_then(|s| s.pointer.as_mut())
        {
            Some(handler) => {
                handler(ev, state);
                true
            }
            None => false,
        }
    }

    /// Run `id`'s wheel handler. Returns whether there was one.
    pub(crate) fn dispatch_wheel(&mut self, id: PointerId, ev: &WheelEvent, state: &mut S) -> bool {
        match self
            .slots
            .get_mut(id.0 as usize)
            .and_then(|s| s.wheel.as_mut())
        {
            Some(handler) => {
                handler(ev, state);
                true
            }
            None => false,
        }
    }

    /// Run `id`'s pinch handler. Returns whether there was one.
    pub(crate) fn dispatch_pinch(&mut self, id: PointerId, ev: &PinchEvent, state: &mut S) -> bool {
        match self
            .slots
            .get_mut(id.0 as usize)
            .and_then(|s| s.pinch.as_mut())
        {
            Some(handler) => {
                handler(ev, state);
                true
            }
            None => false,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }
}

/// The top-most node under `point` (window coordinates) with a pointer id
/// that `wants`, through transforms and clipping, as the other `*_at`
/// queries find theirs.
pub(crate) fn pointer_at(
    node: &LayoutNode,
    point: Point,
    wants: &dyn Fn(PointerId) -> bool,
) -> Option<PointerId> {
    let point = node.to_local(point)?;
    if crate::core::runtime::clipped_out(node, point) {
        return None;
    }
    for child in node.children.iter().rev() {
        if let Some(id) = pointer_at(child, point, wants) {
            return Some(id);
        }
    }
    match node.pointer {
        Some(id) if wants(id) && node.bounds.contains(point) => Some(id),
        _ => None,
    }
}

/// What takes a wheel event at a point: the innermost of an element with a
/// wheel handler and a scroll container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WheelTarget {
    Handler(PointerId),
    Scroll,
}

/// The innermost node under `point` that takes the wheel: one with a wheel
/// handler (`wants`) or a scroll container.
pub(crate) fn wheel_target_at(
    node: &LayoutNode,
    point: Point,
    wants: &dyn Fn(PointerId) -> bool,
) -> Option<WheelTarget> {
    let point = node.to_local(point)?;
    if crate::core::runtime::clipped_out(node, point) {
        return None;
    }
    for child in node.children.iter().rev() {
        if let Some(target) = wheel_target_at(child, point, wants) {
            return Some(target);
        }
    }
    if !node.bounds.contains(point) {
        return None;
    }
    match node.pointer {
        Some(id) if wants(id) => Some(WheelTarget::Handler(id)),
        _ => node.scroll.map(|_| WheelTarget::Scroll),
    }
}

/// Where element `id` is: `point` (window coordinates) in its own
/// coordinates, through every transform on the way, and its size. `None` if
/// it isn't in `tree`.
pub(crate) fn pointer_local(
    tree: &LayoutNode,
    id: PointerId,
    point: Point,
) -> Option<(Point, Size)> {
    let is = |n: &LayoutNode| n.pointer == Some(id);
    let node = find_pointer(tree, id)?;
    let at = crate::core::local_point(tree, &is, point).unwrap_or(point);
    let b = node.bounds;
    Some((
        Point::new(at.x - b.min_x(), at.y - b.min_y()),
        Size::new(b.width(), b.height()),
    ))
}

/// The node carrying pointer id `id`.
pub(crate) fn find_pointer(node: &LayoutNode, id: PointerId) -> Option<&LayoutNode> {
    if node.pointer == Some(id) {
        return Some(node);
    }
    node.children.iter().find_map(|c| find_pointer(c, id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_sets() {
        let held = PointerButtons::NONE
            .with(PointerButton::Middle)
            .with(PointerButton::Other(12));
        assert!(held.contains(PointerButton::Middle));
        assert!(held.contains(PointerButton::Other(12)));
        assert!(!held.contains(PointerButton::Left));
        let held = held
            .without(PointerButton::Middle)
            .without(PointerButton::Other(12));
        assert!(held.is_empty());
    }
}
