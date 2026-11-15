//! A pixel-art scene at 320×180: a hero walks over a tilemap, animated from
//! a sprite sheet, the camera following them, coins spinning, clouds
//! drifting behind. Everything is drawn with Stipple's game helpers:
//! `SpriteSheet`, `SpriteAnimation`, `Tilemap`, `Camera2D` and
//! `PixelViewport` (which scales the 320×180 frame by the largest whole
//! number that fits the window, with bars around it). The sprite sheet is
//! painted pixel by pixel at startup: no image files.
//!
//! Click to send the hero somewhere (the click is turned back into virtual
//! pixels, then into the world through the camera); the camera shakes a
//! little when they set off. The frame rate is printed every second.
//!
//! Flags:
//! - `--png FILE`: render one 1280×720 frame off-screen to `FILE` and quit.
//! - `--bench`: time the software renderer drawing a 1920×1080 frame full of
//!   32×32 sprites (about 2000 plus a background), through the fast blits and
//!   through the general rasterizer path, and print ms per frame.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use stipple::game::{Camera2D, PixelViewport, Playback, SpriteAnimation, SpriteSheet, Tilemap};
use stipple::prelude::*;
use stipple::render::{ImageOptions, Scene, SoftwareRenderer};

const GAME: CanvasId = CanvasId(1);
const TILE: u32 = 16;
const MAP_COLS: u32 = 96;
const MAP_ROWS: u32 = 12;
/// The ground's top row.
const GROUND: u32 = 10;

// Sheet frame numbers (a 16×16 grid, 8 per row).
const GRASS: usize = 0;
const DIRT: usize = 1;
const BRICK: usize = 2;
const WATER: usize = 3;
const PLANK: usize = 4;
const FLOWER: usize = 5;
const BUSH: usize = 6;
const CLOUD: usize = 7;
const WALK: usize = 8; // 8..12
const IDLE: usize = 12;
const COIN: usize = 16; // 16..20

/// Straight RGBA bytes being painted.
struct Painter {
    w: u32,
    h: u32,
    data: Vec<u8>,
}

impl Painter {
    fn new(w: u32, h: u32) -> Self {
        Self {
            w,
            h,
            data: vec![0; (w * h * 4) as usize],
        }
    }

    fn px(&mut self, x: i32, y: i32, c: [u8; 4]) {
        if x >= 0 && y >= 0 && (x as u32) < self.w && (y as u32) < self.h {
            let i = ((y as u32 * self.w + x as u32) * 4) as usize;
            self.data[i..i + 4].copy_from_slice(&c);
        }
    }

    fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: [u8; 4]) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.px(xx, yy, c);
            }
        }
    }

    fn ellipse(&mut self, cx: f64, cy: f64, rx: f64, ry: f64, c: [u8; 4]) {
        for y in (cy - ry).floor() as i32..=(cy + ry).ceil() as i32 {
            for x in (cx - rx).floor() as i32..=(cx + rx).ceil() as i32 {
                let (dx, dy) = ((x as f64 + 0.5 - cx) / rx, (y as f64 + 0.5 - cy) / ry);
                if dx * dx + dy * dy <= 1.0 {
                    self.px(x, y, c);
                }
            }
        }
    }

    fn into_pixmap(self) -> Pixmap {
        Pixmap::from_rgba8(PhysicalSize::new(self.w, self.h), self.data)
    }
}

/// A repeatable hash of a position, for speckles.
fn hash(x: i32, y: i32, salt: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x9e37_79b1) ^ (y as u32).wrapping_mul(0x85eb_ca77) ^ salt;
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h
}

