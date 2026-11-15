//! The GPU scene renderer against the software renderer: each scene is drawn
//! both ways and the frames compared pixel by pixel.
//!
//! They run on the platform's GPU API, with the same scenes and tolerances:
//! Metal on macOS (feature `mtl`), GLES2 on Linux (feature `gl`; Mesa's
//! llvmpipe does, through surfaceless EGL), Direct3D 11 on Windows (feature
//! `d3d`; WARP does, on CI's runner). Without a device they're skipped,
//! unless `STIPPLE_REQUIRE_GPU` is set (as CI's GPU jobs do), when they fail.
//! `STIPPLE_GPU_DUMP=<dir>` writes both frames of each
//! comparison there as PNGs.

use super::*;
use crate::geometry::{Affine, Point, Size, Vec2};
use crate::render::{Path as SPath, SoftwareRenderer, StrokeStyle};
use std::sync::Arc;

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
    let bg = Color::rgb(0x18, 0x1b, 0x22);
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

fn rotate_about(radians: f64, c: Point) -> Affine {
    Affine::translate(Vec2::new(-c.x, -c.y))
        .then(Affine::rotate(radians))
        .then(Affine::translate(Vec2::new(c.x, c.y)))
}

/// A 16×16 image: a color ramp with a transparent corner and a half-alpha band.
fn ramp() -> Arc<Pixmap> {
    let mut data = Vec::new();
    for y in 0..16u32 {
        for x in 0..16u32 {
            let a = if x + y < 6 {
                0
            } else if y == 8 {
                128
            } else {
                255
            };
            data.extend_from_slice(&[(x * 16) as u8, (y * 16) as u8, 200, a]);
        }
    }
    Arc::new(Pixmap::from_rgba8(PhysicalSize::new(16, 16), data))
}

fn boxes() -> Scene {
    let mut s = Scene::new(Size::new(160.0, 120.0));
    s.fill_rect(
        Rect::from_xywh(4.0, 4.0, 60.0, 30.0),
        Color::rgb(0xef, 0x68, 0x68),
    );
    s.fill_rect(
        Rect::from_xywh(10.3, 40.6, 50.2, 20.5),
        Color::rgba(0x34, 0xd3, 0x99, 0xa0),
    );
    s.fill_round_rect(
        Rect::from_xywh(70.0, 6.0, 80.0, 40.0),
        10.0,
        Color::rgb(0x60, 0x9c, 0xff),
    );
    s.stroke_round_rect(
        Rect::from_xywh(70.5, 50.5, 80.0, 30.0),
        8.0,
        Color::WHITE,
        1.5,
    );
    s.stroke_rect(
        Rect::from_xywh(8.0, 70.0, 50.0, 40.0),
        Color::rgb(0xf5, 0x9e, 0x0b),
        3.0,
    );
    s.fill_round_rect(
        Rect::from_xywh(100.0, 88.0, 50.0, 24.0),
        12.0,
        Color::rgba(255, 255, 255, 0x60),
    );
    s
}

#[test]
fn boxes_match_the_software_renderer() {
    agree("boxes", &boxes(), 1.0, 0.5, 0.004);
    agree("boxes@2", &boxes(), 2.0, 0.5, 0.004);
    agree("boxes@1.5", &boxes(), 1.5, 0.5, 0.004);
}

fn paths() -> Scene {
    let mut s = Scene::new(Size::new(160.0, 120.0));
    let mut curve = SPath::new();
    curve
        .move_to(Point::new(10.0, 10.0))
        .cubic_to(
            Point::new(40.0, 100.0),
            Point::new(70.0, -20.0),
            Point::new(90.0, 60.0),
        )
        .line_to(Point::new(20.0, 70.0))
        .close();
    s.fill_path(&curve, Color::rgb(0x60, 0x9c, 0xff));
    // A five-pointed star: its middle is wound twice (filled under non-zero).
    let mut star = SPath::new();
    for i in 0..5 {
        let a = std::f64::consts::PI * (0.5 + 0.8 * i as f64);
        let p = Point::new(120.0 + 30.0 * a.cos(), 40.0 - 30.0 * a.sin());
        if i == 0 {
            star.move_to(p);
        } else {
            star.line_to(p);
        }
    }
    star.close();
    s.fill_path(&star, Color::rgb(0xf5, 0x9e, 0x0b));
    s.stroke_path(
        &SPath::line(Point::new(10.0, 100.0), Point::new(150.0, 90.0)),
        Color::WHITE,
        &StrokeStyle::new(3.0).dashed(&[8.0, 5.0]),
    );
    let mut zig = SPath::new();
    zig.move_to(Point::new(10.0, 80.0))
        .line_to(Point::new(40.0, 110.0))
        .line_to(Point::new(70.0, 80.0))
        .quad_to(Point::new(90.0, 110.0), Point::new(110.0, 80.0));
    s.stroke_path(
        &zig,
        Color::rgba(0xef, 0x68, 0x68, 0xc0),
        &StrokeStyle::new(4.0).join(crate::render::LineJoin::Round),
    );
    s.draw_line(
        Point::new(5.0, 5.0),
        Point::new(155.0, 115.0),
        Color::WHITE,
        1.0,
    );
    s
}

