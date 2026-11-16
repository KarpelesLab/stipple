//! Frame pacing, checked: a bar sweeps across the window for a few seconds,
//! asking for a frame each time it's drawn, then stops, and the app sits idle.
//!
//! Prints how many frames were presented while it moved, against the display's
//! refresh, where they came from (the display source, such as Present,
//! `wl_surface.frame` or a display link, or the fallback timer), the refresh
//! the frame clock measured, and how many frames were presented and how many
//! times the event loop woke while idle, which must be none. Exits with 1 if
//! the animation wasn't paced at roughly the display's rate or the idle app
//! did anything; CI runs it on each desktop platform.
//!
//! `--animate SECS` and `--idle SECS` set the two phases (3 s each by
//! default); `--rate-only` and `--idle-only` check just one of them (a
//! virtual display may not pace frames at a real rate). Run with
//! `STIPPLE_DEBUG_FRAMES=1` to see the frame clock's own log as well.
//!
//! `--hidden SECS` adds a phase after the animation: the app minimizes its
//! window, checks that nothing is presented for that long while it's
//! hidden (`Event::Occluded`), restores it and checks the animation draws
//! again. It needs a window manager that minimizes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use stipple::platform::frame::{FrameStats, stats};
use stipple::prelude::*;

struct State {
    animating: bool,
}

fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    let t = cx.frame_time().as_secs_f64();
    let x = if state.animating {
        cx.request_frame();
        // Across and back once a second.
        let phase = (t % 1.0) * 2.0;
        (if phase > 1.0 { 2.0 - phase } else { phase }) * 280.0
    } else {
        0.0
    };
    column(vec![
        label(
            cx,
            if state.animating {
                "Animating…"
            } else {
                "Idle"
            },
        ),
        row(vec![
            column(vec![]).width(x),
            column(vec![])
                .width(40.0)
                .height(40.0)
                .fill(theme.palette.primary)
                .radius(6.0),
        ]),
    ])
    .gap(theme.spacing.md)
    .padding(Insets::uniform(theme.spacing.lg))
}

fn arg(name: &str, default: f64) -> f64 {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

static FAILED: AtomicBool = AtomicBool::new(false);

fn main() {
    let animate = Duration::from_secs_f64(arg("--animate", 3.0));
    let idle = Duration::from_secs_f64(arg("--idle", 3.0));
    let check_rate = !std::env::args().any(|a| a == "--idle-only");
    let check_idle = !std::env::args().any(|a| a == "--rate-only");
    let hidden = Duration::from_secs_f64(arg("--hidden", 0.0));
    let app = App::new(State { animating: true }, view)
        .title("Frame pacing")
        .logical_size(Size::new(360.0, 140.0));
    let handle = app.handle();
    std::thread::spawn(move || {
        // Let the window come up and the first frames settle.
        std::thread::sleep(Duration::from_secs(1));
        let (a, ta) = (stats(), Instant::now());
        std::thread::sleep(animate);
        let (b, tb) = (stats(), Instant::now());
        if !hidden.is_zero() {
            check_hidden(&handle, hidden);
        }
        handle.update(|s: &mut State| s.animating = false);
        // The last frame, and any fade of it.
        std::thread::sleep(Duration::from_secs(1));
        let c = stats();
        std::thread::sleep(idle);
        let d = stats();
        report(&a, &b, tb - ta, &c, &d, idle, check_rate, check_idle);
        handle.exit();
    });
    app.run();
    if FAILED.load(Ordering::Relaxed) {
        std::process::exit(1);
    }
}

/// Minimize the window for `hidden`, still animating, and check nothing is
/// presented meanwhile; then restore it and check frames come back.
fn check_hidden(handle: &AppHandle<State>, hidden: Duration) {
    use stipple::platform::WindowRequest;
    handle.window(WindowRequest::Minimized(true));
    // The window manager takes a moment.
    std::thread::sleep(Duration::from_secs(1));
    let (a, ta) = (stats(), Instant::now());
    std::thread::sleep(hidden);
    let (b, tb) = (stats(), Instant::now());
    handle.window(WindowRequest::Minimized(false));
    std::thread::sleep(Duration::from_secs(1));
    let c = stats();
    std::thread::sleep(Duration::from_secs(1));
    let d = stats();
    let (presents, wakeups) = (b.presents - a.presents, b.wakeups - a.wakeups);
    let back = d.presents - c.presents;
    println!(
        "framepacing: hidden: {presents} presents, {wakeups} wakeups in {:.2} s; shown again: {back} presents in 1 s",
        (tb - ta).as_secs_f64()
    );
    // A frame or two may still be on its way as the window goes.
    let quiet = presents <= 2;
    let resumed = back as f64 >= 0.3 / stipple::platform::frame::interval().as_secs_f64();
    println!(
        "framepacing: hidden {}, shown again {}",
        if quiet { "ok" } else { "busy" },
        if resumed { "ok" } else { "stalled" }
    );
    if !(quiet && resumed) {
        FAILED.store(true, Ordering::Relaxed);
    }
}

#[allow(clippy::too_many_arguments)]
fn report(
    a: &FrameStats,
    b: &FrameStats,
    took: Duration,
    c: &FrameStats,
    d: &FrameStats,
    idle: Duration,
    check_rate: bool,
    check_idle: bool,
) {
    let interval = stipple::platform::frame::interval();
    let refresh = 1.0 / interval.as_secs_f64();
    let presents = b.presents - a.presents;
    let rate = presents as f64 / took.as_secs_f64();
    let measured = b
        .measured_interval
        .map(|m| format!("{:.1} Hz", 1.0 / m.as_secs_f64()))
        .unwrap_or_else(|| "not measured".into());
    println!(
        "framepacing: animating: {presents} presents in {:.2} s = {rate:.1}/s against a \
         {refresh:.1} Hz refresh; {} from the display ({}), {} from the timer; measured {measured}",
        took.as_secs_f64(),
        b.from_display - a.from_display,
        stipple::platform::frame::source_name(),
        b.from_timer - a.from_timer,
    );
    let idle_presents = d.presents - c.presents;
    let idle_wakeups = d.wakeups - c.wakeups;
    println!(
        "framepacing: idle: {idle_presents} presents, {idle_wakeups} wakeups in {:.2} s",
        idle.as_secs_f64()
    );
    // Tolerant: a loaded runner drops frames, and a virtual display's
    // nominal rate is only nominal.
    let paced = (0.5 * refresh..=1.25 * refresh).contains(&rate);
    let quiet = idle_presents == 0 && idle_wakeups == 0;
    let rate_ok = !check_rate || paced;
    let idle_ok = !check_idle || quiet;
    println!(
        "framepacing: rate {}, idle {}",
        if paced { "ok" } else { "off" },
        if quiet { "ok" } else { "busy" }
    );
    if rate_ok && idle_ok && !FAILED.load(Ordering::Relaxed) {
        println!("framepacing: ok");
    } else {
        println!("framepacing: FAILED");
        FAILED.store(true, Ordering::Relaxed);
    }
}
