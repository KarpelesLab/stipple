//! The pointer as of the event being handled: where it is, where the current
//! drag started, and which modifier keys are held.
//!
//! Tap and drag handlers receive only the app state, so a widget that needs
//! more — a list extending its selection on Shift+click, a column header
//! resized by dragging its edge — reads it here while handling the event. The
//! app records the position before dispatching; each backend records the
//! modifiers with every button event. It is per thread, and the UI runs on
//! one, so tests drive it with [`set_pointer_modifiers`].

use crate::geometry::Point;
use crate::platform::Modifiers;
pub use crate::platform::set_pointer_modifiers;
use std::cell::Cell;

/// The pointer state for the event being handled. See [`pointer`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PointerInfo {
    /// The pointer position, in window logical pixels.
    pub position: Point,
    /// Where the press that started the current drag landed, while dragging.
    pub drag_origin: Option<Point>,
    /// The modifier keys held at the last pointer button event.
    pub modifiers: Modifiers,
}

thread_local! {
    static POSITION: Cell<(Point, Option<Point>)> =
        const { Cell::new((Point::new(0.0, 0.0), None)) };
}

/// The pointer state for the event being handled.
pub fn pointer() -> PointerInfo {
    let (position, drag_origin) = POSITION.with(Cell::get);
    PointerInfo {
        position,
        drag_origin,
        modifiers: crate::platform::pointer_modifiers(),
    }
}

/// Record the pointer position (and the current drag's origin) before
/// dispatching a pointer handler.
pub(crate) fn set_pointer_position(position: Point, drag_origin: Option<Point>) {
    POSITION.with(|p| p.set((position, drag_origin)));
}
