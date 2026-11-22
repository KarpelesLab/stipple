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
//! - on desktops, `touchdemo: remember on` / `off` as the "Remember"
//!   checkbox beside the box is toggled, and `touchdemo: toast "Row 2
//!   tapped"` when a tap on "Row 2" raises a toast (for the screen readers'
//!   checks: UIA's Toggle pattern, the toast's announcement);
//! - `touchdemo: text "..."` and `touchdemo: preedit ...` as the field's text
//!   and composition change;
//! - `touchdemo: keyboard inset N` as the on-screen keyboard comes and goes;
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
//! background (where it would save the text); there it asks UIKit to destroy
//! its scene session (`touchdemo: closing session Some("...")`), as the user
//! swiping it away in the app switcher does. Nothing is kept for the session
//! from then on, and the next launch starts afresh, even when UIKit hands it
//! a session of the same identifier (as an iPad simulator does). UIKit only
//! takes the request on a device that supports multiple scenes (an iPad, with
//! `UIApplicationSupportsMultipleScenes` in the manifest); an iPhone refuses
//! it (`touchdemo: closing the session failed: ...`).
//!
//! **Typing something to keep (iOS).** Launched with `TOUCHDEMO_TYPE` set,
//! it types that text into the field (`touchdemo: typed, waiting for
//! background`), for the state saved in the background and restored after a
//! system kill.
//!
//! **Ranges and a table.** Launched with `TOUCHDEMO_RANGES` set (CI's
//! accessibility depth checks), it shows a "Volume" slider, a "Level"
//! number input captioned by a label, a progress bar following the volume
//! and a small table above the list, for the screen readers' range values,
//! tables and labelled-by relations: it logs `touchdemo: volume V` and
//! `touchdemo: level N` as they change.
//!
//! **Services (real touches).** Launched with `TOUCHDEMO_SERVICES` set (CI's
//! XCUITest sets it in the launch environment), it shows a few more
//! controls above the list, for a test that drives the app from outside with
//! real touches (`.github/a11y/ios/UITests/StippleRealTouchTests.swift`)
//! and reads what they did back from its accessibility tree: a "Ticks: N"
//! label an `AppHandle::every` timer counts up, a "Timer" button that arms
//! an `AppHandle::after` timer ("Timer: fired") and a thread that wakes the
//! loop through `AppHandle::update` ("Worker: done"), "Set clip" and "Get
//! clip" buttons that write and read the system clipboard
//! (`platform::clipboard`), an "Open URL" button (`AppHandle::open_url`),
//! and labels with the list's offset, the rows pinned through their
//! context menu and the drops the pinch box took (a long press on "Rows
//! tapped" lifts its text, which the test drops there). The list is shorter
//! to make room. It logs
//! `touchdemo: services ...` lines.
//!
//! **Self-test (iOS).** Launched with `TOUCHDEMO_SELFTEST` set (`xcrun simctl
//! launch` passes it on as `SIMCTL_CHILD_TOUCHDEMO_SELFTEST=1`), the app drives
//! itself, since a simulator can't script touches or typing:
//!
//! 0. it reads the bundle's packaged files (`Stipple.app/assets/`, copied
//!    from `examples/touchdemo/assets` by CI) through `platform::assets`,
//!    and loads one through `AppHandle::load_asset`;
//! 1. it taps the text field, types through the view's `UIKeyInput` methods
//!    (`insertText:` and `deleteBackward`, sent through objc dispatch),
//!    composes through its `UITextInput` methods (`setMarkedText:selectedRange:`,
//!    a committing `insertText:`, an autocorrect-style `replaceRange:withText:`),
//!    then shows a barcode field beside it (a number pad with a Done key):
//!    Return in the text field (its action is Next) moves focus there, the
//!    view's `UITextInputTraits` must say so (`keyboardType` 4,
//!    `returnKeyType` 9), and digits then Return submit it
//!    (`touchdemo: code submitted ...`); then it
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
//!    `touchdemo: selftest FAILED: ...`. Then mobile input, reported on its
//!    own (`touchdemo: mobile input passed` or `FAILED: ...`): a hardware
//!    keyboard's dead key as UIKit's text system types it (the accent as
//!    marked text, then the composed letter), a text and a file dropped on
//!    the pinch box through the view's `UIDropInteraction` delegate (with
//!    real item providers), and the list scrolled by the trackpad's scroll
//!    recognizer. Then dragging out and the trackpad's depth (`touchdemo:
//!    drag out and trackpad passed`): its two drag sources ("Rows tapped",
//!    a text; "Drag file", a file) lifted through the view's
//!    `UIDragInteraction` delegate and their items dropped on the pinch box,
//!    a secondary click opening a row's menu, the I-beam pointer over the
//!    field, and a trackpad flick coasting on.
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
use stipple::widgets::TextInput;
use stipple::widgets::{Column, NumberInput, SelectionMode, TableSpec, TableState};

const ROWS: usize = 30;
const PAD: f64 = 16.0;
const FIELD_H: f64 = 44.0;
const LIST_H: f64 = 400.0;
/// The list's height with the services' controls shown above it.
const LIST_H_SERVICES: f64 = 220.0;
/// What "Set clip" puts on the clipboard.
const COPIED: &str = "Copied by touchdemo";
/// What "Open URL" opens.
const URL: &str = "https://example.com/stipple-touchdemo";
/// What a long press on "Rows tapped" lifts out of the app.
const LIFTED_TEXT: &str = "Lifted out of touchdemo";
/// What the file "Drag file" lifts out of the app holds.
const LIFTED_FILE: &str = "A file lifted out of touchdemo\n";
const STRIP_H: f64 = 44.0;
const PINCH_H: f64 = 100.0;
/// Whether it runs on a desktop (where the screen readers' checks use the
/// "Remember" checkbox and a toast).
const DESKTOP: bool = !cfg!(any(target_os = "ios", target_os = "android"));

