//! Opens two real OS windows that share one app state, each with its own view.
//!
//! On Linux with `$DISPLAY` set this uses the native X11 backend, which drives
//! multiple top-level windows on one connection (the parent `App` owns the
//! global state; each window is a `Pane` with its own view + render state).
//! Used by the visual-test CI job: it positions the two windows side by side and
//! screenshots the root to confirm both painted, with distinct colors.
//!
//! With `--selftest` it then changes its windows while it runs, on a timer,
//! printing a `multiwindow: step N` line after each change so a test can
//! check it from outside (`xprop`, `xdotool`, `swaymsg`):
//!
//! 1. the first window's title follows the state (`Cx::window_title`), the
//!    second is retitled, moved to 360,260 and resized to 300x200
//!    (`AppHandle::window_to`), and the first snaps its resizing to 8x16
//!    steps;
//! 2. a third window opens (`AppHandle::open_window`), without taking focus;
//! 3. every window's geometry is printed (`AppHandle::window_info`);
//! 4. the third window closes (`AppHandle::close_window`);
//! 5. the open windows are counted, then the app exits.

use std::time::Duration;

use stipple::platform::{WindowAttributes, WindowId, WindowRequest};
use stipple::prelude::*;

/// Shared application state both windows read from.
struct Shared {
    label_a: &'static str,
    label_b: &'static str,
    /// How far the self-test got (0 = not running).
    step: u32,
    selftest: bool,
}

/// A window that fills itself with `color` and centers `text` — the colored
/// fill makes each window unmistakable in the root screenshot.
fn colored_window(color: Color, text: &str, cx: &mut Cx<Shared>) -> Element {
    let theme = *cx.theme();
    let panel = Element::boxed(BoxStyle {
        fill: Some(color),
        radius: 0.0,
        border: None,
    })
    .width(320.0)
    .height(240.0)
    .align(Align::Center, Align::Center);
    column(vec![panel, label(&theme, text)])
        .grow(1.0)
        .align(Align::Center, Align::Center)
}

/// Print a line the self-test looks for, at once.
fn say(line: std::fmt::Arguments<'_>) {
    use std::io::Write;
    println!("multiwindow: {line}");
    let _ = std::io::stdout().flush();
}

/// Change the windows step by step (see the module docs).
fn selftest(handle: &AppHandle<Shared>, b: WindowId) {
    let step = Duration::from_millis(1500);
    let h = handle.clone();
    handle.after(step, move |s: &mut Shared| {
        s.step = 1;
        h.window_to(b, WindowRequest::Title("Stipple — B ✓ retitled".into()));
        h.window_to(b, WindowRequest::Position(Point::new(360.0, 260.0)));
        h.window_to(b, WindowRequest::InnerSize(Size::new(300.0, 200.0)));
        h.window(WindowRequest::ResizeIncrements(Some(Size::new(8.0, 16.0))));
        say(format_args!("step 1"));
    });
    let h = handle.clone();
    handle.after(step * 2, move |s: &mut Shared| {
        s.step = 2;
        let c = h.open_window(
            WindowAttributes::new()
                .with_title("Stipple — C")
                .with_logical_size(Size::new(200.0, 150.0))
                .with_position(700, 0)
                .with_focus(false),
            |_: &Shared, cx: &mut Cx<Shared>| {
                let text = format!("{:?}", cx.window_id());
                colored_window(Color::rgb(0x40, 0xb0, 0x60), &text, cx)
            },
        );
        say(format_args!("step 2 opened {c:?}"));
        let h2 = h.clone();
        h.after(step * 2, move |_: &mut Shared| {
            h2.close_window(c);
            say(format_args!("step 4"));
        });
    });
    let h = handle.clone();
    handle.after(step * 3, move |s: &mut Shared| {
        s.step = 3;
        for id in h.windows() {
            if let Some(info) = h.window_info(id) {
                let pos = info
                    .position
                    .map_or("unknown".to_string(), |p| format!("{:.0},{:.0}", p.x, p.y));
                say(format_args!(
                    "info {id:?} pos={pos} inner={:.0}x{:.0} maximized={} fullscreen={}",
                    info.inner_size.width, info.inner_size.height, info.maximized, info.fullscreen
                ));
            }
        }
        say(format_args!("step 3"));
    });
    let h = handle.clone();
    handle.after(step * 5, move |s: &mut Shared| {
        s.step = 5;
        say(format_args!("windows={}", h.windows().len()));
        say(format_args!("step 5"));
        let h2 = h.clone();
        h.after(Duration::from_secs(2), move |_: &mut Shared| h2.exit());
    });
}

fn main() {
    let selftest_mode = std::env::args().any(|a| a == "--selftest");
    let mut app = App::new(
        Shared {
            label_a: "Window A",
            label_b: "Window B",
            step: 0,
            selftest: selftest_mode,
        },
        |s: &Shared, cx: &mut Cx<Shared>| {
            // The title follows the state.
            if s.selftest {
                cx.window_title(format!("Stipple — A step {}", s.step));
            }
            colored_window(Color::rgb(0xd8, 0x40, 0x40), s.label_a, cx)
        },
    )
    .title("Stipple — A")
    .theme(Theme::dark())
    .logical_size(Size::new(320.0, 240.0));
    if let Some(font) = Font::system_default() {
        app = app.font(font);
    }

    // A second, independent OS window placed to the right of the first.
    let b = app.open_window(
        WindowAttributes::new()
            .with_title("Stipple — B")
            .with_logical_size(Size::new(320.0, 240.0))
            .with_position(340, 0),
        |s: &Shared, cx: &mut Cx<Shared>| {
            colored_window(Color::rgb(0x40, 0x80, 0xd8), s.label_b, cx)
        },
    );

    if selftest_mode {
        selftest(&app.handle(), b);
    }
    app.run();
    if selftest_mode {
        say(format_args!("event loop exited"));
    }
}
