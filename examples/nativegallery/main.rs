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

use stipple::prelude::*;

struct Gallery {
    checked: bool,
    choice: usize,
    on: bool,
    level: f64,
    name: TextInput,
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
    column(vec![
        heading(cx, "Native controls"),
        name,
        row(vec![
            button(cx, "Save"),
            Button::new("Cancel").variant(Variant::Secondary).view(cx),
        ])
        .gap(theme.spacing.sm),
        check,
        row(radios).gap(theme.spacing.lg),
        toggle,
        level,
        progress_bar(cx, state.level),
    ])
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
    let app = App::new(gallery(), view)
        .title("Stipple")
        .theme(Theme::native())
        .logical_size(Size::new(360.0, 340.0));
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
