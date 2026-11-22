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
//! - `androiddemo: cjk 日本語 glyphs=true ink=N` at start: the default
//!   font draws Japanese, from the system's CJK font;
//! - `androiddemo: tapped N` when the button in the middle of the screen is
//!   tapped (`adb shell input tap`);
//! - `androiddemo: text "..."` as the field takes keys (`adb shell input text`,
//!   after Tab moves focus to it), and the input method's edits once it reads
//!   "Stipple", when the demo runs the backend's `InputConnection` self-test
//!   (composing, committing an emoji, deleting around the caret);
//! - `androiddemo: back handled` / `back not handled` for the Back button: it
//!   resets the tap count when there is one (handled), and otherwise leaves
//!   the app to the system. With predictive back (the manifest's
//!   `enableOnBackInvokedCallback`, Android 13 and later), the app says
//!   beforehand that it only takes Back while the count isn't 0
//!   (`App::back_enabled`), so the system goes back without asking it when
//!   it is; `androiddemo: back gesture ...` logs the gesture's progress
//!   (Android 14 and later): its start, a quarter of the way, and its
//!   cancellation;
//! - `androiddemo: saved taps=N` when Android asks for the state to keep
//!   (`onSaveInstanceState`, as the app goes to the background), and
//!   `androiddemo: restored taps=N name="..."` when it starts the app again
//!   from it after ending the process (`App::save_state`,
//!   `App::restore_state`);
//! - `androiddemo: lifecycle Background` / `Foreground` / `LowMemory` as the
//!   app goes to the background, comes back, or is asked to free memory;
//! - `androiddemo: scrolled to N` as a finger drag (`adb shell input swipe`)
//!   scrolls the list at the bottom of the screen, and `androiddemo: row N
//!   tapped` if one of its rows is tapped (a swipe must not tap one);
//! - `androiddemo: row N pinned` when the item of a row's context menu is
//!   picked: a long press (`adb shell input swipe x y x y 800`) opens it;
//! - `stipple: lifted drag started: Some(true) (...)` when a long press on
//!   the greeting (a drag source of a text) or the heading (of a file)
//!   lifts it out of the app as a system drag (the file through the app's
//!   FileProvider), and `androiddemo: dropped ...` when one lands on the
//!   app (test-input.sh; test-drag-apps.sh drops them in another app);
//! - `androiddemo: pinch Began|Changed|Ended scale S` as two fingers pinch
//!   the button (`Element::on_pinch`), without tapping it; CI's script
//!   touches it with two fingers on a virtual touch screen it makes with the
//!   shell's `uinput` command, since `adb shell input` has one finger;
//! - `androiddemo: strip scrolled to N` as a sideways swipe scrolls the strip
//!   of chips above the list; its last three chips are *Set clip*, *Get
//!   clip* and *Open link*: `androiddemo: clipboard set true`,
//!   `androiddemo: clipboard got Some("...")` and `androiddemo: opened URL
//!   Ok(())` (test-services.sh);
//! - `androiddemo: notify Ok(())` (or the error) when *Notify* posts a
//!   notification, which asks for `POST_NOTIFICATIONS` first on Android 13;
//! - `androiddemo: opened "name" (N bytes): "text"` / `open cancelled` once
//!   *Open*'s document picker answers, and `androiddemo: saved true|false`
//!   once *Save*'s has written `stipple-saved.txt` (or was cancelled);
//! - `androiddemo: safe area T R B L Portrait|Landscape` as the window's
//!   safe area changes: the app draws edge to edge (`App::edge_to_edge`),
//!   its background under the status and navigation bars and its content
//!   inside the safe area. It also stands upright only
//!   (`App::orientations`) and keeps the screen on
//!   (`AppHandle::set_keep_awake`), which CI reads back from `dumpsys`.
//!
//! - `androiddemo: asset hello.txt bytes=N fnv1a=H text="..."` and
//!   `androiddemo: assets list=... img=... svg=... streamed=... exists=...
//!   missing=...` at start: the APK's packaged files (`assets/`, which
//!   build-apk.sh adds) read through `platform::assets`.
//! - `androiddemo: share Shared|Cancelled|Unavailable` once *Share*'s chooser
//!   answers (a text, a URL and a small file, which goes through `MediaStore`);
//!   `androiddemo: haptic` for *Buzz* (`View.performHapticFeedback`);
//! - `androiddemo: scheduled Ok(())` when *Later* schedules a notification
//!   ten seconds on (`androiddemo-later`), `androiddemo: pending [...]` with
//!   what's still to show, and `androiddemo: notification tapped "..."` when a
//!   tap on it starts the activity. The APK carries the `Alarm` receiver
//!   (`classes.dex`, see `build-apk.sh`), so it shows with the process gone.
//! - `androiddemo: opened url "..."` when a URL opens the app
//!   (`App::on_open_url`): its manifest claims the `stipple-demo` scheme on
//!   Stipple's `Link` activity, so `am start -a android.intent.action.VIEW -d
//!   stipple-demo://callback?code=1` reaches it, at launch and while it runs
//!   (`test-open-url.sh`).
//! - `androiddemo: store products N` or `androiddemo: store error ...` at
//!   start: `platform::store` asking Google Play about two products. Without
//!   the Play Billing Library in the APK the error says so; with it
//!   (`STIPPLE_BILLING=1 build-apk.sh`) the library connects to the Play
//!   Store, and `stipple: billing: setup finished: CODE` logs its answer.
//!
//! Non-Android builds compile to an empty cdylib so `cargo build --examples`
//! works everywhere.

