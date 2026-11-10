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
//!   after Tab moves focus to it), and the input method's edits once it reads
//!   "Stipple", when the demo runs the backend's `InputConnection` self-test
//!   (composing, committing an emoji, deleting around the caret);
//! - `androiddemo: back handled` / `back not handled` for the Back button: it
//!   resets the tap count when there is one (handled), and otherwise leaves
//!   the app to the system;
//! - `androiddemo: lifecycle Background` / `Foreground` / `LowMemory` as the
//!   app goes to the background, comes back, or is asked to free memory;
//! - `androiddemo: scrolled to N` as a finger drag (`adb shell input swipe`)
//!   scrolls the list at the bottom of the screen, and `androiddemo: row N
//!   tapped` if one of its rows is tapped (a swipe must not tap one).
//!
//! Non-Android builds compile to an empty cdylib so `cargo build --workspace`
//! works everywhere.

#[cfg(target_os = "android")]
mod app {
    use std::ffi::c_void;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
    use stipple::platform::backend::android::{self, ANativeActivity};
    use stipple::prelude::*;

    struct State {
        taps: u32,
        ticks: u32,
        name: EditBuffer,
    }

    /// The field's text in the frame last built, to log its changes.
    static TEXT: Mutex<String> = Mutex::new(String::new());
    /// Whether the input connection self-test ran.
    static SELF_TEST: AtomicBool = AtomicBool::new(false);
    /// The frame to run it in.
    static SELF_TEST_AT: AtomicU32 = AtomicU32::new(0);

    /// Frames built, to show the spinner is driven by the display.
    static FRAMES: AtomicU32 = AtomicU32::new(0);
    /// The list's scroll offset in the frame last built (`f64` bits).
    static OFFSET: AtomicU64 = AtomicU64::new(0);

    fn view(state: &State, cx: &mut Cx<State>) -> Element {
        FRAMES.fetch_add(1, Ordering::Relaxed);
        let theme = *cx.theme();
        let field = text_editor(cx, &theme, &state.name, |s: &mut State| &mut s.name);
        let name = state.name.text();
        let mut last = TEXT.lock().unwrap();
        if *last != name {
            name.clone_into(&mut last);
            println!("androiddemo: text {name:?}");
            // Typed through key events: now try the input method's way, a
            // few frames on, once the backend has passed the field's text on.
            if name == "Stipple" && !SELF_TEST.swap(true, Ordering::Relaxed) {
                SELF_TEST_AT.store(FRAMES.load(Ordering::Relaxed) + 3, Ordering::Relaxed);
            }
        }
        drop(last);
        if FRAMES.load(Ordering::Relaxed) == SELF_TEST_AT.load(Ordering::Relaxed) {
            android::input_connection_self_test();
        }
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
        let greeting = if name.is_empty() {
            format!("Tapped {} times", state.taps)
        } else {
            format!("Hello, {name}! Tapped {} times", state.taps)
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
            name: EditBuffer::new(),
        };
        let mut app = App::new(state, view)
            .theme(theme)
            .on_back(|s: &mut State| {
                if s.taps > 0 {
                    s.taps = 0;
                    println!("androiddemo: back handled");
                    Handled::Yes
                } else {
                    println!("androiddemo: back not handled");
                    Handled::No
                }
            })
            .on_lifecycle(|_: &mut State, change| println!("androiddemo: lifecycle {change:?}"));
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