#[test]
fn paths_match_the_software_renderer() {
    agree("paths", &paths(), 1.0, 0.6, 0.006);
    agree("paths@2", &paths(), 2.0, 0.6, 0.006);
}

fn images() -> Scene {
    let image = ramp();
    let mut s = Scene::new(Size::new(160.0, 120.0));
    s.fill_rect(
        Rect::from_xywh(0.0, 60.0, 160.0, 60.0),
        Color::rgb(0x40, 0x40, 0x40),
    );
    s.draw_image(Rect::from_xywh(4.0, 4.0, 16.0, 16.0), image.clone());
    s.draw_image(Rect::from_xywh(30.0, 4.0, 64.0, 48.0), image.clone());
    s.draw_image(Rect::from_xywh(100.5, 10.25, 8.0, 8.0), image.clone());
    s.draw_image(Rect::from_xywh(110.0, 64.0, 40.0, 40.0), image);
    s
}

#[test]
fn images_match_the_software_renderer() {
    agree("images", &images(), 1.0, 1.0, 0.01);
    agree("images@2", &images(), 2.0, 1.0, 0.01);
}

/// Image regions: unscaled, scaled nearest and linear, flipped, turned,
/// tinted and faded.
fn regions() -> Scene {
    use crate::render::ImageOptions;
    let image = ramp();
    let mut s = Scene::new(Size::new(160.0, 120.0));
    s.fill_rect(
        Rect::from_xywh(0.0, 60.0, 160.0, 60.0),
        Color::rgb(0x40, 0x40, 0x40),
    );
    let quarter = Rect::from_xywh(8.0, 0.0, 8.0, 8.0);
    let o = ImageOptions::new();
    s.draw_image_region(quarter, Rect::from_xywh(4.0, 4.0, 8.0, 8.0), &image, &o);
    let big = Rect::from_xywh(20.0, 4.0, 32.0, 32.0);
    s.draw_image_region(quarter, big, &image, &o.nearest().flip_x(true));
    let lin = Rect::from_xywh(60.0, 4.0, 40.0, 40.0);
    s.draw_image_region(quarter, lin, &image, &o.flip_y(true));
    let turned = o.rotate(0.6).tint(Color::rgb(255, 160, 60)).opacity(0.7);
    s.draw_image_with(Rect::from_xywh(100.0, 60.0, 40.0, 40.0), &image, &turned);
    s
}

#[test]
fn regions_match_the_software_renderer() {
    agree("regions", &regions(), 1.0, 1.5, 0.02);
    agree("regions@2", &regions(), 2.0, 1.5, 0.02);
}

