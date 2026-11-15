//! Golden-pixel tests of game drawing: image regions (flips, rotation, tint,
//! opacity, nearest and linear sampling), pixel-art scaling and letterboxing,
//! tilemap culling and the camera, through the software renderer.
//!
//! Set `STIPPLE_DUMP=dir` to write each test's frame there as a PNG.

use crate::game::{Camera2D, PixelViewport, SpriteSheet, Tilemap};
use crate::geometry::{Affine, PhysicalSize, Point, Rect, ScaleFactor, Size, Vec2};
use crate::render::{Color, ImageOptions, Pixmap, Scene, SoftwareRenderer};
use std::sync::Arc;

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];

/// A `w`×`h` image whose pixels are `f(x, y)`.
fn image(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Arc<Pixmap> {
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            data.extend_from_slice(&f(x, y));
        }
    }
    Arc::new(Pixmap::from_rgba8(PhysicalSize::new(w, h), data))
}

/// A 4×4 sheet of four 2×2 frames: red, green / blue, white, each with a
/// black top-left pixel (so flips show).
fn quads() -> Arc<Pixmap> {
    image(4, 4, |x, y| {
        if x % 2 == 0 && y % 2 == 0 {
            return BLACK;
        }
        match (x / 2, y / 2) {
            (0, 0) => RED,
            (1, 0) => GREEN,
            (0, 1) => BLUE,
            _ => WHITE,
        }
    })
}

/// Render over opaque black at 1×, dumping it if asked.
fn render(name: &str, scene: Scene) -> Pixmap {
    let out = SoftwareRenderer::new()
        .background(Color::BLACK)
        .render(scene, ScaleFactor::IDENTITY);
    dump(name, &out);
    out
}

fn dump(name: &str, pm: &Pixmap) {
    if let Ok(dir) = std::env::var("STIPPLE_DUMP") {
        pm.save_png(format!("{dir}/{name}.png")).expect("write");
    }
}

fn px(pm: &Pixmap, x: u32, y: u32) -> [u8; 4] {
    pm.pixel(x, y).expect("in range")
}

#[test]
fn a_region_is_drawn_unscaled_and_scaled_by_whole_numbers() {
    let sheet = quads();
    let mut s = Scene::new(Size::new(32.0, 16.0));
    // The green frame, as it is, at (1, 1).
    let green = Rect::from_xywh(2.0, 0.0, 2.0, 2.0);
    s.draw_image_region(
        green,
        Rect::from_xywh(1.0, 1.0, 2.0, 2.0),
        &sheet,
        &ImageOptions::new(),
    );
    // The white frame, 4× and nearest, at (8, 4).
    let white = Rect::from_xywh(2.0, 2.0, 2.0, 2.0);
    let opts = ImageOptions::new().nearest();
    s.draw_image_region(white, Rect::from_xywh(8.0, 4.0, 8.0, 8.0), &sheet, &opts);
    let out = render("region", s);
    assert_eq!(px(&out, 1, 1), BLACK);
    assert_eq!(px(&out, 2, 1), GREEN);
    assert_eq!(px(&out, 2, 2), GREEN);
    assert_eq!(px(&out, 3, 3), BLACK, "outside the region");
    // Each source pixel is a 4×4 block.
    for (x, y) in [(8, 4), (11, 7)] {
        assert_eq!(px(&out, x, y), BLACK, "{x},{y}");
    }
    for (x, y) in [(12, 4), (15, 11), (8, 8)] {
        assert_eq!(px(&out, x, y), WHITE, "{x},{y}");
    }
    assert_eq!(px(&out, 16, 4), BLACK, "past the right edge");
}

#[test]
fn flips_mirror_the_region_in_place() {
    let sheet = quads();
    let red = Rect::from_xywh(0.0, 0.0, 2.0, 2.0);
    let mut s = Scene::new(Size::new(16.0, 4.0));
    let at = |x: f64| Rect::from_xywh(x, 0.0, 4.0, 4.0);
    let o = ImageOptions::new().nearest();
    s.draw_image_region(red, at(0.0), &sheet, &o);
    s.draw_image_region(red, at(4.0), &sheet, &o.flip_x(true));
    s.draw_image_region(red, at(8.0), &sheet, &o.flip_y(true));
    s.draw_image_region(red, at(12.0), &sheet, &o.flip_x(true).flip_y(true));
    let out = render("flips", s);
    // Where the black corner pixel (a 2×2 block) went in each.
    let black_at = |x0: u32| -> Vec<(u32, u32)> {
        let mut v = Vec::new();
        for y in 0..4 {
            for x in 0..4 {
                if px(&out, x0 + x, y) != RED {
                    v.push((x, y));
                }
            }
        }
        v
    };
    assert_eq!(black_at(0), vec![(0, 0), (1, 0), (0, 1), (1, 1)]);
    assert_eq!(black_at(4), vec![(2, 0), (3, 0), (2, 1), (3, 1)]);
    assert_eq!(black_at(8), vec![(0, 2), (1, 2), (0, 3), (1, 3)]);
    assert_eq!(black_at(12), vec![(2, 2), (3, 2), (2, 3), (3, 3)]);
}

