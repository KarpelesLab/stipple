//! Drag and drop: a drop zone that takes files and text dragged from other
//! applications, and chips you can drag onto it inside the app.
//!
//! The zone animates from its [`DropState`]: when a drag enters it grows on a
//! spring and glows, tinted red when it won't take what is dragged (the zone
//! refuses text dropped on its "files only" half), and it eases back on leave
//! or drop. Each drop prints a line:
//!
//! ```text
//! dropdemo: drop files ["/tmp/a.txt"] at (120, 80)
//! dropdemo: drop text "hello" at (120, 80)
//! dropdemo: drop internal "chip-2" at (120, 80)
//! ```
//!
//! With `--expect N` the app exits once N drops have landed, printing
//! `dropdemo: selftest ok`, or fails after 30 s (`--timeout SECS`). CI drives
//! it that way with an XDND source on X11 and an OLE drag on Windows.

use std::cell::RefCell;
use std::time::Instant;
use stipple::prelude::*;

struct State {
    drops: Vec<String>,
    /// The zone's grow, on a spring, and the frame time it was last stepped.
    grow: RefCell<(Spring, Duration)>,
}

/// One half of the drop zone: `accepts` decides what it takes, and `grow`
/// (0 to 1, or the eased drag level when `None`) how much it has grown.
/// Returns the zone and its drag state.
fn zone(
    cx: &mut Cx<State>,
    title: &str,
    accepts: DragOffer,
    grow: Option<f64>,
) -> (Element, DropState) {
    let theme = *cx.theme();
    let id = cx.register_drop(accepts, |s: &mut State, data, at| {
        let line = match data {
            DropData::Files(paths) => format!("files {paths:?}"),
            DropData::Text(text) => format!("text {text:?}"),
            DropData::Internal(id) => format!("internal {id:?}"),
        };
        println!("dropdemo: drop {line} at ({:.0}, {:.0})", at.x, at.y);
        s.drops.push(line);
    });
    let drag = cx.drop_state(id);
    let grow = grow.unwrap_or(drag.level);
    // Glow toward the accent, or toward red when the drag is refused.
    let glow = if drag.accepted {
        theme.palette.primary
    } else {
        theme.palette.danger
    };
    let fill = theme.palette.surface.mix(glow, 0.25 * drag.level);
    let border = theme.palette.border.mix(glow, drag.level);
    let hint = match (drag.over, drag.accepted) {
        (true, true) => "Release to drop".to_string(),
        (true, false) => "Can't drop that here".to_string(),
        _ => title.to_string(),
    };
    let pos = drag
        .position
        .map(|p| format!("({:.0}, {:.0})", p.x, p.y))
        .unwrap_or_default();
    let element = column(vec![label(&theme, hint), label(&theme, pos)])
        .gap(theme.spacing.sm)
        .align(Align::Center, Align::Center)
        .padding(Insets::uniform(theme.spacing.lg + 10.0 * grow))
        .fill(fill)
        .border(border, 1.0 + 3.0 * drag.level)
        .radius(8.0 + 4.0 * grow)
        .grow(1.0)
        .drop_target(id);
    (element, drag)
}

fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    // The first zone grows on a spring, stepped to this frame; it overshoots
    // a little on enter. Its target follows whether a drag is over the zone.
    let now = cx.frame_time();
    let mut grow = state.grow.borrow_mut();
    let (spring, last) = &mut *grow;
    let dt = now.saturating_sub(*last).as_secs_f64().min(0.05);
    *last = now;
    spring.step(dt);
    let (any, over) = zone(
        cx,
        "Drop files or text here",
        DragOffer::ANY,
        Some(spring.position),
    );
    spring.target = if over.over { 1.0 } else { 0.0 };
    if !spring.is_at_rest() {
        cx.request_frame();
    }
    // The second grows with the eased level alone.
    let (files, _) = zone(cx, "Files only", DragOffer::FILES, None);

    let chips: Vec<Element> = (1..=3)
        .map(|n| {
            let name = format!("chip-{n}");
            column(vec![label(&theme, format!("Chip {n}"))])
                .padding(Insets::uniform(theme.spacing.sm))
                .fill(theme.palette.surface)
                .border(theme.palette.border, 1.0)
                .radius(12.0)
                .cursor(CursorIcon::Grab)
                .drag_source(cx, move |_| Some(DropData::Internal(name.clone())))
        })
        .collect();
    let last = state
        .drops
        .last()
        .cloned()
        .unwrap_or_else(|| "nothing yet".into());
    column(vec![
        heading(&theme, "Drag and drop"),
        row(vec![any, files])
            .gap(theme.spacing.md)
            .align(Align::Start, Align::Stretch)
            .grow(1.0),
        row(chips).gap(theme.spacing.sm),
        label(
            &theme,
            format!("{} dropped; last: {last}", state.drops.len()),
        ),
    ])
    .gap(theme.spacing.md)
    .padding(Insets::uniform(theme.spacing.lg))
    .align(Align::Start, Align::Stretch)
}

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn main() {
    let expect: Option<usize> = arg("--expect").and_then(|n| n.parse().ok());
    let timeout = arg("--timeout")
        .and_then(|n| n.parse().ok())
        .unwrap_or(30u64);
    let mut spring = Spring::new(0.0, 0.0);
    spring.damping = 0.45;
    spring.stiffness = 180.0;
    let state = State {
        drops: Vec::new(),
        grow: RefCell::new((spring, Duration::ZERO)),
    };
    let mut app = App::new(state, view)
        .title("Stipple drop demo")
        .logical_size(Size::new(520.0, 360.0));
    if let Some(font) = Font::system_default() {
        app = app.font(font);
    }
    println!("dropdemo: ready");
    if let Some(expect) = expect {
        let handle = app.handle();
        let quit = handle.clone();
        let started = Instant::now();
        handle.every(Duration::from_millis(100), move |s: &mut State| {
            if s.drops.len() >= expect {
                println!("dropdemo: selftest ok ({} drops)", s.drops.len());
                quit.exit();
            } else if started.elapsed() > Duration::from_secs(timeout) {
                println!(
                    "dropdemo: selftest FAILED: {} of {expect} drops after {timeout}s",
                    s.drops.len()
                );
                std::process::exit(1);
            }
        });
    }
    app.run();
}