#[cfg(target_os = "android")]
mod app {
    use std::ffi::c_void;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
    use stipple::core::{PinchEvent, PinchPhase};
    use stipple::platform::backend::android::{self, ANativeActivity};
    use stipple::platform::dialog::{self, FileDialog};
    use stipple::platform::{
        BackGesture, Haptic, ProductId, ScheduledNotification, Share, ShareOutcome, haptic,
    };
    use stipple::prelude::*;

    struct State {
        taps: u32,
        ticks: u32,
        name: TextInput,
        code: TextInput,
        /// The "Volume" slider (accessibility checks: its RangeInfo and
        /// ACTION_SET_PROGRESS).
        volume: f64,
    }

    /// The field's text in the frame last built, to log its changes.
    static TEXT: Mutex<String> = Mutex::new(String::new());
    /// The barcode field's text in the frame last built.
    static CODE: Mutex<String> = Mutex::new(String::new());
    /// The barcode CI types, after which the app presses the keyboard's
    /// action key (Done) through the input connection.
    const BARCODE: &str = "4006381333931";
    /// Whether the input connection self-test ran.
    static SELF_TEST: AtomicBool = AtomicBool::new(false);
    /// The frame to run it in.
    static SELF_TEST_AT: AtomicU32 = AtomicU32::new(0);

    /// Whether the back gesture's progress passed a quarter (logged once).
    static QUARTER: AtomicBool = AtomicBool::new(false);
    /// The pinch's scale in quarters from 1, as last logged.
    static PINCH_STEP: AtomicI64 = AtomicI64::new(0);
    /// Frames built, to show the spinner is driven by the display.
    static FRAMES: AtomicU32 = AtomicU32::new(0);
    /// The list's scroll offset in the frame last built (`f64` bits).
    static OFFSET: AtomicU64 = AtomicU64::new(0);
    /// The strip's scroll offset in the frame last built (`f64` bits).
    static STRIP_OFFSET: AtomicU64 = AtomicU64::new(0);
    /// The safe area and orientation last logged.
    static SAFE: Mutex<String> = Mutex::new(String::new());
    /// When the greeting last started a system drag (a press calls its drag
    /// handler on every move).
    static DRAG_STARTED: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    /// The text the greeting drags out of the app.
    const DRAGGED: &str = "Dropped by androiddemo";
    /// What the file the heading drags out of the app holds.
    const LIFTED_FILE: &str = "A file lifted out of androiddemo";
    /// What the file the "Running for" label drags holds.
    const DRAGGED_FILE: &str = "A file dragged by androiddemo";