/// The sprite sheet: tiles, the hero's walk cycle and a spinning coin.
fn paint_sheet() -> Pixmap {
    let mut p = Painter::new(8 * TILE, 3 * TILE);
    let cell = |i: usize| ((i % 8) as i32 * TILE as i32, (i / 8) as i32 * TILE as i32);
    let t = TILE as i32;

    // Dirt, with grass on top for the grass tile.
    for (i, grass) in [(GRASS, true), (DIRT, false)] {
        let (ox, oy) = cell(i);
        for y in 0..t {
            for x in 0..t {
                let n = hash(x, y, i as u32) % 7;
                let mut c = [124, 82, 48, 255];
                if n == 0 {
                    c = [98, 62, 36, 255];
                } else if n == 1 {
                    c = [146, 100, 60, 255];
                }
                let edge = 3 + (hash(x, 0, 9) % 2) as i32;
                if grass && y < edge {
                    c = if n < 2 {
                        [72, 160, 64, 255]
                    } else {
                        [92, 188, 76, 255]
                    };
                }
                p.px(ox + x, oy + y, c);
            }
        }
    }
    // Brick.
    let (ox, oy) = cell(BRICK);
    p.rect(ox, oy, t, t, [92, 92, 108, 255]);
    for row in 0..4 {
        let y = oy + row * 4;
        p.rect(ox, y + 3, t, 1, [60, 60, 72, 255]);
        let shift = if row % 2 == 0 { 0 } else { 4 };
        for k in 0..3 {
            p.rect(ox + (shift + k * 8) % t, y, 1, 3, [60, 60, 72, 255]);
        }
        p.rect(ox, y, t, 1, [120, 120, 138, 255]);
    }
    // Water.
    let (ox, oy) = cell(WATER);
    p.rect(ox, oy, t, t, [40, 96, 184, 220]);
    for x in 0..t {
        let y = ((x as f64 / 3.0).sin() * 1.5 + 3.0) as i32;
        p.px(ox + x, oy + y, [150, 200, 255, 255]);
        p.px(ox + (x + 5) % t, oy + y + 8, [96, 150, 230, 255]);
    }
    // Wooden planks.
    let (ox, oy) = cell(PLANK);
    p.rect(ox, oy, t, 6, [164, 112, 60, 255]);
    p.rect(ox, oy, t, 1, [200, 150, 90, 255]);
    p.rect(ox, oy + 5, t, 1, [100, 66, 34, 255]);
    p.rect(ox + 3, oy + 1, 1, 4, [120, 80, 40, 255]);
    p.rect(ox + 11, oy + 1, 1, 4, [120, 80, 40, 255]);
    // A flower and a bush (transparent around them).
    let (ox, oy) = cell(FLOWER);
    p.rect(ox + 7, oy + 8, 1, 8, [60, 140, 60, 255]);
    p.rect(ox + 8, oy + 11, 2, 1, [60, 140, 60, 255]);
    for (dx, dy) in [(-2, 0), (2, 0), (0, -2), (0, 2)] {
        p.ellipse(
            ox as f64 + 7.5 + dx as f64,
            oy as f64 + 6.5 + dy as f64,
            1.6,
            1.6,
            [236, 96, 140, 255],
        );
    }
    p.px(ox + 7, oy + 6, [255, 220, 80, 255]);
    let (ox, oy) = cell(BUSH);
    p.ellipse(
        ox as f64 + 8.0,
        oy as f64 + 11.0,
        7.5,
        5.5,
        [48, 128, 56, 255],
    );
    p.ellipse(
        ox as f64 + 6.0,
        oy as f64 + 9.0,
        3.5,
        3.0,
        [72, 160, 72, 255],
    );
    // A cloud.
    let (ox, oy) = cell(CLOUD);
    for (cx, cy, r) in [(5.0, 10.0, 4.0), (10.0, 8.0, 5.0), (12.0, 11.0, 3.5)] {
        p.ellipse(
            ox as f64 + cx,
            oy as f64 + cy,
            r,
            r * 0.8,
            [250, 250, 255, 255],
        );
    }

    // The hero, facing right: four walk frames, then standing.
    for f in 0..5 {
        let (ox, oy) = cell(WALK + f);
        let skin = [240, 192, 150, 255];
        let hair = [80, 48, 30, 255];
        let shirt = [214, 64, 56, 255];
        let pants = [48, 72, 150, 255];
        let boot = [40, 28, 24, 255];
        let bob = if f < 4 && f % 2 == 1 { 1 } else { 0 };
        let y0 = oy + bob;
        p.rect(ox + 5, y0 + 1, 6, 2, hair);
        p.rect(ox + 5, y0 + 3, 6, 4, skin);
        p.rect(ox + 5, y0 + 3, 1, 2, hair);
        p.px(ox + 9, y0 + 4, [20, 20, 30, 255]);
        p.rect(ox + 4, y0 + 7, 8, 4, shirt);
        let arm = if f < 4 { [0, 1, 0, -1][f] } else { 0 };
        p.rect(ox + 7 + arm, y0 + 8, 2, 3, skin);
        // Legs: apart, crossing, apart the other way, crossing.
        let (a, b) = if f < 4 {
            [(-2, 2), (0, 0), (2, -2), (0, 0)][f]
        } else {
            (-1, 1)
        };
        p.rect(ox + 6 + a, oy + 11, 2, 4, pants);
        p.rect(ox + 8 + b, oy + 11, 2, 4, pants);
        p.rect(ox + 6 + a, oy + 15, 2, 1, boot);
        p.rect(ox + 8 + b, oy + 15, 2, 1, boot);
    }
    // A coin, turning.
    for (f, rx) in [6.0, 4.0, 1.2, 4.0].into_iter().enumerate() {
        let (ox, oy) = cell(COIN + f);
        p.ellipse(
            ox as f64 + 8.0,
            oy as f64 + 8.0,
            rx,
            6.0,
            [200, 150, 30, 255],
        );
        p.ellipse(
            ox as f64 + 8.0,
            oy as f64 + 8.0,
            (rx - 1.2).max(0.6),
            4.8,
            [255, 214, 70, 255],
        );
    }
    p.into_pixmap()
}