#[test]
fn nearest_keeps_blocks_and_linear_blends() {
    let two = image(2, 1, |x, _| if x == 0 { RED } else { BLUE });
    let mut s = Scene::new(Size::new(16.0, 16.0));
    let dst = |y: f64| Rect::from_xywh(0.0, y, 16.0, 8.0);
    s.draw_image_with(dst(0.0), &two, &ImageOptions::new().nearest());
    s.draw_image_with(dst(8.0), &two, &ImageOptions::new());
    let out = render("filters", s);
    assert_eq!(px(&out, 7, 4), RED);
    assert_eq!(px(&out, 8, 4), BLUE);
    // Linear: pure at the ends (clamped, no wrap), mixed in the middle.
    assert_eq!(px(&out, 0, 12), RED);
    assert_eq!(px(&out, 15, 12), BLUE);
    let [r, g, b, a] = px(&out, 7, 12);
    assert!(r > 100 && b > 100 && g == 0 && a == 255, "{r},{g},{b}");
    let [r8, _, b8, _] = px(&out, 8, 12);
    assert!(r8 < r && b8 > b, "moves toward blue");
}

#[test]
fn tint_multiplies_and_opacity_fades() {
    let white = image(2, 2, |_, _| WHITE);
    let mut s = Scene::new(Size::new(8.0, 2.0));
    let at = |x: f64| Rect::from_xywh(x, 0.0, 2.0, 2.0);
    let tint = Color::rgb(255, 128, 0);
    s.draw_image_with(at(0.0), &white, &ImageOptions::new().tint(tint));
    s.draw_image_with(at(2.0), &white, &ImageOptions::new().opacity(0.5));
    let half_red = Color::rgba(255, 0, 0, 128);
    s.draw_image_with(at(4.0), &white, &ImageOptions::new().tint(half_red));
    let out = render("tint", s);
    assert_eq!(px(&out, 0, 0), [255, 128, 0, 255]);
    let [r, g, b, _] = px(&out, 2, 0);
    assert!((126..=129).contains(&r) && r == g && g == b, "{r}");
    let [r, g, _, _] = px(&out, 4, 0);
    assert!((126..=129).contains(&r) && g == 0, "{r},{g}");
    assert_eq!(px(&out, 6, 0), BLACK);
}

#[test]
fn a_region_turns_about_its_pivot() {
    // Red over blue (1×2), drawn 4×8 and turned a quarter clockwise about
    // its center: red on the right, blue on the left.
    let bar = image(1, 2, |_, y| if y == 0 { RED } else { BLUE });
    let mut s = Scene::new(Size::new(16.0, 16.0));
    let opts = ImageOptions::new()
        .nearest()
        .rotate(std::f64::consts::FRAC_PI_2);
    s.draw_image_with(Rect::from_xywh(6.0, 4.0, 4.0, 8.0), &bar, &opts);
    let out = render("rotate", s);
    assert_eq!(px(&out, 11, 8), RED);
    assert_eq!(px(&out, 5, 7), BLUE);
    assert_eq!(px(&out, 8, 4), BLACK, "no longer upright");
    // Linear and turned a little: drawn, smooth edged.
    let mut s = Scene::new(Size::new(16.0, 16.0));
    let white = image(4, 4, |_, _| WHITE);
    s.draw_image_with(
        Rect::from_xywh(4.0, 4.0, 8.0, 8.0),
        &white,
        &ImageOptions::new().rotate(0.3),
    );
    let out = render("rotate_linear", s);
    assert_eq!(px(&out, 8, 8), WHITE);
    let partial = (0..16)
        .flat_map(|y| (0..16).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let p = px(&out, x, y)[0];
            p > 20 && p < 235
        })
        .count();
    assert!(partial > 8, "antialiased edges: {partial}");
}