    /// Whether a press's drag handler should start a system drag: once per
    /// press (the handler runs on every move).
    fn start_drag() -> bool {
        let mut last = DRAG_STARTED.lock().unwrap();
        let start = last.is_none_or(|t| t.elapsed() > Duration::from_secs(2));
        if start {
            *last = Some(std::time::Instant::now());
        }
        start
    }

    fn view(state: &State, cx: &mut Cx<State>) -> Element {
        FRAMES.fetch_add(1, Ordering::Relaxed);
        let theme = *cx.theme();
        let field = text_field(cx, &state.name, |s| &mut s.name).width(170.0);
        // Digits only: a number pad, nothing corrected, and Done.
        let code_field = text_field(cx, &state.code, |s| &mut s.code)
            .placeholder("Barcode")
            .input_kind(InputKind::Number)
            .enter_action(EnterAction::Done)
            .width(110.0)
            .on_submit(cx, |s| {
                println!("androiddemo: code submitted {}", s.code.text())
            });
        let code = state.code.text().to_string();
        let mut last = CODE.lock().unwrap();
        if *last != code {
            println!("androiddemo: code {code:?}");
            if code == BARCODE {
                SELF_TEST_AT.store(FRAMES.load(Ordering::Relaxed) + 3, Ordering::Relaxed);
            }
            *last = code;
        }
        drop(last);
        let name = state.name.text();
        // What the field shows: its text, and a word the keyboard is still
        // composing (typed at the end here).
        let shown = format!(
            "{name}{}",
            state.name.buffer().preedit().unwrap_or_default()
        );
        let mut last = TEXT.lock().unwrap();
        if *last != shown {
            shown.clone_into(&mut last);
            println!("androiddemo: text {shown:?}");
            // Typed through key events: now try the input method's way, a
            // few frames on, once the backend has passed the field's text on.
            if shown == "Stipple" && !SELF_TEST.swap(true, Ordering::Relaxed) {
                SELF_TEST_AT.store(FRAMES.load(Ordering::Relaxed) + 3, Ordering::Relaxed);
            }
        }
        drop(last);
        if FRAMES.load(Ordering::Relaxed) == SELF_TEST_AT.load(Ordering::Relaxed) {
            android::input_connection_self_test();
        }
        // A long press on the heading lifts a file out of the app (a system
        // drag, the file through the app's FileProvider), which other apps
        // (and this one) can drop.
        let title = heading(cx, "Stipple on Android").drag_source(cx, |_: &mut State| {
            let dir = stipple::platform::dirs().ok()?.cache;
            let path = dir.join("androiddemo-lifted.txt");
            std::fs::create_dir_all(&dir).ok()?;
            std::fs::write(&path, LIFTED_FILE).ok()?;
            Some(DropData::Files(vec![path]))
        });
        let top = column(vec![
            title,
            label(cx, "Type your name:"),
            row(vec![field, code_field]).gap(theme.spacing.sm),
            row(vec![
                spinner(cx),
                // A slider a screen reader reads and sets as a SeekBar (before the
                // counter, which grows, so it stays put).
                slider(cx, state.volume, |s: &mut State, v| {
                    s.volume = (v * 100.0).round() / 100.0;
                    println!("androiddemo: volume {:.2}", s.volume);
                })
                .a11y_label("Volume")
                .width(80.0),
                // Pressing it and moving drags a file (a content: URI).
                label(cx, format!("Running for {} s", state.ticks)).on_drag(
                    cx,
                    |_: &mut State, _| {
                        if start_drag() {
                            android::drag_file_self_test(
                                "stipple-dragged.txt",
                                DRAGGED_FILE.as_bytes(),
                                "text/plain",
                            );
                        }
                    },
                ),
            ])
            .gap(theme.spacing.sm),
            platform_row(cx, &theme),
        ])
        .gap(theme.spacing.md);
        // The button takes the space the rest leaves, so it covers the middle
        // of the screen and CI can tap it at the screen's center. Two fingers
        // pinch it (and don't tap it).
        let tap = button(cx, "Tap me")
            .width(280.0)
            .grow(1.0)
            .on_tap(cx, |s| {
                s.taps += 1;
                println!("androiddemo: tapped {}", s.taps);
            })
            .on_pinch(cx, |ev: &PinchEvent, _: &mut State| match ev.phase {
                // Each quarter further from 1 (logged once), and the start
                // and end.
                PinchPhase::Changed => {
                    let step = ((ev.scale - 1.0) * 4.0).round() as i64;
                    if PINCH_STEP.swap(step, Ordering::Relaxed) != step {
                        println!("androiddemo: pinch Changed scale {:.2}", ev.scale);
                    }
                }
                phase => {
                    PINCH_STEP.store(0, Ordering::Relaxed);
                    println!("androiddemo: pinch {phase:?} scale {:.2}", ev.scale);
                }
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
                button(cx, format!("Row {i}"))
                    .width(280.0)
                    .on_tap(cx, move |_| println!("androiddemo: row {i} tapped"))
                    // A long press opens it.
                    .context_menu(
                        cx,
                        Menu::new("").item(MenuItem::new(
                            format!("Pin row {i}"),
                            move |_: &mut State| println!("androiddemo: row {i} pinned"),
                        )),
                    )
            })
            .collect();
        let id = cx.register_scroll();
        let offset = cx.scroll_offset(id);
        if OFFSET.swap(offset.to_bits(), Ordering::Relaxed) != offset.to_bits() {
            println!("androiddemo: scrolled to {offset:.0}");
        }
        // (Short enough to leave the button room with the two rows of
        // platform buttons on CI's small screen.)
        let list = Element::stack(Axis::Vertical, vec![column(rows).gap(theme.spacing.sm)])
            .height(196.0)
            .scrollable(id)
            .align(Align::Start, Align::Center);
        // A strip of chips a sideways swipe scrolls. The last three, out of
        // view until the strip is scrolled, are the system's services: the
        // clipboard and opening a URL (test-services.sh).
        let mut chips: Vec<Element> = (0..9)
            .map(|i| button(cx, format!("Chip {i}")).width(90.0))
            .collect();
        chips.extend(service_chips(cx));
        let strip_id = cx.register_scroll();
        let strip_offset = cx.scroll_offset(strip_id);
        if STRIP_OFFSET.swap(strip_offset.to_bits(), Ordering::Relaxed) != strip_offset.to_bits() {
            println!("androiddemo: strip scrolled to {strip_offset:.0}");
        }
        let strip = Element::stack(Axis::Horizontal, vec![row(chips).gap(theme.spacing.sm)])
            .width(280.0)
            .scrollable(strip_id);
        // Drawn edge to edge: the content keeps inside the safe area.
        let safe = cx.safe_area();
        let turned = cx.window_info().orientation();
        let line = format!(
            "{} {} {} {} {turned:?}",
            safe.top, safe.right, safe.bottom, safe.left
        );
        let mut last = SAFE.lock().unwrap();
        if *last != line {
            println!("androiddemo: safe area {line}");
            *last = line;
        }
        drop(last);
        let pad = theme.spacing.lg;
        // A long press on the greeting lifts its text out of the app (a
        // system drag, `View.startDragAndDrop`), which the whole screen
        // takes as a drop, as other apps can.
        let greeting =
            label(cx, greeting).drag_source(cx, |_| Some(DropData::Text(DRAGGED.into())));
        column(vec![top, tap, greeting, strip, list])
            .on_drop(cx, |_: &mut State, data, at| match data {
                DropData::Text(t) => {
                    println!("androiddemo: dropped text {t:?} at {:.0},{:.0}", at.x, at.y)
                }
                DropData::Files(f) => println!(
                    "androiddemo: dropped files {f:?} holding {:?}",
                    f.first().and_then(|p| std::fs::read_to_string(p).ok())
                ),
                other => println!("androiddemo: dropped {other:?}"),
            })
            .on_drop_hover(cx, |_: &mut State, over| {
                println!("androiddemo: drag {}", if over { "over" } else { "away" })
            })
            .gap(theme.spacing.lg)
            .align(Align::Center, Align::Center)
            .padding(Insets::new(
                pad + safe.top,
                pad + safe.right,
                pad + safe.bottom,
                pad + safe.left,
            ))
            .grow(1.0)
    }

