//! The GPU scene renderer against the software renderer: each scene is drawn
//! both ways and the frames compared pixel by pixel.
//!
//! They run on the platform's GPU API, with the same scenes and tolerances:
//! Metal on macOS and iOS (feature `mtl`), Vulkan on Linux (feature `vk`;
//! Mesa's lavapipe in CI), on Android or on macOS over MoltenVK, GLES2 on
//! Linux (feature `gl`; Mesa's llvmpipe, through surfaceless EGL) and
//! Android, Direct3D 11 on Windows (feature `d3d`; WARP does, on CI's
//! runner). On iOS and Android the test executable runs in the simulator or
//! on the device: `tools/gpu-mobile.sh`. Without a device they're skipped,
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
fn a_text_page_matches_the_software_renderer() {
    let Some(font) = Font::system_default() else {
        eprintln!("skipping: no system font found");
        return;
    };
    let s = text_page(&font);
    agree("text-page", &s, 1.0, 1.0, 0.01);
    agree("text-page@2", &s, 2.0, 1.0, 0.01);
    // Its glyphs are in the atlas: a frame drawn again uploads nothing and
    // draws in a single call.
    gpu(&s, BACKGROUND, 2.0).unwrap();
    assert_eq!(uploads(), 0);
    let draws = with_default(|r| Ok(r.draws())).unwrap_or(0);
    assert!(draws <= 1, "{draws} draw calls");
}

/// A terminal's grid of glyph runs: from the glyph atlas on the GPU, from
/// coverage masks on the CPU.
#[test]
fn a_glyph_grid_matches_the_software_renderer() {
    let Some(font) = Font::system_default() else {
        eprintln!("skipping: no system font found");
        return;
    };
    let s = glyph_grid(&font);
    agree("glyph-grid", &s, 1.0, 1.0, 0.01);
    agree("glyph-grid@2", &s, 2.0, 1.0, 0.01);
    agree("glyph-grid@1.5", &s, 1.5, 1.0, 0.01);
}

#[test]
fn a_sprite_field_matches_the_software_renderer() {
    let s = sprite_field();
    agree("sprite-field", &s, 1.0, 1.5, 0.02);
    agree("sprite-field@2", &s, 2.0, 1.5, 0.02);
}

