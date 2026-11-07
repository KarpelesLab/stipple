//! Timers and background work: a repeating timer animates a progress bar, and a
//! background thread delivers a result, both through an [`AppHandle`].
//!
//! With `--selftest`, the app quits on its own once both have happened, printing
//! what it saw. CI runs it that way on each desktop platform.

use std::time::Instant;
use stipple::prelude::*;

struct State {
    ticks: u32,
    result: Option<String>,
    started: Instant,
}

fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    let handle = cx.handle();
    let status = match &state.result {
        Some(result) => format!("Background work: {result}"),
        None => String::from("Background work: running…"),
    };
    let reload = button_labeled(&theme, "Run again").on_tap(cx, move |s: &mut State| {
        s.result = None;
        start_work(&handle);
    });
    column(vec![
        heading(&theme, format!("Ticks: {}", state.ticks)),
        progress_bar(&theme, (state.ticks % 20) as f64 / 19.0),
        label(&theme, status),
        reload,
    ])
    .gap(theme.spacing.md)
    .padding(Insets::uniform(theme.spacing.lg))
}

/// Slow work on another thread, handing its result back to the UI thread.
fn start_work(handle: &AppHandle<State>) {
    let handle = handle.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(600));
        let answer = (1..=10u64).product::<u64>(); // stands in for real work
        handle.update(move |s: &mut State| s.result = Some(format!("10! = {answer}")));
    });
}

fn main() {
    let selftest = std::env::args().any(|a| a == "--selftest");
    let state = State {
        ticks: 0,
        result: None,
        started: Instant::now(),
    };
    let mut app = App::new(state, view)
        .title("Stipple")
        .logical_size(Size::new(360.0, 200.0));
    if let Some(font) = Font::system_default() {
        app = app.font(font);
    }
    let handle = app.handle();
    let quit = handle.clone();
    handle.every(Duration::from_millis(100), move |s: &mut State| {
        s.ticks += 1;
        if selftest && s.ticks >= 10 && s.result.is_some() {
            let ms = s.started.elapsed().as_millis();
            println!(
                "asyncdemo: ticks={} result={:?} after {ms}ms",
                s.ticks,
                s.result.as_deref().unwrap_or("")
            );
            quit.exit();
        }
    });
    start_work(&handle);
    app.run();
    if selftest {
        println!("asyncdemo: event loop exited");
    }
}