    /// What *Set clip* puts on the clipboard.
    const COPIED: &str = "Copied by androiddemo";
    /// What *Open link* opens.
    const URL: &str = "https://example.com/stipple-androiddemo";

    /// The chips at the strip's end: *Set clip* and *Get clip* write and
    /// read the system clipboard (`platform::clipboard`, the
    /// `ClipboardManager`), *Open link* opens a URL (an `ACTION_VIEW`
    /// intent).
    fn service_chips(cx: &mut Cx<State>) -> Vec<Element> {
        let set = button(cx, "Set clip").width(90.0).on_tap(cx, |_| {
            let ok = stipple::platform::clipboard::set_text(COPIED);
            println!("androiddemo: clipboard set {ok}");
        });
        let get = button(cx, "Get clip").width(90.0).on_tap(cx, |_| {
            let text = stipple::platform::clipboard::text();
            println!("androiddemo: clipboard got {text:?}");
        });
        let handle = cx.handle();
        let open = button(cx, "Open link").width(90.0).on_tap(cx, move |_| {
            let opened = handle.open_url(URL);
            println!("androiddemo: opened {URL} {opened:?}");
        });
        vec![set, get, open]
    }

    /// Buttons for the system's services: a notification, and the document
    /// picker to open a text file or save one.
    fn platform_row(cx: &mut Cx<State>, theme: &Theme) -> Element {
        let notify = button(cx, "Notify").on_tap(cx, |_| {
            let sent = stipple::platform::notify(
                &Notification::new("Stipple test").body("A notification from androiddemo"),
            );
            println!("androiddemo: notify {sent:?}");
        });
        let open = button(cx, "Open").on_tap(cx, |_| {
            dialog::open_file_contents(&FileDialog::new().accept(".txt"), |file| match file {
                Some(f) => println!(
                    "androiddemo: opened {:?} ({} bytes): {:?}",
                    f.name,
                    f.bytes.len(),
                    String::from_utf8_lossy(&f.bytes).trim_end()
                ),
                None => println!("androiddemo: open cancelled"),
            });
            println!("androiddemo: open requested");
        });
        let save = button(cx, "Save").on_tap(cx, |s| {
            let text = format!("Saved by androiddemo after {} taps\n", s.taps);
            dialog::save_file_contents_then(
                &FileDialog::new().file_name("stipple-saved.txt"),
                text.as_bytes(),
                |saved| println!("androiddemo: saved {saved}"),
            );
            println!("androiddemo: save requested");
        });
        // A folder (ACTION_OPEN_DOCUMENT_TREE), read and written by name
        // through the document tree, off the UI thread.
        let folder = button(cx, "Dir").on_tap(cx, |_| {
            dialog::pick_folder_then(&FileDialog::new(), |folder| {
                let Some(folder) = folder else {
                    println!("androiddemo: folder cancelled");
                    return;
                };
                println!(
                    "androiddemo: folder picked {:?} uri={}",
                    folder.name(),
                    folder.uri().is_some_and(|u| u.starts_with("content://"))
                );
                std::thread::spawn(move || {
                    let names = |f: &dialog::Folder| {
                        f.entries().map(|e| {
                            e.iter()
                                .map(|e| format!("{}{}", e.name, if e.is_dir { "/" } else { "" }))
                                .collect::<Vec<_>>()
                        })
                    };
                    let before = names(&folder).map_err(|e| e.to_string());
                    let read = folder
                        .read("in.txt")
                        .map(|b| String::from_utf8_lossy(&b).trim_end().to_owned())
                        .map_err(|e| e.to_string());
                    let wrote = folder
                        .write("out.txt", b"written by androiddemo\n")
                        .and_then(|()| folder.write("out.txt", b"replaced\n"))
                        .map_err(|e| e.to_string());
                    let made = folder
                        .create_folder("made")
                        .and_then(|sub| {
                            sub.write("deep.txt", b"deep\n")?;
                            folder.folder("made")?.read("deep.txt")
                        })
                        .map(|b| String::from_utf8_lossy(&b).trim_end().to_owned())
                        .map_err(|e| e.to_string());
                    println!(
                        "androiddemo: folder before={before:?} read={read:?} wrote={wrote:?} \
                         made={made:?} after={:?}",
                        names(&folder).map_err(|e| e.to_string())
                    );
                });
            });
            println!("androiddemo: folder requested");
        });
        let handle = cx.handle();
        let share = button(cx, "Share").on_tap(cx, move |_| {
            handle.share(
                Share::new()
                    .text("Shared from androiddemo")
                    .url("https://github.com/KarpelesLab/stipple")
                    .subject("Stipple")
                    .file(
                        "stipple-share.txt",
                        b"A file shared by androiddemo\n".to_vec(),
                        "text/plain",
                    ),
                |_: &mut State, outcome: ShareOutcome| println!("androiddemo: share {outcome:?}"),
            );
            println!("androiddemo: share requested");
        });
        let buzz = button(cx, "Buzz").on_tap(cx, |_| {
            haptic(Haptic::Success);
            println!("androiddemo: haptic");
        });
        let later = button(cx, "Later").on_tap(cx, |_| {
            let scheduled = stipple::platform::schedule_notification(
                &ScheduledNotification::new(
                    "androiddemo-later",
                    Notification::new("Scheduled from androiddemo").body("Tap to open the app"),
                )
                .after(Duration::from_secs(10)),
            );
            println!("androiddemo: scheduled {scheduled:?}");
            stipple::platform::pending_notifications(|ids| {
                println!("androiddemo: pending {ids:?}");
            });
        });
        column(vec![
            row(vec![notify, open, save]).gap(theme.spacing.sm),
            row(vec![share, buzz, later, folder]).gap(theme.spacing.sm),
        ])
        .gap(theme.spacing.sm)
    }