/// The level: ground with a pond and a bridge, brick platforms, flowers.
fn level() -> Tilemap {
    let mut map = Tilemap::new(MAP_COLS, MAP_ROWS, Size::new(TILE as f64, TILE as f64));
    for col in 0..MAP_COLS {
        let pond = (30..36).contains(&col);
        map.set(col, GROUND, Some(if pond { WATER } else { GRASS }));
        map.set(col, GROUND + 1, Some(if pond { WATER } else { DIRT }));
        if pond {
            map.set(col, GROUND - 1, Some(PLANK));
        } else if hash(col as i32, 1, 3).is_multiple_of(5) {
            map.set(col, GROUND - 1, Some(FLOWER));
        } else if hash(col as i32, 2, 3).is_multiple_of(7) {
            map.set(col, GROUND - 1, Some(BUSH));
        }
    }
    for (start, row, len) in [(8, 6, 5), (20, 4, 4), (44, 6, 6), (60, 5, 3), (72, 7, 8)] {
        for col in start..start + len {
            map.set(col, row, Some(BRICK));
        }
    }
    map
}

struct Game {
    sheet: SpriteSheet,
    map: Tilemap,
    walk: SpriteAnimation,
    coin: SpriteAnimation,
    coins: Vec<Point>,
    vp: PixelViewport,
    cam: Camera2D,
    hero: Point,
    facing_left: bool,
    target: Option<f64>,
    /// Seconds left standing still.
    rest: f64,
    time: f64,
    /// Steps taken.
    ticks: u64,
    last: Option<Duration>,
    // Frame rate.
    frames: u32,
    paint_time: Duration,
    since: Instant,
}

impl Game {
    fn new() -> Self {
        let sheet = SpriteSheet::grid(Arc::new(paint_sheet()), TILE, TILE);
        let map = level();
        let vp = PixelViewport::new(320, 180)
            .bar_color(Color::rgb(12, 12, 18))
            .background(Color::rgb(120, 180, 240));
        let mut cam = Camera2D::new(vp.size());
        let hero = Point::new(40.0, (GROUND * TILE - TILE) as f64);
        cam.position = hero;
        cam.shake_strength = 4.0;
        let coins = (0..24)
            .map(|i| {
                let x = 40.0 + i as f64 * 52.0;
                let y = (GROUND * TILE) as f64 - 40.0 - (i % 3) as f64 * 14.0;
                Point::new(x, y)
            })
            .collect();
        Self {
            sheet,
            map,
            walk: SpriteAnimation::new(WALK..WALK + 4, 0.12),
            coin: SpriteAnimation::new(COIN..COIN + 4, 0.15).playback(Playback::PingPong),
            coins,
            vp,
            cam,
            hero,
            facing_left: false,
            target: None,
            rest: 0.0,
            time: 0.0,
            ticks: 0,
            last: None,
            frames: 0,
            paint_time: Duration::ZERO,
            since: Instant::now(),
        }
    }

