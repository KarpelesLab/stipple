//! The scenes the GPU renderers are compared with the software renderer on
//! (`compare` on Metal and GLES2, `examples/webdemo/gpucompare.html` on
//! WebGPU and WebGL2), with the tolerances each is held to.

use crate::geometry::{Affine, PhysicalSize, Point, Rect, Size, Vec2};
use crate::render::{Color, Font, Path as SPath, Pixmap, Scene, StrokeStyle};
use std::sync::Arc;

/// A scene, the scale it's drawn at, and how close the two frames must be:
/// a mean channel difference under `mean` (of 255), and under `bad` of the
/// pixels off by more than 64 in a channel.
pub(crate) struct Case {
    pub name: String,
    pub scene: Scene,
    pub scale: f64,
    pub mean: f64,
    pub bad: f64,
    /// A frame drawn first, as the same window's, and what changed from it
    /// to `scene` (logical rects): a canvas kept as a texture is updated
    /// only there.
    pub before: Option<(Scene, Vec<Rect>)>,
}

/// The background the corpus is drawn on.
pub(crate) const BACKGROUND: Color = Color::rgb(0x18, 0x1b, 0x22);

/// Every case, the text ones only with a `font`.
pub(crate) fn cases(font: Option<&Font>) -> Vec<Case> {
    let mut out = Vec::new();
    let mut add = |name: &str, scene: Scene, scales: &[f64], mean: f64, bad: f64| {
        for &scale in scales {
            let name = if scale == 1.0 {
                name.to_string()
            } else {
                format!("{name}@{scale}")
            };
            out.push(Case {
                name,
                scene: scene.clone(),
                scale,
                mean,
                bad,
                before: None,
            });
        }
    };
    add("boxes", boxes(), &[1.0, 2.0, 1.5], 0.5, 0.004);
    add("paths", paths(), &[1.0, 2.0], 0.6, 0.006);
    add("images", images(), &[1.0, 2.0], 1.0, 0.01);
    add("regions", regions(), &[1.0, 2.0], 1.5, 0.02);
    add("nine-slices", nine_slices(), &[1.0, 2.0], 1.5, 0.02);
    add("nine-slices", nine_slices(), &[1.5], 2.0, 0.03);
    add("layers", layers(), &[1.0, 2.0], 0.8, 0.01);
    add("clips", clipped(), &[1.0, 2.0], 0.5, 0.005);
    add("canvas", canvas(), &[1.0], 0.5, 0.005);
    add("sprite-field", sprite_field(), &[1.0, 2.0], 1.5, 0.02);
    add("pixel-viewport", pixel_viewport(), &[1.0, 2.0], 0.8, 0.01);
    if let Some(font) = font {
        add("text", text(font), &[1.0, 2.0], 1.0, 0.01);
        add("text-effects", text_effects(font), &[1.0, 2.0], 1.5, 0.02);
        add("text-page", text_page(font), &[1.0, 2.0], 1.0, 0.01);
        add("glyph-grid", glyph_grid(font), &[1.0, 2.0, 1.5], 1.0, 0.01);
    }
    let (before, after, changed) = canvas_update();
    out.push(Case {
        name: "canvas-update".into(),
        scene: after,
        scale: 1.0,
        mean: 0.5,
        bad: 0.005,
        before: Some((before, vec![changed])),
    });
    out
}

/// The partial-redraw case: a frame drawn whole (`before`), then the next
/// one (`after`) redrawn only in `region` (logical pixels) over it, as a
/// window redraws what changed. The two differ everywhere, so the result
/// shows both that `region` was redrawn as a whole frame would draw it and
/// that the rest was kept: `before` outside, `after` inside (see
/// [`composite`]). `after` has layers, a rotated clip (stencil) and an image
/// crossing the region's edges.
pub(crate) fn partial() -> (Scene, Scene, Rect) {
    (paths(), layers(), Rect::from_xywh(50.0, 30.0, 70.0, 60.0))
}

/// `region` (logical) at `scale`, in whole device pixels.
pub(crate) fn device_region(region: Rect, scale: f64) -> [i32; 4] {
    [
        (region.min_x() * scale).floor() as i32,
        (region.min_y() * scale).floor() as i32,
        (region.max_x() * scale).ceil() as i32,
        (region.max_y() * scale).ceil() as i32,
    ]
}

