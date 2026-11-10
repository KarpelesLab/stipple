//! A text field above a list of rows: on a touch screen a finger drag scrolls
//! the list (and flings it), a tap on a row taps it, and the text field brings
//! up the keyboard. With a mouse the wheel scrolls, as everywhere.
//!
//! It logs what happens, on stdout and, on iOS, to `touchdemo.log` in the
//! app's tmp directory, which CI reads back from the simulator:
//!
//! - `touchdemo: scrolled to N` as the list's offset changes;
//! - `touchdemo: row N tapped`;
//! - `touchdemo: text "..."` and `touchdemo: preedit ...` as the field's text
//!   and composition change;
//! - `touchdemo: theme dark` / `light` as the system's look changes;
//! - `touchdemo: lifecycle Background` (and `Foreground`, `LowMemory`).
//!
//! **Self-test (iOS).** Launched with `TOUCHDEMO_SELFTEST` set (`xcrun simctl
//! launch` passes it on as `SIMCTL_CHILD_TOUCHDEMO_SELFTEST=1`), the app drives
//! itself, since a simulator can't script touches or typing:
//!
//! 1. it taps the text field, types through the view's `UIKeyInput` methods
//!    (`insertText:` and `deleteBackward`, sent through objc dispatch),
//!    composes through its `UITextInput` methods (`setMarkedText:selectedRange:`,
//!    a committing `insertText:`, an autocorrect-style `replaceRange:withText:`),
//!    drags the list with a synthetic finger through the backend's touch path,
//!    taps outside the field, then reads the view's accessibility elements as
//!    VoiceOver does and activates a row through one. It ends this part with
//!    `touchdemo: selftest passed` or `touchdemo: selftest FAILED: ...`.
//! 2. It then animates and waits for CI to switch the simulator to dark mode
//!    (`xcrun simctl ui ... appearance dark`) and to send it to the background
//!    and back; on coming back it logs how many frames it built while away
//!    (`touchdemo: frames while away N`), posts a memory warning, sends a
//!    notification (which asks for permission), and ends with
//!    `touchdemo: phase 2 passed` or `touchdemo: phase 2 FAILED: ...`.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use stipple::platform::Lifecycle;
use stipple::prelude::*;
use stipple::widgets::{EditBuffer, text_editor};

const ROWS: usize = 30;
const PAD: f64 = 16.0;
const FIELD_H: f64 = 44.0;
const LIST_H: f64 = 400.0;

#[derive(Default)]
struct State {
    field: EditBuffer,
    tapped: Vec<usize>,
    /// Ask for a frame at every build (the self-test's second part).
    animate: bool,
    /// The builds counted when the app went to the background.
    builds_at_background: Option<usize>,
    /// The views built while in the background, counted on coming back.
    built_away: Option<usize>,
    lifecycle: Vec<Lifecycle>,
}

/// The list's scroll offset in the frame last built (`f64` bits).
static OFFSET: AtomicU64 = AtomicU64::new(0);
/// Views built so far.
static BUILDS: AtomicUsize = AtomicUsize::new(0);
/// Whether the theme's look changed while the app ran.
static THEME_CHANGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// What was logged last, so changes are logged once.
static LAST: std::sync::Mutex<(String, String, Option<bool>)> =
    std::sync::Mutex::new((String::new(), String::new(), None));

fn log(line: &str) {
    println!("{line}");
    if cfg!(target_os = "ios") {
        use std::io::Write;
        let path = std::env::temp_dir().join("touchdemo.log");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{line}");
        }
    }
}

/// Log the field's text, its composition and the theme's look when they
/// change.
fn log_changes(state: &State, dark: bool) {
    let mut last = LAST.lock().unwrap();
    let text = state.field.text().to_string();
    if last.0 != text {
        log(&format!("touchdemo: text {text:?}"));
        last.0 = text;
    }
    let preedit = format!("{:?}", state.field.preedit());
    if last.1 != preedit {
        log(&format!("touchdemo: preedit {preedit}"));
        last.1 = preedit;
    }
    if last.2 != Some(dark) {
        if last.2.is_some() {
            THEME_CHANGED.store(true, Ordering::Relaxed);
            log(&format!(
                "touchdemo: theme {}",
                if dark { "dark" } else { "light" }
            ));
        }
        last.2 = Some(dark);
    }
}