    /// Advance to the frame at `now` on the app's timeline.
    fn advance(&mut self, now: Duration) {
        let dt = match self.last.replace(now) {
            Some(last) => now.saturating_sub(last).as_secs_f64().min(0.1),
            None => 0.0,
        };
        self.step(dt);
    }

    fn step(&mut self, dt: f64) {
        self.ticks += 1;
        self.time += dt;
        let speed = 70.0;
        let limit = (MAP_COLS * TILE - TILE) as f64;
        let goal = self
            .target
            .unwrap_or(if self.facing_left { 0.0 } else { limit });
        let dx = goal - self.hero.x;
        if self.rest > 0.0 {
            self.rest -= dt;
        } else if dx.abs() < 1.0 {
            self.target = None;
            self.rest = 0.8;
            // Patrol: turn around at the ends.
            if goal <= 0.0 || goal >= limit {
                self.facing_left = !self.facing_left;
            }
        } else {
            self.facing_left = dx < 0.0;
            self.hero.x += dx.signum() * (speed * dt).min(dx.abs());
        }
        let center = Point::new(self.hero.x + 8.0, self.hero.y - 20.0);
        self.cam.follow(center, 5.0, dt);
        self.cam.clamp_to(self.map.bounds());
        self.cam.update(dt);
    }

    fn walking(&self) -> bool {
        self.rest <= 0.0
    }

    /// Draw the 320×180 frame.
    fn draw(&self, s: &mut Scene) {
        let nearest = ImageOptions::new().nearest();
        // Sky bands and clouds drifting behind the level (parallax).
        let bands = [
            Color::rgb(108, 168, 236),
            Color::rgb(124, 182, 240),
            Color::rgb(144, 196, 244),
            Color::rgb(164, 208, 246),
        ];
        for (i, c) in bands.iter().enumerate() {
            s.fill_rect(Rect::from_xywh(0.0, i as f64 * 45.0, 320.0, 45.0), *c);
        }
        for i in 0..6 {
            let x = (i as f64 * 97.0 - self.cam.position.x * 0.3 + self.time * 6.0)
                .rem_euclid(420.0)
                - 50.0;
            let y = 14.0 + (i % 3) as f64 * 18.0;
            let dst = Rect::from_xywh(x.round(), y, 32.0, 32.0);
            self.sheet.draw(s, CLOUD, dst, &nearest);
        }
        self.cam.draw(s, |s| {
            let view = self.cam.visible_rect();
            self.map.draw(s, &self.sheet, view, &nearest);
            for (i, c) in self.coins.iter().enumerate() {
                let frame = self.coin.frame_at(self.time + i as f64 * 0.2);
                let bob = (self.time * 3.0 + i as f64).sin() * 2.0;
                let at = Point::new(c.x, (c.y + bob).round());
                if view
                    .intersection(Rect::from_xywh(at.x, at.y, 16.0, 16.0))
                    .is_some()
                {
                    self.sheet.draw_at(s, frame, at, &nearest);
                }
            }
            let frame = if self.walking() {
                self.walk.frame_at(self.time)
            } else {
                IDLE
            };
            let at = Point::new(self.hero.x.round(), self.hero.y);
            self.sheet
                .draw_at(s, frame, at, &nearest.flip_x(self.facing_left));
        });
    }

    /// Paint the canvas, counting frames.
    fn paint(&mut self, c: &mut Canvas) {
        let start = Instant::now();
        let game = &*self;
        game.vp.paint(c, |s| game.draw(s));
        self.paint_time += start.elapsed();
        self.frames += 1;
        let elapsed = self.since.elapsed();
        if elapsed >= Duration::from_secs(1) {
            println!(
                "spritedemo: {:.0} fps, {:.2} ms per paint",
                self.frames as f64 / elapsed.as_secs_f64(),
                self.paint_time.as_secs_f64() * 1000.0 / self.frames as f64
            );
            self.frames = 0;
            self.paint_time = Duration::ZERO;
            self.since = Instant::now();
        }
    }

    fn click(&mut self, p: Point) {
        if let Some(v) = self.vp.pointer_to_virtual(p) {
            let world = self.cam.screen_to_world(v);
            let limit = (MAP_COLS * TILE - TILE) as f64;
            self.target = Some((world.x - 8.0).clamp(0.0, limit));
            self.rest = 0.0;
            self.cam.add_trauma(0.5);
        }
    }
}