/// Upright, unscaled, at whole pixels: the fast blit gives what the
/// rasterizer gives for the same bitmap, translucent pixels included.
#[test]
fn the_fast_blit_matches_the_rasterizer() {
    let img = image(8, 8, |x, y| {
        [
            (x * 30) as u8,
            (y * 30) as u8,
            200,
            [255, 0, 128, 40][((x + y) % 4) as usize],
        ]
    });
    let make = |fast: bool| {
        let mut s = Scene::new(Size::new(20.0, 20.0));
        s.fill_rect(
            Rect::from_xywh(0.0, 0.0, 20.0, 10.0),
            Color::rgb(30, 90, 60),
        );
        let dst = Rect::from_xywh(5.0, 6.0, 8.0, 8.0);
        if fast {
            s.draw_image_with(dst, &img, &ImageOptions::new());
        } else {
            s.draw_image(dst, img.clone());
        }
        s
    };
    let fast = render("fast", make(true));
    let slow = render("slow", make(false));
    assert_eq!(fast, slow);
}

#[test]
fn pixel_art_scales_by_whole_numbers_and_letterboxes() {
    let vp = PixelViewport::new(32, 18).bar_color(Color::rgb(0, 0, 255));
    // 100×60 fits 3× (96×54) with 2 px bars left and right, 3 px top and
    // bottom.
    let mut s = Scene::new(Size::new(100.0, 60.0));
    let layout = vp.paint(&mut s, |v| {
        v.fill_rect(
            Rect::from_xywh(0.0, 0.0, 32.0, 18.0),
            Color::rgb(255, 255, 255),
        );
        v.fill_rect(Rect::from_xywh(1.0, 1.0, 1.0, 1.0), Color::rgb(255, 0, 0));
    });
    assert_eq!(layout.factor, 3);
    assert_eq!(layout.rect, Rect::from_xywh(2.0, 3.0, 96.0, 54.0));
    let out = render("pixel", s);
    assert_eq!(px(&out, 1, 30), BLUE, "left bar");
    assert_eq!(px(&out, 50, 2), BLUE, "top bar");
    assert_eq!(px(&out, 98, 30), BLUE, "right bar");
    assert_eq!(px(&out, 2, 3), WHITE);
    // Virtual (1, 1) is the 3×3 block at (5, 6).
    for (x, y) in [(5, 6), (7, 8)] {
        assert_eq!(px(&out, x, y), RED, "{x},{y}");
    }
    assert_eq!(px(&out, 8, 8), WHITE);
    assert_eq!(px(&out, 4, 6), WHITE);
    // The pointer, back in virtual pixels.
    let v = vp.pointer_to_virtual(Point::new(6.5, 7.5)).unwrap();
    assert_eq!((v.x.floor(), v.y.floor()), (1.0, 1.0));
    assert_eq!(vp.pointer_to_virtual(Point::new(0.5, 0.5)), None);

    // At 2× density, 100×60 logical is 200×120 device: 6× fits.
    let l = vp.layout(Size::new(100.0, 60.0), 2.0);
    assert_eq!(l.factor, 6);
    assert_eq!(l.rect.width() * 2.0, 192.0);
}

#[test]
fn a_tilemap_draws_only_the_visible_cells() {
    let tiles = SpriteSheet::grid(quads(), 2, 2);
    let mut map = Tilemap::new(1000, 1000, Size::new(4.0, 4.0));
    map.fill(Some(1)); // green
    map.set(2, 1, Some(3)); // white
    map.set(3, 1, None);
    let view = Rect::from_xywh(0.0, 0.0, 16.0, 8.0);
    let mut s = Scene::new(Size::new(16.0, 8.0));
    let drawn = map.draw(&mut s, &tiles, view, &ImageOptions::new().nearest());
    assert_eq!(drawn, 4 * 2 - 1, "a 4×2 window less the empty cell");
    assert_eq!(s.primitive_count(), drawn);
    let out = render("tilemap", s);
    assert_eq!(px(&out, 2, 2), GREEN);
    assert_eq!(px(&out, 0, 0), BLACK, "each tile's corner pixel");
    assert_eq!(px(&out, 10, 6), WHITE);
    assert_eq!(px(&out, 14, 6), BLACK, "the empty cell");
    // A view off the map draws nothing.
    let mut s = Scene::new(Size::new(16.0, 8.0));
    let far = Rect::from_xywh(-100.0, -100.0, 50.0, 50.0);
    assert_eq!(map.draw(&mut s, &tiles, far, &ImageOptions::new()), 0);
}

