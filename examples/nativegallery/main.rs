//! Every control under [`Theme::native`]: on Windows, macOS and iOS the OS draws
//! them, and on Linux they stay self-drawn in the system's colors. CI screenshots
//! this on each platform.
//!
//! `--gpu-check [dir]` renders the gallery off screen on the CPU and, built
//! with a GPU backend (`--features mtl`, `d3d`, `vk` or `gl`), through
//! `stipple::gpu`, at scales 1, 1.5 and 2; writes both frames to `dir` as
//! PNGs; and prints `nativegallery gpu: ok` when they match. It's the
//! comparison `src/gpu/compare.rs` makes, here on the main thread, which
//! AppKit needs to draw controls. Where the OS draws controls it also checks
//! that it did: the native frame must differ from the self-drawn one.
//!
//! **Self-test.** With `NATIVEGALLERY_SELFTEST` set (on the iOS simulator,
//! `SIMCTL_CHILD_NATIVEGALLERY_SELFTEST=1`), the window also shows a short
//! scrolled list of buttons, and the app checks, where the OS hosts real
//! controls (iOS, macOS, Windows), that they are real: it reads the live
//! controls back from the OS's view hierarchy (their classes, titles,
//! frames, and whether a tap at their middle lands on them), acts on them
//! as the user would (a press on Save, a flip of the switch, the slider
//! moved) and checks the app's handlers ran, picks a size from the select
//! through the OS's own menu (a `UIAction` of the `UIButton`'s `UIMenu`,
//! an `NSPopUpButton`'s menu item, the `COMBOBOX`'s selection) and checks
//! the app took it, then has the app refuse a pick and checks the OS
//! control went back to the app's choice, opens a panel over the top of
//! the window and checks the controls under it became pictures (for about
//! six seconds: CI screenshots it), and checks the list's half-hidden row
//! is live, in its scroll area's clipping container and cut by it, and
//! that once the list scrolls the cut rows follow it, still live. Then the
//! text field, a real `UITextField` / `NSTextField` / `EDIT`: it focuses it
//! and types into it through the control's own text input, and checks the
//! app's state follows; sets the state and checks the control follows;
//! composes in it as an input method would (iOS and macOS) and checks the
//! field's preedit; commits, and presses Return (the field's submit handler
//! runs). It logs to stdout and, on iOS, to `nativegallery.log` in its tmp
//! dir, ending with `nativegallery selftest: passed` or `... FAILED:
//! <what>`; and, after that, each change of the field's text
//! (`nativegallery: name "..."`), for CI's real typing.

// On Android (emulated TLS) clippy flags even a `const` thread_local.
#![cfg_attr(target_os = "android", allow(clippy::missing_const_for_thread_local))]

use stipple::prelude::*;

struct Gallery {
    checked: bool,
    choice: usize,
    on: bool,
    level: f64,
    name: TextInput,
    /// Presses on Save.
    saves: u32,
    /// Returns pressed in the name field.
    submits: u32,
    /// The self-test's panel over the top of the window shows.
    panel: bool,
    /// The self-test runs (and the list shows).
    test: bool,
    /// The select's choice.
    size: Select,
    /// Where picks go while the app refuses them (the self-test's
    /// `locked`): the select keeps showing `size`.
    refused: Select,
    locked: bool,
}

/// The size options.
const SIZES: [&str; 4] = ["Small", "Medium", "Large", "Extra large"];

thread_local! {
    /// The self-test's list and how to scroll it.
    static LIST: std::cell::RefCell<Option<(stipple::core::ScrollRequests, stipple::core::ScrollId)>> =
        const { std::cell::RefCell::new(None) };
}

