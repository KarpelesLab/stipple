//! A custom-paint canvas drawing a terminal-sized glyph grid (120×36 cells)
//! that a background thread animates at the display's refresh rate, the way a
//! terminal's output thread would. Each tick changes a few rows and
//! invalidates just those rows: the view isn't rebuilt, the canvas's paint
//! callback is asked for those rows alone, and only they are re-rasterized and
//! presented. A second canvas, the status line, shows the counts.
//!
//! The grid uses what a terminal needs: background runs, glyph runs placed
//! cell by cell with a color per run, an undercurl (a Bézier path), a dashed
//! underline, a rounded cursor, and an RGBA bitmap (think sixel) that is drawn
//! from the same `Arc` every time without being copied.
//!
//! With `--selftest`, it runs 180 ticks after the first frame, then checks
//! with [`RasterStats`] that the pixels rasterized add up to the invalidated
//! rows (not to whole frames), prints `canvasdemo: selftest ok` (or
//! `FAILED`), and quits. CI runs it that way.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use stipple::core::CanvasId;
use stipple::prelude::*;
use stipple::render::RasterStats;

const COLS: usize = 120;
const ROWS: usize = 36;
const CELL_W: f64 = 8.0;
const CELL_H: f64 = 17.0;
const FONT_SIZE: f64 = 13.0;
const STATUS_H: f64 = 24.0;
const PAD: f64 = 8.0;

const GRID: CanvasId = CanvasId(1);
const STATUS: CanvasId = CanvasId(2);

/// Ticks the selftest runs for (3 s at 60 Hz).
const SELFTEST_TICKS: u64 = 180;

const BG: Color = Color::rgb(0x12, 0x14, 0x1a);
const FG: [Color; 6] = [
    Color::rgb(0xd8, 0xdc, 0xe4),
    Color::rgb(0x7e, 0xc6, 0x99),
    Color::rgb(0xe5, 0xc0, 0x7b),
    Color::rgb(0x61, 0xaf, 0xef),
    Color::rgb(0xe0, 0x6c, 0x75),
    Color::rgb(0xc6, 0x78, 0xdd),
];
const ROW_BG: [Color; 3] = [
    BG,
    Color::rgb(0x1c, 0x22, 0x2e),
    Color::rgb(0x2a, 0x1e, 0x24),
];

#[derive(Clone, Copy)]
struct Cell {
    ch: char,
    fg: u8,
    bg: u8,
}

/// What the grid shows: written by the animation thread, read by the paint
/// callback.
struct Screen {
    cells: Vec<Cell>,
    cursor: (usize, usize),
    rng: u64,
    tick: u64,
}

impl Screen {
    fn new() -> Self {
        let mut s = Screen {
            cells: vec![
                Cell {
                    ch: ' ',
                    fg: 0,
                    bg: 0
                };
                COLS * ROWS
            ],
            cursor: (0, ROWS - 1),
            rng: 0x2545_f491_4f6c_dd1d,
            tick: 0,
        };
        for row in 0..ROWS {
            s.write_line(row);
        }
        s
    }

    fn next(&mut self) -> u64 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// Fill `row` with a made-up log line.
    fn write_line(&mut self, row: usize) {
        const WORDS: [&str; 12] = [
            "GET",
            "/api/v1/items",
            "200",
            "ok",
            "cache",
            "miss",
            "→",
            "worker-3",
            "λ",
            "12ms",
            "retry",
            "ünïcödé",
        ];
        let mut line: Vec<(char, u8)> = Vec::with_capacity(COLS);
        let stamp = format!("{:06} ", self.tick * 7 + row as u64);
        line.extend(stamp.chars().map(|c| (c, 5)));
        while line.len() < COLS {
            let r = self.next();
            let word = WORDS[(r % WORDS.len() as u64) as usize];
            let fg = (r >> 8) as u8 % FG.len() as u8;
            line.extend(word.chars().map(|c| (c, fg)));
            line.push((' ', 0));
        }
        let bg = if self.next().is_multiple_of(9) {
            1 + (row % 2) as u8
        } else {
            0
        };
        for (col, &(ch, fg)) in line.iter().take(COLS).enumerate() {
            self.cells[row * COLS + col] = Cell { ch, fg, bg };
        }
    }