/// A frame drawn at a low resolution and scaled up (`PixelViewport`): drawn
/// into a resampled layer on the GPU, so after the first frame (the sprite
/// sheet's upload) nothing is uploaded, where the bitmap the software
/// renderer makes of it would be a new texture every frame.
#[test]
fn a_pixel_viewport_matches_the_software_renderer_without_uploads() {
    let sheet = ramp();
    let s = pixel_viewport_with(&sheet);
    if agree("pixel-viewport", &s, 1.0, 0.8, 0.01).is_none() {
        return;
    }
    agree("pixel-viewport@2", &s, 2.0, 0.8, 0.01);
    // A new frame of it from the same sheet, as a game paints each frame.
    gpu(&pixel_viewport_with(&sheet), BACKGROUND, 2.0).unwrap();
    assert_eq!(uploads(), 0, "the low-resolution frame stays on the GPU");
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

/// A canvas kept as a texture, a band of it changed: only the band is
/// rasterized and uploaded into the texture it had, and the frame matches
/// the software renderer's of the new content.
#[test]
fn a_canvas_texture_updated_in_place_matches_the_software_renderer() {
    let (before, after, changed) = canvas_update();
    if gpu(&before, BACKGROUND, 1.0).is_none() {
        return;
    }
    let owner = u64::MAX - 1;
    let frame = with_default(|r| {
        r.next_frame(owner, None);
        r.render(&before, BACKGROUND, ScaleFactor::IDENTITY)?;
        r.next_frame(owner, Some(vec![changed]));
        let p = r.render(&after, BACKGROUND, ScaleFactor::IDENTITY)?;
        assert_eq!(r.uploads(), 0, "updated in place, not uploaded anew");
        assert_eq!(r.canvas_bytes().0, 140 * 20 * 4, "just the band");
        Ok(p)
    })
    .unwrap();
    let cpu = SoftwareRenderer::new()
        .background(BACKGROUND)
        .render(after, ScaleFactor::IDENTITY);
    dump("canvas-update", "gpu", &frame);
    dump("canvas-update", "cpu", &cpu);
    let d = diff(&frame, &cpu);
    eprintln!("canvas-update: {d:?}");
    assert!(d.mean < 0.5 && d.bad < 0.005, "{d:?}");
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

/// The native theme on the GPU: a screen of every control under
/// `Theme::native()`, through `App::render_with` the GPU renderer, against
/// the software default. The bitmaps the OS draws (uxtheme on Windows, which
/// draws on any thread) reach the GPU as images, cached as textures by their
/// `Arc`, which the OS-drawn bitmap cache keeps the same from frame to frame:
/// a second frame uploads none of them again, and once the cache is dropped
/// (the system's look changed, the window moved to another scale) the next
/// frame uploads the new ones. AppKit and UIKit only draw on the main thread,
/// which a test isn't on: there `nativegallery --gpu-check` compares the same
/// way, and here the controls are self-drawn. Linux has no OS-drawn controls.
#[test]
fn a_native_themed_screen_matches_the_software_renderer() {
    use crate::prelude::*;
    fn view(_: &(), cx: &mut Cx<()>) -> Element {
        let theme = *cx.theme();
        column(vec![
            row(vec![
                button(cx, "Save"),
                Button::new("Cancel").variant(Variant::Secondary).view(cx),
            ])
            .gap(theme.spacing.sm),
            checkbox(cx, "Show hidden files", true, |_| {}),
            checkbox(cx, "Compress", false, |_| {}),
            row(vec![
                radio(cx, "Small", true, |_| {}),
                radio(cx, "Large", false, |_| {}),
            ])
            .gap(theme.spacing.lg),
            switch(cx, "Sync", true, |_| {}),
            slider(cx, 0.6, |_, _| {}),
            progress_bar(cx, 0.4),
        ])
        .gap(theme.spacing.md)
        .padding(Insets::uniform(theme.spacing.lg))
        .align(Align::Start, Align::Start)
    }
    if gpu(&Scene::new(Size::new(1.0, 1.0)), Color::BLACK, 1.0).is_none() {
        return;
    }
    // A switch's first frame is the first step of its animation; reduced
    // motion shows where it ends up, on both paths alike.
    crate::anim::set_reduced_motion(Some(true));
    let make = |theme: Theme, scale: f64| {
        let mut app = App::new((), view)
            .theme(theme)
            .logical_size(Size::new(320.0, 300.0))
            .scale(ScaleFactor::new(scale));
        if let Some(font) = Font::system_default() {
            app = app.font(font);
        }
        app
    };
    let gpu_frame = |scale: f64| {
        make(Theme::native(), scale)
            .render_with(|scene: &Scene, bg, scale| render_scene_scaled(scene, bg, scale).unwrap())
            .render_once()
    };
    // Whether the OS draws this screen's controls on this thread.
    let os_draws = crate::platform::native::available()
        && cfg!(not(any(target_os = "macos", target_os = "ios")));
    for scale in [1.0, 1.5, 2.0] {
        let cpu = make(Theme::native(), scale).render_once();
        let frame = gpu_frame(scale);
        dump(&format!("native@{scale}"), "gpu", &frame);
        dump(&format!("native@{scale}"), "cpu", &cpu);
        let d = diff(&frame, &cpu);
        eprintln!("native@{scale}: {d:?}");
        assert!(d.mean < 1.0 && d.bad < 0.01, "native@{scale}: {d:?}");
        if os_draws {
            // The OS drew them: self-drawn, the screen looks different.
            let plain = make(Theme::native().native_controls(false), scale).render_once();
            let d = diff(&plain, &cpu);
            assert!(
                d.mean > 0.2,
                "native@{scale}: the OS drew no controls ({d:?})"
            );
            // A second frame finds every bitmap the OS drew already a texture.
            gpu_frame(scale);
            let again = uploads();
            // A new look drops them: the next frame uploads them anew.
            crate::platform::native::clear_cache();
            gpu_frame(scale);
            let fresh = uploads();
            eprintln!("native@{scale}: uploads {again} then {fresh} after the cache was dropped");
            assert!(
                fresh > again,
                "native@{scale}: {again} then {fresh} uploads"
            );
        }
    }
    crate::anim::set_reduced_motion(None);
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

/// A canvas on an app drawn on the GPU is kept as a texture: a row
/// invalidated is all that is rasterized and uploaded again, and every frame
/// still matches the software renderer's.
#[test]
fn an_invalidated_canvas_row_is_all_that_is_uploaded() {
    use crate::core::CanvasId;
    use crate::prelude::*;
    use crate::render::{Renderer, Surface};
    use std::cell::RefCell;
    use std::rc::Rc;
    if gpu(&Scene::new(Size::new(1.0, 1.0)), Color::BLACK, 1.0).is_none() {
        return;
    }
    let Some(font) = Font::system_default() else {
        return;
    };
    const GRID: CanvasId = CanvasId(7);
    const ROW: f64 = 20.0;
    struct Keep(PhysicalSize, Option<Pixmap>);
    impl Surface for Keep {
        fn resize(&mut self, size: PhysicalSize) {
            self.0 = size;
        }
        fn size(&self) -> PhysicalSize {
            self.0
        }
        fn present(&mut self, pixmap: &Pixmap, _: &[Rect]) {
            self.1 = Some(pixmap.clone());
        }
    }
    let make = |renderer: Renderer, rows: Rc<RefCell<Vec<String>>>| {
        let f = font.clone();
        App::new((), move |_: &(), _: &mut Cx<()>| {
            let rows = rows.clone();
            let font = f.clone();
            Element::canvas(GRID, move |c: &mut Canvas| {
                c.fill_rect(
                    Rect::from_xywh(0.0, 0.0, 300.0, 200.0),
                    Color::rgb(20, 24, 30),
                );
                for (i, text) in rows.borrow().iter().enumerate() {
                    let top = i as f64 * ROW;
                    if c.needs_paint(Rect::from_xywh(0.0, top, 300.0, ROW)) {
                        let paint = TextPaint::new(&font, 14.0, Color::rgb(220, 230, 220));
                        c.fill_text(text, Point::new(6.0, top + 15.0), &paint);
                    }
                }
            })
            .width(300.0)
            .height(200.0)
        })
        .theme(Theme::dark())
        .logical_size(Size::new(300.0, 200.0))
        .scale(ScaleFactor::new(2.0))
        .font(font.clone())
        .renderer(renderer)
    };
    let lines = |n: usize| {
        (0..10)
            .map(|i| format!("Line {i} · tick {n}"))
            .collect::<Vec<_>>()
    };
    let cpu_rows = Rc::new(RefCell::new(lines(0)));
    let gpu_rows = Rc::new(RefCell::new(lines(0)));
    crate::app_gpu::reset(None);
    let mut cpu = make(Renderer::Software, cpu_rows.clone());
    let mut gpu_app = make(Renderer::Gpu, gpu_rows.clone());
    let (mut a, mut b) = (
        Keep(PhysicalSize::new(0, 0), None),
        Keep(PhysicalSize::new(0, 0), None),
    );
    for tick in 0..4 {
        if tick > 0 {
            let row = tick * 2;
            for rows in [&cpu_rows, &gpu_rows] {
                rows.borrow_mut()[row] = format!("Row {row} changed at tick {tick}");
            }
            let rect = Rect::from_xywh(0.0, row as f64 * ROW, 300.0, ROW);
            cpu.handle().invalidate_canvas_rect(GRID, rect);
            gpu_app.handle().invalidate_canvas_rect(GRID, rect);
        }
        cpu.present_frame(&mut a);
        gpu_app.present_frame(&mut b);
        let (c, g) = (a.1.as_ref().unwrap(), b.1.as_ref().unwrap());
        dump(&format!("canvas-row-{tick}"), "gpu", g);
        dump(&format!("canvas-row-{tick}"), "cpu", c);
        let d = diff(g, c);
        eprintln!("canvas row, tick {tick}: {d:?}");
        assert!(d.mean < 1.0 && d.bad < 0.01, "tick {tick}: {d:?}");
        let (uploaded, kept) = crate::app_gpu::current(|r| r.unwrap().canvas_bytes());
        // At scale 2, the canvas is 600x400 pixels; a row is 600x40. (Both
        // apps draw on the GPU when `STIPPLE_RENDERER` says so.)
        let apps = kept / (600 * 400 * 4);
        assert!(apps >= 1 && kept == apps * 600 * 400 * 4, "{kept}");
        if tick == 0 {
            assert_eq!(uploaded, 600 * 400 * 4, "the first frame: all of it");
        } else {
            assert_eq!(uploaded, 600 * 40 * 4, "tick {tick}: just the row");
            assert_eq!(crate::app_gpu::current(|r| r.unwrap().uploads()), 0);
        }
    }
    crate::app_gpu::reset(None);
}

/// A terminal's canvas scrolled a line at a time (`scroll_canvas_rect`):
/// its texture moves its pixels, only the new line is painted, and every
/// frame matches the software renderer's (which moves the window's pixels).
#[test]
fn a_scrolled_canvas_matches_the_software_renderer() {
    use crate::core::CanvasId;
    use crate::geometry::Vec2;
    use crate::prelude::*;
    use crate::render::{GlyphRun, Renderer, Surface};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    if gpu(&Scene::new(Size::new(1.0, 1.0)), Color::BLACK, 1.0).is_none() {
        return;
    }
    let Some(font) = Font::system_default() else {
        return;
    };
    const GRID: CanvasId = CanvasId(8);
    const ROW: f64 = 20.0;
    struct Keep(PhysicalSize, Option<Pixmap>);
    impl Surface for Keep {
        fn resize(&mut self, size: PhysicalSize) {
            self.0 = size;
        }
        fn size(&self) -> PhysicalSize {
            self.0
        }
        fn present(&mut self, pixmap: &Pixmap, _: &[Rect]) {
            self.1 = Some(pixmap.clone());
        }
    }
    let make = |renderer: Renderer, rows: Rc<RefCell<Vec<String>>>, painted: Rc<Cell<f64>>| {
        let f = font.clone();
        App::new((), move |_: &(), _: &mut Cx<()>| {
            let (rows, painted) = (rows.clone(), painted.clone());
            let font = f.clone();
            Element::canvas(GRID, move |c: &mut Canvas| {
                let d = c.dirty();
                painted.set(painted.get() + d.width() * d.height());
                c.fill_rect(
                    Rect::from_xywh(0.0, 0.0, 300.0, 200.0),
                    Color::rgb(20, 24, 30),
                );
                for (i, text) in rows.borrow().iter().enumerate() {
                    let top = i as f64 * ROW;
                    if c.needs_paint(Rect::from_xywh(0.0, top, 300.0, ROW)) {
                        let mut run = GlyphRun::new(&font, 14.0, Color::rgb(220, 230, 220));
                        run.push_cells(text, Point::new(6.0, top + 2.0), 8.0);
                        c.fill_glyph_run(&run);
                    }
                }
            })
            .width(300.0)
            .height(200.0)
        })
        .theme(Theme::dark())
        .logical_size(Size::new(300.0, 200.0))
        .scale(ScaleFactor::new(2.0))
        .font(font.clone())
        .renderer(renderer)
    };
    let lines = (0..10).map(|i| format!("$ line {i}")).collect::<Vec<_>>();
    let cpu_rows = Rc::new(RefCell::new(lines.clone()));
    let gpu_rows = Rc::new(RefCell::new(lines));
    let painted = Rc::new(Cell::new(0.0));
    crate::app_gpu::reset(None);
    let mut cpu = make(
        Renderer::Software,
        cpu_rows.clone(),
        Rc::new(Cell::new(0.0)),
    );
    let mut gpu_app = make(Renderer::Gpu, gpu_rows.clone(), painted.clone());
    let (mut a, mut b) = (
        Keep(PhysicalSize::new(0, 0), None),
        Keep(PhysicalSize::new(0, 0), None),
    );
    for tick in 0..4 {
        if tick > 0 {
            for rows in [&cpu_rows, &gpu_rows] {
                let mut rows = rows.borrow_mut();
                rows.remove(0);
                rows.push(format!("$ output at tick {tick}"));
            }
            let whole = Rect::from_xywh(0.0, 0.0, 300.0, 200.0);
            for app in [&cpu, &gpu_app] {
                app.handle()
                    .scroll_canvas_rect(GRID, whole, Vec2::new(0.0, -ROW));
            }
        }
        painted.set(0.0);
        cpu.present_frame(&mut a);
        gpu_app.present_frame(&mut b);
        let (c, g) = (a.1.as_ref().unwrap(), b.1.as_ref().unwrap());
        dump(&format!("canvas-scroll-{tick}"), "gpu", g);
        dump(&format!("canvas-scroll-{tick}"), "cpu", c);
        let d = diff(g, c);
        eprintln!("canvas scroll, tick {tick}: {d:?}");
        assert!(d.mean < 1.0 && d.bad < 0.01, "tick {tick}: {d:?}");
        if tick > 0 {
            assert_eq!(painted.get(), 300.0 * ROW, "tick {tick}: just the new line");
            assert_eq!(crate::app_gpu::current(|r| r.unwrap().uploads()), 0);
        }
    }
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
        // Draw calls: what the executor made, against one per box and
        // image, three per path and one per clip polygon and layer pass
        // (as before batching).
        let draws = with_default(|r| Ok(r.draws())).unwrap_or(0);
        // A hover's worth of change (a 120x40 button, logical): only that
        // redrawn over the last frame, as a window's next frame is.
        let r = device_region(Rect::from_xywh(200.0, 300.0, 120.0, 40.0), scale);
        let partial = time(&mut || {
            with_default(|g| g.finish(scene, bg, s, Some(r))).unwrap();
        });
        let plan = super::ops::plan(
            scene,
            bg,
            s.to_physical(scene.logical_size()),
            &mut super::ops::RasterCache::default(),
        );
        let unbatched: usize = plan
            .ops
            .iter()
            .map(|op| match op {
                super::ops::Op::Path { .. } => 3,
                super::ops::Op::PushClip(super::ops::Clip::Polygon(_)) => 2,
                super::ops::Op::PushClip(_)
                | super::ops::Op::PopClip
                | super::ops::Op::PushLayer { .. } => 0,
                _ => 1,
            })
            .sum();
        eprintln!(
            "{name}: CPU {cpu:.2} ms, GPU {gpu:.2} ms (with read-back {read:.2} ms, \
             a 120x40 change redrawn alone {partial:.2} ms) per frame, \
             {draws} draw calls ({unbatched} unbatched)"
        );
    }
}