    /// Read the APK's packaged files (`assets/`, which build-apk.sh adds)
    /// through `platform::assets`, the AAssetManager: a whole file with its
    /// size and FNV-1a hash, the listings, an SVG decoded, a file read in
    /// pieces after a seek, and a missing one.
    fn log_assets() {
        use std::io::{Read, Seek, SeekFrom};
        let files = stipple::platform::assets();
        match files.read("hello.txt") {
            Ok(bytes) => println!(
                "androiddemo: asset hello.txt bytes={} fnv1a={:08x} text={:?}",
                bytes.len(),
                fnv1a(&bytes),
                String::from_utf8_lossy(&bytes).trim_end()
            ),
            Err(e) => println!("androiddemo: asset hello.txt failed: {e}"),
        }
        let streamed = files.open("hello.txt").map(|mut r| {
            let mut rest = String::new();
            let _ = r.seek(SeekFrom::Start(6));
            let _ = r.read_to_string(&mut rest);
            (r.len(), rest.trim_end().to_owned())
        });
        let svg = files
            .svg("img/dot.svg")
            .map(|s| format!("{}x{}", s.size().width, s.size().height));
        println!(
            "androiddemo: assets list={:?} img={:?} svg={:?} streamed={:?} exists={} missing={:?}",
            files.list("").unwrap_or_default(),
            files.list("img").unwrap_or_default(),
            svg.map_err(|e| e.to_string()),
            streamed.map_err(|e| e.to_string()),
            files.exists("img/dot.svg"),
            files.read("nope.txt").err().map(|e| e.to_string()),
        );
        // An asset as a range of the APK (AAsset_openFileDescriptor64):
        // media/dot.png is stored uncompressed, so it has one, whose bytes
        // read through the descriptor are the asset's; img/dot.svg is
        // deflated, so it hasn't (aapt stores hello.txt: too small to gain).
        let fd = files.open_fd("media/dot.png").map(|fd| {
            let same = fd.read_all().ok() == files.read("media/dot.png").ok();
            (fd.offset() > 0, fd.len(), same)
        });
        println!(
            "androiddemo: asset fd media/dot.png={:?} img/dot.svg={:?}",
            fd.map_err(|e| e.to_string()),
            files.open_fd("img/dot.svg").err().map(|e| e.to_string()),
        );
    }

