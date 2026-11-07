//! The modifier keys held at the last pointer button event.
//!
//! [`Event::PointerButton`](crate::platform::Event::PointerButton) carries no
//! modifiers, so each backend records them here just before delivering a
//! button event, and widgets read them back (through
//! `stipple::core::pointer`) to tell a Shift+click or Ctrl+click from a plain
//! one. Per thread: events are delivered on the UI thread.

use crate::platform::Modifiers;
use std::cell::Cell;

thread_local! {
    static MODIFIERS: Cell<Modifiers> = const {
        Cell::new(Modifiers {
            shift: false,
            ctrl: false,
            alt: false,
            meta: false,
        })
    };
}

/// The modifier keys held at the last pointer button event on this thread.
pub fn pointer_modifiers() -> Modifiers {
    MODIFIERS.with(Cell::get)
}

/// Record the modifier keys held at a pointer button event. Backends call this
/// before delivering the event; tests call it to simulate Shift/Ctrl+click.
pub fn set_pointer_modifiers(modifiers: Modifiers) {
    MODIFIERS.with(|m| m.set(modifiers));
}