fn app(game: Rc<RefCell<Game>>) -> App<()> {
    App::new((), move |_: &(), cx: &mut Cx<()>| {
        let tick = {
            let mut g = game.borrow_mut();
            g.advance(cx.frame_time());
            g.ticks
        };
        cx.request_frame();
        let (paint, click) = (game.clone(), game.clone());
        Element::canvas(GAME, move |c: &mut Canvas| paint.borrow_mut().paint(c))
            // A new revision each frame repaints the canvas.
            .revision(tick)
            .on_pointer(cx, move |e: &PointerEvent, _: &mut ()| {
                if e.kind == PointerEventKind::Down {
                    click.borrow_mut().click(e.position);
                }
            })
            .grow(1.0)
    })
    .title("Stipple sprites")
    .logical_size(Size::new(1280.0, 720.0))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--bench") {
        return bench();
    }
    if let Some(i) = args.iter().position(|a| a == "--png") {
        let path = args.get(i + 1).expect("--png FILE");
        let game = Rc::new(RefCell::new(Game::new()));
        for _ in 0..150 {
            game.borrow_mut().step(1.0 / 60.0);
        }
        let mut app = app(game).scale(ScaleFactor::IDENTITY);
        let frame = app.render_once();
        std::fs::write(path, png(&frame)).expect("write the PNG");
        println!("spritedemo: wrote {path}");
        return;
    }
    let game = Rc::new(RefCell::new(Game::new()));
    // The view is rebuilt every frame (it asks for the next one), which
    // advances the game and bumps the canvas's revision so it repaints.
    app(game).run();
}

