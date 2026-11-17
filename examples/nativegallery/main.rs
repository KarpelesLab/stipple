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
//! moved) and checks the app's handlers ran, opens a panel over the top
//! of the window and checks the controls under it became pictures (for
//! about six seconds: CI screenshots it), and checks the list's
//! half-hidden row isn't live. It logs to stdout and, on iOS, to
//! `nativegallery.log` in its tmp dir, ending with `nativegallery
//! selftest: passed` or `... FAILED: <what>`.

use stipple::prelude::*;

struct Gallery {
    checked: bool,
    choice: usize,
    on: bool,
    level: f64,
    name: TextInput,
    /// Presses on Save.
    saves: u32,
    /// The self-test's panel over the top of the window shows.
    panel: bool,
    /// The self-test runs (and the list shows).
    test: bool,
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
    let name = text_field(cx, &state.name, |s| &mut s.name);
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
        // Two rows and a half show: the half-hidden one is a picture.
        let id = cx.register_scroll();
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
        panel: false,
        test: false,
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
    let height = if test { 480.0 } else { 340.0 };
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
    use stipple::platform::native::{LiveControl, live_controls, poke_control};
    use stipple::prelude::AppHandle;

    static FAILURES: Mutex<Vec<String>> = Mutex::new(Vec::new());

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

    const SLIDER: &[&str] = &["slider", "trackbar"];
    const SWITCH: &[&str] = &["switch"];

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
                log(&format!(
                    "nativegallery: live {} {:?} at {:.0},{:.0} {:.0}x{:.0} hit={}",
                    c.class,
                    c.title,
                    c.frame.min_x(),
                    c.frame.min_y(),
                    c.frame.width(),
                    c.frame.height(),
                    c.hit
                ));
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
            // Half hidden by its list, or scrolled out of it: pictures.
            check(
                titled(&live, "Row 3").is_none(),
                "the half-hidden row is a picture",
            );
            check(
                titled(&live, "Row 4").is_none(),
                "the hidden row isn't shown",
            );
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
            s.panel = true;
        });
        at(handle, 5500, move |_| {
            let live = live_controls();
            if hosts {
                check(
                    titled(&live, "Save").is_none(),
                    "Save under the panel is a picture",
                );
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
        at(handle, 12500, move |_| {
            let live = live_controls();
            if hosts {
                check(
                    titled(&live, "Save").is_some(),
                    "Save is live again once the panel goes",
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
                for (label, class) in [("Save", "UIButton"), ("Sync", "UISwitch")] {
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
