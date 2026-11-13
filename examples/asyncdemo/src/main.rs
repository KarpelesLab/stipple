//! Timers and background work: a repeating timer animates a progress bar, and a
//! background thread delivers a result, both through an [`AppHandle`]. While
//! the work runs, a spinner turns at the display's refresh rate; once the result
//! is in, the spinner goes and the app stops asking for frames.
//!
//! With `--selftest`, the app quits on its own once both have happened, printing
//! what it saw, and checks that the spinner was animated by the frame clock and
//! stopped afterwards. CI runs it that way on each desktop platform.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Instant;
use stipple::prelude::*;

struct State {
    ticks: u32,
    result: Option<String>,
    started: Instant,
    /// Timer ticks while the work ran, and since it finished (selftest).
    ticks_loading: u32,
    ticks_idle: u32,
}

/// Frames built while the spinner showed, and since (selftest).
static FRAMES_LOADING: AtomicU32 = AtomicU32::new(0);
static FRAMES_IDLE: AtomicU32 = AtomicU32::new(0);
static FAILED: AtomicBool = AtomicBool::new(false);

fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    let handle = cx.handle();
    let status = match &state.result {
        Some(result) => {
            FRAMES_IDLE.fetch_add(1, Ordering::Relaxed);
            row(vec![label(&theme, format!("Background work: {result}"))])
        }
        None => {
            FRAMES_LOADING.fetch_add(1, Ordering::Relaxed);
            row(vec![
                spinner(cx, &theme),
                label(&theme, "Background work: running…"),
            ])
            .gap(theme.spacing.sm)
            .align(Align::Start, Align::Center)
        }
    };
    let reload = button_labeled(&theme, "Run again").on_tap(cx, move |s: &mut State| {
        s.result = None;
        start_work(&handle);
    });
    column(vec![
        heading(&theme, format!("Ticks: {}", state.ticks)),
        progress_bar(&theme, (state.ticks % 20) as f64 / 19.0),
        status,
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

/// The selftest's check that the frame clock drove the spinner: while loading,
/// far more frames than the 100 ms timer alone would build (it builds one per
/// tick), and once loaded, no more than the timer's.
fn check_frames(s: &State) {
    let loading = FRAMES_LOADING.load(Ordering::Relaxed);
    let idle = FRAMES_IDLE.load(Ordering::Relaxed);
    // At 60 Hz a 600 ms load builds ~36 frames against ~6 ticks. Runners
    // vary (macOS CI used to manage only 10–12, see ROADMAP 7a), so this only
    // asks for more frames than the timer's one per tick could build; CI
    // reads the count from the line below.
    let animated = loading >= s.ticks_loading + 2;
    // One frame per tick, plus the one that showed the result.
    let settled = idle <= 2 * s.ticks_idle + 2;
    let verdict = if animated && settled { "ok" } else { "FAILED" };
    println!(
        "asyncdemo: spinner {verdict} ({loading} frames over {} ticks while loading, \
         {idle} frames over {} ticks after)",
        s.ticks_loading, s.ticks_idle
    );
    // Where the frames came from (the display or the fallback timer) and
    // what they cost, to explain a low count.
    let f = stipple::platform::frame::stats();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    println!(
        "asyncdemo: frame clock: {} frames from the display, {} from the timer \
         (refresh {:.1} ms); {} presents, {:.2} ms mean, {:.2} ms max; \
         {} displays, {:.2} ms mean, {:.2} ms max",
        f.from_display,
        f.from_timer,
        ms(stipple::platform::frame::interval()),
        f.presents,
        ms(f.present_mean()),
        ms(f.present_max),
        f.displays,
        ms(f.display_mean()),
        ms(f.display_max),
    );
    if verdict != "ok" {
        FAILED.store(true, Ordering::Relaxed);
    }
}

fn main() {
    let selftest = std::env::args().any(|a| a == "--selftest");
    let state = State {
        ticks: 0,
        result: None,
        started: Instant::now(),
        ticks_loading: 0,
        ticks_idle: 0,
    };
    let mut app = App::new(state, view)
        .title("Stipple")
        .logical_size(Size::new(360.0, 200.0));
    if let Some(font) = Font::system_default() {
        app = app.font(font);
    }
    let handle = app.handle();
    let quit = handle.clone();
    let worker = handle.clone();
    handle.every(Duration::from_millis(100), move |s: &mut State| {
        s.ticks += 1;
        // The selftest starts the work once the loop is running, so a slow
        // first frame (macOS CI takes over a second) can't swallow the load.
        if selftest && s.ticks == 1 {
            start_work(&worker);
        }
        if s.result.is_some() {
            s.ticks_idle += 1;
        } else {
            s.ticks_loading += 1;
        }
        if selftest && s.ticks >= 10 && s.result.is_some() {
            let ms = s.started.elapsed().as_millis();
            println!(
                "asyncdemo: ticks={} result={:?} after {ms}ms",
                s.ticks,
                s.result.as_deref().unwrap_or("")
            );
            check_frames(s);
            quit.exit();
        }
    });
    if !selftest {
        start_work(&handle);
    }
    app.run();
    if selftest {
        println!("asyncdemo: event loop exited");
        if FAILED.load(Ordering::Relaxed) {
            std::process::exit(1);
        }
    }
}
