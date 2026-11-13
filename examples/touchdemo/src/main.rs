//! A text field above a list of rows, a sideways strip of chips and a box to
//! pinch: on a touch screen a finger drag scrolls the list (and flings it), a
//! sideways one the strip, a tap on a row taps it and a long press opens its
//! context menu, two fingers pinch the box, and the text field brings up the
//! keyboard. With a mouse the wheel scrolls, as everywhere, and a right-click
//! opens a row's menu.
//!
//! It logs what happens, on stdout and, on iOS, to `touchdemo.log` in the
//! app's tmp directory, which CI reads back from the simulator:
//!
//! - `touchdemo: scrolled to N` as the list's offset changes;
//! - `touchdemo: row N tapped`, and `touchdemo: row N pinned` when its
//!   context menu's item is picked;
//! - `touchdemo: strip scrolled to N` as the strip's offset changes;
//! - `touchdemo: pinch <phase> scale S` as two fingers pinch the box;
//! - `touchdemo: text "..."` and `touchdemo: preedit ...` as the field's text
//!   and composition change;
//! - `touchdemo: theme dark` / `light` as the system's look changes;
//! - `touchdemo: lifecycle Background` (and `Foreground`, `LowMemory`);
//! - `touchdemo: back Started` / `back Cancelled` and `touchdemo: went back`
//!   for the back swipe from the left edge, which it takes while "a screen
//!   is open" (the self-test opens one);
//! - `touchdemo: restored text "..."` when it starts again with the state it
//!   saved (the field's text) after iOS ended it in the background;
//! - `touchdemo: started in session "..."` on iOS, the scene session it runs
//!   in (the same after a system kill, a new one after the user closed it).
//!
//! **Closing the scene (iOS).** Launched with `TOUCHDEMO_CLOSE_SCENE` set, it
//! types "Not for later" into the field and waits to be sent to the
//! background (where it saves the text); there it asks UIKit to destroy its
//! scene session (`touchdemo: closing session Some("...")`), as the user
//! swiping it away in the app switcher does. The state saved under the
//! session is dropped with it, and the next launch starts afresh.
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
//!    VoiceOver does and activates a row through one. Then it holds a finger
//!    on a row until its context menu opens and picks the item (through its
//!    accessibility element), drags the strip sideways, and pinches the box
//!    with two synthetic fingers. It swipes from the left edge over the strip:
//!    while it has nothing to go back from the strip scrolls; with a screen
//!    open the swipe is the back gesture (its progress, then `on_back`) and
//!    the strip doesn't move; a short swipe taken back is cancelled. Then it
//!    opens the document picker for a file to open, then one to save to,
//!    cancelling each (as its Cancel button would) and checking the app's
//!    callback heard, and opens each again and answers it as picking a file
//!    would (handing the picker's delegate a file it wrote): the open
//!    callback gets the file's bytes, the save callback `true`. It ends this
//!    part with `touchdemo: selftest passed` or
//!    `touchdemo: selftest FAILED: ...`.
//! 2. It then animates and waits for CI to switch the simulator to dark mode
//!    (`xcrun simctl ui ... appearance dark`) and to send it to the background
//!    and back; on coming back it logs how many frames it built while away
//!    (`touchdemo: frames while away N`), posts a memory warning, sends a
//!    notification (which asks for permission), and ends with
//!    `touchdemo: phase 2 passed` or `touchdemo: phase 2 FAILED: ...`.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use stipple::core::{PinchEvent, PinchPhase};
use stipple::platform::Lifecycle;
use stipple::prelude::*;
use stipple::widgets::{EditBuffer, text_editor};

const ROWS: usize = 30;
const PAD: f64 = 16.0;
const FIELD_H: f64 = 44.0;
const LIST_H: f64 = 400.0;
const STRIP_H: f64 = 44.0;
const PINCH_H: f64 = 100.0;

#[derive(Default)]
struct State {
    field: EditBuffer,
    tapped: Vec<usize>,
    /// The rows whose context menu item was picked.
    pinned: Vec<usize>,
    /// The box's zoom, and what it was when the pinch began.
    zoom: f64,
    zoom_at_start: f64,
    /// The pinch phases seen, and the scale when the last one ended.
    pinch_phases: Vec<PinchPhase>,
    pinch_scale: f64,
    /// Ask for a frame at every build (the self-test's second part).
    animate: bool,
    /// The builds counted when the app went to the background.
    builds_at_background: Option<usize>,
    /// The views built while in the background, counted on coming back.
    built_away: Option<usize>,
    lifecycle: Vec<Lifecycle>,
    /// "A screen is open": there is something to go back from, so the back
    /// swipe is the app's.
    detail: bool,
    /// The back gesture's steps, and how many times it went back.
    back_gestures: Vec<BackGesture>,
    backs: usize,
}

