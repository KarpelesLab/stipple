//! The pictures: a sprite sheet painted pixel by pixel at startup (no image
//! files), the garden's tilemap, the dark with the lantern's light cut out
//! of it, and the frame drawn at 320×180 through a `PixelViewport`.

use std::sync::Arc;

use stipple::game::{Camera2D, PixelViewport, SpriteAnimation, SpriteSheet, Tilemap};
use stipple::prelude::*;
use stipple::render::{ImageOptions, Scene};

use crate::world::{self, World};

const T: u32 = 16;
// Sheet frames, 8 per row.
const WICK: usize = 0; // 0..4: idle (2), walk (2)
const MOTH: usize = 4; // 4, 5
const EMBER: usize = 6; // 6, 7
const GRASS: usize = 8; // 8, 9
const FLOWERS: usize = 10;
const STONE: usize = 11;
const DARK_GRASS: usize = 12;
const MUSHROOM: usize = 13;
const FERN: usize = 14;
const PEBBLE: usize = 15;

type Rgba = [u8; 4];

/// Straight RGBA bytes being painted.
struct Painter {
    w: u32,
    data: Vec<u8>,
}

impl Painter {
    fn px(&mut self, x: i32, y: i32, c: Rgba) {
        if x >= 0
            && y >= 0
            && (x as u32) < self.w
            && ((y as u32) * self.w * 4) < self.data.len() as u32
        {
            let i = ((y as u32 * self.w + x as u32) * 4) as usize;
            self.data[i..i + 4].copy_from_slice(&c);
        }
    }

    fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: Rgba) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.px(xx, yy, c);
            }
        }
    }

    fn ellipse(&mut self, cx: f64, cy: f64, rx: f64, ry: f64, c: Rgba) {
        for y in (cy - ry).floor() as i32..=(cy + ry).ceil() as i32 {
            for x in (cx - rx).floor() as i32..=(cx + rx).ceil() as i32 {
                let (dx, dy) = ((x as f64 + 0.5 - cx) / rx, (y as f64 + 0.5 - cy) / ry);
                if dx * dx + dy * dy <= 1.0 {
                    self.px(x, y, c);
                }
            }
        }
    }
}

/// A repeatable hash, for speckles and the map.
fn hash(x: i32, y: i32, salt: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x9e37_79b1) ^ (y as u32).wrapping_mul(0x85eb_ca77) ^ salt;
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h
}

fn cell(i: usize) -> (i32, i32) {
    ((i % 8) as i32 * T as i32, (i / 8) as i32 * T as i32)
}

