//! A pad that prints every pointer and wheel event it gets.
//!
//! Used by the CI X11 job to verify raw pointer events end to end: `xdotool`
//! middle-clicks, double-clicks, scrolls sideways (buttons 6 and 7) and moves
//! out of the window, and the job checks what arrived. The pad also lights up
//! while hovered (an interaction-state look), which must go away when the
//! pointer leaves the window, and hides the cursor while the user types
//! until the pointer moves.
//!
//! The window is 400×300, at the screen's top-left corner under Xvfb (no
//! window manager), all of it the pad.

use std::cell::Cell;

use stipple::core::{PointerEvent, PointerEventKind, WheelEvent};
use stipple::prelude::*;

#[derive(Default)]
struct Pad {
    /// The cursor is hidden: a key was typed since the pointer last moved.
    typing: bool,
    /// The hover state last printed.
    hovered: Cell<Option<bool>>,
}

fn view(pad: &Pad, cx: &mut Cx<Pad>) -> Element {
    let theme = *cx.theme();
    let hovered = cx.interaction("pad").hovered();
    if pad.hovered.replace(Some(hovered)) != Some(hovered) {
        println!("hovered: {hovered}");
    }
    let hint = if pad.typing {
        "Typing: the cursor is hidden"
    } else {
        "Click, scroll, type"
    };
    Element::stack(
        Axis::Vertical,
        vec![Element::text(hint, 20.0, theme.palette.text)],
    )
    .fill(theme.palette.surface)
    .align(Align::Center, Align::Center)
    .key("pad")
    .hovered(|e| e.fill(theme.palette.primary))
    .cursor(if pad.typing {
        CursorIcon::Hidden
    } else {
        CursorIcon::Crosshair
    })
    .on_pointer(cx, |ev: &PointerEvent, pad: &mut Pad| {
        println!(
            "pointer {:?} button={:?} at=({:.0},{:.0}) clicks={} shift={} ctrl={}",
            ev.kind,
            ev.button,
            ev.position.x,
            ev.position.y,
            ev.click_count,
            ev.modifiers.shift,
            ev.modifiers.ctrl,
        );
        if ev.kind == PointerEventKind::Move && pad.typing {
            pad.typing = false;
            println!("cursor shown");
        }
    })
    .on_wheel(cx, |ev: &WheelEvent, _: &mut Pad| {
        println!(
            "wheel dx={} dy={} unit={:?} phase={:?}",
            ev.dx, ev.dy, ev.unit, ev.phase
        );
    })
    // Clicking focuses the pad; typing then hides the cursor.
    .on_key(cx, |pad: &mut Pad, input: &KeyInput| {
        if !pad.typing && matches!(input, KeyInput::Text(_)) {
            pad.typing = true;
            println!("cursor hidden");
        }
    })
}

fn main() {
    let mut app = App::new(Pad::default(), view)
        .title("Stipple Pointer")
        .theme(Theme::dark())
        .logical_size(Size::new(400.0, 300.0));
    if let Some(font) = Font::system_default() {
        app = app.font(font);
    }
    app.run();
}