/// The list's scroll offset in the frame last built (`f64` bits).
static OFFSET: AtomicU64 = AtomicU64::new(0);
/// The strip's scroll offset in the frame last built (`f64` bits).
static STRIP_OFFSET: AtomicU64 = AtomicU64::new(0);
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
                // A long press (or a right-click) opens it.
                .context_menu(
                    cx,
                    Menu::new("").item(MenuItem::new(
                        format!("Pin row {i}"),
                        move |s: &mut State| {
                            s.pinned.push(i);
                            log(&format!("touchdemo: row {i} pinned"));
                        },
                    )),
                )
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
    // A sideways strip of chips, scrolled by a sideways drag.
    let chips = (0..12)
        .map(|i| button_labeled(&theme, format!("Chip {i}")).width(90.0))
        .collect();
    let strip_id = cx.register_scroll();
    let strip_offset = cx.scroll_offset(strip_id);
    if STRIP_OFFSET.swap(strip_offset.to_bits(), Ordering::Relaxed) != strip_offset.to_bits() {
        log(&format!("touchdemo: strip scrolled to {strip_offset:.0}"));
    }
    let strip = Element::stack(Axis::Horizontal, vec![row(chips).gap(4.0)])
        .height(STRIP_H)
        .scrollable(strip_id);
    // A box two fingers pinch.
    let pinch = label(&theme, format!("Pinch me: {:.2}x", state.zoom))
        .height(PINCH_H)
        .on_pinch(cx, |ev: &PinchEvent, s: &mut State| {
            if ev.phase == PinchPhase::Began {
                s.zoom_at_start = s.zoom;
            }
            s.zoom = (s.zoom_at_start * ev.scale).clamp(0.25, 8.0);
            if s.pinch_phases.last() != Some(&ev.phase) {
                log(&format!(
                    "touchdemo: pinch {:?} scale {:.2}",
                    ev.phase, ev.scale
                ));
            }
            s.pinch_phases.push(ev.phase);
            s.pinch_scale = ev.scale;
        });
    // Everything but the strip keeps off the screen's edges; the strip runs
    // edge to edge, where the back swipe starts.
    let inset = |e: Element| {
        column(vec![e])
            .align(Align::Start, Align::Stretch)
            .padding(Insets::symmetric(PAD, 0.0))
    };
    column(vec![
        inset(field),
        inset(label(
            &theme,
            format!("Rows tapped: {}", state.tapped.len()),
        )),
        inset(list),
        strip,
        inset(pinch),
    ])
    .gap(12.0)
    .align(Align::Start, Align::Stretch)
    .padding(Insets::symmetric(0.0, PAD))
    .grow(1.0)
}