/// The sprite sheet: Wick, a moth, an ember, the garden's tiles.
fn paint_sheet() -> Pixmap {
    let mut p = Painter {
        w: 8 * T,
        data: vec![0; (8 * T * 2 * T * 4) as usize],
    };
    // Wick: a hooded little keeper with a lantern on a crook.
    for f in 0..4 {
        let (ox, oy) = cell(WICK + f);
        let bob = if f == 1 || f == 3 { 1 } else { 0 };
        let (ox, oy) = (ox, oy + bob);
        let cloak = [76, 60, 120, 255];
        let shade = [52, 40, 88, 255];
        p.ellipse(ox as f64 + 7.0, oy as f64 + 10.5, 4.6, 4.8, shade);
        p.ellipse(ox as f64 + 6.6, oy as f64 + 10.0, 4.0, 4.4, cloak);
        // The hood's point, leaning back.
        p.rect(ox + 5, oy + 4, 4, 2, cloak);
        p.rect(ox + 4, oy + 3, 3, 1, cloak);
        p.px(ox + 3, oy + 2, cloak);
        p.px(ox + 8, oy + 5, [100, 84, 150, 255]);
        // The face in shadow, two warm eyes.
        p.rect(ox + 5, oy + 7, 5, 3, [24, 18, 40, 255]);
        p.px(ox + 6, oy + 8, [255, 228, 150, 255]);
        p.px(ox + 8, oy + 8, [255, 228, 150, 255]);
        // Feet, stepping on the walk frames.
        let (l, r) = match f {
            2 => (0, 1),
            3 => (1, 0),
            _ => (0, 0),
        };
        p.rect(ox + 4, oy + 15 - bob - l, 2, 1, [30, 24, 44, 255]);
        p.rect(ox + 8, oy + 15 - bob - r, 2, 1, [30, 24, 44, 255]);
        // The crook and the lantern.
        p.rect(ox + 11, oy + 4, 1, 9, [120, 90, 60, 255]);
        p.rect(ox + 11, oy + 4, 3, 1, [120, 90, 60, 255]);
        p.rect(ox + 12, oy + 6, 3, 1, [70, 56, 44, 255]);
        p.rect(ox + 12, oy + 7, 3, 3, [255, 190, 70, 255]);
        p.px(ox + 13, oy + 8, [255, 250, 210, 255]);
        p.rect(ox + 12, oy + 10, 3, 1, [70, 56, 44, 255]);
    }
    // A moth: wings up, then down.
    for f in 0..2 {
        let (ox, oy) = cell(MOTH + f);
        let wing = [200, 188, 220, 255];
        let spot = [140, 120, 170, 255];
        if f == 0 {
            p.ellipse(ox as f64 + 4.5, oy as f64 + 6.0, 3.5, 4.5, wing);
            p.ellipse(ox as f64 + 11.5, oy as f64 + 6.0, 3.5, 4.5, wing);
            p.px(ox + 4, oy + 5, spot);
            p.px(ox + 11, oy + 5, spot);
        } else {
            p.ellipse(ox as f64 + 4.0, oy as f64 + 10.0, 4.0, 2.5, wing);
            p.ellipse(ox as f64 + 12.0, oy as f64 + 10.0, 4.0, 2.5, wing);
            p.px(ox + 3, oy + 10, spot);
            p.px(ox + 12, oy + 10, spot);
        }
        p.rect(ox + 7, oy + 5, 2, 7, [110, 92, 76, 255]);
        p.px(ox + 6, oy + 4, [150, 130, 110, 255]);
        p.px(ox + 9, oy + 4, [150, 130, 110, 255]);
    }
    // An ember, flickering between two sizes.
    for f in 0..2 {
        let (ox, oy) = cell(EMBER + f);
        let r = if f == 0 { 4.5 } else { 3.6 };
        p.ellipse(ox as f64 + 8.0, oy as f64 + 8.0, r, r, [220, 80, 30, 150]);
        p.ellipse(
            ox as f64 + 8.0,
            oy as f64 + 8.0,
            r - 1.4,
            r - 1.4,
            [255, 160, 50, 255],
        );
        p.rect(ox + 7, oy + 7, 2, 2, [255, 244, 190, 255]);
    }
    // Night grass, in two speckles.
    for (i, salt) in [(GRASS, 1), (GRASS + 1, 2), (FLOWERS, 3), (DARK_GRASS, 4)] {
        let (ox, oy) = cell(i);
        for y in 0..T as i32 {
            for x in 0..T as i32 {
                let n = hash(x, y, salt) % 9;
                let base: Rgba = if i == DARK_GRASS {
                    [26, 44, 34, 255]
                } else {
                    [34, 58, 42, 255]
                };
                let c = match n {
                    0 => [46, 76, 52, 255],
                    1 => [26, 46, 34, 255],
                    _ => base,
                };
                p.px(ox + x, oy + y, c);
            }
        }
        // Blades.
        for k in 0..3 {
            let bx = (hash(k, 0, salt) % 14) as i32 + 1;
            let by = (hash(k, 1, salt) % 12) as i32 + 2;
            p.rect(ox + bx, oy + by, 1, 2, [58, 94, 62, 255]);
        }
    }
    let (ox, oy) = cell(FLOWERS);
    for (x, y) in [(3, 4), (11, 9), (6, 12)] {
        p.px(ox + x, oy + y, [196, 214, 255, 255]);
        p.px(ox + x - 1, oy + y, [150, 170, 230, 255]);
        p.px(ox + x + 1, oy + y, [150, 170, 230, 255]);
        p.px(ox + x, oy + y - 1, [150, 170, 230, 255]);
    }
    // A stepping stone on grass.
    let (ox, oy) = cell(STONE);
    p.rect(ox, oy, T as i32, T as i32, [34, 58, 42, 255]);
    p.ellipse(
        ox as f64 + 8.0,
        oy as f64 + 8.5,
        6.5,
        5.0,
        [58, 62, 78, 255],
    );
    p.ellipse(
        ox as f64 + 7.5,
        oy as f64 + 7.5,
        5.5,
        4.0,
        [84, 90, 108, 255],
    );
    p.px(ox + 5, oy + 6, [112, 118, 138, 255]);
    // Decorations, transparent around them.
    let (ox, oy) = cell(MUSHROOM);
    p.rect(ox + 7, oy + 10, 2, 4, [220, 210, 190, 255]);
    p.ellipse(
        ox as f64 + 8.0,
        oy as f64 + 9.5,
        4.5,
        2.5,
        [170, 60, 70, 255],
    );
    p.px(ox + 6, oy + 9, [240, 230, 220, 255]);
    p.px(ox + 9, oy + 8, [240, 230, 220, 255]);
    let (ox, oy) = cell(FERN);
    for k in 0..5 {
        p.rect(
            ox + 3 + k * 2,
            oy + 6 + (k - 2).abs(),
            1,
            8 - (k - 2).abs(),
            [64, 112, 70, 255],
        );
    }
    let (ox, oy) = cell(PEBBLE);
    p.ellipse(
        ox as f64 + 6.0,
        oy as f64 + 12.0,
        3.0,
        2.0,
        [90, 96, 112, 255],
    );
    p.ellipse(
        ox as f64 + 10.5,
        oy as f64 + 13.0,
        2.0,
        1.5,
        [76, 80, 96, 255],
    );
    Pixmap::from_rgba8(PhysicalSize::new(8 * T, 2 * T), p.data)
}

