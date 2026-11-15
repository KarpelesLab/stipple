//! The GPU scene renderer against the software renderer: each scene is drawn
//! both ways and the frames compared pixel by pixel.
//!
//! They run on the platform's GPU API, with the same scenes and tolerances:
//! Metal on macOS (feature `mtl`), Vulkan on Linux (feature `vk`; Mesa's
//! lavapipe in CI) or on macOS over MoltenVK, GLES2 on Linux (feature `gl`;
//! Mesa's llvmpipe, through surfaceless EGL), Direct3D 11 on Windows (feature
//! `d3d`; WARP does, on CI's runner). Without a device they're skipped,
//! unless `STIPPLE_REQUIRE_GPU` is set (as CI's GPU jobs do), when they fail.
//! `STIPPLE_GPU_DUMP=<dir>` writes both frames of each
//! comparison there as PNGs.

use super::corpus::*;
use super::*;
use crate::geometry::Size;
use crate::render::SoftwareRenderer;

fn gpu(scene: &Scene, bg: Color, scale: f64) -> Option<Pixmap> {
    match render_scene_scaled(scene, bg, ScaleFactor::new(scale)) {
        Ok(p) => Some(p),
        Err(e) if std::env::var_os("STIPPLE_REQUIRE_GPU").is_some() => {
            panic!("GPU unavailable: {e}")
        }
        Err(e) => {
            eprintln!("skipping: GPU unavailable: {e}");
            None
        }
    }
}

/// Texture uploads in the thread.s last GPU frame.
fn uploads() -> usize {
    with_default(|r| Ok(r.uploads())).unwrap_or(0)
}

fn dump(name: &str, which: &str, p: &Pixmap) {
    let Some(dir) = std::env::var_os("STIPPLE_GPU_DUMP") else {
        return;
    };
    let path = std::path::Path::new(&dir).join(format!("{name}-{which}.png"));
    let _ = p.save_png(path);
}

/// How two frames differ.
#[derive(Debug)]
struct Diff {
    /// Mean absolute difference per channel, `0..=255`.
    mean: f64,
    /// Share of pixels with a channel off by more than 64.
    bad: f64,
}

fn diff(a: &Pixmap, b: &Pixmap) -> Diff {
    assert_eq!(a.size(), b.size());
    let (mut sum, mut bad) = (0u64, 0u64);
    let pixels = a.as_bytes().len() / 4;
    for (p, q) in a
        .as_bytes()
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_bytes().as_chunks::<4>().0)
    {
        let mut worst = 0;
        for c in 0..4 {
            let d = p[c].abs_diff(q[c]);
            sum += d as u64;
            worst = worst.max(d);
        }
        if worst > 64 {
            bad += 1;
        }
    }
    Diff {
        mean: sum as f64 / (pixels * 4) as f64,
        bad: bad as f64 / pixels as f64,
    }
}

/// Draw `scene` both ways at `scale` and check they agree: a mean channel
/// difference under `mean`, and under `bad` of the pixels off by much (edges
/// can land a pixel apart).
fn agree(name: &str, scene: &Scene, scale: f64, mean: f64, bad: f64) -> Option<Diff> {
    let bg = BACKGROUND;
    let gpu = gpu(scene, bg, scale)?;
    let cpu = SoftwareRenderer::new()
        .background(bg)
        .render(scene.clone(), ScaleFactor::new(scale));
    dump(name, "gpu", &gpu);
    dump(name, "cpu", &cpu);
    let d = diff(&gpu, &cpu);
    eprintln!("{name}: {d:?}");
    assert!(
        d.mean < mean && d.bad < bad,
        "{name}: the GPU frame differs from the software one: {d:?}"
    );
    Some(d)
}

#[test]
fn boxes_match_the_software_renderer() {
    agree("boxes", &boxes(), 1.0, 0.5, 0.004);
    agree("boxes@2", &boxes(), 2.0, 0.5, 0.004);
    agree("boxes@1.5", &boxes(), 1.5, 0.5, 0.004);
}

#[test]
fn paths_match_the_software_renderer() {
    agree("paths", &paths(), 1.0, 0.6, 0.006);
    agree("paths@2", &paths(), 2.0, 0.6, 0.006);
}

#[test]
fn images_match_the_software_renderer() {
    agree("images", &images(), 1.0, 1.0, 0.01);
    agree("images@2", &images(), 2.0, 1.0, 0.01);
}

#[test]
fn regions_match_the_software_renderer() {
    agree("regions", &regions(), 1.0, 1.5, 0.02);
    agree("regions@2", &regions(), 2.0, 1.5, 0.02);
}