fn main() {
    let state = State {
        zoom: 1.0,
        ..State::default()
    };
    let mut app = App::new(state, view)
        .title("Stipple Touch")
        .logical_size(Size::new(360.0, 720.0))
        // Follows the system's light and dark mode.
        .theme(Theme::native())
        .on_lifecycle(|s, change| {
            log(&format!("touchdemo: lifecycle {change:?}"));
            s.lifecycle.push(change);
            match change {
                Lifecycle::Background => {
                    #[cfg(target_os = "ios")]
                    selftest::on_background();
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
        })
        // Back (iOS's swipe from the left edge) closes "the screen", when
        // one is open.
        .back_enabled(|s| s.detail)
        .on_back(|s| {
            s.detail = false;
            s.backs += 1;
            log("touchdemo: went back");
            Handled::Yes
        })
        .on_back_progress(|s, gesture| {
            match gesture {
                BackGesture::Started { edge } => log(&format!("touchdemo: back Started {edge:?}")),
                BackGesture::Cancelled => log("touchdemo: back Cancelled"),
                BackGesture::Progressed { .. } => {}
            }
            s.back_gestures.push(gesture);
        })
        // The field's text comes back after iOS ends the app in the
        // background.
        .save_state(|s| s.field.text().as_bytes().to_vec())
        .restore_state(|s, bytes| {
            let text = String::from_utf8_lossy(bytes).into_owned();
            log(&format!("touchdemo: restored text {text:?}"));
            s.field = EditBuffer::from_text(text);
        });
    if let Some(font) = Font::system_default() {
        app = app.font(font);
    }
    #[cfg(target_os = "ios")]
    {
        use stipple::platform::backend::ios::simulate;
        // Which scene session this launch is in: the same one after a
        // system kill, a new one after the user closed the app.
        app.handle().after(Duration::from_millis(500), |_| {
            let session = simulate::scene_session();
            log(&format!("touchdemo: started in session {session:?}"));
        });
        if std::env::var_os("TOUCHDEMO_SELFTEST").is_some() {
            selftest::schedule(&app.handle());
        }
        if std::env::var_os("TOUCHDEMO_CLOSE_SCENE").is_some() {
            selftest::close_scene(&app.handle());
        }
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
    use stipple::platform::dialog::{self, FileDialog};

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
    /// Where the current synthetic gesture is (view coordinates).
    static TOUCH_AT: Mutex<Point> = Mutex::new(Point::new(0.0, 0.0));
    /// The notification permission, as the center reported it.
    static PERMISSION: Mutex<Option<&'static str>> = Mutex::new(None);
    /// What the open picker's callback got (the file's name and bytes),
    /// once it ran.
    static PICKED: Mutex<Option<Option<Picked>>> = Mutex::new(None);
    /// A picked file's name and bytes.
    type Picked = (String, Vec<u8>);
    /// The strip's offset when a back swipe began (`f64` bits).
    static STRIP_BEFORE: AtomicU64 = AtomicU64::new(0);
    /// What the save picker's callback got, once it ran.
    static SAVED: Mutex<Option<bool>> = Mutex::new(None);
    /// The document picker's class.
    const PICKER: &str = "UIDocumentPickerViewController";

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

    /// Close the scene's session the next time the app goes to the
    /// background.
    static CLOSE_ON_BACKGROUND: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);

    /// Type something to save, then, once in the background (the state
    /// saved), close the scene's session as the user swiping the app away
    /// does: the state goes with the session.
    pub(super) fn close_scene(handle: &AppHandle<State>) {
        handle.after(Duration::from_millis(1500), |s| {
            s.field = EditBuffer::from_text("Not for later");
            CLOSE_ON_BACKGROUND.store(true, Ordering::Relaxed);
            log("touchdemo: waiting for background to close the session");
        });
    }

    /// The app went to the background: close the session if asked to. UIKit
    /// takes the request later; the state is saved first, as the app goes.
    pub(super) fn on_background() {
        if CLOSE_ON_BACKGROUND.swap(false, Ordering::Relaxed) {
            let session = simulate::destroy_scene_session(|error| {
                log(&format!("touchdemo: closing the session failed: {error}"));
            });
            log(&format!("touchdemo: closing session {session:?}"));
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

    /// The index and label of an open context menu's item, read through
    /// accessibility.
    fn menu_item() -> Option<(usize, String)> {
        simulate::accessibility_elements()
            .into_iter()
            .enumerate()
            .find(|(_, e)| e.label.starts_with("Pin row"))
            .map(|(i, e)| (i, e.label))
    }

    /// The middle of the first element whose label starts with `label`, in
    /// the view's coordinates (where touches go): its accessibility frame,
    /// which is on screen, less where the view is.
    fn element_center(label: &str) -> Option<Point> {
        let screen = simulate::view_frame_on_screen();
        let e = simulate::accessibility_elements()
            .into_iter()
            .find(|e| e.label.starts_with(label))?;
        let c = e.frame.center();
        Some(Point::new(c.x - screen.min_x(), c.y - screen.min_y()))
    }

    /// The middle of a row well inside the list's visible part (it starts
    /// about 105 points down and is 400 tall), in the view's coordinates.
    fn visible_row() -> Option<Point> {
        let screen = simulate::view_frame_on_screen();
        let (top, bottom) = (screen.min_y() + 140.0, screen.min_y() + 420.0);
        let e = simulate::accessibility_elements().into_iter().find(|e| {
            e.label.starts_with("Row ") && e.frame.min_y() > top && e.frame.max_y() < bottom
        })?;
        let c = e.frame.center();
        Some(Point::new(c.x - screen.min_x(), c.y - screen.min_y()))
    }

    fn strip_offset() -> f64 {
        f64::from_bits(STRIP_OFFSET.load(Ordering::Relaxed))
    }

    /// Swipe finger `id` from the left edge over the strip through each of
    /// `xs` (16 ms apart), hold still a moment (so nothing flings), and lift.
    fn edge_swipe(steps: &mut Steps, id: u64, xs: &'static [f64]) {
        steps.run(300, move |_| {
            let y = element_center("Chip").map_or(560.0, |p| p.y);
            *TOUCH_AT.lock().unwrap() = Point::new(3.0, y);
            STRIP_BEFORE.store(strip_offset().to_bits(), Ordering::Relaxed);
            simulate::touch(id, TouchPhase::Started, Point::new(3.0, y));
        });
        let at = |x: f64| Point::new(x, TOUCH_AT.lock().unwrap().y);
        for &x in xs {
            steps.run(16, move |_| simulate::touch(id, TouchPhase::Moved, at(x)));
        }
        let last = *xs.last().unwrap_or(&3.0);
        steps.run(150, move |_| {
            simulate::touch(id, TouchPhase::Moved, at(last))
        });
        steps.run(16, move |_| {
            simulate::touch(id, TouchPhase::Ended, at(last))
        });
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
        });

        // A long press on a row opens its context menu (and doesn't tap the
        // row); pick its item through its accessibility element.
        steps.run(300, |_| {
            let at = visible_row().unwrap_or(Point::new(120.0, 300.0));
            log(&format!("touchdemo: long press at {at:?}"));
            *TOUCH_AT.lock().unwrap() = at;
            simulate::touch(3, TouchPhase::Started, at);
        });
        steps.run(800, |_| {
            simulate::touch(3, TouchPhase::Ended, *TOUCH_AT.lock().unwrap())
        });
        steps.until(|_| menu_item().is_some());
        steps.run(0, |s| {
            let item = menu_item();
            log(&format!("touchdemo: menu item {item:?}"));
            check(item.is_some(), "the long press opened no menu");
            check(s.tapped.len() == 1, format!("rows tapped {:?}", s.tapped));
            if let Some((i, _)) = item {
                simulate::activate_accessibility_element(i);
            }
        });
        steps.until(|s| !s.pinned.is_empty());
        steps.run(0, |s| {
            check(s.pinned.len() == 1, format!("rows pinned {:?}", s.pinned));
            check(s.tapped.len() == 1, format!("rows tapped {:?}", s.tapped));
        });

        // Drag the strip of chips sideways, 240 points left.
        // Measured once: the chips move with the strip.
        steps.run(300, |_| {
            let at = element_center("Chip 1").unwrap_or(Point::new(150.0, 560.0));
            *TOUCH_AT.lock().unwrap() = Point::new(at.x + 140.0, at.y);
        });
        let strip = |dx: f64| {
            let at = *TOUCH_AT.lock().unwrap();
            Point::new(at.x + dx, at.y)
        };
        steps.run(0, move |_| {
            simulate::touch(4, TouchPhase::Started, strip(0.0))
        });
        for i in 1..=8u32 {
            let dx = -240.0 * f64::from(i) / 8.0;
            steps.run(16, move |_| {
                simulate::touch(4, TouchPhase::Moved, strip(dx))
            });
        }
        steps.run(16, move |_| {
            simulate::touch(4, TouchPhase::Ended, strip(-240.0))
        });
        steps.until(|_| f64::from_bits(STRIP_OFFSET.load(Ordering::Relaxed)) >= 200.0);
        steps.run(0, |s| {
            let offset = f64::from_bits(STRIP_OFFSET.load(Ordering::Relaxed));
            check(
                offset >= 200.0,
                format!("the strip scrolled only {offset:.0}"),
            );
            check(s.tapped.len() == 1, format!("rows tapped {:?}", s.tapped));
        });

        // Pinch the box: two fingers 60 points apart spread to 150.
        let fingers = |gap: f64| {
            let at = *TOUCH_AT.lock().unwrap();
            (
                Point::new(at.x - gap / 2.0, at.y),
                Point::new(at.x + gap / 2.0, at.y),
            )
        };
        steps.run(300, move |_| {
            *TOUCH_AT.lock().unwrap() =
                element_center("Pinch me").unwrap_or(Point::new(180.0, 650.0));
            let (a, b) = fingers(60.0);
            simulate::touch(5, TouchPhase::Started, a);
            simulate::touch(6, TouchPhase::Started, b);
        });
        for i in 1..=6u32 {
            let gap = 60.0 + 90.0 * f64::from(i) / 6.0;
            steps.run(16, move |_| {
                let (a, b) = fingers(gap);
                simulate::touch(5, TouchPhase::Moved, a);
                simulate::touch(6, TouchPhase::Moved, b);
            });
        }
        steps.run(16, move |_| {
            let (a, b) = fingers(150.0);
            simulate::touch(6, TouchPhase::Ended, b);
            simulate::touch(5, TouchPhase::Ended, a);
        });
        steps.until(|s| s.pinch_phases.contains(&PinchPhase::Ended));
        steps.run(0, |s| {
            log(&format!(
                "touchdemo: pinched to {:.2}x ({} events)",
                s.zoom,
                s.pinch_phases.len()
            ));
            check(
                s.pinch_phases.first() == Some(&PinchPhase::Began)
                    && s.pinch_phases.last() == Some(&PinchPhase::Ended),
                format!("pinch phases {:?}", s.pinch_phases),
            );
            check(
                (s.pinch_scale - 2.5).abs() < 0.05,
                format!("pinch scale {:.3}", s.pinch_scale),
            );
            check(s.tapped.len() == 1, format!("rows tapped {:?}", s.tapped));
        });

        // The back swipe from the left edge. With nothing to go back from,
        // the edge is the app's: the drag scrolls the strip back.
        const OUT: &[f64] = &[20.0, 40.0, 60.0, 80.0, 100.0, 120.0, 140.0, 160.0];
        edge_swipe(&mut steps, 7, OUT);
        steps.until(|_| {
            strip_offset() < f64::from_bits(STRIP_BEFORE.load(Ordering::Relaxed)) - 50.0
        });
        steps.run(300, |s| {
            let (before, after) = (
                f64::from_bits(STRIP_BEFORE.load(Ordering::Relaxed)),
                strip_offset(),
            );
            log(&format!(
                "touchdemo: edge drag with back off: strip {before:.0} -> {after:.0}"
            ));
            check(
                after < before - 50.0,
                format!("with back off the edge drag scrolled the strip {before:.0} -> {after:.0}"),
            );
            check(
                s.back_gestures.is_empty() && s.backs == 0,
                format!("back with back off: {:?}", s.back_gestures),
            );
            // "Open a screen": now there is something to go back from.
            s.detail = true;
        });
        // With a screen open, the same drag is the back gesture: its
        // progress, then on_back, and the strip stays put.
        const FAR: &[f64] = &[20.0, 50.0, 80.0, 110.0, 140.0, 170.0, 200.0, 230.0, 260.0];
        edge_swipe(&mut steps, 8, FAR);
        steps.until(|s| s.backs > 0);
        steps.run(0, |s| {
            let (before, after) = (
                f64::from_bits(STRIP_BEFORE.load(Ordering::Relaxed)),
                strip_offset(),
            );
            let progress: Vec<f32> = s
                .back_gestures
                .iter()
                .filter_map(|g| match g {
                    BackGesture::Progressed { progress } => Some(*progress),
                    _ => None,
                })
                .collect();
            log(&format!(
                "touchdemo: back swipe: {} progress steps, last {:?}, strip {before:.0} -> {after:.0}",
                progress.len(),
                progress.last()
            ));
            check(s.backs == 1 && !s.detail, format!("went back {} times", s.backs));
            check(
                s.back_gestures.first()
                    == Some(&BackGesture::Started {
                        edge: stipple::platform::BackEdge::Left,
                    })
                    && !s.back_gestures.contains(&BackGesture::Cancelled),
                format!("back gesture {:?}", s.back_gestures.first()),
            );
            check(
                progress.len() >= 3 && progress.last().is_some_and(|p| *p > 0.5),
                format!("back progress {progress:?}"),
            );
            check(
                (after - before).abs() < 20.0,
                format!("the back swipe scrolled the strip {before:.0} -> {after:.0}"),
            );
            check(s.tapped.len() == 1, format!("rows tapped {:?}", s.tapped));
            s.back_gestures.clear();
            s.detail = true;
        });
        // A short swipe, taken back: cancelled, and nothing goes back.
        const BACK: &[f64] = &[20.0, 40.0, 60.0, 50.0, 40.0, 30.0, 20.0];
        edge_swipe(&mut steps, 9, BACK);
        steps.until(|s| s.back_gestures.contains(&BackGesture::Cancelled));
        steps.run(0, |s| {
            check(
                s.back_gestures.last() == Some(&BackGesture::Cancelled) && s.backs == 1 && s.detail,
                format!(
                    "a swipe taken back: {:?}, went back {} times",
                    s.back_gestures.last(),
                    s.backs
                ),
            );
            s.detail = false;
        });

        // The document pickers: each must come up, and cancelling it (as its
        // Cancel button does; its contents run in another process, which
        // nothing here can tap) must reach the app's callback.
        steps.run(0, |_| {
            dialog::open_file_contents(&FileDialog::new().with_accept(".txt,image/*"), |file| {
                log(&format!(
                    "touchdemo: open picker answered {:?}",
                    file.as_ref().map(|f| &f.name)
                ));
                *PICKED.lock().unwrap() = Some(file.map(|f| (f.name, f.bytes)));
            });
        });
        steps.until(|_| simulate::presented_controller() == PICKER);
        steps.run(600, |_| {
            let shown = simulate::presented_controller();
            log(&format!("touchdemo: open picker presented {shown}"));
            check(
                shown.starts_with(PICKER),
                format!("open picker: {shown:?} on top"),
            );
            check(
                simulate::cancel_document_picker(),
                "no open picker to cancel",
            );
        });
        steps.until(|_| PICKED.lock().unwrap().is_some());
        steps.run(0, |_| {
            let picked = PICKED.lock().unwrap().take();
            check(
                picked == Some(None),
                format!("open picker answered {picked:?}"),
            );
            dialog::save_file_contents_then(
                &FileDialog::new().with_file_name("touchdemo.txt"),
                b"Saved by touchdemo\n",
                |saved| {
                    log(&format!("touchdemo: save picker answered {saved}"));
                    *SAVED.lock().unwrap() = Some(saved);
                },
            );
        });
        steps.until(|_| simulate::presented_controller() == PICKER);
        steps.run(600, |_| {
            let shown = simulate::presented_controller();
            log(&format!("touchdemo: save picker presented {shown}"));
            check(
                shown.starts_with(PICKER),
                format!("save picker: {shown:?} on top"),
            );
            check(
                simulate::cancel_document_picker(),
                "no save picker to cancel",
            );
        });
        steps.until(|_| SAVED.lock().unwrap().is_some());
        steps.run(0, |_| {
            let saved = SAVED.lock().unwrap().take();
            check(
                saved == Some(false),
                format!("save picker answered {saved:?}"),
            );
        });

        // The receiving side: open each picker again and answer it as picking
        // a file does (the picker's contents can't be tapped here), handing
        // its delegate a file written for it, as UIKit hands over its copy.
        let picked_path = std::env::temp_dir().join("touchdemo-picked.txt");
        let path = picked_path.clone();
        steps.run(0, move |_| {
            let _ = std::fs::write(&path, b"Picked by touchdemo\n");
            dialog::open_file_contents(&FileDialog::new().with_accept(".txt"), |file| {
                log(&format!(
                    "touchdemo: open picker picked {:?}",
                    file.as_ref()
                        .map(|f| (&f.name, String::from_utf8_lossy(&f.bytes)))
                ));
                *PICKED.lock().unwrap() = Some(file.map(|f| (f.name, f.bytes)));
            });
        });
        steps.until(|_| simulate::presented_controller() == PICKER);
        let path = picked_path.clone();
        steps.run(600, move |_| {
            check(
                simulate::pick_document(&path.to_string_lossy()),
                "no open picker to pick from",
            );
        });
        steps.until(|_| PICKED.lock().unwrap().is_some());
        let path = picked_path.clone();
        steps.run(0, move |_| {
            let picked = PICKED.lock().unwrap().take();
            check(
                picked
                    == Some(Some((
                        "touchdemo-picked.txt".to_string(),
                        b"Picked by touchdemo\n".to_vec(),
                    ))),
                format!("open picker picked {picked:?}"),
            );
            // The copy UIKit hands over is the app's to delete, and was.
            check(!path.exists(), "the picked copy was left behind");
            dialog::save_file_contents_then(
                &FileDialog::new().with_file_name("touchdemo.txt"),
                b"Saved by touchdemo\n",
                |saved| {
                    log(&format!("touchdemo: save picker exported {saved}"));
                    *SAVED.lock().unwrap() = Some(saved);
                },
            );
        });
        steps.until(|_| simulate::presented_controller() == PICKER);
        steps.run(600, |_| {
            // Where the user "saved" it.
            let dest = std::env::temp_dir().join("touchdemo-exported.txt");
            check(
                simulate::pick_document(&dest.to_string_lossy()),
                "no save picker to pick from",
            );
        });
        steps.until(|_| SAVED.lock().unwrap().is_some());
        steps.run(0, |_| {
            let saved = SAVED.lock().unwrap().take();
            check(
                saved == Some(true),
                format!("save picker exported {saved:?}"),
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