fn view(state: &State, cx: &mut Cx<State>) -> Element {
    BUILDS.fetch_add(1, Ordering::Relaxed);
    let theme = *cx.theme();
    log_changes(state, theme.is_dark());
    if state.animate {
        cx.request_frame();
    }
    let field = text_editor(cx, &theme, &state.field, |s: &mut State| &mut s.field).height(FIELD_H);
    let rows = (0..ROWS)
        .map(|i| {
            button_labeled(&theme, format!("Row {i}"))
                .height(48.0)
                .on_tap(cx, move |s: &mut State| {
                    s.tapped.push(i);
                    log(&format!("touchdemo: row {i} tapped"));
                })
        })
        .collect();
    // A scroll container built by hand (as `scroll` does) to read its offset.
    let id = cx.register_scroll();
    let offset = cx.scroll_offset(id);
    if OFFSET.swap(offset.to_bits(), Ordering::Relaxed) != offset.to_bits() {
        log(&format!("touchdemo: scrolled to {offset:.0}"));
    }
    let list = Element::stack(
        Axis::Vertical,
        vec![column(rows).gap(4.0).align(Align::Start, Align::Stretch)],
    )
    .height(LIST_H)
    .scrollable(id)
    .align(Align::Start, Align::Stretch);
    column(vec![
        field,
        label(&theme, format!("Rows tapped: {}", state.tapped.len())),
        list,
    ])
    .gap(12.0)
    .align(Align::Start, Align::Stretch)
    .padding(Insets::uniform(PAD))
    .grow(1.0)
}

fn main() {
    let mut app = App::new(State::default(), view)
        .title("Stipple Touch")
        .logical_size(Size::new(360.0, 560.0))
        // Follows the system's light and dark mode.
        .theme(Theme::native())
        .on_lifecycle(|s, change| {
            log(&format!("touchdemo: lifecycle {change:?}"));
            s.lifecycle.push(change);
            match change {
                Lifecycle::Background => {
                    s.builds_at_background = Some(BUILDS.load(Ordering::Relaxed));
                }
                Lifecycle::Foreground => {
                    if let Some(then) = s.builds_at_background.take() {
                        let away = BUILDS.load(Ordering::Relaxed) - then;
                        log(&format!("touchdemo: frames while away {away}"));
                        s.built_away = Some(away);
                    }
                }
                _ => {}
            }
        });
    if let Some(font) = Font::system_default() {
        app = app.font(font);
    }
    #[cfg(target_os = "ios")]
    if std::env::var_os("TOUCHDEMO_SELFTEST").is_some() {
        selftest::schedule(&app.handle());
    }
    app.run();
}

#[cfg(target_os = "ios")]
mod selftest {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use stipple::platform::TouchPhase;
    use stipple::platform::backend::ios::simulate;

    enum Step {
        /// Run once.
        Do(Box<dyn FnOnce(&mut State) + Send>),
        /// Wait until this holds, checking every 250 ms, at most this many
        /// more times (then go on, and let the later checks report it).
        Until(Box<dyn Fn(&State) -> bool + Send>, u32),
    }

    /// What went wrong so far.
    static FAILURES: Mutex<Vec<String>> = Mutex::new(Vec::new());
    /// The row activated through its accessibility element.
    static ACTIVATED: AtomicUsize = AtomicUsize::new(usize::MAX);
    /// The notification permission, as the center reported it.
    static PERMISSION: Mutex<Option<&'static str>> = Mutex::new(None);

