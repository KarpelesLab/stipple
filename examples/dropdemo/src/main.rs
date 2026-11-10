//! Drag and drop: a drop zone that takes files and text dragged from other
//! applications, and chips you can drag onto it inside the app. The "File"
//! and "Text" chips can also be dragged out of the window, into other
//! applications.
//!
//! The zones style their drag-over look with per-state overrides
//! (`.drag_over(...)`, `.drag_refused(...)`) and let a `Transition` animate
//! between looks: when a drag enters, a zone grows and glows, tinted red when
//! it won't take what is dragged (the "files only" zone refuses text), and it
//! goes back when the drag leaves or drops. The first zone moves on a spring
//! (it overshoots a little), the second eases. Neither keeps any animation
//! state of its own. Each drop prints a line:
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

use std::time::Instant;
use stipple::prelude::*;

struct State {
    drops: Vec<String>,
    /// The file the "File" chip drags.
    file: std::path::PathBuf,
}

/// One drop zone: `accepts` decides what it takes, and `motion` how it moves
/// between its resting and drag-over looks.
fn zone(cx: &mut Cx<State>, title: &str, accepts: DragOffer, motion: Transition) -> Element {
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
    // The words follow the drag (and the pointer); the look is the overrides'.
    let drag = cx.drop_state(id);
    let hint = match (drag.over, drag.accepted) {
        (true, true) => "Release to drop".to_string(),
        (true, false) => "Can't drop that here".to_string(),
        _ => title.to_string(),
    };
    let pos = drag
        .position
        .map(|p| format!("({:.0}, {:.0})", p.x, p.y))
        .unwrap_or_default();
    let pad = theme.spacing.lg;
    let p = theme.palette;
    // Grown and glowing toward `glow`: the accent, or red when refused.
    let hot = move |e: Element, glow: Color| {
        e.padding(Insets::uniform(pad + 10.0))
            .fill(p.surface.mix(glow, 0.25))
            .border(glow, 4.0)
            .radius(12.0)
    };
    column(vec![label(&theme, hint), label(&theme, pos)])
        .gap(theme.spacing.sm)
        .align(Align::Center, Align::Center)
        .padding(Insets::uniform(pad))
        .fill(p.surface)
        .border(p.border, 1.0)
        .radius(8.0)
        .drag_over(move |e| hot(e, p.primary))
        .drag_refused(move |e| hot(e, p.danger))
        .transition(motion)
        .grow(1.0)
        .drop_target(id)
}

fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    // The first zone springs, with a little overshoot; the second eases.
    let any = zone(
        cx,
        "Drop files or text here",
        DragOffer::ANY,
        Transition::spring(SpringParams::new(180.0, 0.45)),
    );
    let files = zone(
        cx,
        "Files only",
        DragOffer::FILES,
        Transition::ease(DROP_FADE),
    );

    // A fixed width keeps the chips where CI's scripted drags expect them.
    let chip = |text: String| {
        column(vec![label(&theme, text)])
            .align(Align::Center, Align::Center)
            .width(68.0)
            .padding(Insets::uniform(theme.spacing.sm))
            .fill(theme.palette.surface)
            .border(theme.palette.border, 1.0)
            .radius(12.0)
            .cursor(CursorIcon::Grab)
    };
    let mut chips: Vec<Element> = (1..=3)
        .map(|n| {
            let name = format!("chip-{n}");
            chip(format!("Chip {n}"))
                .drag_source(cx, move |_| Some(DropData::Internal(name.clone())))
        })
        .collect();
    // A file and some text: these drop in the app too, and dragged out of
    // the window they go to other applications.
    let file = state.file.clone();
    chips.push(
        chip("File".into()).drag_source(cx, move |_| Some(DropData::Files(vec![file.clone()]))),
    );
    chips.push(
        chip("Text".into()).drag_source(cx, |_| Some(DropData::Text("Hello from Stipple".into()))),
    );
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
    // A file to drag out of the app.
    let file = std::env::temp_dir().join("stipple-dropdemo.txt");
    let _ = std::fs::write(&file, "Dragged out of the Stipple drop demo.\n");
    let state = State {
        drops: Vec::new(),
        file,
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