    /// One animation step: a few rows get new lines and the cursor moves.
    /// Returns the rows that changed.
    fn step(&mut self) -> Vec<usize> {
        self.tick += 1;
        let mut changed = Vec::new();
        let a = (self.tick as usize * 5) % ROWS;
        let b = (self.next() % ROWS as u64) as usize;
        for row in [a, b] {
            self.write_line(row);
            changed.push(row);
        }
        // The cursor "types" along the bottom row.
        let (col, row) = self.cursor;
        let ch = char::from(b'a' + (self.tick % 26) as u8);
        self.cells[row * COLS + col] = Cell { ch, fg: 0, bg: 0 };
        self.cursor = ((col + 1) % COLS, row);
        changed.push(row);
        changed.sort_unstable();
        changed.dedup();
        changed
    }
}

/// Counters the status line and the selftest read.
static FIRST_PAINT: AtomicBool = AtomicBool::new(false);
static ROWS_PAINTED: AtomicU64 = AtomicU64::new(0);
static ROWS_INVALIDATED: AtomicU64 = AtomicU64::new(0);
static STATUS_PAINTS: AtomicU64 = AtomicU64::new(0);
static TICKS: AtomicU64 = AtomicU64::new(0);
static SCALE_BITS: AtomicU64 = AtomicU64::new(0);

fn row_rect(row: usize) -> Rect {
    Rect::from_xywh(0.0, row as f64 * CELL_H, COLS as f64 * CELL_W, CELL_H)
}

/// A 64×64 RGBA bitmap standing in for an inline image (sixel, iTerm2).
fn logo() -> Arc<Pixmap> {
    let n = 64u32;
    let mut data = Vec::with_capacity((n * n * 4) as usize);
    for y in 0..n {
        for x in 0..n {
            let (dx, dy) = (x as f64 - 31.5, y as f64 - 31.5);
            let d = (dx * dx + dy * dy).sqrt();
            let a = if d < 30.0 { 230 } else { 0 };
            data.extend_from_slice(&[(x * 4) as u8, (y * 4) as u8, 200, a]);
        }
    }
    Arc::new(Pixmap::from_rgba8(PhysicalSize::new(n, n), data))
}

/// The grid's paint callback: every row crossing the dirty area.
fn paint_grid(c: &mut Canvas, screen: &Mutex<Screen>, font: Option<&Font>, image: &Arc<Pixmap>) {
    SCALE_BITS.store(c.scale().to_bits(), Ordering::Relaxed);
    FIRST_PAINT.store(true, Ordering::Relaxed);
    let screen = screen.lock().unwrap();
    let dirty = c.dirty();
    let first = (dirty.min_y() / CELL_H).floor().max(0.0) as usize;
    let last = ((dirty.max_y() / CELL_H).ceil() as usize).min(ROWS);
    for row in first..last {
        let y = row as f64 * CELL_H;
        let cells = &screen.cells[row * COLS..(row + 1) * COLS];
        // Backgrounds, a rect per run of equal color.
        let mut start = 0;
        for col in 1..=COLS {
            if col == COLS || cells[col].bg != cells[start].bg {
                let r = Rect::from_xywh(
                    start as f64 * CELL_W,
                    y,
                    (col - start) as f64 * CELL_W,
                    CELL_H,
                );
                c.fill_rect(r, ROW_BG[cells[start].bg as usize]);
                start = col;
            }
        }
        // Glyphs, a run per color, each cell at its exact place.
        if let Some(font) = font {
            let baseline =
                y + font.baseline(FONT_SIZE) + (CELL_H - font.line_height(FONT_SIZE)) / 2.0;
            // The yellow words in bold: a run per font too.
            let bold = font.bold();
            for (i, color) in FG.iter().enumerate() {
                let face = if i == 2 { &bold } else { font };
                let mut run = GlyphRun::new(face, FONT_SIZE, *color);
                let mut buf = [0u8; 4];
                for (col, cell) in cells.iter().enumerate() {
                    if cell.fg as usize == i && cell.ch != ' ' {
                        run.push(
                            cell.ch.encode_utf8(&mut buf),
                            Point::new(col as f64 * CELL_W, baseline),
                        );
                    }
                }
                c.fill_glyph_run(&run);
            }
        }
        // Decorations: an undercurl and a dashed underline on some rows.
        if row % 7 == 3 {
            let mut wave = Path::new();
            let base = y + CELL_H - 2.5;
            wave.move_to(Point::new(16.0 * CELL_W, base));
            for i in 0..24 {
                let x = (16 + i) as f64 * CELL_W;
                wave.quad_to(
                    Point::new(x + CELL_W / 2.0, base + if i % 2 == 0 { -2.5 } else { 2.5 }),
                    Point::new(x + CELL_W, base),
                );
            }
            c.stroke_path(&wave, FG[4], &StrokeStyle::new(1.2));
        }
        if row % 11 == 5
            && let Some(font) = font
        {
            // A dashed underline from the font's metrics.
            let baseline =
                y + font.baseline(FONT_SIZE) + (CELL_H - font.line_height(FONT_SIZE)) / 2.0;
            c.draw_decoration(
                &font.metrics(FONT_SIZE),
                Point::new(50.0 * CELL_W, baseline),
                30.0 * CELL_W,
                &Decoration::underline(UnderlineStyle::Dashed),
                FG[3],
            );
        }
        if row == screen.cursor.1 {
            let x = screen.cursor.0 as f64 * CELL_W;
            c.stroke_round_rect(
                Rect::from_xywh(x + 0.5, y + 0.5, CELL_W - 1.0, CELL_H - 1.0),
                2.0,
                FG[1],
                1.0,
            );
        }
        ROWS_PAINTED.fetch_add(1, Ordering::Relaxed);
    }
    // The inline image, top right: drawn from the same `Arc` every time.
    let at = Rect::from_xywh((COLS - 10) as f64 * CELL_W, CELL_H, 64.0, 64.0);
    if c.needs_paint(at) {
        c.draw_image(at, image.clone());
    }
}