    fn check(ok: bool, what: impl Into<String>) {
        if !ok {
            let what = what.into();
            log(&format!("touchdemo: check failed: {what}"));
            FAILURES.lock().unwrap().push(what);
        }
    }

    /// Finish a part: log whether every check passed.
    fn report(part: &str) {
        let failures = std::mem::take(&mut *FAILURES.lock().unwrap());
        if failures.is_empty() {
            log(&format!("touchdemo: {part} passed"));
        } else {
            log(&format!(
                "touchdemo: {part} FAILED: {}",
                failures.join("; ")
            ));
        }
    }

    /// Run `steps` one after another, each `ms` after the previous one ran
    /// (not after launch): a slow first frame then delays the whole sequence
    /// instead of making every step due at once, ahead of the touches and
    /// typing the earlier steps queued.
    fn chain(handle: &AppHandle<State>, mut steps: VecDeque<(u64, Step)>) {
        let Some((ms, step)) = steps.pop_front() else {
            return;
        };
        let next = handle.clone();
        handle.after(Duration::from_millis(ms), move |s| {
            match step {
                Step::Do(f) => f(s),
                Step::Until(cond, left) => {
                    if !cond(s) && left > 0 {
                        steps.push_front((250, Step::Until(cond, left - 1)));
                    }
                }
            }
            chain(&next, steps);
        });
    }

    struct Steps(VecDeque<(u64, Step)>);

    impl Steps {
        fn run(&mut self, ms: u64, f: impl FnOnce(&mut State) + Send + 'static) {
            self.0.push_back((ms, Step::Do(Box::new(f))));
        }
        fn until(&mut self, cond: impl Fn(&State) -> bool + Send + 'static) {
            self.0.push_back((250, Step::Until(Box::new(cond), 40)));
        }
    }

    fn tap(x: f64, y: f64) {
        simulate::touch(1, TouchPhase::Started, Point::new(x, y));
        simulate::touch(1, TouchPhase::Ended, Point::new(x, y));
    }

    fn holds_keyboard() -> bool {
        simulate::keyboard_status().contains("first_responder=true")
    }

    /// The `key=(x,y,w,h)` rectangle in a status line.
    fn rect_in(status: &str, key: &str) -> Option<(f64, f64, f64, f64)> {
        let at = status.find(&format!("{key}=("))? + key.len() + 2;
        let end = at + status[at..].find(')')?;
        let v: Vec<f64> = status[at..end]
            .split(',')
            .filter_map(|n| n.parse().ok())
            .collect();
        (v.len() == 4).then(|| (v[0], v[1], v[2], v[3]))
    }

    fn document_is(text: &'static str) -> impl Fn(&State) -> bool + Send {
        move |_| simulate::text_input_status().contains(&format!("doc={text:?} "))
    }

