//! The connected gamepads, live: each pad's name and ids, which buttons are
//! down and where its sticks and triggers are. Every [`GamepadEvent`] is also
//! printed (`connected: …`, `button: …`, `axis: …`), and the newest ones are
//! listed under the pads. South (A / ✕) rumbles the pad where that's offered.
//!
//! `--list` skips the window: it starts gamepad support, prints the pads it
//! finds (`pad <id>: <name> [vendor:product]`, or `no gamepads`), and exits.
//! CI runs it against a virtual uinput pad.

use std::time::Duration;
use stipple::gamepad::{self, GamepadAxis, GamepadButton, GamepadEvent, GamepadId, GamepadInfo};
use stipple::prelude::*;

#[derive(Default)]
struct State {
    pads: Vec<(GamepadId, GamepadInfo)>,
    log: Vec<String>,
}

fn describe(id: GamepadId, info: &GamepadInfo) -> String {
    match (info.vendor_id, info.product_id) {
        (Some(v), Some(p)) => format!("pad {}: {} [{v:04x}:{p:04x}]", id.get(), info.name),
        _ => format!("pad {}: {}", id.get(), info.name),
    }
}

fn on_gamepad(s: &mut State, event: &GamepadEvent) {
    let line = match event {
        GamepadEvent::Connected { id, info } => {
            s.pads.push((*id, info.clone()));
            format!("connected: {}", describe(*id, info))
        }
        GamepadEvent::Disconnected { id } => {
            s.pads.retain(|(p, _)| p != id);
            format!("disconnected: pad {}", id.get())
        }
        GamepadEvent::Button {
            id,
            button,
            pressed,
        } => {
            if *button == GamepadButton::South && *pressed {
                gamepad::set_rumble(*id, 0.6, 0.3, Duration::from_millis(200));
            }
            let what = if *pressed { "down" } else { "up" };
            format!("button: pad {} {button:?} {what}", id.get())
        }
        GamepadEvent::Axis { id, axis, value } => {
            format!("axis: pad {} {axis:?} {value:.3}", id.get())
        }
        _ => format!("{event:?}"),
    };
    println!("{line}");
    s.log.push(line);
    if s.log.len() > 8 {
        s.log.remove(0);
    }
}

/// A button's name, lit while it's down.
fn chip(theme: &Theme, name: &str, down: bool) -> Element {
    let p = &theme.palette;
    let (fill, text) = if down {
        (p.primary, p.on_primary)
    } else {
        (p.surface, p.text_muted)
    };
    Element::text(name, theme.typography.body * 0.85, text)
        .padding(Insets::symmetric(8.0, 4.0))
        .fill(fill)
        .radius(6.0)
}

fn pad_view(theme: &Theme, id: GamepadId, info: &GamepadInfo) -> Element {
    let state = gamepad::state(id).unwrap_or_default();
    let buttons: Vec<Element> = GamepadButton::ALL
        .iter()
        .map(|&b| chip(theme, &format!("{b:?}"), state.pressed(b)))
        .collect();
    let axes: Vec<Element> = GamepadAxis::ALL
        .iter()
        .map(|&a| {
            let v = state.axis(a);
            // Sticks run −1..1, triggers 0..1.
            let fraction = if a.is_trigger() { v } else { (v + 1.0) / 2.0 };
            row(vec![
                label(theme, format!("{a:?}")).width(110.0),
                progress_bar(theme, fraction as f64).width(240.0),
                label(theme, format!("{v:+.2}")),
            ])
            .gap(12.0)
        })
        .collect();
    panel(
        theme,
        vec![
            heading(theme, describe(id, info)),
            grid(vec![Track::Auto; 6], buttons).gap(6.0),
            column(axes).gap(6.0),
        ],
    )
    .gap(12.0)
}

fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    let mut children = vec![heading(&theme, "Gamepads")];
    if state.pads.is_empty() {
        children.push(paragraph(
            &theme,
            "No gamepad connected. Plug one in (on Linux, you need read access \
             to /dev/input/event*: the input group, or a udev uaccess rule).",
        ));
    }
    for (id, info) in &state.pads {
        children.push(pad_view(&theme, *id, info));
    }
    if !state.log.is_empty() {
        children.push(divider(&theme));
        children.extend(state.log.iter().map(|l| label(&theme, l.as_str())));
    }
    column(children).gap(16.0).padding(Insets::uniform(24.0))
}

fn main() {
    if std::env::args().any(|a| a == "--list") {
        gamepad::start();
        // Pads found at start are listed at once; give a hot-plug a moment.
        std::thread::sleep(Duration::from_millis(300));
        let pads = gamepad::gamepads();
        if pads.is_empty() {
            println!("no gamepads");
        }
        for (id, info) in pads {
            println!("{}", describe(id, &info));
        }
        return;
    }
    let app = App::new(State::default(), view)
        .title("Gamepads")
        .logical_size(Size::new(720.0, 760.0))
        .on_gamepad(on_gamepad);
    app.run();
}
