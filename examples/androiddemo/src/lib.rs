//! A Stipple app on Android: a `NativeActivity` that runs a full `App`, with
//! touch, the keyboard, timers and an animated spinner. It builds as a cdylib
//! (`libstipple_android.so`) that an APK loads through its NativeActivity.
//!
//! Android calls `ANativeActivity_onCreate`, which hands the activity to
//! `stipple::platform::backend::android::start`. That runs `main` on the UI
//! thread, where the `App` runs as on any other platform. Its `println!`s go
//! to logcat, under the `stipple` tag; CI reads them back:
//!
//! - `androiddemo: tick N` once a second, from an `AppHandle::every` timer, so
//!   the loop wakes for timers;
//! - `androiddemo: tapped N` when the button in the middle of the screen is
//!   tapped (`adb shell input tap`);
//! - `androiddemo: text "..."` as the field takes keys (`adb shell input text`,
//!   after Tab moves focus to it);
//! - `androiddemo: scrolled to N` as a finger drag (`adb shell input swipe`)
//!   scrolls the list at the bottom of the screen, and `androiddemo: row N
//!   tapped` if one of its rows is tapped (a swipe must not tap one).
//!
//! Non-Android builds compile to an empty cdylib so `cargo build --workspace`
//! works everywhere.

#[cfg(target_os = "android")]
mod app {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
    use stipple::platform::backend::android::{self, ANativeActivity};
    use stipple::prelude::*;

    struct State {
        taps: u32,
        ticks: u32,
        name: String,
    }

    /// Frames built, to show the spinner is driven by the display.
    static FRAMES: AtomicU32 = AtomicU32::new(0);
    /// The list's scroll offset in the frame last built (`f64` bits).
    static OFFSET: AtomicU64 = AtomicU64::new(0);

    fn view(state: &State, cx: &mut Cx<State>) -> Element {
        FRAMES.fetch_add(1, Ordering::Relaxed);
        let theme = *cx.theme();
        let field = text_field(cx, &theme, &state.name, |s: &mut State, key| {
            edit_string(&mut s.name, key);
            println!("androiddemo: text {:?}", s.name);
        });
        let top = column(vec![
            heading(&theme, "Stipple on Android"),
            label(&theme, "Type your name:"),
            field,
            row(vec![
                spinner(cx, &theme),
                label(&theme, format!("Running for {} s", state.ticks)),
            ])
            .gap(theme.spacing.sm)
            .align(Align::Start, Align::Center),
        ])
        .gap(theme.spacing.md);
        // The button takes the space the rest leaves, so it covers the middle
        // of the screen and CI can tap it at the screen's center.
        let tap = button_labeled(&theme, "Tap me")
            .width(280.0)
            .grow(1.0)
            .on_tap(cx, |s: &mut State| {
                s.taps += 1;
                println!("androiddemo: tapped {}", s.taps);
            });
        let greeting = if state.name.is_empty() {
            format!("Tapped {} times", state.taps)
        } else {
            format!("Hello, {}! Tapped {} times", state.name, state.taps)
        };
        // A list of rows at the bottom of the screen, which a finger drag
        // scrolls. Built as `scroll` builds it, to read its offset.
        let rows = (0..30)
            .map(|i| {
                button_labeled(&theme, format!("Row {i}"))
                    .width(280.0)
                    .on_tap(cx, move |_: &mut State| {
                        println!("androiddemo: row {i} tapped")
                    })
            })
            .collect();
        let id = cx.register_scroll();
        let offset = cx.scroll_offset(id);
        if OFFSET.swap(offset.to_bits(), Ordering::Relaxed) != offset.to_bits() {
            println!("androiddemo: scrolled to {offset:.0}");
        }
        let list = Element::stack(Axis::Vertical, vec![column(rows).gap(theme.spacing.sm)])
            .height(240.0)
            .scrollable(id)
            .align(Align::Start, Align::Center);
        column(vec![top, tap, label(&theme, greeting), list])
            .gap(theme.spacing.lg)
            .align(Align::Center, Align::Center)
            .padding(Insets::uniform(theme.spacing.lg))
            .grow(1.0)
    }

    fn main() {
        let theme = Theme::native();
        println!(
            "androiddemo: starting, dark={} background={:?}",
            theme.is_dark(),
            theme.palette.background
        );
        let state = State {
            taps: 0,
            ticks: 0,
            name: String::new(),
        };
        let mut app = App::new(state, view).theme(theme);
        if let Some(font) = Font::system_default() {
            app = app.font(font);
        }
        app.handle().every(Duration::from_secs(1), |s: &mut State| {
            s.ticks += 1;
            println!(
                "androiddemo: tick {} ({} frames)",
                s.ticks,
                FRAMES.load(Ordering::Relaxed)
            );
        });
        app.run();
        println!("androiddemo: event loop exited");
    }

    /// The symbol Android resolves to start a NativeActivity.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn ANativeActivity_onCreate(
        activity: *mut ANativeActivity,
        saved_state: *mut c_void,
        saved_state_size: usize,
    ) {
        unsafe { android::start(activity, saved_state, saved_state_size, main) };
    }
}