#[test]
fn nine_slices_match_the_software_renderer() {
    agree("nine-slices", &nine_slices(), 1.0, 1.5, 0.02);
    agree("nine-slices@2", &nine_slices(), 2.0, 1.5, 0.02);
    agree("nine-slices@1.5", &nine_slices(), 1.5, 2.0, 0.03);
}

#[test]
fn an_unchanged_image_is_uploaded_once() {
    let scene = images();
    let bg = Color::BLACK;
    if gpu(&scene, bg, 1.0).is_none() {
        return;
    }
    assert!(uploads() >= 1);
    gpu(&scene, bg, 1.0).unwrap();
    assert_eq!(uploads(), 0, "the second frame reuses the texture");
    // A new image (a new `Arc`) is uploaded.
    let mut other = Scene::new(Size::new(160.0, 120.0));
    other.draw_image(Rect::from_xywh(4.0, 4.0, 16.0, 16.0), ramp());
    gpu(&other, bg, 1.0).unwrap();
    assert_eq!(uploads(), 1);
}

#[test]
fn layers_match_the_software_renderer() {
    agree("layers", &layers(), 1.0, 0.8, 0.01);
    agree("layers@2", &layers(), 2.0, 0.8, 0.01);
}

#[test]
fn clips_match_the_software_renderer() {
    agree("clips", &clipped(), 1.0, 0.5, 0.005);
    agree("clips@2", &clipped(), 2.0, 0.5, 0.005);
}

#[test]
fn text_matches_the_software_renderer() {
    let Some(font) = Font::system_default() else {
        eprintln!("skipping: no system font found");
        return;
    };
    let s = text(&font);
    agree("text", &s, 1.0, 1.0, 0.01);
    agree("text@2", &s, 2.0, 1.0, 0.01);
}

#[test]
fn text_effects_match_the_software_renderer() {
    let Some(font) = Font::system_default() else {
        eprintln!("skipping: no system font found");
        return;
    };
    let s = text_effects(&font);
    agree("text-effects", &s, 1.0, 1.5, 0.02);
    agree("text-effects@2", &s, 2.0, 1.5, 0.02);
}

#[test]
fn canvases_match_the_software_renderer() {
    let s = canvas();
    agree("canvas", &s, 1.0, 0.5, 0.005);
}

/// A window's partial redraw: a frame drawn whole, then the next redrawn
/// only in a region over it (the executor's target keeps the first), against
/// the software renderer's two frames put together the same way.
#[test]
fn a_partial_redraw_matches_the_software_renderer() {
    let (before, after, region) = partial();
    let bg = BACKGROUND;
    for scale in [1.0, 2.0] {
        let name = format!("partial@{scale}");
        if gpu(&before, bg, scale).is_none() {
            return;
        }
        let r = device_region(region, scale);
        let got = with_default(|g| g.render_partial(&after, bg, ScaleFactor::new(scale), r))
            .expect("a partial frame");
        let cpu = |s: &Scene| {
            SoftwareRenderer::new()
                .background(bg)
                .render(s.clone(), ScaleFactor::new(scale))
        };
        let want = composite(&cpu(&before), &cpu(&after), r);
        dump(&name, "gpu", &got);
        dump(&name, "cpu", &want);
        let d = diff(&got, &want);
        eprintln!("{name}: {d:?}");
        assert!(
            d.mean < 0.8 && d.bad < 0.01,
            "{name}: the partial GPU frame differs from the software one: {d:?}"
        );
        // And it isn't the whole `after` frame: what's outside was kept.
        let whole = diff(&got, &cpu(&after));
        assert!(
            whole.mean > 2.0,
            "{name}: the frame was redrawn whole: {whole:?}"
        );
    }
}

