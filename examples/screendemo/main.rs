//! The screen on a phone: the app drawn edge to edge (`App::edge_to_edge`),
//! a colored backdrop under the status bar, the notch and the home
//! indicator, with the safe area (`Cx::safe_area`) outlined and the controls
//! kept inside it; which way the app is turned (`WindowInfo::orientation`),
//! buttons locking it upright or sideways (`AppHandle::set_orientations`),
//! and a switch keeping the screen awake (`AppHandle::set_keep_awake`). A
//! navigation bar along the bottom runs its background under the home
//! indicator by itself.
//!
//! On a desktop or in a browser the safe area is zero (a page with
//! `viewport-fit=cover` on a phone reports its notch) and the orientation
//! follows the window's shape; keeping the screen awake works everywhere.
//!
//! **Self-test (iOS).** Launched with `SCREENDEMO_SELFTEST` set
//! (`SIMCTL_CHILD_SCREENDEMO_SELFTEST=1`), it checks the status bar is in
//! the safe area, locks itself to landscape (the simulator's interface
//! turns) and checks the view followed (wider than tall, a notch's side in
//! the safe area), locks itself upright again and checks it came back,
//! then keeps the screen awake and checks the idle timer is off. It logs to
//! `screendemo.log` in the app's tmp directory, ending with
//! `screendemo: selftest passed` or `screendemo: selftest FAILED: ...`.

use stipple::prelude::*;

#[derive(Default)]
struct State {
    awake: bool,
    tab: usize,
}

fn log(line: &str) {
    println!("{line}");
    if cfg!(target_os = "ios") {
        use std::io::Write;
        let path = std::env::temp_dir().join("screendemo.log");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{line}");
        }
    }
}

/// The safe area and the window's shape, as a line of text.
fn describe(info: &WindowInfo) -> String {
    let s = info.safe_area;
    format!(
        "{:?} {}x{} safe area {} {} {} {}",
        info.orientation(),
        info.inner_size.width,
        info.inner_size.height,
        s.top,
        s.right,
        s.bottom,
        s.left
    )
}

fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    let info = cx.window_info();
    let handle = cx.handle();
    let lock = |cx: &mut Cx<State>, text: &str, set: Orientations| {
        let handle = handle.clone();
        button(cx, text).on_tap(cx, move |_| handle.set_orientations(set))
    };
    let awake_handle = handle.clone();
    let controls = column(vec![
        heading(cx, "The screen"),
        label(cx, describe(&info)),
        row(vec![
            lock(cx, "Upright", Orientations::PORTRAIT),
            lock(cx, "Sideways", Orientations::LANDSCAPE),
            lock(cx, "Any way", Orientations::ALL),
        ])
        .gap(theme.spacing.sm),
        switch(
            cx,
            "Keep the screen awake",
            state.awake,
            move |s: &mut State| {
                s.awake = !s.awake;
                awake_handle.set_keep_awake(s.awake);
            },
        ),
    ])
    .gap(theme.spacing.md)
    .padding(Insets::uniform(theme.spacing.lg))
    .fill(theme.palette.surface)
    .border(theme.palette.primary, 2.0)
    .grow(1.0);
    let items = vec![
        NavItem::new("Screen", NavIcon::template(DOT)),
        NavItem::new("About", NavIcon::template(DOT)),
    ];
    let bar = navigation_bar(cx, items, state.tab, |s, i| s.tab = i);
    // The backdrop fills the whole screen, under the bars; the outlined
    // panel is the safe area, above the navigation bar.
    let safe = cx.safe_area();
    column(vec![
        column(vec![controls])
            .padding(Insets::new(safe.top, safe.right, 0.0, safe.left))
            .grow(1.0),
        bar,
    ])
    .align(Align::Start, Align::Stretch)
    .fill(theme.palette.primary.mix(theme.palette.background, 0.6))
    .grow(1.0)
}

const DOT: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"><circle cx="12" cy="12" r="6" fill="currentColor"/></svg>"#;