/// The dark around the lantern: clear in the middle with a warm tint,
/// darker in dithered steps toward the edge, the night's color past it.
pub const NIGHT: Color = Color::rgba(8, 7, 22, 236);
const MASK: i32 = 96;

fn paint_mask() -> Pixmap {
    const BAYER: [[f32; 4]; 4] = [
        [0.0, 8.0, 2.0, 10.0],
        [12.0, 4.0, 14.0, 6.0],
        [3.0, 11.0, 1.0, 9.0],
        [15.0, 7.0, 13.0, 5.0],
    ];
    let mut p = Painter {
        w: MASK as u32,
        data: vec![0; (MASK * MASK * 4) as usize],
    };
    let half = MASK as f32 / 2.0;
    for y in 0..MASK {
        for x in 0..MASK {
            let (dx, dy) = (
                (x as f32 + 0.5 - half) / half,
                (y as f32 + 0.5 - half) / half,
            );
            let d = (dx * dx + dy * dy).sqrt();
            let dither = (BAYER[(y & 3) as usize][(x & 3) as usize] + 0.5) / 16.0;
            // Four steps of darkness from 0.5 out, dithered between.
            let t = ((d - 0.5) / 0.5).clamp(0.0, 1.0) * 4.0;
            let step = (t.floor() + if t.fract() > dither { 1.0 } else { 0.0 }) / 4.0;
            let c = if step <= 0.0 {
                let warm = ((0.5 - d) / 0.5).clamp(0.0, 1.0);
                [255, 176, 80, (warm * 34.0) as u8]
            } else {
                [NIGHT.r, NIGHT.g, NIGHT.b, (step * f32::from(NIGHT.a)) as u8]
            };
            p.px(x, y, c);
        }
    }
    Pixmap::from_rgba8(PhysicalSize::new(MASK as u32, MASK as u32), p.data)
}