/// Nine-slices: stretched, tiled and rounded edges, a hollow one, scaled
/// pixel art, one from part of an image, tinted and faded.
fn nine_slices() -> Scene {
    use crate::geometry::Insets;
    use crate::render::{NineSlice, SliceFill};
    let image = ramp();
    let mut s = Scene::new(Size::new(160.0, 120.0));
    let frame = NineSlice::new(image.clone(), Insets::uniform(4.0));
    s.draw_nine_slice(Rect::from_xywh(4.0, 4.0, 70.0, 30.0), &frame);
    let tiled = frame.clone().edges(SliceFill::Tile).center(SliceFill::Tile);
    s.draw_nine_slice(Rect::from_xywh(84.0, 4.0, 70.0, 30.0), &tiled);
    let round = frame
        .clone()
        .edges(SliceFill::Round)
        .center(SliceFill::Hidden);
    s.draw_nine_slice(Rect::from_xywh(4.0, 40.0, 50.0, 35.0), &round);
    let pixel = frame.clone().scale(2.0).nearest();
    s.draw_nine_slice(Rect::from_xywh(60.0, 40.0, 90.0, 40.0), &pixel);
    let part = NineSlice::new(image, Insets::uniform(2.0))
        .region(Rect::from_xywh(8.0, 0.0, 8.0, 8.0))
        .tint(Color::rgb(255, 180, 90))
        .opacity(0.8);
    s.draw_nine_slice(Rect::from_xywh(4.0, 84.0, 150.0, 30.0), &part);
    s
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

fn layers() -> Scene {
    let mut s = Scene::new(Size::new(160.0, 120.0));
    s.fill_rect(
        Rect::from_xywh(0.0, 0.0, 80.0, 120.0),
        Color::rgb(0x30, 0x60, 0x90),
    );
    // Group opacity: the two overlapping boxes blend once, as a unit.
    s.push_layer(Affine::IDENTITY, 0.5);
    s.fill_rect(
        Rect::from_xywh(10.0, 10.0, 50.0, 40.0),
        Color::rgb(0xef, 0x68, 0x68),
    );
    s.fill_round_rect(
        Rect::from_xywh(30.0, 25.0, 50.0, 40.0),
        8.0,
        Color::rgb(0x34, 0xd3, 0x99),
    );
    s.pop_layer();
    // A rotated, translucent card with a clip and an image inside.
    s.push_layer(rotate_about(0.35, Point::new(115.0, 70.0)), 0.8);
    s.fill_round_rect(
        Rect::from_xywh(90.0, 45.0, 50.0, 50.0),
        6.0,
        Color::rgb(0xf5, 0x9e, 0x0b),
    );
    s.push_clip(Rect::from_xywh(95.0, 50.0, 40.0, 20.0));
    s.draw_image(Rect::from_xywh(95.0, 50.0, 40.0, 40.0), ramp());
    s.pop_clip();
    s.stroke_rect(Rect::from_xywh(95.0, 75.0, 40.0, 15.0), Color::WHITE, 2.0);
    s.pop_layer();
    // A scaled layer nested in a translated one.
    s.push_layer(Affine::translate(Vec2::new(10.0, 80.0)), 1.0);
    s.push_layer(Affine::scale(1.5, 1.5), 0.6);
    s.fill_rect(Rect::from_xywh(0.0, 0.0, 20.0, 15.0), Color::WHITE);
    s.pop_layer();
    s.pop_layer();
    s
}

#[test]
fn layers_match_the_software_renderer() {
    agree("layers", &layers(), 1.0, 0.8, 0.01);
    agree("layers@2", &layers(), 2.0, 0.8, 0.01);
}

fn clipped() -> Scene {
    let mut s = Scene::new(Size::new(160.0, 120.0));
    s.push_clip(Rect::from_xywh(10.0, 10.0, 100.0, 80.0));
    s.fill_rect(
        Rect::from_xywh(0.0, 0.0, 160.0, 120.0),
        Color::rgb(0x30, 0x30, 0x50),
    );
    s.push_clip(Rect::from_xywh(40.0, 30.0, 100.0, 30.0));
    s.fill_round_rect(
        Rect::from_xywh(20.0, 20.0, 120.0, 60.0),
        20.0,
        Color::rgb(0x60, 0x9c, 0xff),
    );
    s.fill_path(
        &{
            let mut p = SPath::new();
            p.move_to(Point::new(30.0, 25.0))
                .line_to(Point::new(150.0, 40.0))
                .line_to(Point::new(60.0, 70.0))
                .close();
            p
        },
        Color::rgba(255, 255, 255, 0x90),
    );
    s.pop_clip();
    s.pop_clip();
    s
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
    let mut s = Scene::new(Size::new(220.0, 100.0));
    s.fill_text(
        "Stipple on the GPU",
        Point::new(6.0, 4.0),
        &crate::render::TextPaint::new(&font, 16.0, Color::WHITE),
    );
    s.fill_text(
        "Kerning: AVAWAY",
        Point::new(6.3, 28.4),
        &crate::render::TextPaint::new(&font, 13.0, Color::rgb(0x60, 0x9c, 0xff)),
    );
    s.fill_text(
        "Big",
        Point::new(120.0, 40.0),
        &crate::render::TextPaint::new(&font, 40.0, Color::rgb(0xef, 0x68, 0x68)),
    );
    s.push_layer(rotate_about(-0.2, Point::new(50.0, 75.0)), 1.0);
    s.fill_text(
        "rotated",
        Point::new(10.0, 65.0),
        &crate::render::TextPaint::new(&font, 18.0, Color::WHITE),
    );
    s.pop_layer();
    agree("text", &s, 1.0, 1.0, 0.01);
    agree("text@2", &s, 2.0, 1.0, 0.01);
}

#[test]
fn text_effects_match_the_software_renderer() {
    use crate::render::{RichText, SpanStyle, TextOutline, TextPaint, TextShadow};
    let Some(font) = Font::system_default() else {
        eprintln!("skipping: no system font found");
        return;
    };
    let mut s = Scene::new(Size::new(220.0, 100.0));
    let art = crate::geometry::Rect::from_xywh(0.0, 0.0, 220.0, 100.0);
    s.fill_rect(art, Color::rgb(0xf4, 0xa2, 0x61));
    let text = RichText::new().push("Gold ").styled(
        "120",
        SpanStyle::new().color(Color::rgb(0xff, 0xd7, 0x00)).bold(),
    );
    let paint = TextPaint::new(&font, 28.0, Color::WHITE)
        .outline(TextOutline::new(2.0, Color::BLACK))
        .shadow(TextShadow::new(Vec2::new(2.0, 3.0), Color::rgba(0, 0, 0, 140)).blur(2.0));
    s.fill_rich_text(&text, Point::new(8.0, 8.0), &paint);
    agree("text-effects", &s, 1.0, 1.5, 0.02);
    agree("text-effects@2", &s, 2.0, 1.5, 0.02);
}

#[test]
fn canvases_match_the_software_renderer() {
    let mut s = Scene::new(Size::new(160.0, 120.0));
    let paint: crate::render::CanvasPaint =
        std::rc::Rc::new(|c: &mut crate::render::Canvas<'_>| {
            let size = c.size();
            c.fill_rect(
                Rect::from_xywh(0.0, 0.0, size.width, size.height),
                Color::rgb(0x22, 0x44, 0x22),
            );
            c.draw_line(
                Point::new(0.0, 0.0),
                Point::new(size.width, size.height),
                Color::WHITE,
                2.0,
            );
            // Past the canvas's bounds: clipped.
            c.fill_round_rect(
                Rect::from_xywh(60.0, 40.0, 100.0, 100.0),
                10.0,
                Color::rgb(0xef, 0x68, 0x68),
            );
        });
    s.draw_canvas(Rect::from_xywh(20.0, 20.0, 100.0, 70.0), &paint);
    agree("canvas", &s, 1.0, 0.5, 0.005);
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
    use crate::prelude::*;
    use std::time::Instant;
    let Some(font) = Font::system_default() else {
        return;
    };
    // A busy widget window: text, boxes, rounded rects, borders.
    fn widgets(_: &(), cx: &mut Cx<()>) -> Element {
        let rows: Vec<Element> = (0..24)
            .map(|i| {
                row(vec![
                    checkbox(cx, format!("Item {i}"), i % 3 == 0, |_| {}),
                    label(cx, "A line of text that says something"),
                    button(cx, "Edit"),
                ])
                .gap(8.0)
            })
            .collect();
        column(vec![heading(cx, "Widgets"), column(rows).gap(4.0)])
            .gap(8.0)
            .padding(crate::geometry::Insets::uniform(16.0))
    }
    let mut app = App::new((), widgets)
        .theme(Theme::dark())
        .font(font)
        .logical_size(Size::new(1280.0, 800.0))
        .scale(ScaleFactor::new(2.0));
    app.render_once();
    let scene = app.panes[0].paint_scene(&app.theme, app.font.get());
    // ~2000 sprites over a background, as `spritedemo --bench` draws.
    let sheet = ramp();
    let mut sprites = Scene::new(Size::new(1920.0, 1080.0));
    sprites.fill_rect(
        Rect::from_xywh(0.0, 0.0, 1920.0, 1080.0),
        Color::rgb(30, 40, 80),
    );
    for i in 0..2000 {
        let (x, y) = ((i * 37 % 1888) as f64, (i * 53 % 1048) as f64);
        sprites.draw_image(Rect::from_xywh(x, y, 32.0, 32.0), sheet.clone());
    }
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
            with_default(|r| r.finish(scene, bg, s)).unwrap();
        });
        eprintln!(
            "{name}: CPU {cpu:.2} ms, GPU {gpu:.2} ms (with read-back {read:.2} ms) per frame"
        );
    }
}