fn view(state: &Gallery, cx: &mut Cx<Gallery>) -> Element {
    let theme = *cx.theme();
    let radios: Vec<Element> = ["Small", "Large"]
        .iter()
        .enumerate()
        .map(|(i, text)| radio(cx, *text, state.choice == i, move |s| s.choice = i))
        .collect();
    let check = checkbox(cx, "Show hidden files", state.checked, |s| {
        s.checked = !s.checked;
    });
    let toggle = switch(cx, "Sync", state.on, |s| s.on = !s.on);
    let level = slider(cx, state.level, |s, v| s.level = v);
    let size = select(cx, &SIZES, &state.size, |s| {
        if s.locked {
            &mut s.refused
        } else {
            &mut s.size
        }
    })
    .a11y_label("Size");
    let name = text_field(cx, &state.name, |s| &mut s.name)
        .placeholder("Name")
        .on_change(cx, |s| {
            if s.test {
                println!("nativegallery: name {:?}", s.name.text());
            }
        })
        .on_submit(cx, |s| s.submits += 1);
    let mut items = vec![
        heading(cx, "Native controls"),
        name,
        row(vec![
            button(cx, "Save").on_tap(cx, |s| {
                s.saves += 1;
                if s.test {
                    println!("nativegallery: saves {}", s.saves);
                }
            }),
            Button::new("Cancel").variant(Variant::Secondary).view(cx),
        ])
        .gap(theme.spacing.sm),
        check,
        row(radios).gap(theme.spacing.lg),
        toggle,
        level,
        progress_bar(cx, state.level),
    ];
    if state.test {
        // A select (the window shows it in the self-test, where it's taller).
        items.push(size);
        // What the app has chosen, when it changes (the Windows script picks
        // with a real click).
        static SHOWN: std::sync::Mutex<Option<Option<usize>>> = std::sync::Mutex::new(None);
        let now = Some(state.size.selected());
        if std::mem::replace(&mut *SHOWN.lock().unwrap(), now) != now {
            println!("nativegallery: size {:?}", state.size.selected());
        }
        // Two rows and a half show: the half-hidden one is a picture.
        let id = cx.register_scroll();
        let requests = cx.scroll_requests();
        LIST.with(|l| *l.borrow_mut() = Some((requests, id)));
        let rows = (1..=4).map(|i| button(cx, format!("Row {i}"))).collect();
        let control = stipple::platform::native::NativeControl::Button { primary: true };
        let row = stipple::platform::native::size_hint(control)
            .1
            .unwrap_or(32.0);
        let list = column(rows).gap(8.0).height(2.5 * row + 8.0);
        items.push(list.scrollable(id));
    }
    if state.panel {
        let panel = column(vec![label(cx, "A panel over the controls")])
            .fill(theme.palette.surface)
            .border(theme.palette.border, 1.0)
            .padding(Insets::uniform(theme.spacing.lg))
            .width(360.0)
            .height(170.0);
        cx.overlay(OverlaySpec::new(panel, Anchor::At(Point::new(0.0, 0.0))));
    }
    column(items)
        .gap(theme.spacing.md)
        .padding(Insets::uniform(theme.spacing.lg))
        .align(Align::Start, Align::Start)
}

fn gallery() -> Gallery {
    Gallery {
        checked: true,
        choice: 0,
        on: true,
        level: 0.6,
        name: TextInput::from_text("Stipple"),
        saves: 0,
        submits: 0,
        panel: false,
        test: false,
        size: Select::new(Some(1)),
        refused: Select::new(Some(1)),
        locked: false,
    }
}

fn main() {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut args = std::env::args().skip(1);
        if args.next().as_deref() == Some("--gpu-check") {
            gpu_check(args.next().unwrap_or_else(|| "visual_output".into()));
            return;
        }
    }
    let test = std::env::var_os("NATIVEGALLERY_SELFTEST").is_some();
    let height = if test { 560.0 } else { 340.0 };
    let app = App::new(Gallery { test, ..gallery() }, view)
        .title("Stipple")
        .theme(Theme::native())
        .logical_size(Size::new(360.0, height));
    #[cfg(not(target_arch = "wasm32"))]
    if test {
        selftest::schedule(&app.handle());
    }
    app.run();
}