/// What a partial redraw leaves: `before`, with `after`'s pixels in `r`.
pub(crate) fn composite(before: &Pixmap, after: &Pixmap, r: [i32; 4]) -> Pixmap {
    let mut out = before.clone();
    let w = before.size().width as usize;
    let (src, dst) = (after.as_bytes(), out.as_bytes_mut());
    for y in r[1].max(0) as usize..r[3] as usize {
        let row = (y * w + r[0].max(0) as usize) * 4..(y * w + r[2] as usize) * 4;
        dst[row.clone()].copy_from_slice(&src[row]);
    }
    out
}

pub(crate) fn rotate_about(radians: f64, c: Point) -> Affine {
    Affine::translate(Vec2::new(-c.x, -c.y))
        .then(Affine::rotate(radians))
        .then(Affine::translate(Vec2::new(c.x, c.y)))
}

/// A 16×16 image: a color ramp with a transparent corner and a half-alpha band.
pub(crate) fn ramp() -> Arc<Pixmap> {
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

pub(crate) fn boxes() -> Scene {
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

pub(crate) fn paths() -> Scene {
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

pub(crate) fn images() -> Scene {
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

/// Image regions: unscaled, scaled nearest and linear, flipped, turned,
/// tinted and faded.
pub(crate) fn regions() -> Scene {
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

/// Nine-slices: stretched, tiled and rounded edges, a hollow one, scaled
/// pixel art, one from part of an image, tinted and faded.
pub(crate) fn nine_slices() -> Scene {
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

pub(crate) fn layers() -> Scene {
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

pub(crate) fn clipped() -> Scene {
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

/// Text: kerned, at fractional positions, big, and rotated.
pub(crate) fn text(font: &Font) -> Scene {
    let mut s = Scene::new(Size::new(220.0, 100.0));
    s.fill_text(
        "Stipple on the GPU",
        Point::new(6.0, 4.0),
        &crate::render::TextPaint::new(font, 16.0, Color::WHITE),
    );
    s.fill_text(
        "Kerning: AVAWAY",
        Point::new(6.3, 28.4),
        &crate::render::TextPaint::new(font, 13.0, Color::rgb(0x60, 0x9c, 0xff)),
    );
    s.fill_text(
        "Big",
        Point::new(120.0, 40.0),
        &crate::render::TextPaint::new(font, 40.0, Color::rgb(0xef, 0x68, 0x68)),
    );
    s.push_layer(rotate_about(-0.2, Point::new(50.0, 75.0)), 1.0);
    s.fill_text(
        "rotated",
        Point::new(10.0, 65.0),
        &crate::render::TextPaint::new(font, 18.0, Color::WHITE),
    );
    s.pop_layer();
    s
}

/// A page of text: many lines, sizes and colors over boxes, every glyph
/// from the atlas, the boxes and text batched together.
pub(crate) fn text_page(font: &Font) -> Scene {
    use crate::render::TextPaint;
    let mut s = Scene::new(Size::new(320.0, 240.0));
    s.fill_round_rect(
        Rect::from_xywh(4.0, 4.0, 312.0, 232.0),
        6.0,
        Color::rgb(0x22, 0x26, 0x30),
    );
    let colors = [
        Color::WHITE,
        Color::rgb(0x60, 0x9c, 0xff),
        Color::rgb(0xef, 0x68, 0x68),
        Color::rgb(0x9a, 0xd0, 0x7a),
    ];
    let mut y = 8.0;
    for i in 0..18 {
        let size = 8.0 + (i % 6) as f64 * 2.5;
        if i % 4 == 1 {
            s.fill_rect(
                Rect::from_xywh(8.0, y, 304.0, size * 1.2),
                Color::rgb(0x30, 0x36, 0x44),
            );
        }
        let line = format!("{i:02}: The quick brown fox jumps over the lazy dog 0123456789");
        let x = 10.0 + (i as f64 * 0.37) % 1.0;
        s.fill_text(
            &line,
            Point::new(x, y),
            &TextPaint::new(font, size, colors[i % colors.len()]),
        );
        y += size * 1.25;
    }
    s
}

/// A terminal's grid: glyph runs a cell apart, in several colors over cell
/// backgrounds, bold, italic and undercurled, at a fractional offset, one
/// clipped and one faded.
pub(crate) fn glyph_grid(font: &Font) -> Scene {
    use crate::render::{Decoration, GlyphRun, UnderlineStyle};
    let mut s = Scene::new(Size::new(240.0, 140.0));
    let colors = [
        Color::rgb(0xcc, 0xcc, 0xcc),
        Color::rgb(0x60, 0x9c, 0xff),
        Color::rgb(0xef, 0x68, 0x68),
        Color::rgb(0x9a, 0xd0, 0x7a),
    ];
    let line = "ls -la ~/src | grep 'stipple' && echo $?";
    for row in 0..6 {
        let top = 4.0 + row as f64 * 16.0;
        if row % 2 == 1 {
            s.fill_rect(
                Rect::from_xywh(4.0 + 7.0 * 3.0, top, 7.0 * 9.0, 16.0),
                Color::rgb(0x30, 0x36, 0x44),
            );
        }
        let f = match row % 3 {
            0 => font.clone(),
            1 => font.bold(),
            _ => font.italic(),
        };
        let mut run = GlyphRun::new(&f, 12.0 + (row % 2) as f64, colors[row % colors.len()]);
        if row == 4 {
            run = run.decoration(Decoration::NONE.underline(UnderlineStyle::Curly));
        }
        run.push_cells(line, Point::new(4.0 + row as f64 * 0.3, top), 7.0);
        s.fill_glyph_run(&run);
    }
    s.push_clip(Rect::from_xywh(10.0, 100.0, 100.0, 9.0));
    let mut run = GlyphRun::new(font, 13.0, Color::WHITE);
    run.push_cells("clipped through the middle", Point::new(10.0, 100.0), 7.0);
    s.fill_glyph_run(&run);
    s.pop_clip();
    s.push_layer(Affine::IDENTITY, 0.5);
    let mut run = GlyphRun::new(font, 13.0, Color::rgb(0xff, 0xd7, 0x00));
    run.push_cells("faded", Point::new(130.0, 116.0), 8.0);
    s.fill_glyph_run(&run);
    s.pop_layer();
    s
}

/// Rich text with an outline and a blurred shadow.
pub(crate) fn text_effects(font: &Font) -> Scene {
    use crate::render::{RichText, SpanStyle, TextOutline, TextPaint, TextShadow};
    let mut s = Scene::new(Size::new(220.0, 100.0));
    let art = crate::geometry::Rect::from_xywh(0.0, 0.0, 220.0, 100.0);
    s.fill_rect(art, Color::rgb(0xf4, 0xa2, 0x61));
    let text = RichText::new().push("Gold ").styled(
        "120",
        SpanStyle::new().color(Color::rgb(0xff, 0xd7, 0x00)).bold(),
    );
    let paint = TextPaint::new(font, 28.0, Color::WHITE)
        .outline(TextOutline::new(2.0, Color::BLACK))
        .shadow(TextShadow::new(Vec2::new(2.0, 3.0), Color::rgba(0, 0, 0, 140)).blur(2.0));
    s.fill_rich_text(&text, Point::new(8.0, 8.0), &paint);
    s
}

/// Many sprites from one sheet: scaled nearest and bilinear, flipped,
/// tinted and faded, over a background (batched per texture and filter).
pub(crate) fn sprite_field() -> Scene {
    use crate::render::ImageOptions;
    let sheet = ramp();
    let mut s = Scene::new(Size::new(320.0, 240.0));
    s.fill_rect(
        Rect::from_xywh(0.0, 0.0, 320.0, 240.0),
        Color::rgb(30, 40, 80),
    );
    for i in 0..400 {
        let (x, y) = ((i * 37 % 300) as f64, (i * 53 % 220) as f64 + 0.3);
        let src = Rect::from_xywh((i % 2) as f64 * 8.0, (i / 2 % 2) as f64 * 8.0, 8.0, 8.0);
        let size = 8.0 + (i % 3) as f64 * 8.0;
        let mut o = ImageOptions::new().flip_x(i % 5 == 0).flip_y(i % 7 == 0);
        if i % 2 == 0 {
            o = o.nearest();
        }
        if i % 11 == 0 {
            o = o.tint(Color::rgb(255, 160, 60)).opacity(0.7);
        }
        s.draw_image_region(src, Rect::from_xywh(x, y, size, size), &sheet, &o);
    }
    s
}

/// A canvas, its painting clipped to its bounds.
pub(crate) fn canvas() -> Scene {
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
    s
}

/// A canvas (with an id, so the GPU renderers keep it as a texture) before
/// and after a band of it changed, and the band (in the scene's
/// coordinates).
pub(crate) fn canvas_update() -> (Scene, Scene, Rect) {
    let bounds = Rect::from_xywh(10.0, 10.0, 140.0, 100.0);
    let band = Rect::from_xywh(0.0, 40.0, 140.0, 20.0);
    let frame = |changed: bool| {
        let mut s = Scene::new(Size::new(160.0, 120.0));
        let paint: crate::render::CanvasPaint =
            std::rc::Rc::new(move |c: &mut crate::render::Canvas<'_>| {
                for i in 0..5 {
                    let row = Rect::from_xywh(0.0, i as f64 * 20.0, 140.0, 20.0);
                    let lit = changed && row == band;
                    let color = if lit {
                        Color::rgb(0xef, 0xc0, 0x40)
                    } else {
                        Color::rgb(0x30, 0x40 + i * 0x20, 0x60)
                    };
                    c.fill_rect(row, color);
                    c.fill_round_rect(
                        Rect::from_xywh(8.0 + i as f64 * 20.0, row.min_y() + 4.0, 40.0, 12.0),
                        6.0,
                        if lit { Color::BLACK } else { Color::WHITE },
                    );
                }
            });
        s.draw_canvas_with_id(bounds, &paint, Some(crate::core::CanvasId(1)));
        s
    };
    let changed = band.translate(Vec2::new(bounds.min_x(), bounds.min_y()));
    (frame(false), frame(true), changed)
}

/// A busy widget window at 1280x800 (drawn at scale 2): text, checkboxes,
/// buttons, rounded rects and borders. For timing.
pub(crate) fn widgets(font: Font) -> Scene {
    use crate::prelude::*;
    fn view(_: &(), cx: &mut Cx<()>) -> Element {
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
    let mut app = App::new((), view)
        .theme(Theme::dark())
        .font(font)
        .logical_size(Size::new(1280.0, 800.0))
        .scale(ScaleFactor::new(2.0));
    app.render_once();
    app.panes[0].paint_scene(&app.theme, app.font.get())
}

/// 2000 sprites over a 1920x1080 background, as `spritedemo --bench` draws.
/// For timing.
pub(crate) fn sprites() -> Scene {
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
    sprites
}

/// A frame drawn at 40×30 and scaled up nearest-neighbor by 3 into the
/// scene ([`Scene::draw_offscreen`], what `PixelViewport` draws): fills,
/// sprites flipped and faded, a translucent layer, over bars. On the GPU
/// renderers (the web's too) a resampled layer, no bitmap uploaded.
pub(crate) fn pixel_viewport() -> Scene {
    pixel_viewport_with(&ramp())
}

/// [`pixel_viewport`] drawing from `sheet`.
pub(crate) fn pixel_viewport_with(sheet: &Arc<Pixmap>) -> Scene {
    use crate::render::{Filter, ImageOptions};
    let mut low = Scene::new(Size::new(40.0, 30.0));
    low.fill_rect(
        Rect::from_xywh(0.0, 0.0, 40.0, 30.0),
        Color::rgb(90, 150, 220),
    );
    low.fill_rect(
        Rect::from_xywh(0.0, 22.0, 40.0, 8.0),
        Color::rgb(70, 140, 60),
    );
    let o = ImageOptions::new().nearest();
    let quarter = Rect::from_xywh(8.0, 0.0, 8.0, 8.0);
    low.draw_image_region(quarter, Rect::from_xywh(3.0, 14.0, 8.0, 8.0), sheet, &o);
    let flipped = o.flip_x(true).opacity(0.6);
    low.draw_image_region(
        quarter,
        Rect::from_xywh(15.0, 12.0, 8.0, 8.0),
        sheet,
        &flipped,
    );
    low.draw_image_with(Rect::from_xywh(23.0, 3.0, 16.0, 16.0), sheet, &o);
    low.push_layer(Affine::IDENTITY, 0.5);
    low.fill_rect(
        Rect::from_xywh(5.0, 3.0, 14.0, 6.0),
        Color::rgb(250, 250, 255),
    );
    low.fill_rect(
        Rect::from_xywh(10.0, 5.0, 14.0, 6.0),
        Color::rgb(250, 250, 255),
    );
    low.pop_layer();
    let mut s = Scene::new(Size::new(160.0, 120.0));
    s.fill_rect(
        Rect::from_xywh(0.0, 0.0, 160.0, 120.0),
        Color::rgb(12, 12, 18),
    );
    s.draw_offscreen(
        Rect::from_xywh(20.0, 15.0, 120.0, 90.0),
        PhysicalSize::new(40, 30),
        low,
        Filter::Nearest,
    );
    s
}