#[derive(Default)]
struct State {
    field: TextInput,
    /// A barcode field beside the text field, shown once the self-test asks
    /// (so Tab order on desktops stays field, then rows): a number pad with
    /// a Done key, reached from the field by its Next key.
    code: Option<TextInput>,
    code_submitted: bool,
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
    /// The "Remember" checkbox (desktops).
    remember: bool,
    /// The toasts a tap on "Row 2" raises (desktops).
    toasts: Toasts,
    /// What was dropped on the pinch box, from another app.
    dropped: Vec<stipple::platform::DropData>,
    /// The services' controls and what they did (`TOUCHDEMO_SERVICES`).
    services: Option<Services>,
    /// The slider, number input and table (`TOUCHDEMO_RANGES`).
    ranges: Option<Ranges>,
}

/// What `TOUCHDEMO_RANGES` shows.
struct Ranges {
    volume: f64,
    level: NumberInput,
    table: TableState,
}

/// The rows of the ranges' table.
const TABLE_ROWS: usize = 20;

/// A "Volume" slider, a "Level" number input with its caption, a progress
/// bar at the volume, and a table of 20 rows (a few built at a time).
fn ranges_view(r: &Ranges, cx: &mut Cx<State>) -> Element {
    let volume = slider(cx, r.volume, |s: &mut State, v| {
        if let Some(r) = s.ranges.as_mut() {
            r.volume = (v * 100.0).round() / 100.0;
            log(&format!("touchdemo: volume {:.2}", r.volume));
        }
    })
    .a11y_label("Volume");
    let level = number_input(cx, &r.level, |s| {
        &mut s.ranges.as_mut().expect("ranges shown").level
    });
    if LEVEL.swap(r.level.value().to_bits(), Ordering::Relaxed) != r.level.value().to_bits() {
        log(&format!("touchdemo: level {}", r.level.value()));
    }
    let level = row(vec![
        label(cx, "Level").a11y_id("level-label"),
        level.labelled_by("level-label"),
    ])
    .gap(8.0);
    let bar = progress_bar(cx, r.volume).a11y_label("Progress");
    let cols = [Column::new("Item", 120.0), Column::new("Count", 80.0)];
    let grid = stipple::widgets::table(
        cx,
        &r.table,
        |s| &mut s.ranges.as_mut().expect("ranges shown").table,
        TableSpec::new(&cols, TABLE_ROWS)
            .row_height(24.0)
            .height(96.0),
        |cx, row, column| {
            let text = match column {
                0 => format!("Item {row}"),
                _ => format!("{}", row * 10),
            };
            label(cx, text)
        },
    )
    .a11y_label("Items");
    column(vec![row(vec![volume, level]).gap(12.0), bar, grid]).gap(8.0)
}

/// What the services' controls show.
#[derive(Default)]
struct Services {
    ticks: u32,
    timer: &'static str,
    worker: &'static str,
    clipboard: String,
    opened: String,
}

/// The services' controls: buttons for timers, the clipboard and opening a
/// URL, and labels saying what they did.
fn services_view(sv: &Services, state: &State, offset: f64, cx: &mut Cx<State>) -> Element {
    let set = button(cx, "Set clip").on_tap(cx, |s: &mut State| {
        let ok = stipple::platform::clipboard::set_text(COPIED);
        log(&format!("touchdemo: services clipboard set {ok}"));
        if let Some(sv) = s.services.as_mut() {
            sv.clipboard = format!("Clipboard: set {ok}");
        }
    });
    let get = button(cx, "Get clip").on_tap(cx, |s: &mut State| {
        let text = stipple::platform::clipboard::text();
        log(&format!("touchdemo: services clipboard got {text:?}"));
        if let Some(sv) = s.services.as_mut() {
            sv.clipboard = format!("Clipboard: got {text:?}");
        }
    });
    let handle = cx.handle();
    let timer = button(cx, "Timer").on_tap(cx, move |s: &mut State| {
        log("touchdemo: services timer armed");
        if let Some(sv) = s.services.as_mut() {
            sv.timer = "waiting";
            sv.worker = "working";
        }
        handle.after(Duration::from_millis(1500), |s: &mut State| {
            log("touchdemo: services timer fired");
            if let Some(sv) = s.services.as_mut() {
                sv.timer = "fired";
            }
        });
        // Work off the UI thread, which wakes the loop when it's done.
        let handle = handle.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(800));
            handle.update(|s: &mut State| {
                log("touchdemo: services worker done");
                if let Some(sv) = s.services.as_mut() {
                    sv.worker = "done";
                }
            });
        });
    });
    let handle = cx.handle();
    let open = button(cx, "Open URL").on_tap(cx, move |s: &mut State| {
        let opened = handle.open_url(URL);
        log(&format!("touchdemo: services opened {URL} {opened:?}"));
        if let Some(sv) = s.services.as_mut() {
            sv.opened = format!("Open: {opened:?}");
        }
    });
    column(vec![
        row(vec![set, get]).gap(8.0),
        row(vec![timer, open]).gap(8.0),
        row(vec![
            label(cx, format!("Ticks: {}", sv.ticks)),
            label(cx, format!("Timer: {}", sv.timer)),
            label(cx, format!("Worker: {}", sv.worker)),
        ])
        .gap(12.0),
        row(vec![
            label(cx, format!("List offset: {offset:.0}")),
            label(cx, format!("Pinned: {}", state.pinned.len())),
            label(cx, format!("Drops: {}", state.dropped.len())),
            label(cx, sv.opened.clone()),
        ])
        .gap(12.0),
        label(cx, sv.clipboard.clone()),
    ])
    .gap(8.0)
}

