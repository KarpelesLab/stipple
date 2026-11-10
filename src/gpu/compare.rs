//! The GPU scene renderer against the software renderer: each scene is drawn
//! both ways and the frames compared pixel by pixel.
//!
//! These need a GL device (Mesa's llvmpipe does: surfaceless EGL). Without
//! one they're skipped, unless `STIPPLE_REQUIRE_GPU` is set (as CI's GPU job
//! does), when they fail. `STIPPLE_GPU_DUMP=<dir>` writes both frames of each
//! comparison there as PPMs.

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

fn dump(name: &str, which: &str, p: &Pixmap) {
    let Some(dir) = std::env::var_os("STIPPLE_GPU_DUMP") else {
        return;
    };
    let (w, h) = (p.size().width, p.size().height);
    let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
    for px in p.as_bytes().as_chunks::<4>().0 {
        out.extend_from_slice(&px[..3]);
    }
    let path = std::path::Path::new(&dir).join(format!("{name}-{which}.ppm"));
    let _ = std::fs::write(path, out);
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
        .with_background(bg)
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

#[test]
fn an_unchanged_image_is_uploaded_once() {
    let scene = images();
    let bg = Color::BLACK;
    if gpu(&scene, bg, 1.0).is_none() {
        return;
    }
    assert!(gl::last_uploads() >= 1);
    gpu(&scene, bg, 1.0).unwrap();
    assert_eq!(gl::last_uploads(), 0, "the second frame reuses the texture");
    // A new image (a new `Arc`) is uploaded.
    let mut other = Scene::new(Size::new(160.0, 120.0));
    other.draw_image(Rect::from_xywh(4.0, 4.0, 16.0, 16.0), ramp());
    gpu(&other, bg, 1.0).unwrap();
    assert_eq!(gl::last_uploads(), 1);
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
        &font,
        "Stipple on the GPU",
        Point::new(6.0, 4.0),
        16.0,
        Color::WHITE,
    );
    s.fill_text(
        &font,
        "Kerning: AVAWAY",
        Point::new(6.3, 28.4),
        13.0,
        Color::rgb(0x60, 0x9c, 0xff),
    );
    s.fill_text(
        &font,
        "Big",
        Point::new(120.0, 40.0),
        40.0,
        Color::rgb(0xef, 0x68, 0x68),
    );
    s.push_layer(rotate_about(-0.2, Point::new(50.0, 75.0)), 1.0);
    s.fill_text(&font, "rotated", Point::new(10.0, 65.0), 18.0, Color::WHITE);
    s.pop_layer();
    agree("text", &s, 1.0, 1.0, 0.01);
    agree("text@2", &s, 2.0, 1.0, 0.01);
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
        let theme = *cx.theme();
        column(vec![
            panel(
                &theme,
                vec![
                    heading(&theme, "Settings"),
                    label(&theme, "Rendered on the GPU"),
                    image(ramp()).width(48.0).height(48.0),
                    button_labeled(&theme, "OK"),
                ],
            )
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
