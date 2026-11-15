//! A pad that prints every pointer and wheel event it gets.
//!
//! Used by the CI X11 job to verify raw pointer events end to end: `xdotool`
//! middle-clicks, double-clicks, scrolls sideways (buttons 6 and 7) and moves
//! out of the window, and the job checks what arrived. The pad also lights up
//! while hovered (an interaction-state look), which must go away when the
//! pointer leaves the window, and hides the cursor while the user types
//! until the pointer moves.
//!
//! `L` locks the pointer (and `L` or Escape releases it): the cursor hides
//! and stays put, and the pad prints each relative motion and the total so
//! far, as a game reading mouse-look would. The lock ending by itself (the
//! window losing focus) is printed too. `--lock-test` (CI) locks on the
//! first click, without a key.
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
    /// The pointer is locked, and how far it moved while locked.
    locked: bool,
    moved: (f64, f64),
    /// Lock on the first click (`--lock-test`).
    lock_on_click: bool,
}

fn view(pad: &Pad, cx: &mut Cx<Pad>) -> Element {
    let theme = *cx.theme();
    let hovered = cx.interaction("pad").hovered();
    if pad.hovered.replace(Some(hovered)) != Some(hovered) {
        println!("hovered: {hovered}");
    }
    let hint = if cx.pointer_locked() {
        format!(
            "Locked: moved ({:.0}, {:.0}). L or Esc releases",
            pad.moved.0, pad.moved.1
        )
    } else if pad.typing {
        "Typing: the cursor is hidden".to_string()
    } else {
        "Click, scroll, type; L locks the pointer".to_string()
    };
    let handle = cx.handle();
    let click_handle = cx.handle();
    Element::stack(
        Axis::Vertical,
        vec![Element::text(hint, 18.0, theme.palette.text)],
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
    .on_pointer(cx, move |ev: &PointerEvent, pad: &mut Pad| {
        if ev.kind == PointerEventKind::Motion {
            pad.moved.0 += ev.movement.dx;
            pad.moved.1 += ev.movement.dy;
            println!(
                "motion dx={} dy={} total=({},{})",
                ev.movement.dx, ev.movement.dy, pad.moved.0, pad.moved.1
            );
            return;
        }
        if ev.kind == PointerEventKind::Down && std::mem::take(&mut pad.lock_on_click) {
            click_handle.set_pointer_lock(true);
        }
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
    .on_key(cx, move |pad, input: &KeyInput| {
        let toggle = matches!(input, KeyInput::Text(t) if t.eq_ignore_ascii_case("l"));
        if toggle || (pad.locked && *input == KeyInput::Escape) {
            handle.set_pointer_lock(toggle && !pad.locked);
            return;
        }
        if !pad.typing && matches!(input, KeyInput::Text(_)) {
            pad.typing = true;
            println!("cursor hidden");
        }
    })
}

fn main() {
    let pad = Pad {
        lock_on_click: std::env::args().any(|a| a == "--lock-test"),
        ..Pad::default()
    };
    let app = App::new(pad, view)
        .on_pointer_lock(|pad: &mut Pad, _, locked| {
            pad.locked = locked;
            if locked {
                pad.moved = (0.0, 0.0);
            }
            println!("pointer locked: {locked}");
        })
        .title("Stipple Pointer")
        .theme(Theme::dark())
        .logical_size(Size::new(400.0, 300.0));
    app.run();
}

// In the browser, `web.rs` builds this file as a library and the page
// runs `main`'s app (on the web, `App::run` hands it to the page).
stipple::web_app!(main);