/// The list's scroll offset in the frame last built (`f64` bits).
static OFFSET: AtomicU64 = AtomicU64::new(0);
/// The strip's scroll offset in the frame last built (`f64` bits).
static STRIP_OFFSET: AtomicU64 = AtomicU64::new(0);
/// The keyboard inset in the frame last built (`f64` bits).
static KEYBOARD: AtomicU64 = AtomicU64::new(0);
/// The "Level" input's value last logged.
static LEVEL: AtomicU64 = AtomicU64::new(0);
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
    let preedit = format!("{:?}", state.field.buffer().preedit());
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
    let field = text_field(cx, &state.field, |s| &mut s.field).height(FIELD_H);
    let field = match &state.code {
        Some(code) => {
            let code = text_field(cx, code, |s| s.code.get_or_insert_with(TextInput::new))
                .placeholder("Barcode")
                .input_kind(InputKind::Number)
                .enter_action(EnterAction::Done)
                .width(110.0)
                .height(FIELD_H)
                .on_submit(cx, |s| {
                    let code = s.code.as_ref().map(|c| c.text().to_string());
                    log(&format!(
                        "touchdemo: code submitted {}",
                        code.unwrap_or_default()
                    ));
                    s.code_submitted = true;
                });
            row(vec![field.enter_action(EnterAction::Next).grow(1.0), code]).gap(8.0)
        }
        None => field,
    };
    // The on-screen keyboard's height, as it changes (zero with the
    // simulator's hardware keyboard connected).
    let keyboard = cx.keyboard_inset();
    if KEYBOARD.swap(keyboard.to_bits(), Ordering::Relaxed) != keyboard.to_bits() {
        log(&format!("touchdemo: keyboard inset {keyboard:.0}"));
    }
    // On Linux and macOS the field has a caption, which a screen reader reads
    // with it (AT-SPI's labelled-by relation, checked by the AT-SPI walk in
    // CI; the field's AXTitleUIElement, checked by `axprobe --links`). The
    // mobile self-tests tap the field where it is without one.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let field = row(vec![
        label(cx, "Note").a11y_id("note-label"),
        field.labelled_by("note-label").grow(1.0),
    ])
    .gap(8.0);
    let rows = (0..ROWS)
        .map(|i| {
            button(cx, format!("Row {i}"))
                .height(48.0)
                .on_tap(cx, move |s| {
                    s.tapped.push(i);
                    log(&format!("touchdemo: row {i} tapped"));
                    // A toast the screen readers announce (desktops; the
                    // mobile self-tests tap rows near the bottom edge).
                    if DESKTOP && i == 2 {
                        let message = format!("Row {i} tapped");
                        log(&format!("touchdemo: toast {message:?}"));
                        s.toasts.push(message, ToastKind::Info);
                    }
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
    let list_h = if state.services.is_some() || state.ranges.is_some() {
        LIST_H_SERVICES
    } else {
        LIST_H
    };
    let list = Element::stack(Axis::Vertical, vec![column(rows).gap(4.0)])
        .height(list_h)
        .scrollable(id);
    let services = state
        .services
        .as_ref()
        .map(|sv| services_view(sv, state, offset, cx));
    // A sideways strip of chips, scrolled by a sideways drag.
    let chips = (0..12)
        .map(|i| button(cx, format!("Chip {i}")).width(90.0))
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
    let pinch = label(cx, format!("Pinch me: {:.2}x", state.zoom))
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
        })
        // Text and files dragged from another app land here.
        .on_drop(cx, |s: &mut State, data, at| {
            log(&format!(
                "touchdemo: dropped {data:?} at {:.0},{:.0}",
                at.x, at.y
            ));
            s.dropped.push(data.clone());
        });
    // On desktops, a checkbox beside the box (UIA's Toggle pattern, the
    // checked value macOS and AT-SPI report), and the toasts.
    let pinch = if DESKTOP {
        toasts(cx, &state.toasts, |s| &mut s.toasts);
        let remember = checkbox(cx, "Remember", state.remember, |s| {
            s.remember = !s.remember;
            log(&format!(
                "touchdemo: remember {}",
                if s.remember { "on" } else { "off" }
            ));
        });
        row(vec![pinch.grow(1.0), remember]).gap(8.0)
    } else {
        pinch
    };
    // Everything but the strip keeps off the screen's edges; the strip runs
    // edge to edge, where the back swipe starts.
    let inset = |e: Element| column(vec![e]).padding(Insets::symmetric(PAD, 0.0));
    let mut parts = vec![inset(field)];
    parts.extend(services.map(inset));
    if let Some(r) = &state.ranges {
        parts.push(inset(ranges_view(r, cx)));
    }
    // Long-pressed, these lift a text and a file out of the app (iOS's
    // UIDragInteraction, Android's system drag), into another app or onto
    // the pinch box.
    let lift_text = label(cx, format!("Rows tapped: {}", state.tapped.len()))
        .drag_source(cx, |_| Some(DropData::Text(LIFTED_TEXT.into())));
    let lift_file = label(cx, "Drag file").drag_source(cx, |_| {
        let path = std::env::temp_dir().join("touchdemo-lifted.txt");
        std::fs::write(&path, LIFTED_FILE).ok()?;
        Some(DropData::Files(vec![path]))
    });
    parts.extend([
        inset(row(vec![lift_text, lift_file]).gap(24.0)),
        inset(list),
        strip,
        inset(pinch),
    ]);
    column(parts)
        .gap(12.0)
        .padding(Insets::symmetric(0.0, PAD))
        .grow(1.0)
}

fn main() {
    let services = std::env::var_os("TOUCHDEMO_SERVICES").is_some();
    let ranges = std::env::var_os("TOUCHDEMO_RANGES").is_some();
    let state = State {
        zoom: 1.0,
        services: services.then(|| Services {
            timer: "idle",
            worker: "idle",
            clipboard: "Clipboard: untouched".into(),
            opened: "Open: not yet".into(),
            ..Services::default()
        }),
        ranges: ranges.then(|| Ranges {
            volume: 0.5,
            level: NumberInput::new(3.0).range(0.0, 10.0),
            table: TableState::new(SelectionMode::Single),
        }),
        ..State::default()
    };
    let app = App::new(state, view)
        .title("Stipple Touch")
        .logical_size(Size::new(360.0, 720.0))
        // Follows the system's light and dark mode. Its field stays
        // Stipple's own (the OS draws only its frame): the self-tests drive
        // Stipple's text input, its input methods and edit menu, through the
        // view (nativegallery's drives a real OS field). With the ranges
        // shown, the light theme: its slider and table are Stipple's own
        // (under the native theme a live OS slider would speak for itself),
        // whose accessibility the checks read.
        .theme(if ranges {
            Theme::light()
        } else {
            Theme::native().native_text_editing(false)
        })
        // URLs of the stipple-demo scheme that open the app (CI's iOS job
        // adds it to the bundle's CFBundleURLTypes and opens two).
        .on_open_url(|_, url| log(&format!("touchdemo: opened url {url:?}")))
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
                _ => {}
            }
            s.back_gestures.push(gesture);
        })
        // The field's text comes back after iOS ends the app in the
        // background.
        .save_state(|s| s.field.text().as_bytes().to_vec())
        .restore_state(|s, bytes| {
            let text = String::from_utf8_lossy(bytes).into_owned();
            log(&format!("touchdemo: restored text {text:?}"));
            s.field = TextInput::from_text(text);
        });
    if services {
        log("touchdemo: services shown");
        // The visible text a timer changes: one tick a second.
        app.handle().every(Duration::from_secs(1), |s: &mut State| {
            if let Some(sv) = s.services.as_mut() {
                sv.ticks += 1;
            }
        });
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
        } else if let Ok(text) = std::env::var("TOUCHDEMO_TYPE") {
            selftest::type_text(&app.handle(), text);
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
    /// The list's offset before the trackpad scrolled it (`f64` bits).
    static LIST_BEFORE: AtomicU64 = AtomicU64::new(0);
    /// The field's text before the dead keys typed.
    static BEFORE_TEXT: Mutex<String> = Mutex::new(String::new());
    /// What the folder picker's callback saw, once it ran.
    static FOLDER: Mutex<Option<String>> = Mutex::new(None);
    /// What the save picker's callback got, once it ran.
    static SAVED: Mutex<Option<bool>> = Mutex::new(None);
    /// The document picker's class.
    const PICKER: &str = "UIDocumentPickerViewController";
    /// The barcode typed into the number field.
    const BARCODE: &str = "4006381333931";

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

    /// Type something to save, then, once in the background, close the
    /// scene's session as the user swiping the app away does: the state goes
    /// with the session.
    pub(super) fn close_scene(handle: &AppHandle<State>) {
        handle.after(Duration::from_millis(1500), |s| {
            s.field = TextInput::from_text("Not for later");
            CLOSE_ON_BACKGROUND.store(true, Ordering::Relaxed);
            log("touchdemo: waiting for background to close the session");
        });
    }

    /// The app went to the background: close the session if asked to. The
    /// backend then keeps none of the state it would save next, as the app
    /// goes, whenever UIKit takes the request.
    pub(super) fn on_background() {
        if CLOSE_ON_BACKGROUND.swap(false, Ordering::Relaxed) {
            let session = simulate::destroy_scene_session(|error| {
                log(&format!("touchdemo: closing the session failed: {error}"));
            });
            log(&format!("touchdemo: closing session {session:?}"));
        }
    }

    /// Type `text` into the field, to be saved when the app goes to the
    /// background (and restored after a system kill).
    pub(super) fn type_text(handle: &AppHandle<State>, text: String) {
        handle.after(Duration::from_millis(1500), move |s| {
            s.field = TextInput::from_text(text);
            log("touchdemo: typed, waiting for background");
        });
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

    /// The list's offset in the frame last built.
    fn list_offset() -> f64 {
        f64::from_bits(OFFSET.load(Ordering::Relaxed))
    }

    /// Mobile input, reported on its own (`touchdemo: mobile input passed`):
    ///
    /// - a dead key on a hardware keyboard, as UIKit's text system types it:
    ///   the accent as marked text, then the composed letter committed in
    ///   its place (`insertText:`), or marked and then `unmarkText`;
    /// - drops from another app: the view's `UIDropInteraction` delegate
    ///   driven with a stand-in session over real `NSItemProvider`s, a text
    ///   and a file (copied into the app's tmp directory), onto the pinch
    ///   box;
    /// - trackpad scrolling: the scroll recognizer is on the view, taking
    ///   continuous and discrete scrolling and no touches, and its steps
    ///   (through the same code as its action) scroll the list; a hover
    ///   over it moves the pointer.
    fn mobile_input(steps: &mut Steps) {
        steps.run(500, |_| tap(120.0, PAD + FIELD_H / 2.0));
        steps.until(|_| holds_keyboard());
        steps.run(300, |s| {
            *BEFORE_TEXT.lock().unwrap() = s.field.text().to_string();
            simulate::set_marked_text("´", (1, 1));
        });
        steps.until(|s| s.field.buffer().preedit() == Some("´"));
        steps.run(0, |s| {
            log(&format!(
                "touchdemo: dead key marked {}",
                simulate::text_input_status()
            ));
            check(
                s.field.buffer().preedit() == Some("´"),
                "the dead key's accent isn't the preedit",
            );
            simulate::insert_text("é");
        });
        steps.until(|s| s.field.text().ends_with('é'));
        steps.run(0, |s| {
            let before = BEFORE_TEXT.lock().unwrap().clone();
            check(
                s.field.text() == format!("{before}é") && s.field.buffer().preedit().is_none(),
                format!("the dead key typed {:?} after {before:?}", s.field.text()),
            );
            // The other way UIKit finishes one: the composed letter marked,
            // then unmarked.
            simulate::set_marked_text("¨", (1, 1));
            simulate::set_marked_text("ü", (1, 1));
            simulate::unmark_text();
        });
        steps.until(|s| s.field.text().ends_with("éü"));
        steps.run(0, |s| {
            let before = BEFORE_TEXT.lock().unwrap().clone();
            log(&format!("touchdemo: dead keys typed {:?}", s.field.text()));
            check(
                s.field.text() == format!("{before}éü") && s.field.buffer().preedit().is_none(),
                format!("dead keys typed {:?}", s.field.text()),
            );
            // Back to the text before (which the saved state's check reads
            // later): any composition dropped, then what was typed deleted.
            if s.field.buffer().preedit().is_some() {
                simulate::set_marked_text("", (0, 0));
            }
            let typed = s
                .field
                .text()
                .strip_prefix(before.as_str())
                .map_or(0, |t| t.chars().count());
            for _ in 0..typed {
                simulate::delete_backward();
            }
        });
        steps.until(|s| *s.field.text() == *BEFORE_TEXT.lock().unwrap());
        steps.run(300, |_| tap(300.0, PAD + FIELD_H + 12.0 + 8.0));
        steps.until(|_| !holds_keyboard());

        // A text dropped on the pinch box.
        steps.run(300, |_| {
            let to = element_center("Pinch me").unwrap_or(Point::new(180.0, 640.0));
            let from = Point::new(to.x - 60.0, to.y - 40.0);
            simulate::drop_item(Some("Dropped from another app"), None, from, to, |copy| {
                log(&format!("touchdemo: text drag proposed copy={copy}"));
                check(copy, "the pinch box didn't take the dragged text");
            });
        });
        steps.until(|s| !s.dropped.is_empty());
        steps.run(0, |s| {
            check(
                s.dropped
                    == [stipple::platform::DropData::Text(
                        "Dropped from another app".into(),
                    )],
                format!("text drop gave {:?}", s.dropped),
            );
            // A file: the provider hands over a file, which the app gets a
            // copy of.
            let path = std::env::temp_dir().join("touchdemo-dragged.txt");
            check(
                std::fs::write(&path, b"Dragged into touchdemo\n").is_ok(),
                "can't write the file to drag",
            );
            let to = element_center("Pinch me").unwrap_or(Point::new(180.0, 640.0));
            simulate::drop_item(
                None,
                Some(&path.to_string_lossy()),
                Point::new(to.x, to.y - 50.0),
                to,
                |copy| {
                    log(&format!("touchdemo: file drag proposed copy={copy}"));
                    check(copy, "the pinch box didn't take the dragged file");
                },
            );
        });
        steps.until(|s| s.dropped.len() >= 2);
        steps.run(0, |s| {
            let original = std::env::temp_dir().join("touchdemo-dragged.txt");
            match s.dropped.get(1) {
                Some(stipple::platform::DropData::Files(files)) => {
                    let copy = files.first().cloned().unwrap_or_default();
                    log(&format!("touchdemo: file dropped as {}", copy.display()));
                    check(
                        files.len() == 1
                            && copy != original
                            && copy.file_name() == original.file_name()
                            && std::fs::read(&copy).ok().as_deref()
                                == Some(b"Dragged into touchdemo\n".as_slice()),
                        format!("file drop gave {files:?}"),
                    );
                }
                other => check(false, format!("file drop gave {other:?}")),
            }
        });

        // Trackpad scrolling over the list, and a hover.
        steps.run(300, |_| {
            let status = simulate::pointer_status();
            log(&format!("touchdemo: pointer {status}"));
            check(
                status.contains("scroll_pan=true scroll_types=3 touch_types=0 hover=true"),
                format!("pointer recognizers {status}"),
            );
            let at = visible_row().unwrap_or(Point::new(180.0, 300.0));
            simulate::hover(Some(at));
            LIST_BEFORE.store(list_offset().to_bits(), Ordering::Relaxed);
            // Towards the top if there's room, else towards the end: the
            // fingers move down 150 points (or up).
            let sign = if list_offset() > 200.0 { 1.0 } else { -1.0 };
            let moves: Vec<Point> = (0..=5)
                .map(|i| Point::new(0.0, sign * 30.0 * f64::from(i)))
                .collect();
            simulate::trackpad_scroll(at, &moves);
            simulate::hover(None);
        });
        steps.until(|_| {
            (list_offset() - f64::from_bits(LIST_BEFORE.load(Ordering::Relaxed))).abs() >= 100.0
        });
        steps.run(0, |_| {
            let (before, after) = (
                f64::from_bits(LIST_BEFORE.load(Ordering::Relaxed)),
                list_offset(),
            );
            log(&format!(
                "touchdemo: trackpad scrolled the list {before:.0} -> {after:.0}"
            ));
            check(
                ((after - before).abs() - 150.0).abs() < 10.0,
                format!("trackpad scroll moved the list {before:.0} -> {after:.0}"),
            );
            report("mobile input");
        });
    }

    /// What the last lift said (its items, and the drop's proposal).
    static LIFTED: Mutex<String> = Mutex::new(String::new());
    /// Drops before the lift.
    static DROPS_BEFORE: AtomicUsize = AtomicUsize::new(0);

    /// Dragging out and the trackpad's depth, reported on its own
    /// (`touchdemo: drag out and trackpad passed`):
    ///
    /// - lifting: the view's `UIDragInteraction` delegate asked for the items
    ///   at "Rows tapped" (a drag source of a text) and at "Drag file" (of a
    ///   file) with a stand-in session, as UIKit asks after its long press;
    ///   the items' providers (an `NSString`; a file representation of
    ///   `public.plain-text` named after the file), and the lift's preview
    ///   (the App's picture of the element), then those very items dropped
    ///   on the pinch box through the drop interaction, which reads them as
    ///   another app would: the text, and a copy of the file;
    /// - a secondary click on a row (a trackpad's two-finger click, as the
    ///   view maps one) opens its context menu;
    /// - the pointer over the text field takes the I-beam (the pointer
    ///   interaction's style for the app's cursor, asked as UIKit asks);
    /// - a trackpad scroll whose fingers lift while moving coasts on after
    ///   them, slowing down.
    fn drag_out_and_trackpad(steps: &mut Steps) {
        steps.run(300, |s| {
            DROPS_BEFORE.store(s.dropped.len(), Ordering::Relaxed);
            let at = element_center("Rows tapped").unwrap_or(Point::new(60.0, 105.0));
            let to = element_center("Pinch me").unwrap_or(Point::new(180.0, 640.0));
            simulate::lift_and_drop(at, to, |said| {
                log(&format!("touchdemo: lift text {said}"));
                *LIFTED.lock().unwrap() = said;
            });
        });
        steps.until(|s| s.dropped.len() > DROPS_BEFORE.load(Ordering::Relaxed));
        steps.run(0, |s| {
            let said = LIFTED.lock().unwrap().clone();
            check(
                said.contains("drag_interaction=true enabled=true"),
                format!("drag interaction: {said}"),
            );
            check(
                said.contains("lifted 1 item(s)")
                    && said.contains("preview=true")
                    && said.contains("drop copy=true"),
                format!("text lift: {said}"),
            );
            check(
                s.dropped.last() == Some(&DropData::Text(LIFTED_TEXT.into())),
                format!("the lifted text dropped as {:?}", s.dropped.last()),
            );
            DROPS_BEFORE.store(s.dropped.len(), Ordering::Relaxed);
            let at = element_center("Drag file").unwrap_or(Point::new(200.0, 105.0));
            let to = element_center("Pinch me").unwrap_or(Point::new(180.0, 640.0));
            simulate::lift_and_drop(at, to, |said| {
                log(&format!("touchdemo: lift file {said}"));
                *LIFTED.lock().unwrap() = said;
            });
        });
        steps.until(|s| s.dropped.len() > DROPS_BEFORE.load(Ordering::Relaxed));
        steps.run(0, |s| {
            let said = LIFTED.lock().unwrap().clone();
            check(
                said.contains("lifted 1 item(s)")
                    && said.contains("public.plain-text")
                    && said.contains("name=Some(\"touchdemo-lifted.txt\")")
                    && said.contains("drop copy=true"),
                format!("file lift: {said}"),
            );
            let original = std::env::temp_dir().join("touchdemo-lifted.txt");
            match s.dropped.last() {
                Some(DropData::Files(files)) => {
                    let copy = files.first().cloned().unwrap_or_default();
                    log(&format!(
                        "touchdemo: lifted file dropped as {}",
                        copy.display()
                    ));
                    check(
                        files.len() == 1
                            && copy != original
                            && copy.file_name() == original.file_name()
                            && std::fs::read_to_string(&copy).ok().as_deref() == Some(LIFTED_FILE),
                        format!("the lifted file dropped as {files:?}"),
                    );
                }
                other => check(false, format!("the lifted file dropped as {other:?}")),
            }
            // A secondary click on a row opens its context menu.
            let at = visible_row().unwrap_or(Point::new(120.0, 300.0));
            log(&format!("touchdemo: secondary click at {at:?}"));
            DROPS_BEFORE.store(s.pinned.len(), Ordering::Relaxed);
            simulate::secondary_click(at);
        });
        steps.until(|_| menu_item().is_some());
        steps.run(0, |_| {
            let item = menu_item();
            log(&format!("touchdemo: secondary click menu {item:?}"));
            check(item.is_some(), "the secondary click opened no menu");
            if let Some((i, _)) = item {
                simulate::activate_accessibility_element(i);
            }
        });
        steps.until(|s| s.pinned.len() > DROPS_BEFORE.load(Ordering::Relaxed));
        // The pointer over the text field: the I-beam.
        steps.run(300, |_| {
            simulate::hover(Some(Point::new(120.0, PAD + FIELD_H / 2.0)))
        });
        steps.until(|_| simulate::pointer_style().contains("style=beam"));
        steps.run(0, |_| {
            let style = simulate::pointer_style();
            log(&format!("touchdemo: pointer style {style}"));
            check(
                style.contains("pointer_interaction=true cursor=Text style=beam uikit_style=true"),
                format!("pointer style over the field: {style}"),
            );
            simulate::hover(None);
        });
        // A trackpad flick: the fingers move 60 points and lift at 1500
        // points a second; the list coasts on well past the 60.
        steps.run(300, |_| {
            let at = visible_row().unwrap_or(Point::new(180.0, 300.0));
            LIST_BEFORE.store(list_offset().to_bits(), Ordering::Relaxed);
            let sign = if list_offset() > 600.0 { 1.0 } else { -1.0 };
            let moves: Vec<Point> = (0..=3)
                .map(|i| Point::new(0.0, sign * 20.0 * f64::from(i)))
                .collect();
            simulate::trackpad_fling(at, &moves, Point::new(0.0, sign * 1500.0));
        });
        steps.run(100, |_| {
            log(&format!(
                "touchdemo: pointer after the flick {}",
                simulate::pointer_status()
            ));
        });
        steps.until(|_| {
            (list_offset() - f64::from_bits(LIST_BEFORE.load(Ordering::Relaxed))).abs() >= 200.0
                && !simulate::pointer_status().contains("coasting=true")
        });
        steps.run(0, |_| {
            let (before, after) = (
                f64::from_bits(LIST_BEFORE.load(Ordering::Relaxed)),
                list_offset(),
            );
            log(&format!(
                "touchdemo: trackpad flick coasted the list {before:.0} -> {after:.0}"
            ));
            check(
                (after - before).abs() >= 200.0,
                format!("the flick moved the list {before:.0} -> {after:.0}"),
            );
            report("drag out and trackpad");
        });
    }

    fn document_is(text: &'static str) -> impl Fn(&State) -> bool + Send {
        move |_| simulate::text_input_status().contains(&format!("doc={text:?} "))
    }

    /// What the edit menu's part found wrong (kept apart from the rest, so
    /// it reports on its own line: it is new, and hasn't run in CI yet).
    static EDIT_FAILURES: Mutex<Vec<String>> = Mutex::new(Vec::new());

    fn edit_check(ok: bool, what: impl Into<String>) {
        if !ok {
            let what = what.into();
            log(&format!("touchdemo: edit menu check failed: {what}"));
            EDIT_FAILURES.lock().unwrap().push(what);
        }
    }

    /// Editing with a finger and the system's edit menu
    /// (`UIEditMenuInteraction`), on the text field holding "HiJP": a long
    /// press selects the word and asks for the menu, which the view backs
    /// with `canPerformAction:withSender:`; Copy and Cut picked through the
    /// view's `copy:` / `cut:` reach the field and the pasteboard; a tap on
    /// the emptied field's caret brings the menu back with Paste only, and
    /// `paste:` puts the word back. Reports "touchdemo: edit menu passed" (or FAILED) on its
    /// own.
    fn edit_menu_steps(steps: &mut Steps) {
        use stipple::platform::EditAction;
        let at = Point::new(40.0, PAD + FIELD_H / 2.0);
        steps.run(500, move |_| {
            simulate::touch(7, TouchPhase::Started, at);
        });
        steps.run(900, move |_| simulate::touch(7, TouchPhase::Ended, at));
        steps.until(|s| s.field.buffer().selection().is_some());
        steps.run(300, |s| {
            let status = simulate::edit_menu_status();
            log(&format!("touchdemo: edit menu after long press {status}"));
            edit_check(
                s.field.buffer().selection() == Some((0, 4)),
                format!("long press selected {:?}", s.field.buffer().selection()),
            );
            edit_check(
                status.contains("interaction=true shown=true cut=true copy=true paste=true"),
                format!("menu status {status}"),
            );
            simulate::edit_menu_action(EditAction::Copy);
        });
        steps.until(|_| stipple::platform::clipboard::text().as_deref() == Some("HiJP"));
        steps.run(0, move |s| {
            let pasteboard = stipple::platform::clipboard::text();
            log(&format!("touchdemo: edit menu copied {pasteboard:?}"));
            edit_check(pasteboard.as_deref() == Some("HiJP"), "Copy");
            edit_check(s.field.text() == "HiJP", "Copy changed the text");
            // Select it again, and cut.
            simulate::touch(7, TouchPhase::Started, at);
        });
        steps.run(900, move |_| simulate::touch(7, TouchPhase::Ended, at));
        steps.until(|s| s.field.buffer().selection().is_some());
        steps.run(300, |_| simulate::edit_menu_action(EditAction::Cut));
        steps.until(|s| s.field.text().is_empty());
        steps.run(300, |s| {
            log(&format!(
                "touchdemo: edit menu cut, text {:?}",
                s.field.text()
            ));
            edit_check(s.field.text().is_empty(), "Cut");
            // A tap on the caret (the field is empty and keeps focus, so
            // anywhere in it is on the caret): the menu, with Paste. (A
            // second tap would hide it again.)
            tap(30.0, PAD + FIELD_H / 2.0);
        });
        steps.run(700, |_| {
            let status = simulate::edit_menu_status();
            log(&format!("touchdemo: edit menu on the caret {status}"));
            edit_check(
                status.contains("shown=true cut=false copy=false paste=true"),
                format!("caret menu {status}"),
            );
            simulate::edit_menu_action(EditAction::Paste);
        });
        steps.until(|s| s.field.text() == "HiJP");
        steps.run(0, |s| {
            log(&format!("touchdemo: edit menu pasted {:?}", s.field.text()));
            edit_check(s.field.text() == "HiJP", "Paste");
            let failures = std::mem::take(&mut *EDIT_FAILURES.lock().unwrap());
            if failures.is_empty() {
                log("touchdemo: edit menu passed");
            } else {
                log(&format!(
                    "touchdemo: edit menu FAILED: {}",
                    failures.join("; ")
                ));
            }
        });
    }

    /// `platform::secure_store` (the Keychain on iOS): a round trip of every
    /// byte value, set, read back, removed.
    fn log_secure_store() {
        let store = stipple::platform::secure_store();
        let value: Vec<u8> = (0..=255u8).collect();
        let set = store.set("touchdemo-secret", &value);
        let back = store
            .get("touchdemo-secret")
            .map(|v| v.as_deref() == Some(&value[..]));
        let removed = store
            .remove("touchdemo-secret")
            .and_then(|()| store.get("touchdemo-secret"))
            .map(|v| v.is_none());
        log(&format!(
            "touchdemo: secure store set={set:?} same={back:?} removed={removed:?}"
        ));
    }

    /// The bundle's packaged files (`Stipple.app/assets/`, which CI copies
    /// from `examples/touchdemo/assets`) through `platform::assets`: where
    /// they are, a whole file with its size and FNV-1a hash, the listings,
    /// an SVG decoded, a missing file, and one loaded off the UI thread
    /// into the state (`AppHandle::load_asset`).
    fn log_assets(handle: &AppHandle<State>) {
        let files = stipple::platform::assets();
        let dir = files.dir();
        log(&format!(
            "touchdemo: assets dir={}",
            dir.map_or("-".into(), |d| d.display().to_string())
        ));
        match files.read("hello.txt") {
            Ok(bytes) => log(&format!(
                "touchdemo: asset hello.txt bytes={} fnv1a={:08x} text={:?}",
                bytes.len(),
                bytes
                    .iter()
                    .fold(0x811c_9dc5_u32, |h, &b| (h ^ u32::from(b))
                        .wrapping_mul(0x0100_0193)),
                String::from_utf8_lossy(&bytes).trim_end()
            )),
            Err(e) => log(&format!("touchdemo: asset hello.txt failed: {e}")),
        }
        log(&format!(
            "touchdemo: assets list={:?} img={:?} exists={} missing={:?}",
            files.list("").unwrap_or_default(),
            files.list("img").unwrap_or_default(),
            files.exists("img/dot.svg"),
            files.read("nope.txt").err().map(|e| e.to_string()),
        ));
        // The asset as a file range: on iOS its own file in the bundle.
        let fd = files.open_fd("hello.txt").map(|fd| {
            let path = fd.path().is_some_and(|p| p.ends_with("assets/hello.txt"));
            (
                fd.offset(),
                fd.len(),
                path,
                fd.read_all().ok() == files.read("hello.txt").ok(),
            )
        });
        log(&format!(
            "touchdemo: asset fd hello.txt={:?}",
            fd.map_err(|e| e.to_string())
        ));
        handle.load_asset("img/dot.svg", Svg::parse, |_, r| {
            let size = r.map(|s| format!("{}x{}", s.size().width, s.size().height));
            log(&format!("touchdemo: loaded img/dot.svg {size:?}"));
        });
    }

    pub fn schedule(handle: &AppHandle<State>) {
        let _ = std::fs::remove_file(std::env::temp_dir().join("touchdemo.log"));
        log("touchdemo: selftest starting");
        // The bundle's Info.plist, and the app's container directories.
        let info = stipple::platform::app_info();
        log(&format!(
            "touchdemo: app id={} name={:?} version={} build={}",
            info.id,
            info.name,
            info.version.as_deref().unwrap_or("-"),
            info.build.as_deref().unwrap_or("-"),
        ));
        match stipple::platform::dirs() {
            Ok(d) => log(&format!(
                "touchdemo: dirs data={} cache={} temp={} exist={}",
                d.data.display(),
                d.cache.display(),
                d.temp.display(),
                [&d.data, &d.cache, &d.temp].iter().all(|p| p.is_dir()),
            )),
            Err(e) => log(&format!("touchdemo: dirs unavailable: {e}")),
        }
        log_secure_store();
        log_assets(handle);
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
        steps.until(|s| s.field.buffer().preedit() == Some("にほん"));
        steps.run(0, |s| {
            let status = simulate::text_input_status();
            log(&format!("touchdemo: composing {status}"));
            check(
                s.field.buffer().preedit() == Some("にほん") && s.field.text() == "Hi",
                format!(
                    "preedit {:?} over {:?}",
                    s.field.buffer().preedit(),
                    s.field.text()
                ),
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
                s.field.text() == "Hi日本" && s.field.buffer().preedit().is_none(),
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
            check(
                simulate::text_input_status().contains("keyboard_type=0 return_key=0"),
                "a plain text field's traits",
            );
            // A barcode field appears beside it, and the field's Return key
            // becomes Next.
            s.code = Some(TextInput::new());
        });
        // Return (a newline from the keyboard) moves on to the barcode
        // field, whose traits are a number pad with a Done key.
        steps.run(500, |_| simulate::insert_text("\n"));
        steps.until(|_| simulate::text_input_status().contains("keyboard_type=4 "));
        steps.run(0, |s| {
            let status = simulate::text_input_status();
            log(&format!("touchdemo: number field {status}"));
            check(
                status
                    .contains("keyboard_type=4 return_key=9 autocapitalization=0 autocorrection=1"),
                format!("number field traits {status}"),
            );
            check(s.field.text() == "HiJP", "Next added no text to the field");
            simulate::insert_text(BARCODE);
        });
        steps.until(|s| s.code.as_ref().is_some_and(|c| c.text() == BARCODE));
        // Done submits it.
        steps.run(0, |_| simulate::insert_text("\n"));
        steps.until(|s| s.code_submitted);
        steps.run(0, |s| {
            check(s.code_submitted, "Done didn't submit the barcode field")
        });
        edit_menu_steps(&mut steps);

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
            dialog::open_file_contents(&FileDialog::new().accept(".txt,image/*"), |file| {
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
                &FileDialog::new().file_name("touchdemo.txt"),
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
            dialog::open_file_contents(&FileDialog::new().accept(".txt"), |file| {
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
                &FileDialog::new().file_name("touchdemo.txt"),
                b"Saved by touchdemo\n",
                |saved| {
                    log(&format!("touchdemo: save picker exported {saved}"));
                    *SAVED.lock().unwrap() = Some(saved);
                },
            );
        });
        steps.until(|_| simulate::presented_controller() == PICKER);
        steps.run(600, |_| {
            log(&format!(
                "touchdemo: save picker to pick from: {}",
                simulate::presented_controller()
            ));
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
        // The folder picker (not part of the checks yet: its outcome is
        // logged): answered as picking a directory does, the app lists, reads
        // and writes it through the Folder it gets.
        let folder_path = std::env::temp_dir().join("touchdemo-folder");
        let path = folder_path.clone();
        steps.run(0, move |_| {
            let _ = std::fs::remove_dir_all(&path);
            let _ = std::fs::create_dir_all(&path);
            let _ = std::fs::write(path.join("a.txt"), b"in the folder\n");
            dialog::pick_folder_then(&FileDialog::new(), |folder| {
                let line = match folder {
                    Some(f) => {
                        let wrote = f.write("b.txt", b"written\n").map_err(|e| e.to_string());
                        let names: Vec<String> = f
                            .entries()
                            .unwrap_or_default()
                            .into_iter()
                            .map(|e| e.name)
                            .collect();
                        let read = f
                            .read("a.txt")
                            .map(|b| String::from_utf8_lossy(&b).trim_end().to_owned())
                            .map_err(|e| e.to_string());
                        format!(
                            "name={:?} entries={names:?} read={read:?} wrote={wrote:?}",
                            f.name()
                        )
                    }
                    None => "None".into(),
                };
                log(&format!("touchdemo: folder picked {line}"));
                *FOLDER.lock().unwrap() = Some(line);
            });
        });
        steps.until(|_| simulate::presented_controller() == PICKER);
        let path = folder_path.clone();
        steps.run(600, move |_| {
            log(&format!(
                "touchdemo: folder picker presented {}",
                simulate::presented_controller()
            ));
            if !simulate::pick_document(&path.to_string_lossy()) {
                log("touchdemo: no folder picker to pick from");
            }
        });
        steps.until(|_| FOLDER.lock().unwrap().is_some());
        mobile_input(&mut steps);
        drag_out_and_trackpad(&mut steps);

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
            // A scheduled one: the center takes the request (or refuses it
            // while permission is undecided; either way its completion
            // handler logs), and the pending list answers.
            stipple::platform::haptic(stipple::platform::Haptic::Success);
            let scheduled = stipple::platform::schedule_notification(
                &stipple::platform::ScheduledNotification::new(
                    "touchdemo-later",
                    Notification::new("Stipple later").body("Scheduled by touchdemo"),
                )
                .after(Duration::from_secs(3600)),
            );
            log(&format!("touchdemo: scheduled {scheduled:?}"));
            check(
                scheduled.is_ok(),
                format!("schedule_notification: {scheduled:?}"),
            );
            stipple::platform::pending_notifications(|ids| {
                log(&format!("touchdemo: pending {ids:?}"));
            });
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

// In the browser, `web.rs` builds this file as a library and the page
// runs `main`'s app (on the web, `App::run` hands it to the page).
stipple::web_app!(main);