#[test]
fn the_camera_maps_the_world_onto_the_canvas() {
    let tiles = SpriteSheet::grid(quads(), 2, 2);
    let mut cam = Camera2D::new(Size::new(40.0, 20.0));
    cam.position = Point::new(100.0, 50.0);
    cam.zoom = 2.0;
    let mut s = Scene::new(Size::new(40.0, 20.0));
    cam.draw(&mut s, |s| {
        // A 2×2 world sprite whose top-left is at the camera's center.
        tiles.draw_at(
            s,
            3,
            Point::new(100.0, 50.0),
            &ImageOptions::new().nearest(),
        );
        // One out of view.
        tiles.draw_at(s, 0, Point::new(0.0, 0.0), &ImageOptions::new().nearest());
    });
    let out = render("camera", s);
    // On screen at (20, 10), 4×4: its black corner then white.
    assert_eq!(px(&out, 20, 10), BLACK);
    assert_eq!(px(&out, 22, 10), WHITE);
    assert_eq!(px(&out, 23, 13), WHITE);
    assert_eq!(px(&out, 24, 10), BLACK);
    assert_eq!(cam.visible_rect(), Rect::from_xywh(90.0, 45.0, 20.0, 10.0));
}

/// Regions inside a translucent layer or a turned clip are drawn by the
/// rasterizer (as plain image nodes) instead; the result is the same.
#[test]
fn regions_in_layers_and_turned_clips_fall_back() {
    let sheet = quads();
    let red = Rect::from_xywh(0.0, 0.0, 2.0, 2.0);
    let mut s = Scene::new(Size::new(16.0, 8.0));
    s.push_layer(Affine::IDENTITY, 0.5);
    s.draw_image_region(
        red,
        Rect::from_xywh(0.0, 0.0, 8.0, 8.0),
        &sheet,
        &ImageOptions::new().nearest(),
    );
    s.pop_layer();
    let about = Affine::translate(Vec2::new(-12.0, -4.0))
        .then(Affine::rotate(0.4))
        .then(Affine::translate(Vec2::new(12.0, 4.0)));
    s.push_layer(about, 1.0);
    s.push_clip(Rect::from_xywh(9.0, 1.0, 6.0, 6.0));
    s.draw_image_region(
        red,
        Rect::from_xywh(8.0, 0.0, 8.0, 8.0),
        &sheet,
        &ImageOptions::new().nearest(),
    );
    s.pop_clip();
    s.pop_layer();
    let out = render("fallback", s);
    let [r, g, _, _] = px(&out, 6, 6);
    assert!((120..=136).contains(&r) && g == 0, "half red: {r}");
    assert_eq!(px(&out, 2, 2), BLACK, "the corner block, faded black");
    assert_eq!(px(&out, 13, 5), RED, "inside the turned clip");
    assert_eq!(px(&out, 8, 0), BLACK, "clipped away");
}

/// A canvas full of regions, rendered whole and by area, agrees with
/// itself; banding and culling don't move or drop a sprite.
#[test]
fn an_area_render_of_regions_matches_the_full_render() {
    use crate::render::CanvasPaint;
    let sheet = SpriteSheet::grid(quads(), 2, 2);
    let paint: CanvasPaint = std::rc::Rc::new(move |c: &mut crate::render::Canvas| {
        c.fill_rect(
            Rect::from_xywh(0.0, 0.0, 200.0, 150.0),
            Color::rgb(20, 20, 40),
        );
        for i in 0..300 {
            let (x, y) = ((i * 37 % 190) as f64, (i * 53 % 140) as f64 + 0.5);
            let o = ImageOptions::new().nearest().flip_x(i % 2 == 0);
            sheet.draw(c, i % 4, Rect::from_xywh(x, y, 6.0, 6.0), &o);
        }
    });
    let make = || {
        let mut s = Scene::new(Size::new(220.0, 170.0));
        s.fill_rect(
            Rect::from_xywh(0.0, 0.0, 220.0, 170.0),
            Color::rgb(200, 200, 200),
        );
        s.draw_canvas(Rect::from_xywh(10.0, 10.0, 200.0, 150.0), &paint);
        s
    };
    let r = SoftwareRenderer::new().background(Color::BLACK);
    let full = r.render(make(), ScaleFactor::IDENTITY);
    dump("area_full", &full);
    let view = Rect::from_xywh(37.0, 41.0, 90.0, 70.0);
    let area = r.render_area(&make(), view, PhysicalSize::new(90, 70));
    for y in 0..70 {
        for x in 0..90 {
            assert_eq!(area.pixel(x, y), full.pixel(37 + x, 41 + y), "at {x},{y}");
        }
    }
    assert_eq!(px(&full, 5, 5), [200, 200, 200, 255], "outside the canvas");
}