/// The status line's paint callback.
fn paint_status(c: &mut Canvas, font: Option<&Font>) {
    STATUS_PAINTS.fetch_add(1, Ordering::Relaxed);
    let b = c.bounds();
    c.fill_rect(b, Color::rgb(0x22, 0x26, 0x30));
    let Some(font) = font else {
        return;
    };
    let stats = RasterStats::get();
    let text = format!(
        "tick {} · {} rows invalidated · {} rows painted · {:.1} Mpx rasterized",
        TICKS.load(Ordering::Relaxed),
        ROWS_INVALIDATED.load(Ordering::Relaxed),
        ROWS_PAINTED.load(Ordering::Relaxed),
        stats.pixels as f64 / 1e6
    );
    c.fill_text(
        font,
        &text,
        Point::new(8.0, (STATUS_H - font.line_height(12.0)) / 2.0),
        12.0,
        FG[0],
    );
}

/// The animation: a tick per display refresh, invalidating the rows it
/// changed. In the selftest, checks the counts at the end and quits.
fn animate(screen: Arc<Mutex<Screen>>, handle: AppHandle<()>, selftest: bool) {
    // Start once the first frame is up, so it isn't counted.
    while !FIRST_PAINT.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(300));
    RasterStats::take();
    ROWS_PAINTED.store(0, Ordering::Relaxed);
    let status_paints = STATUS_PAINTS.load(Ordering::Relaxed);
    loop {
        std::thread::sleep(stipple::platform::frame::interval());
        let tick = TICKS.fetch_add(1, Ordering::Relaxed) + 1;
        let changed = screen.lock().unwrap().step();
        for row in &changed {
            handle.invalidate_canvas_rect(GRID, row_rect(*row));
        }
        ROWS_INVALIDATED.fetch_add(changed.len() as u64, Ordering::Relaxed);
        if tick.is_multiple_of(30) {
            handle.invalidate_canvas(STATUS);
        }
        if selftest && tick >= SELFTEST_TICKS {
            // Let the last ticks reach the screen.
            std::thread::sleep(Duration::from_millis(300));
            let status_paints = STATUS_PAINTS.load(Ordering::Relaxed) - status_paints;
            check(status_paints);
            handle.exit();
            return;
        }
    }
}

static FAILED: AtomicBool = AtomicBool::new(false);