/// The garden: grass, a path of stepping stones, a few plants.
fn garden() -> (Tilemap, Vec<(usize, Point)>) {
    let (cols, rows) = (20u32, 12u32);
    let mut map = Tilemap::new(cols, rows, Size::new(T as f64, T as f64));
    for r in 0..rows {
        for c in 0..cols {
            let h = hash(c as i32, r as i32, 77) % 16;
            let t = match h {
                0 => FLOWERS,
                1 | 2 => DARK_GRASS,
                3..=8 => GRASS + 1,
                _ => GRASS,
            };
            map.set(c, r, Some(t));
        }
    }
    // A winding path across.
    for c in 0..cols {
        let r = 6 + ((c as i32 / 3) % 3 - 1).unsigned_abs();
        if c.is_multiple_of(2) || hash(c as i32, 0, 5).is_multiple_of(2) {
            map.set(c, r, Some(STONE));
        }
    }
    let mut decor = Vec::new();
    for i in 0..18 {
        let x = (hash(i, 1, 9) % 300) as f64 + 4.0;
        let y = (hash(i, 2, 9) % 150) as f64 + 20.0;
        let kind = [MUSHROOM, FERN, PEBBLE, FERN][(hash(i, 3, 9) % 4) as usize];
        decor.push((kind, Point::new(x.round(), y.round())));
    }
    (map, decor)
}

/// Everything drawn, shared by the game and the canvas's paint callback.
#[derive(Debug)]
pub struct Stage {
    pub world: World,
    pub cam: Camera2D,
    pub vp: PixelViewport,
    sheet: SpriteSheet,
    mask: Arc<Pixmap>,
    map: Tilemap,
    decor: Vec<(usize, Point)>,
    walk: SpriteAnimation,
    idle: SpriteAnimation,
    flap: SpriteAnimation,
    flicker: SpriteAnimation,
}

impl Stage {
    pub fn new(world: World) -> Stage {
        let (map, decor) = garden();
        let mut cam = Camera2D::new(Size::new(f64::from(world::W), f64::from(world::H)));
        cam.position = Point::new(f64::from(world::W) / 2.0, f64::from(world::H) / 2.0);
        Stage {
            world,
            cam,
            vp: PixelViewport::new(world::W as u32, world::H as u32)
                .bar_color(Color::rgb(6, 5, 14)),
            sheet: SpriteSheet::grid(Arc::new(paint_sheet()), T, T),
            mask: Arc::new(paint_mask()),
            map,
            decor,
            walk: SpriteAnimation::new(WICK + 2..WICK + 4, 0.16),
            idle: SpriteAnimation::new(WICK..WICK + 2, 0.5),
            flap: SpriteAnimation::new(MOTH..MOTH + 2, 0.09),
            flicker: SpriteAnimation::new(EMBER..EMBER + 2, 0.12),
        }
    }

    /// A point of the virtual screen, in the canvas's coordinates (for
    /// floating text over the HUD).
    pub fn to_canvas(&self, v: world::V2) -> Point {
        match self.vp.last_layout() {
            Some(l) => Point::new(
                l.rect.min_x() + f64::from(v.x) * l.scale,
                l.rect.min_y() + f64::from(v.y) * l.scale,
            ),
            None => Point::new(f64::from(v.x) * 4.0, f64::from(v.y) * 4.0),
        }
    }

    pub fn paint(&self, c: &mut Canvas) {
        self.vp.paint(c, |s| self.cam.draw(s, |s| self.draw(s)));
    }