    /// `platform::secure_store` (an AndroidKeyStore key sealing files in
    /// the no-backup directory): what the last launch left (a launch count,
    /// kept across process death, which examples/androiddemo/
    /// test-secure-store.sh checks), then a round trip of every byte value.
    fn log_secure_store() {
        let store = stipple::platform::secure_store();
        let previous = store.get("androiddemo-launches");
        let launches = match &previous {
            Ok(Some(v)) => String::from_utf8_lossy(v)
                .trim_start_matches("androiddemo secret launch ")
                .parse::<u32>()
                .unwrap_or(0),
            _ => 0,
        } + 1;
        println!(
            "androiddemo: secure store previous={:?}",
            previous.map(|v| v.map(|v| String::from_utf8_lossy(&v).into_owned()))
        );
        let set = store.set(
            "androiddemo-launches",
            format!("androiddemo secret launch {launches}").as_bytes(),
        );
        let value: Vec<u8> = (0..=255u8).collect();
        let round_trip = store
            .set("androiddemo-round-trip", &value)
            .and_then(|()| store.get("androiddemo-round-trip"))
            .map(|v| v.as_deref() == Some(&value[..]));
        let removed = store
            .remove("androiddemo-round-trip")
            .and_then(|()| store.get("androiddemo-round-trip"))
            .map(|v| v.is_none());
        println!(
            "androiddemo: secure store launch={launches} set={set:?} round_trip={round_trip:?} removed={removed:?}"
        );
    }