/// The benchmark: a 1920×1080 frame of a background and ~2000 32×32 sprites.
fn bench() {
    const W: f64 = 1920.0;
    const H: f64 = 1080.0;
    const SPRITES: usize = 2000;
    // A 256×256 sheet of 64 sprites: discs with soft (alpha) edges.
    let mut p = Painter::new(256, 256);
    for i in 0..64 {
        let (ox, oy) = ((i % 8) * 32, (i / 8) * 32);
        let c = [
            (i * 37 % 256) as u8,
            (i * 91 % 256) as u8,
            (i * 53 % 256) as u8,
        ];
        for y in 0..32 {
            for x in 0..32 {
                let d = ((x as f64 - 15.5).powi(2) + (y as f64 - 15.5).powi(2)).sqrt();
                let a = ((15.5 - d) * 255.0).clamp(0.0, 255.0) as u8;
                p.px(ox + x, oy + y, [c[0], c[1], c[2], a]);
            }
        }
    }
    let sheet = SpriteSheet::grid(Arc::new(p.into_pixmap()), 32, 32);
    let mut bg = Painter::new(W as u32, H as u32);
    for y in 0..H as i32 {
        for x in 0..W as i32 {
            bg.px(x, y, [(x / 8) as u8, (y / 5) as u8, 80, 255]);
        }
    }
    let bg = Arc::new(bg.into_pixmap());
    // Pre-cut sprites, for drawing through the general path.
    let cut: Vec<Arc<Pixmap>> = (0..sheet.len())
        .map(|i| {
            let r = sheet.frame(i).unwrap();
            let src = sheet.image();
            let mut data = Vec::with_capacity(32 * 32 * 4);
            for y in r.min_y() as usize..r.max_y() as usize {
                let at = y * src.stride() + r.min_x() as usize * 4;
                data.extend_from_slice(&src.as_bytes()[at..at + 32 * 4]);
            }
            Arc::new(Pixmap::from_rgba8(PhysicalSize::new(32, 32), data))
        })
        .collect();
    let spot = |i: usize| {
        let h = hash(i as i32, 7, 11);
        (
            (h % (W as u32 - 32)) as f64,
            ((h >> 12) % (H as u32 - 32)) as f64,
        )
    };

    type Draw<'a> = Box<dyn Fn(&mut Scene) + 'a>;
    let cases: Vec<(&str, usize, Draw)> = vec![
        (
            "before: draw_image (general rasterizer path)",
            3,
            Box::new(|s: &mut Scene| {
                s.draw_image(Rect::from_xywh(0.0, 0.0, W, H), bg.clone());
                for i in 0..SPRITES {
                    let (x, y) = spot(i);
                    s.draw_image(Rect::from_xywh(x, y, 32.0, 32.0), cut[i % 64].clone());
                }
            }),
        ),
        (
            "fast: unscaled regions",
            30,
            Box::new(|s: &mut Scene| {
                s.draw_image_with(Rect::from_xywh(0.0, 0.0, W, H), &bg, &ImageOptions::new());
                for i in 0..SPRITES {
                    let (x, y) = spot(i);
                    let dst = Rect::from_xywh(x, y, 32.0, 32.0);
                    sheet.draw(s, i % 64, dst, &ImageOptions::new());
                }
            }),
        ),
        (
            "fast: 2x nearest, flipped (16x16 regions to 32x32)",
            30,
            Box::new(|s: &mut Scene| {
                s.draw_image_with(Rect::from_xywh(0.0, 0.0, W, H), &bg, &ImageOptions::new());
                for i in 0..SPRITES {
                    let (x, y) = spot(i);
                    let src = sheet.frame(i % 64).unwrap();
                    let src = Rect::from_xywh(src.min_x() + 8.0, src.min_y() + 8.0, 16.0, 16.0);
                    let o = ImageOptions::new().nearest().flip_x(i % 2 == 0);
                    s.draw_image_region(src, Rect::from_xywh(x, y, 32.0, 32.0), sheet.image(), &o);
                }
            }),
        ),
        (
            "fast: tinted and faded",
            30,
            Box::new(|s: &mut Scene| {
                s.draw_image_with(Rect::from_xywh(0.0, 0.0, W, H), &bg, &ImageOptions::new());
                for i in 0..SPRITES {
                    let (x, y) = spot(i);
                    let o = ImageOptions::new()
                        .tint(Color::rgb(255, 180, 120))
                        .opacity(0.8);
                    sheet.draw(s, i % 64, Rect::from_xywh(x, y, 32.0, 32.0), &o);
                }
            }),
        ),
        (
            "bilinear at fractional positions",
            30,
            Box::new(|s: &mut Scene| {
                s.draw_image_with(Rect::from_xywh(0.0, 0.0, W, H), &bg, &ImageOptions::new());
                for i in 0..SPRITES {
                    let (x, y) = spot(i);
                    let dst = Rect::from_xywh(x + 0.37, y + 0.61, 32.0, 32.0);
                    sheet.draw(s, i % 64, dst, &ImageOptions::new());
                }
            }),
        ),
        (
            "rotated (bilinear)",
            10,
            Box::new(|s: &mut Scene| {
                s.draw_image_with(Rect::from_xywh(0.0, 0.0, W, H), &bg, &ImageOptions::new());
                for i in 0..SPRITES {
                    let (x, y) = spot(i);
                    let o = ImageOptions::new().rotate(i as f64 * 0.1);
                    sheet.draw(s, i % 64, Rect::from_xywh(x, y, 32.0, 32.0), &o);
                }
            }),
        ),
    ];
    println!("spritedemo bench: 1920x1080, a background image and {SPRITES} 32x32 sprites");
    for (name, runs, draw) in &cases {
        let mut total = Duration::ZERO;
        for _ in 0..*runs {
            let start = Instant::now();
            let mut s = Scene::new(Size::new(W, H));
            draw(&mut s);
            let out = SoftwareRenderer::new().render(s, ScaleFactor::IDENTITY);
            std::hint::black_box(&out);
            total += start.elapsed();
        }
        println!(
            "  {name}: {:.2} ms/frame",
            total.as_secs_f64() * 1000.0 / *runs as f64
        );
    }
}

/// `pm` as an uncompressed PNG.
fn png(pm: &Pixmap) -> Vec<u8> {
    fn crc(data: &[u8]) -> u32 {
        let mut c = 0xffff_ffffu32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc(&body).to_be_bytes());
    }
    let (w, h) = (pm.size().width, pm.size().height);
    let mut raw = Vec::with_capacity((w as usize * 4 + 1) * h as usize);
    for row in pm.as_bytes().chunks(w as usize * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    // zlib with stored deflate blocks.
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, b) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        z.extend_from_slice(&(b.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(b.len() as u16)).to_le_bytes());
        z.extend_from_slice(b);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}