    pub fn schedule(handle: &AppHandle<State>) {
        let _ = std::fs::remove_file(std::env::temp_dir().join("touchdemo.log"));
        log("touchdemo: selftest starting");
        let mut steps = Steps(VecDeque::new());

        // Tap the text field: it takes focus, and the view the keyboard.
        steps.run(1500, |_| {
            log(&format!(
                "touchdemo: keyboard before {}",
                simulate::keyboard_status()
            ));
            tap(120.0, PAD + FIELD_H / 2.0);
        });
        steps.until(|_| holds_keyboard());
        // Type through UIKeyInput: "Hi!", then delete the "!".
        steps.run(0, |_| {
            log(&format!(
                "touchdemo: keyboard focused {}",
                simulate::keyboard_status()
            ));
            simulate::insert_text("Hi");
            simulate::insert_text("!");
            simulate::delete_backward();
        });

        // Compose through UITextInput, as the Japanese keyboard does: marked
        // text at the caret, then the chosen kanji committed in its place,
        // then an autocorrect-style replacement of the word before the caret.
        steps.until(document_is("Hi"));
        steps.run(0, |_| {
            log(&format!(
                "touchdemo: text input {}",
                simulate::text_input_status()
            ));
            simulate::set_marked_text("にほん", (3, 3));
        });
        steps.until(|s| s.field.preedit() == Some("にほん"));
        steps.run(0, |s| {
            let status = simulate::text_input_status();
            log(&format!("touchdemo: composing {status}"));
            check(
                s.field.preedit() == Some("にほん") && s.field.text() == "Hi",
                format!("preedit {:?} over {:?}", s.field.preedit(), s.field.text()),
            );
            check(
                status.contains("text_input=true")
                    && status.contains("doc=\"Hiにほん\"")
                    && status.contains("selected=5..5")
                    && status.contains("marked=\"にほん\""),
                format!("composing status {status}"),
            );
            // The candidate bar goes by the caret, inside the field.
            let rect = rect_in(&status, "first_rect");
            check(
                rect.is_some_and(|(x, y, _, h)| {
                    x > PAD && y >= PAD - 1.0 && y + h <= PAD + FIELD_H + 1.0 && h > 0.0
                }),
                format!("firstRectForRange: {rect:?}"),
            );
            simulate::insert_text("日本");
        });
        steps.until(document_is("Hi日本"));
        steps.run(0, |s| {
            log(&format!(
                "touchdemo: committed {}",
                simulate::text_input_status()
            ));
            check(
                s.field.text() == "Hi日本" && s.field.preedit().is_none(),
                format!("commit gave {:?}", s.field.text()),
            );
            simulate::replace_range(2, 4, "JP");
        });
        steps.until(document_is("HiJP"));
        steps.run(0, |s| {
            log(&format!(
                "touchdemo: replaced {}",
                simulate::text_input_status()
            ));
            check(
                s.field.text() == "HiJP",
                format!("replaceRange gave {:?}", s.field.text()),
            );
        });

        // Drag the list up 300 points with a second finger id, a move every
        // 16 ms, so it scrolls and then flings.
        let (x, y0, y1, moves) = (120.0, 450.0, 150.0, 10u64);
        steps.run(1000, move |_| {
            simulate::touch(2, TouchPhase::Started, Point::new(x, y0))
        });
        for i in 1..=moves {
            let y = y0 + (y1 - y0) * i as f64 / moves as f64;
            steps.run(16, move |_| {
                simulate::touch(2, TouchPhase::Moved, Point::new(x, y))
            });
        }
        steps.run(8, move |_| {
            simulate::touch(2, TouchPhase::Ended, Point::new(x, y1))
        });
        // Tap beside the label, on nothing focusable: focus and the keyboard go.
        steps.run(2500, |_| tap(300.0, PAD + FIELD_H + 12.0 + 8.0));
        steps.until(|_| !holds_keyboard());
        steps.run(0, |s| {
            let keyboard = simulate::keyboard_status();
            log(&format!("touchdemo: keyboard after {keyboard}"));
            let offset = f64::from_bits(OFFSET.load(Ordering::Relaxed));
            // 300 points less the slop, then the fling's run.
            check(offset >= 250.0, format!("scrolled only {offset:.0}"));
            check(s.tapped.is_empty(), format!("rows tapped {:?}", s.tapped));
            check(
                keyboard.contains("first_responder=false"),
                "the keyboard stayed",
            );
        });

        // Accessibility: read the elements as VoiceOver does, then activate a
        // row on screen through its element.
        steps.run(500, |_| {
            let elements = simulate::accessibility_elements();
            let screen = simulate::view_frame_on_screen();
            log(&format!(
                "touchdemo: a11y {} elements, view on screen {screen:?}",
                elements.len()
            ));
            for (i, e) in elements.iter().enumerate().take(12) {
                log(&format!("touchdemo: a11y [{i}] {e:?}"));
            }
            let field = elements.iter().find(|e| e.value == "HiJP");
            check(
                field.is_some_and(|e| e.is_element),
                "no element for the text field",
            );
            // Frames are in screen coordinates: the view's offset (the safe
            // area) is in them.
            let in_field = Point::new(screen.min_x() + 120.0, screen.min_y() + PAD + FIELD_H / 2.0);
            check(
                field.is_some_and(|e| {
                    e.frame.contains(in_field) && e.frame.min_y() >= screen.min_y() + PAD - 1.0
                }),
                format!("text field frame {:?}", field.map(|e| e.frame)),
            );
            check(
                elements
                    .iter()
                    .any(|e| e.label == "Rows tapped: 0" && e.traits.contains("static_text")),
                "no static text element for the label",
            );
            // A row well inside the list's visible part (it starts about 105
            // points down and is 400 tall).
            let (top, bottom) = (screen.min_y() + 140.0, screen.min_y() + 420.0);
            let row = elements.iter().position(|e| {
                e.label.starts_with("Row ")
                    && e.traits.contains("button")
                    && e.frame.min_y() > top
                    && e.frame.max_y() < bottom
            });
            match row {
                Some(i) => {
                    let n = elements[i].label[4..].parse().unwrap_or(usize::MAX);
                    log(&format!("touchdemo: activating {:?}", elements[i].label));
                    ACTIVATED.store(n, Ordering::Relaxed);
                    simulate::activate_accessibility_element(i);
                }
                None => check(false, "no row button element on screen"),
            }
        });
        steps.until(|s| !s.tapped.is_empty());
        steps.run(0, |s| {
            let want = ACTIVATED.load(Ordering::Relaxed);
            check(
                s.tapped == [want],
                format!("activating row {want} tapped {:?}", s.tapped),
            );
            let elements = simulate::accessibility_elements();
            check(
                elements.iter().any(|e| e.label == "Rows tapped: 1"),
                "the label's element didn't follow the update",
            );
            report("selftest");
        });

        // Part 2: animate, and wait for CI to switch to dark mode and to
        // send the app to the background and back.
        steps.run(0, |s| {
            s.animate = true;
            log("touchdemo: waiting for appearance");
        });
        steps.0.push_back((
            250,
            Step::Until(Box::new(|_| LAST.lock().unwrap().2 == Some(true)), 240),
        ));
        steps.run(0, |_| log("touchdemo: waiting for background"));
        steps.0.push_back((
            250,
            Step::Until(
                Box::new(|s| s.lifecycle.contains(&Lifecycle::Foreground)),
                480,
            ),
        ));
        steps.run(500, |_| simulate::memory_warning());
        steps.until(|s| s.lifecycle.contains(&Lifecycle::LowMemory));
        steps.run(0, |s| {
            s.animate = false;
            check(
                THEME_CHANGED.load(Ordering::Relaxed) && LAST.lock().unwrap().2 == Some(true),
                "the theme didn't follow dark mode",
            );
            check(
                s.built_away == Some(0),
                format!("built {:?} views in the background", s.built_away),
            );
            check(
                s.lifecycle
                    == [
                        Lifecycle::Background,
                        Lifecycle::Foreground,
                        Lifecycle::LowMemory,
                    ],
                format!("lifecycle {:?}", s.lifecycle),
            );
            // Notifications: the first asks for permission.
            let sent = stipple::platform::notify(
                &Notification::new("Stipple").body("A notification from touchdemo"),
            );
            log(&format!("touchdemo: notify {sent:?}"));
            check(sent.is_ok(), format!("notify: {sent:?}"));
        });
        steps.run(1000, |_| {
            simulate::notification_status(|status| {
                log(&format!("touchdemo: notification permission {status}"));
                *PERMISSION.lock().unwrap() = Some(status);
            });
        });
        steps.until(|_| PERMISSION.lock().unwrap().is_some());
        steps.run(0, |_| {
            let status = *PERMISSION.lock().unwrap();
            check(
                status.is_some_and(|s| s != "unavailable" && s != "unknown"),
                format!("notification center: {status:?}"),
            );
            report("phase 2");
        });
        chain(handle, steps.0);
    }
}