/// The `--gpu-check` mode (see the top of this file).
#[cfg(not(target_arch = "wasm32"))]
fn gpu_check(dir: String) {
    let _ = std::fs::create_dir_all(&dir);
    // A switch's first frame is its animation's first step; with motion off
    // every frame shows where things end up.
    stipple::anim::set_reduced_motion(Some(true));
    let font = Font::system_default();
    let make = |theme: Theme, scale: f64| {
        let app = App::new(gallery(), view)
            .theme(theme)
            .logical_size(Size::new(360.0, 340.0))
            .scale(ScaleFactor::new(scale));
        match font.clone() {
            Some(font) => app.font(font),
            None => app,
        }
    };
    let mut failed = false;
    for scale in [1.0, 1.5, 2.0] {
        let cpu = make(Theme::native(), scale).render_once();
        let _ = cpu.save_png(format!("{dir}/native-gpu@{scale}-cpu.png"));
        // The OS really drew the controls: self-drawn, the frame differs.
        if stipple::platform::native::available() {
            let plain = make(Theme::native().native_controls(false), scale).render_once();
            let (mean, _) = diff(&cpu, &plain);
            println!("nativegallery: native against self-drawn @{scale}: mean {mean:.2}");
            if mean < 0.2 {
                println!("nativegallery gpu: the OS drew no controls @{scale}");
                failed = true;
            }
        }
        #[cfg(feature = "gpu")]
        {
            let gpu = make(Theme::native(), scale)
                .render_with(|scene: &stipple::render::Scene, bg, scale| {
                    stipple::gpu::render_scene_scaled(scene, bg, scale).unwrap_or_else(|e| {
                        println!("nativegallery gpu: no GPU: {e}");
                        std::process::exit(1)
                    })
                })
                .render_once();
            let _ = gpu.save_png(format!("{dir}/native-gpu@{scale}-gpu.png"));
            let (mean, bad) = diff(&gpu, &cpu);
            println!(
                "nativegallery: GPU ({}) against CPU @{scale}: mean {mean:.3}, bad {bad:.4}",
                stipple::gpu::device_name().unwrap_or_default()
            );
            // The tolerance of the App comparison in src/gpu/compare.rs.
            if mean >= 1.0 || bad >= 0.01 {
                failed = true;
            }
        }
    }
    if cfg!(not(feature = "gpu")) {
        println!("nativegallery gpu: built without a GPU backend, CPU frames only");
    }
    if failed {
        println!("nativegallery gpu: FAILED");
        std::process::exit(1);
    }
    println!("nativegallery gpu: ok");
}

/// The mean channel difference of two frames (`0..=255`), and the share of
/// pixels with a channel off by more than 64.
#[cfg(not(target_arch = "wasm32"))]
fn diff(a: &stipple::render::Pixmap, b: &stipple::render::Pixmap) -> (f64, f64) {
    assert_eq!(a.size(), b.size());
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut sum, mut bad) = (0u64, 0u64);
    for (p, q) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0) {
        let d: [u8; 4] = std::array::from_fn(|c| p[c].abs_diff(q[c]));
        sum += d.iter().map(|&v| v as u64).sum::<u64>();
        bad += u64::from(d.iter().any(|&v| v > 64));
    }
    let pixels = (a.len() / 4).max(1) as f64;
    (sum as f64 / (pixels * 4.0), bad as f64 / pixels)
}

// In the browser, `web.rs` builds this file as a library and the page
// runs `main`'s app (on the web, `App::run` hands it to the page).
stipple::web_app!(main);

/// The `NATIVEGALLERY_SELFTEST` mode (see the top of this file).
#[cfg(not(target_arch = "wasm32"))]
mod selftest {
    use super::Gallery;
    use std::sync::Mutex;
    use std::time::Duration;
    use stipple::platform::native::{
        LiveControl, TextPoke, live_controls, poke_control, poke_text, text_trace, trace_text,
    };
    use stipple::prelude::AppHandle;

    static FAILURES: Mutex<Vec<String>> = Mutex::new(Vec::new());
    /// The select's id, and where Row 1 was before the list scrolled.
    static SELECT: Mutex<Option<u64>> = Mutex::new(None);
    static ROW1: Mutex<Option<f64>> = Mutex::new(None);
    /// How far the self-test scrolls the list.
    static SCROLLED: Mutex<f64> = Mutex::new(0.0);