/// The whole App path: `App::render_with` the GPU renderer, against the
/// software default, for a widget tree with text, boxes and an image (the
/// path OS-drawn controls take under the native theme).
#[test]
fn an_app_renders_the_same_through_render_with() {
    use crate::prelude::*;
    fn view(_: &(), cx: &mut Cx<()>) -> Element {
        column(vec![
            panel(cx)
                .children(vec![
                    heading(cx, "Settings"),
                    label(cx, "Rendered on the GPU"),
                    image(ramp()).width(48.0).height(48.0),
                    button(cx, "OK"),
                ])
                .width(220.0),
        ])
        .grow(1.0)
        .align(Align::Center, Align::Center)
    }
    let make = |scale: f64| {
        let mut app = App::new((), view)
            .theme(Theme::dark())
            .logical_size(Size::new(300.0, 200.0))
            .scale(ScaleFactor::new(scale));
        if let Some(font) = Font::system_default() {
            app = app.font(font);
        }
        app
    };
    for scale in [1.0, 2.0] {
        if gpu(&Scene::new(Size::new(1.0, 1.0)), Color::BLACK, 1.0).is_none() {
            return;
        }
        let cpu = make(scale).render_once();
        let gpu_frame = make(scale)
            .render_with(|scene: &Scene, bg, scale| render_scene_scaled(scene, bg, scale).unwrap())
            .render_once();
        dump(&format!("app@{scale}"), "gpu", &gpu_frame);
        dump(&format!("app@{scale}"), "cpu", &cpu);
        let d = diff(&gpu_frame, &cpu);
        eprintln!("app@{scale}: {d:?}");
        assert!(d.mean < 1.0 && d.bad < 0.01, "app@{scale}: {d:?}");
    }
}

/// `App::renderer(Renderer::Gpu)`: the app draws on the GPU, and when the
/// device is lost it draws that frame on the CPU and sets the GPU up again.
#[test]
fn an_app_on_the_gpu_survives_losing_the_device() {
    use crate::prelude::*;
    use crate::render::Renderer;
    if gpu(&Scene::new(Size::new(1.0, 1.0)), Color::BLACK, 1.0).is_none() {
        return;
    }
    fn view(_: &(), cx: &mut Cx<()>) -> Element {
        panel(cx)
            .children(vec![
                heading(cx, "On the GPU"),
                image(ramp()).width(32.0).height(32.0),
                button(cx, "OK"),
            ])
            .width(200.0)
    }
    let make = || {
        let mut app = App::new((), view)
            .theme(Theme::dark())
            .logical_size(Size::new(240.0, 160.0))
            .scale(ScaleFactor::new(2.0));
        if let Some(font) = Font::system_default() {
            app = app.font(font);
        }
        app
    };
    crate::app_gpu::reset(None);
    let cpu = make().render_once();
    let mut app = make().renderer(Renderer::Gpu);
    let first = app.render_once();
    assert!(crate::app_gpu::current(|r| r.is_some()), "a GPU renderer");
    dump("app-renderer", "gpu", &first);
    let d = diff(&first, &cpu);
    eprintln!("app-renderer: {d:?}");
    assert!(d.mean < 1.0 && d.bad < 0.01, "{d:?}");

    crate::app_gpu::current(|r| r.unwrap().lose_device());
    assert_eq!(app.render_once(), cpu, "the lost frame is the CPU's");
    let again = app.render_once();
    assert!(crate::app_gpu::current(|r| r.is_some()), "set up again");
    let d = diff(&again, &cpu);
    assert!(d.mean < 1.0 && d.bad < 0.01, "{d:?}");
    crate::app_gpu::reset(None);
}

/// Frame times, CPU against GPU (`--ignored`; build with `--release`).
#[test]
#[ignore]
fn bench_cpu_against_gpu() {
    use std::time::Instant;
    let Some(font) = Font::system_default() else {
        return;
    };
    let scene = widgets(font);
    let sprites = sprites();
    let bg = Color::rgb(0x18, 0x1b, 0x22);
    let time = |f: &mut dyn FnMut()| {
        f();
        let n = 20;
        let t = Instant::now();
        for _ in 0..n {
            f();
        }
        t.elapsed().as_secs_f64() * 1000.0 / n as f64
    };
    for (name, scene, scale) in [
        ("widgets 2560x1600", &scene, 2.0),
        ("sprites 1920x1080", &sprites, 1.0),
    ] {
        let s = ScaleFactor::new(scale);
        let cpu = time(&mut || {
            SoftwareRenderer::new()
                .background(bg)
                .render(scene.clone(), s);
        });
        let read = time(&mut || {
            render_scene_scaled(scene, bg, s).unwrap();
        });
        let gpu = time(&mut || {
            with_default(|r| r.finish(scene, bg, s, None)).unwrap();
        });
        // A hover's worth of change (a 120x40 button, logical): only that
        // redrawn over the last frame, as a window's next frame is.
        let r = device_region(Rect::from_xywh(200.0, 300.0, 120.0, 40.0), scale);
        let partial = time(&mut || {
            with_default(|g| g.finish(scene, bg, s, Some(r))).unwrap();
        });
        eprintln!(
            "{name}: CPU {cpu:.2} ms, GPU {gpu:.2} ms (with read-back {read:.2} ms, \
             a 120x40 change redrawn alone {partial:.2} ms) per frame"
        );
    }
}