/// The selftest's verdict: what was rasterized adds up to the rows that were
/// invalidated (plus the status line), far below whole frames.
fn check(status_paints: u64) {
    let stats = RasterStats::take();
    let scale = f64::from_bits(SCALE_BITS.load(Ordering::Relaxed));
    let ticks = TICKS.load(Ordering::Relaxed);
    let invalidated = ROWS_INVALIDATED.load(Ordering::Relaxed);
    let painted = ROWS_PAINTED.load(Ordering::Relaxed);
    let px = |w: f64, h: f64| (w * scale).ceil() * (h * scale).ceil();
    let row_px = px(COLS as f64 * CELL_W, CELL_H);
    let status_px = px(COLS as f64 * CELL_W, STATUS_H);
    let window = window_size();
    let full_px = px(window.width, window.height);
    // Each invalidated row is rasterized once at most (rows invalidated twice
    // before a present are rasterized once), plus the status line; two whole
    // frames are allowed for an expose from the window system. At a
    // fractional scale a row's edges round outward: a pixel row more.
    let pixel_row = (COLS as f64 * CELL_W * scale).ceil();
    let budget = invalidated as f64 * (row_px + pixel_row)
        + status_paints as f64 * (status_px + pixel_row)
        + 2.0 * full_px;
    let per_tick = stats.pixels as f64 / ticks as f64;
    println!(
        "canvasdemo: {ticks} ticks, {invalidated} rows invalidated, {painted} rows painted, \
         {} px rasterized in {} runs ({per_tick:.0} px/tick; a whole frame is {full_px:.0} px) at {scale}x",
        stats.pixels, stats.runs
    );
    let mut problems = Vec::new();
    if painted == 0 || stats.pixels == 0 {
        problems.push("nothing was repainted".to_string());
    }
    // At a fractional scale a row's edge pixels are shared with its
    // neighbors, which are then asked to paint that sliver too.
    let per_row = if (CELL_H * scale).fract() == 0.0 {
        1
    } else {
        3
    };
    if painted > per_row * invalidated + 2 * ROWS as u64 {
        problems.push(format!(
            "{painted} rows painted for {invalidated} invalidated"
        ));
    }
    if stats.pixels as f64 > budget {
        problems.push(format!(
            "{} px rasterized, over the {budget:.0} px the invalidated rows account for",
            stats.pixels
        ));
    }
    if per_tick > full_px / 4.0 {
        problems.push(format!(
            "{per_tick:.0} px per tick is a quarter of a frame or more"
        ));
    }
    if problems.is_empty() {
        println!("canvasdemo: selftest ok");
    } else {
        println!("canvasdemo: selftest FAILED: {}", problems.join("; "));
        FAILED.store(true, Ordering::Relaxed);
    }
}

fn window_size() -> Size {
    Size::new(
        COLS as f64 * CELL_W + 2.0 * PAD,
        ROWS as f64 * CELL_H + STATUS_H + 3.0 * PAD,
    )
}

fn main() {
    let selftest = std::env::args().any(|a| a == "--selftest");
    let screen = Arc::new(Mutex::new(Screen::new()));
    let font: Option<Font> = Font::system_default();
    let image = logo();

    let (grid_screen, grid_font, status_font) = (screen.clone(), font.clone(), font.clone());
    let app = App::new((), move |_: &(), _: &mut Cx<()>| {
        let (screen, font, image) = (grid_screen.clone(), grid_font.clone(), image.clone());
        let status_font = status_font.clone();
        column(vec![
            Element::canvas(STATUS, move |c: &mut Canvas| {
                paint_status(c, status_font.as_ref())
            })
            .width(COLS as f64 * CELL_W)
            .height(STATUS_H),
            Element::canvas(GRID, move |c: &mut Canvas| {
                paint_grid(c, &screen, font.as_ref(), &image)
            })
            .width(COLS as f64 * CELL_W)
            .height(ROWS as f64 * CELL_H),
        ])
        .gap(PAD)
        .padding(Insets::uniform(PAD))
        .fill(BG)
    })
    .title("Stipple canvas")
    .theme(Theme::dark())
    .logical_size(window_size());

    let handle = app.handle();
    std::thread::spawn(move || animate(screen, handle, selftest));
    app.run();
    if selftest {
        println!("canvasdemo: event loop exited");
        if FAILED.load(Ordering::Relaxed) || TICKS.load(Ordering::Relaxed) < SELFTEST_TICKS {
            std::process::exit(1);
        }
    }
}