    fn log(line: &str) {
        println!("{line}");
        if cfg!(target_os = "ios") {
            use std::io::Write;
            let path = std::env::temp_dir().join("nativegallery.log");
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                let _ = writeln!(f, "{line}");
            }
        }
    }

    fn check(ok: bool, what: impl Into<String>) {
        if !ok {
            let what = what.into();
            log(&format!("nativegallery: check failed: {what}"));
            FAILURES.lock().unwrap().push(what);
        }
    }

    fn titled<'a>(live: &'a [LiveControl], title: &str) -> Option<&'a LiveControl> {
        live.iter().find(|c| c.title == title)
    }

    /// The live control whose class names it one of `kinds`.
    fn of_kind<'a>(live: &'a [LiveControl], kinds: &[&str]) -> Option<&'a LiveControl> {
        live.iter().find(|c| {
            let class = c.class.to_ascii_lowercase();
            kinds.iter().any(|k| class.contains(k))
        })
    }

    /// Log where live control `c` is.
    fn describe(c: &LiveControl) {
        log(&format!(
            "nativegallery: live {} {:?} at {:.0},{:.0} {:.0}x{:.0} shown {:.0},{:.0} {:.0}x{:.0} in {:?} hit={}",
            c.class,
            c.title,
            c.frame.min_x(),
            c.frame.min_y(),
            c.frame.width(),
            c.frame.height(),
            c.visible.min_x(),
            c.visible.min_y(),
            c.visible.width(),
            c.visible.height(),
            c.container,
            c.hit
        ));
    }

    /// Check live control `c` (named `what`) is cut by the OS: in a
    /// clipping container, shown only in part (at the top or the bottom),
    /// and a tap on that part lands on it.
    fn cut(c: &LiveControl, what: &str, at_top: bool) {
        check(
            !c.container.is_empty(),
            format!("{what} is in a clipping container"),
        );
        let shown = c.visible.height();
        check(
            shown > 1.0 && shown < c.frame.height() - 1.0,
            format!(
                "{what} is cut ({shown:.1} of {:.1} shown)",
                c.frame.height()
            ),
        );
        let edge = if at_top {
            c.visible.min_y() - c.frame.min_y()
        } else {
            c.frame.max_y() - c.visible.max_y()
        };
        check(
            edge > 1.0,
            format!(
                "{what} is cut at the {} ({edge:.1})",
                if at_top { "top" } else { "bottom" }
            ),
        );
        check(c.hit, format!("a tap on {what}'s shown part lands on it"));
    }

    const SLIDER: &[&str] = &["slider", "trackbar"];
    const SWITCH: &[&str] = &["switch"];
    const FIELD: &[&str] = &["textfield", "edit"];

    fn at(handle: &AppHandle<Gallery>, ms: u64, f: impl FnOnce(&mut Gallery) + Send + 'static) {
        handle.after(Duration::from_millis(ms), f);
    }

    pub(super) fn schedule(handle: &AppHandle<Gallery>) {
        let hosts = cfg!(any(
            target_os = "ios",
            target_os = "macos",
            target_os = "windows"
        ));
        // The controls are real ones, where taps land.
        at(handle, 2500, move |_| {
            let live = live_controls();
            for c in &live {
                describe(c);
            }
            if !hosts {
                return;
            }
            for title in ["Save", "Cancel", "Row 1", "Row 2"] {
                let c = titled(&live, title);
                check(c.is_some(), format!("{title} is a live control"));
                check(
                    c.is_some_and(|c| c.hit),
                    format!("a tap on {title} lands on it"),
                );
            }
            check(
                of_kind(&live, SLIDER).is_some(),
                "the slider is a live control",
            );
            // Half hidden by its list: live, in the list's clipping
            // container, which cuts it. Scrolled out of it: not shown.
            let row3 = titled(&live, "Row 3");
            check(row3.is_some(), "the half-hidden row is live");
            if let Some(c) = row3 {
                cut(c, "Row 3", false);
            }
            check(
                titled(&live, "Row 4").is_none(),
                "the hidden row isn't shown",
            );
            if let Some(c) = titled(&live, "Row 1") {
                *ROW1.lock().unwrap() = Some(c.frame.min_y());
                // Row heights: half a row down.
                *SCROLLED.lock().unwrap() = (c.frame.height() / 2.0).round();
            }
            // The select is the OS's pop-up button, showing the app's
            // choice; a pick in its menu reaches the app.
            let size = titled(&live, "Medium");
            check(
                size.is_some(),
                "the select is a live control showing Medium",
            );
            if let Some(size) = size {
                *SELECT.lock().unwrap() = Some(size.id);
                check(size.hit, "a tap on the select lands on it");
                check(poke_control(size.id, Some(2.0)), "the select picks Large");
            }
            if let Some(save) = titled(&live, "Save") {
                check(poke_control(save.id, None), "Save takes a press");
            }
            if let Some(slider) = of_kind(&live, SLIDER) {
                check(poke_control(slider.id, Some(0.2)), "the slider moves");
            }
        });
        at(handle, 3500, move |s| {
            if !hosts {
                return;
            }
            check(
                s.saves == 1,
                format!("Save's handler ran once (saves {})", s.saves),
            );
            check(
                (s.level - 0.2).abs() < 0.01,
                format!("the slider's handler set 0.2 ({})", s.level),
            );
            check(
                s.size.selected() == Some(2),
                format!(
                    "the select's pick reached the app ({:?})",
                    s.size.selected()
                ),
            );
            // From now on the app refuses picks.
            s.locked = true;
            let live = live_controls();
            if let Some(switch) = of_kind(&live, SWITCH) {
                log(&format!("nativegallery: flipping the switch (on {})", s.on));
                check(poke_control(switch.id, None), "the switch flips");
            }
        });
        at(handle, 4500, move |s| {
            if hosts && cfg!(not(target_os = "windows")) {
                check(!s.on, "the switch's handler turned it off");
            }
            if hosts {
                let live = live_controls();
                let id = *SELECT.lock().unwrap();
                let size = id.and_then(|id| live.iter().find(|c| c.id == id));
                check(
                    size.is_some_and(|c| c.title == "Large"),
                    format!(
                        "the select shows Large ({:?})",
                        size.map(|c| c.title.as_str())
                    ),
                );
                // A pick the app refuses.
                if let Some(size) = size {
                    check(poke_control(size.id, Some(0.0)), "the select picks Small");
                }
            }
            // Half a row down: Row 1 cut at the top, Row 3 at the bottom.
            let by = *SCROLLED.lock().unwrap();
            super::LIST.with(|l| {
                if let Some((requests, id)) = &*l.borrow() {
                    requests.scroll_to(*id, by);
                }
            });
            s.panel = true;
        });
        at(handle, 5500, move |_| {
            let live = live_controls();
            for c in &live {
                describe(c);
            }
            if hosts {
                check(
                    titled(&live, "Save").is_none(),
                    "Save under the panel is a picture",
                );
                // The refused pick went back to the app's choice.
                let id = *SELECT.lock().unwrap();
                let size = id.and_then(|id| live.iter().find(|c| c.id == id));
                check(
                    size.is_some_and(|c| c.title == "Large"),
                    format!(
                        "the refused pick went back to Large ({:?})",
                        size.map(|c| c.title.as_str())
                    ),
                );
                // The scrolled list: its rows moved with it, still live,
                // cut by its container at the top and the bottom.
                let by = *SCROLLED.lock().unwrap();
                let row1 = titled(&live, "Row 1");
                check(row1.is_some(), "Row 1, half scrolled out, is live");
                if let Some(c) = row1 {
                    cut(c, "Row 1", true);
                    if let Some(was) = *ROW1.lock().unwrap() {
                        let moved = was - c.frame.min_y();
                        check(
                            (moved - by).abs() < 1.0,
                            format!("Row 1 moved with the list ({moved:.1} for {by})"),
                        );
                    }
                }
                if let Some(c) = titled(&live, "Row 3") {
                    cut(c, "Row 3 (scrolled)", false);
                }
                check(
                    of_kind(&live, SLIDER).is_some() || titled(&live, "Row 1").is_some(),
                    "controls clear of the panel stay live",
                );
            }
            log("nativegallery: panel up");
        });
        at(handle, 11500, move |s| {
            s.panel = false;
        });
        at(handle, 12500, move |s| {
            let live = live_controls();
            if hosts {
                check(
                    titled(&live, "Save").is_some(),
                    "Save is live again once the panel goes",
                );
                check(
                    s.size.selected() == Some(2),
                    format!(
                        "the app kept Large through the refused pick ({:?})",
                        s.size.selected()
                    ),
                );
            }
            // Picks reach the app again (a real one may follow: CI's).
            s.locked = false;
            // The text field is a real one, holding the app's text: focus it
            // and type through its own text input.
            trace_text(true);
            if let Some(field) = of_kind(&live, FIELD) {
                log(&format!(
                    "nativegallery: field {} {:?} focused={}",
                    field.class, field.value, field.focused
                ));
                check(field.value == "Stipple", "the field holds the app's text");
                check(
                    poke_text(field.id, TextPoke::Focus),
                    "the field takes focus",
                );
                check(
                    poke_text(field.id, TextPoke::Type("!".into())),
                    "the field takes typing",
                );
            } else if hosts {
                check(false, "the text field is a live control");
            }
        });
        at(handle, 14500, move |s| {
            if !hosts {
                return;
            }
            check(
                s.name.text() == "Stipple!",
                format!("typing reached the app ({:?})", s.name.text()),
            );
            let live = live_controls();
            check(
                of_kind(&live, FIELD).is_some_and(|f| f.focused),
                "the field being edited stays live, focused",
            );
            // The app sets the text: the field follows.
            s.name = stipple::prelude::TextInput::from_text("Set by app");
        });
        at(handle, 16500, move |_| {
            if !hosts {
                return;
            }
            let live = live_controls();
            let field = of_kind(&live, FIELD);
            check(
                field.is_some_and(|f| f.value == "Set by app"),
                format!(
                    "the field follows the app's text ({:?})",
                    field.map(|f| f.value.as_str())
                ),
            );
            // An input method's composition (UIKit and AppKit take one
            // from outside; Windows' needs an input method installed).
            if cfg!(not(target_os = "windows"))
                && let Some(f) = field
            {
                check(
                    poke_text(f.id, TextPoke::Compose("にほ".into())),
                    "the field takes a composition",
                );
            }
        });
        at(handle, 18500, move |s| {
            if !hosts {
                return;
            }
            if cfg!(not(target_os = "windows")) {
                check(
                    s.name.buffer().preedit() == Some("にほ") && s.name.text() == "Set by app",
                    format!(
                        "the composition is the field's preedit ({:?} in {:?})",
                        s.name.buffer().preedit(),
                        s.name.text()
                    ),
                );
            }
            let live = live_controls();
            if let Some(f) = of_kind(&live, FIELD) {
                check(
                    poke_text(f.id, TextPoke::Type("日本".into())),
                    "the field takes the committed text",
                );
                check(poke_text(f.id, TextPoke::Enter), "the field takes Return");
            }
        });
        at(handle, 20500, move |s| {
            if hosts {
                check(
                    s.name.text() == "Set by app日本" && s.name.buffer().preedit().is_none(),
                    format!("the commit reached the app ({:?})", s.name.text()),
                );
                check(
                    s.submits == 1,
                    format!("Return submitted the field once ({})", s.submits),
                );
            }
            #[cfg(target_os = "ios")]
            {
                // VoiceOver reads each control once: the OS control.
                let elements = stipple::platform::backend::ios::simulate::accessibility_elements();
                for e in &elements {
                    log(&format!(
                        "nativegallery: element {} {:?} {:?}",
                        e.class, e.label, e.traits
                    ));
                }
                for (label, class) in [
                    ("Save", "UIButton"),
                    ("Sync", "UISwitch"),
                    ("Size", "UIButton"),
                    ("Name", "UITextField"),
                ] {
                    let found: Vec<_> = elements.iter().filter(|e| e.label == label).collect();
                    check(
                        found.len() == 1,
                        format!("one element reads {label} ({})", found.len()),
                    );
                    check(
                        found.iter().all(|e| e.class == class),
                        format!("{label} is read as the {class}"),
                    );
                }
            }
            let failures = std::mem::take(&mut *FAILURES.lock().unwrap());
            // What the text field's bookkeeping did, to tell why.
            if !failures.is_empty() {
                for line in text_trace() {
                    log(&format!("nativegallery: text trace: {line}"));
                }
            }
            if failures.is_empty() {
                log("nativegallery selftest: passed");
            } else {
                log(&format!(
                    "nativegallery selftest: FAILED: {}",
                    failures.join("; ")
                ));
            }
        });
    }
}