fn main() {
    let app = App::new(State::default(), view)
        .title("Stipple Screen")
        .logical_size(Size::new(400.0, 640.0))
        .theme(Theme::native())
        .edge_to_edge(true)
        .on_window_change(|_, _, info| log(&format!("screendemo: {}", describe(info))));
    #[cfg(target_os = "ios")]
    if std::env::var_os("SCREENDEMO_SELFTEST").is_some() {
        selftest::run(app.handle());
    }
    app.run();
}

#[cfg(target_os = "ios")]
mod selftest {
    use super::*;
    use std::sync::Mutex;
    use stipple::platform::backend::ios::simulate;

    static FAILURES: Mutex<Vec<String>> = Mutex::new(Vec::new());

    fn check(ok: bool, what: String) {
        if !ok {
            log(&format!("screendemo: check failed: {what}"));
            FAILURES.lock().unwrap().push(what);
        }
    }

    fn info(handle: &AppHandle<State>) -> WindowInfo {
        handle.window_info(WindowId::PRIMARY).unwrap_or_default()
    }

    /// Run `then` once the window is turned `way`, or after 10 s.
    fn when_turned(
        handle: AppHandle<State>,
        way: Orientation,
        tries: u32,
        then: impl FnOnce(AppHandle<State>) + Send + 'static,
    ) {
        let next = handle.clone();
        handle.after(Duration::from_millis(250), move |_| {
            if info(&next).orientation() == way || tries == 0 {
                then(next);
            } else {
                when_turned(next, way, tries - 1, then);
            }
        });
    }

    pub(super) fn run(handle: AppHandle<State>) {
        let start = handle.clone();
        handle.after(Duration::from_millis(1500), move |_| {
            let i = info(&start);
            log(&format!("screendemo: start {}", describe(&i)));
            log(&format!("screendemo: {}", simulate::screen_status()));
            check(
                i.orientation() == Orientation::Portrait,
                format!("starts upright: {}", describe(&i)),
            );
            // The status bar (and on a notched iPhone, the notch) above.
            check(
                i.safe_area.top > 0.0,
                format!("the status bar is in the safe area: {}", describe(&i)),
            );
            // A home indicator below means a notch or Dynamic Island, which
            // stays on one side when turned.
            let notched = i.safe_area.bottom > 0.0;
            start.set_orientations(Orientations::LANDSCAPE);
            when_turned(start, Orientation::Landscape, 40, move |h| {
                let i = info(&h);
                log(&format!("screendemo: sideways {}", describe(&i)));
                log(&format!("screendemo: {}", simulate::screen_status()));
                check(
                    i.inner_size.width > i.inner_size.height,
                    format!("turned sideways: {}", describe(&i)),
                );
                check(
                    !notched || i.safe_area.left > 0.0 || i.safe_area.right > 0.0,
                    format!("the notch's side is in the safe area: {}", describe(&i)),
                );
                h.set_orientations(Orientations::PORTRAIT);
                when_turned(h, Orientation::Portrait, 40, |h| {
                    let i = info(&h);
                    log(&format!("screendemo: upright {}", describe(&i)));
                    check(
                        i.inner_size.height > i.inner_size.width,
                        format!("upright again: {}", describe(&i)),
                    );
                    h.set_keep_awake(true);
                    h.after(Duration::from_millis(500), |_| {
                        let status = simulate::screen_status();
                        log(&format!("screendemo: {status}"));
                        check(
                            status.contains("idle timer off true"),
                            format!("the idle timer is off: {status}"),
                        );
                        let failures = std::mem::take(&mut *FAILURES.lock().unwrap());
                        if failures.is_empty() {
                            log("screendemo: selftest passed");
                        } else {
                            log(&format!(
                                "screendemo: selftest FAILED: {}",
                                failures.join("; ")
                            ));
                        }
                    });
                });
            });
        });
    }
}

// In the browser, `web.rs` builds this file as a library and the page
// runs `main`'s app (on the web, `App::run` hands it to the page).
stipple::web_app!(main);