    fn draw(&self, s: &mut Scene) {
        let w = &self.world;
        let t = f64::from(w.time);
        let px = ImageOptions::new().nearest();
        let at = |v: world::V2| {
            Point::new(
                (f64::from(v.x) - 8.0).round(),
                (f64::from(v.y) - 8.0).round(),
            )
        };
        // The ground reaches past the screen's edges, for the shake.
        s.fill_rect(
            Rect::from_xywh(-16.0, -16.0, 352.0, 212.0),
            Color::rgb(34, 58, 42),
        );
        self.map.draw(s, &self.sheet, self.cam.visible_rect(), &px);
        for (kind, p) in &self.decor {
            self.sheet.draw_at(s, *kind, *p, &px);
        }
        for (i, m) in w.moths.iter().enumerate() {
            let frame = self.flap.frame_at(f64::from(m.age) + i as f64 * 0.05);
            self.sheet
                .draw_at(s, frame, at(m.pos), &px.flip_x(m.vel.x < 0.0));
        }
        // Wick blinks while hurt.
        let blink = w.hurt > 0.0 && ((w.hurt * 10.0) as u32).is_multiple_of(2);
        if !blink {
            let frame = if w.walking {
                self.walk.frame_at(t)
            } else {
                self.idle.frame_at(t)
            };
            self.sheet
                .draw_at(s, frame, at(w.player), &px.flip_x(w.facing < 0.0));
        }

        // The dark, with the lantern's light (wider for a moment after a
        // flare) cut out of it.
        let flare = (1.0 - w.flare_age / 0.35).max(0.0);
        let r = (f64::from(w.radius()) + f64::from(flare) * 40.0).round();
        let (cx, cy) = (f64::from(w.player.x).round(), f64::from(w.player.y).round());
        let hole = Rect::from_xywh(cx - r, cy - r, 2.0 * r, 2.0 * r);
        s.draw_image_with(hole, &self.mask, &px);
        let (x0, y0, x1, y1) = (-24.0, -24.0, 344.0, 204.0);
        for rect in [
            Rect::from_xywh(x0, y0, x1 - x0, hole.min_y() - y0),
            Rect::from_xywh(x0, hole.max_y(), x1 - x0, y1 - hole.max_y()),
            Rect::from_xywh(x0, hole.min_y(), hole.min_x() - x0, hole.height()),
            Rect::from_xywh(hole.max_x(), hole.min_y(), x1 - hole.max_x(), hole.height()),
        ] {
            if rect.width() > 0.0 && rect.height() > 0.0 {
                s.fill_rect(rect, NIGHT);
            }
        }
        if flare > 0.0 {
            // A ring of sparks rushing out with the light.
            let spark = Color::rgba(255, 236, 170, (flare * 255.0) as u8);
            for i in 0..40 {
                let a = f64::from(i) * std::f64::consts::TAU / 40.0;
                let (x, y) = (
                    (cx + r * 0.92 * a.cos()).round(),
                    (cy + r * 0.92 * a.sin()).round(),
                );
                s.fill_rect(Rect::from_xywh(x, y, 1.0, 1.0), spark);
            }
        }

        // What glows shows through the dark: embers, the moths' eyes, the
        // lantern.
        for (i, e) in w.embers.iter().enumerate() {
            let frame = self.flicker.frame_at(t + i as f64 * 0.07);
            self.sheet.draw_at(s, frame, at(e.pos), &px);
        }
        for m in &w.moths {
            if m.pos.minus(w.player).len() > w.radius() * 0.5 {
                let (x, y) = (f64::from(m.pos.x).round(), f64::from(m.pos.y).round() - 4.0);
                let eye = Color::rgb(255, 120, 90);
                s.fill_rect(Rect::from_xywh(x - 2.0, y, 1.0, 1.0), eye);
                s.fill_rect(Rect::from_xywh(x + 1.0, y, 1.0, 1.0), eye);
            }
        }
        if !blink {
            let lx = if w.facing < 0.0 { cx - 7.0 } else { cx + 4.0 };
            s.fill_rect(
                Rect::from_xywh(lx, cy - 1.0, 3.0, 3.0),
                Color::rgb(255, 200, 90),
            );
            s.fill_rect(
                Rect::from_xywh(lx + 1.0, cy, 1.0, 1.0),
                Color::rgb(255, 252, 220),
            );
        }
    }
}