    /// FNV-1a, 32 bits: a hash CI recomputes from the file.
    fn fnv1a(bytes: &[u8]) -> u32 {
        bytes.iter().fold(0x811c_9dc5, |h, &b| {
            (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
        })
    }

    fn main() {
        let theme = Theme::native();
        println!(
            "androiddemo: starting, dark={} background={:?}",
            theme.is_dark(),
            theme.palette.background
        );
        // The package's name, label and version, and its own directories.
        let info = stipple::platform::app_info();
        println!(
            "androiddemo: app id={} name={:?} version={} build={}",
            info.id,
            info.name,
            info.version.as_deref().unwrap_or("-"),
            info.build.as_deref().unwrap_or("-"),
        );
        // Japanese in the default font, off screen: Roboto has no kana or
        // kanji, so they come from the system's CJK fallback (Noto Sans CJK,
        // a collection of CFF fonts), loaded here.
        if let Some(font) = Font::system_default() {
            let text = "日本語";
            let mut scene = stipple::render::Scene::new(Size::new(120.0, 40.0));
            scene.fill_text(
                text,
                Point::new(0.0, 0.0),
                &TextPaint::new(&font, 32.0, Color::BLACK),
            );
            let pm = stipple::render::SoftwareRenderer::new().render(scene, ScaleFactor::IDENTITY);
            let ink = (0..pm.size().height)
                .flat_map(|y| (0..pm.size().width).map(move |x| (x, y)))
                .filter(|&(x, y)| pm.pixel(x, y).is_some_and(|p| p[3] > 127))
                .count();
            println!(
                "androiddemo: cjk {text} glyphs={} ink={ink}",
                text.chars().all(|c| font.has_glyph(c)),
            );
        }
        match stipple::platform::dirs() {
            Ok(d) => println!(
                "androiddemo: dirs data={} cache={} temp={} exist={}",
                d.data.display(),
                d.cache.display(),
                d.temp.display(),
                [&d.data, &d.cache, &d.temp].iter().all(|p| p.is_dir()),
            ),
            Err(e) => println!("androiddemo: dirs unavailable: {e}"),
        }
        log_assets();
        let net = stipple::platform::network();
        println!(
            "androiddemo: network online={} transport={:?} metered={} generation={}",
            net.online, net.transport, net.metered, net.generation
        );
        log_secure_store();
        let state = State {
            taps: 0,
            ticks: 0,
            name: TextInput::new(),
            code: TextInput::new(),
            volume: 0.5,
        };
        let app = App::new(state, view)
            .theme(theme)
            // Under the status and navigation bars, upright only.
            .edge_to_edge(true)
            .orientations(Orientations::PORTRAIT)
            // Back resets the count: there is something to go back from
            // only while it isn't 0.
            .back_enabled(|s: &State| s.taps > 0)
            .on_back_progress(|_: &mut State, gesture| match gesture {
                // The progress once a gesture is a quarter of the way.
                BackGesture::Progressed { progress } => {
                    if progress >= 0.25 && !QUARTER.swap(true, Ordering::Relaxed) {
                        println!("androiddemo: back gesture progress {progress:.2}");
                    }
                }
                _ => {
                    QUARTER.store(false, Ordering::Relaxed);
                    println!("androiddemo: back gesture {gesture:?}");
                }
            })
            // Kept across process death: the count and the field's text,
            // as `taps` and the text on lines of their own.
            .save_state(|s: &State| {
                println!("androiddemo: saved taps={}", s.taps);
                format!("{}\n{}", s.taps, s.name.text()).into_bytes()
            })
            .restore_state(|s: &mut State, bytes| {
                let text = String::from_utf8_lossy(bytes);
                let (taps, name) = text.split_once('\n').unwrap_or((&text, ""));
                s.taps = taps.parse().unwrap_or(0);
                s.name = TextInput::from_text(name);
                println!("androiddemo: restored taps={} name={name:?}", s.taps);
            })
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
            .on_lifecycle(|_: &mut State, change| println!("androiddemo: lifecycle {change:?}"))
            // The default network's changes (test-network.sh turns Wi-Fi
            // and mobile data off and on).
            .on_network(|_: &mut State, net| {
                println!(
                    "androiddemo: network changed online={} transport={:?} metered={} constrained={} generation={}",
                    net.online, net.transport, net.metered, net.constrained, net.generation
                );
            })
            .on_notification_tapped(|_: &mut State, id| {
                println!("androiddemo: notification tapped {id:?}");
            })
            .on_open_url(|_: &mut State, url| println!("androiddemo: opened url {url:?}"))
            .on_transaction(|_: &mut State, t| {
                println!(
                    "androiddemo: transaction {} {:?} {:?} id={} token={} kind={:?}",
                    t.product_id, t.state, t.store, t.id, t.token, t.kind
                );
                // Nothing to grant in a demo: finish it (consumes the gems,
                // acknowledges the rest).
                if t.is_paid() {
                    stipple::platform::store().finish(t);
                }
            });
        // Built with a GPU backend (`STIPPLE_APK_FEATURES=vk` for
        // build-apk.sh), it draws with it, straight to the window.
        #[cfg(any(feature = "vk", feature = "gl"))]
        let app = app.renderer(stipple::render::Renderer::Gpu);
        // The store (Google Play, if the APK carries the Play Billing
        // Library: build-apk.sh with STIPPLE_BILLING=1): what it says about
        // two products. The app isn't in the Play Console, so a device with
        // Google Play answers with no products, or with why it can't; if it
        // does know the gems, the demo buys them.
        let handle = app.handle();
        app.handle().store().products(
            vec![
                ProductId::consumable("stipple_demo_gems"),
                ProductId::subscription("stipple_demo_vip"),
            ],
            move |_: &mut State, r| match r {
                Ok(products) => {
                    println!("androiddemo: store products {}", products.len());
                    for p in &products {
                        println!(
                            "androiddemo: product {} {:?} {:?} {} {} {} {:?}",
                            p.id, p.kind, p.title, p.price, p.price_micros, p.currency, p.period
                        );
                    }
                    if products.iter().any(|p| p.id == "stipple_demo_gems") {
                        handle
                            .store()
                            .purchase("stipple_demo_gems", |_: &mut State, r| {
                                println!("androiddemo: purchase {r:?}");
                            });
                    }
                }
                Err(e) => println!("androiddemo: store error {e:?}"),
            },
        );
        // The screen stays on while the demo shows (CI checks the window's
        // FLAG_KEEP_SCREEN_ON).
        app.handle().set_keep_awake(true);
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
