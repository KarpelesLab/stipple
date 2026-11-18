//! Stipple's benchmarks: named scenarios, each timed over a fixed number of
//! iterations after a warm-up, with fixed sizes and a fixed seed, reported
//! as the median and the 95th percentile.
//!
//! ```sh
//! cargo run --release --example bench                          # the software renderer
//! cargo run --release --example bench --features mtl -- --renderer metal
//! cargo run --release --example bench -- --json out.json --only hover,type_char
//! ```
//!
//! Options: `--renderer software|gpu|metal|vulkan|gl|d3d11` (the GPU ones
//! need the backend's feature: `mtl`, `vk`, `gl`, `d3d`; `gpu` is the
//! platform's preferred one), `--json FILE`, `--only a,b` (by
//! scenario name), `--quick` (a third of the iterations), `--runner NAME`
//! (a label stored in the JSON), `--list`.
//!
//! The scenarios (see `docs/performance.md`):
//!
//! - `first_frame_1x`, `first_frame_2x`: the whole first frame of a window
//!   full of widgets (a toolbar, a text field, 32 rows of checkboxes,
//!   switches, sliders, buttons and progress bars, a 10,000-row list) at
//!   1024×768, at scale 1 and 2.
//! - `hover`: the pointer moving from one button to another, to the frame
//!   ready (only the damage is repainted): time, and pixels rasterized.
//! - `scroll_list`: the 10,000-row `virtual_list` scrolled by a wheel step.
//! - `type_char`: a character typed into the focused text field.
//! - `theme_switch`: a button switching light and dark.
//! - `sprites_2000`: 2000 32×32 sprites over a background at 1920×1080 (the
//!   `spritedemo --bench` scene), read back.
//! - `gallery_4k`: the widget window at 3840×2160 (1920×1080 at scale 2),
//!   a whole frame drawn from its scene; on a GPU renderer drawn offscreen
//!   without a read-back (what a frame presented to a window costs), with
//!   the texture memory the GPU's caches hold after it (`texture_mb`).
//! - `sprites_10k`: 10 000 sprites from a `SpriteSheet` over a background at
//!   3840×2160, likewise.
//! - `canvas_row`: one row of a 120×40 grid of characters (a terminal's
//!   glyph run) drawn in an `Element::canvas` changed and invalidated, to the frame ready: on the
//!   GPU renderers only that row is rasterized and uploaded into the
//!   canvas's texture (`upload_kb`).
//! - `transition`: each frame of a page pushed and popped on a `nav_stack`
//!   and a bottom sheet opened and dismissed, on a phone-sized window
//!   (390×844 at scale 3), animating. On a phone (see
//!   `docs/performance.md`) each should fit in 8.3 ms (120 Hz).
//! - `layout_tree`: laying out a tree of ~4,000 elements.
//! - `text_layout`, `text_layout_cold`, `text_layout_cjk`: laying out a long
//!   paragraph with shaping cached, with a fresh font, and in Japanese
//!   through the system's CJK fallback (when it has one).
//! - `startup`: loading the font, making the app and drawing its first frame.
//! - `calibration`: a fixed CPU loop, which the comparison divides by to
//!   take out the machine's speed (`tools/bench/compare.py`).
//!
//! The interaction scenarios time the input and the frame that answers it
//! (built, laid out, painted, rasterized or drawn on the GPU and read back):
//! what an app spends between an event and a frame it can present.
//!
//! In the browser (`bench_web`, `tools/bench/index.html`) the same scenarios
//! run on the software renderer, and the page also times events dispatched
//! to this app running live to the frame presented after them, on each
//! renderer.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use stipple::core::CanvasId;
use stipple::game::SpriteSheet;
use stipple::platform::time::Instant;
use stipple::prelude::*;
use stipple::render::{ImageOptions, RasterStats, Renderer, Scene, SoftwareRenderer, Surface};

/// The window the app scenarios draw.
const W: f64 = 1024.0;
const H: f64 = 768.0;
/// Rows in the virtual list.
const ROWS: usize = 10_000;
/// Rows of widgets in the grid.
const WIDGET_ROWS: usize = 32;
/// The one seed every generated thing comes from.
const SEED: u64 = 0x5717_9913_2024_0001;

// --- The app ----------------------------------------------------------------

struct State {
    dark: bool,
    name: TextInput,
    checks: Vec<bool>,
    levels: Vec<f64>,
    list: ListState,
    taps: u32,
}

impl State {
    fn new() -> Self {
        let mut rng = Rng(SEED);
        Self {
            dark: false,
            name: TextInput::new(),
            checks: (0..WIDGET_ROWS * 2).map(|_| rng.next() & 1 == 0).collect(),
            levels: (0..WIDGET_ROWS).map(|_| rng.unit()).collect(),
            list: ListState::new(SelectionMode::Single),
            taps: 0,
        }
    }
}

fn view(s: &State, cx: &mut Cx<State>) -> Element {
    let sp = cx.theme().spacing;
    let bar = row(vec![
        heading(cx, "Benchmark"),
        spacer(),
        label(cx, format!("{} taps", s.taps)),
        button(cx, "Theme").on_tap(cx, |s| s.dark = !s.dark),
    ])
    .gap(sp.md)
    .align(Align::Center, Align::Center);
    let field = text_field(cx, &s.name, |s| &mut s.name)
        .placeholder("Type here")
        .width(420.0);
    let mut cells = Vec::with_capacity(WIDGET_ROWS * 6);
    for i in 0..WIDGET_ROWS {
        cells.push(checkbox(
            cx,
            format!("Option {i}"),
            s.checks[2 * i],
            move |s| s.checks[2 * i] = !s.checks[2 * i],
        ));
        cells.push(switch(cx, "On", s.checks[2 * i + 1], move |s| {
            s.checks[2 * i + 1] = !s.checks[2 * i + 1]
        }));
        cells.push(slider(cx, s.levels[i], move |s, v| s.levels[i] = v).width(110.0));
        cells.push(button(cx, format!("Action {i}")).on_tap(cx, |s| s.taps += 1));
        cells.push(progress_bar(cx, s.levels[i]).width(80.0));
        cells.push(label(cx, format!("{:.0}%", s.levels[i] * 100.0)));
    }
    let widgets = grid(
        vec![
            Track::fr(1.0),
            Track::px(70.0),
            Track::px(120.0),
            Track::px(110.0),
            Track::px(90.0),
            Track::px(44.0),
        ],
        cells,
    )
    .gap(sp.sm)
    .align(Align::Start, Align::Center);
    let widgets = scroll(cx).child(widgets).width(700.0);
    let list = virtual_list(
        cx,
        &s.list,
        |s| &mut s.list,
        VirtualList::fixed(ROWS, 24.0).height(620.0),
        |cx, row| {
            label(
                cx,
                format!("Row {} · item {:05}", row.index, row.index * 7 % 9973),
            )
        },
    )
    .width(260.0);
    column(vec![
        bar,
        field,
        row(vec![widgets, list]).gap(sp.lg).grow(1.0),
    ])
    .gap(sp.md)
    .padding(Insets::uniform(sp.lg))
}

fn make_app(font: &Font, scale: f64, renderer: Renderer) -> App<State> {
    App::new(State::new(), view)
        .theme_with(|s, _| {
            if s.dark {
                Theme::dark()
            } else {
                Theme::light()
            }
        })
        .logical_size(Size::new(W, H))
        .scale(ScaleFactor::new(scale))
        .font(font.clone())
        .renderer(renderer)
}

/// The app in a window (the browser's interaction latency runs on it).
#[cfg(target_arch = "wasm32")]
fn main() {
    App::new(State::new(), view)
        .theme_with(|s, _| {
            if s.dark {
                Theme::dark()
            } else {
                Theme::light()
            }
        })
        .title("Stipple benchmark")
        .logical_size(Size::new(W, H))
        .run();
}

// --- Measuring ----------------------------------------------------------------

/// A small deterministic generator (SplitMix64).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn ms_since(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// One scenario's result.
struct Outcome {
    name: &'static str,
    /// What drew it: the renderer, or "cpu" for one that draws nothing.
    renderer: String,
    samples: Vec<f64>,
    warmup: usize,
    /// Other figures (pixels rasterized per frame), as medians.
    extra: Vec<(&'static str, f64)>,
    note: String,
}

impl Outcome {
    fn sorted(&self) -> Vec<f64> {
        let mut s = self.samples.clone();
        s.sort_by(f64::total_cmp);
        s
    }

    fn median(&self) -> f64 {
        median(&self.sorted())
    }

    fn p95(&self) -> f64 {
        let s = self.sorted();
        if s.is_empty() {
            return 0.0;
        }
        let rank = ((0.95 * s.len() as f64).ceil() as usize).clamp(1, s.len());
        s[rank - 1]
    }
}

fn median(sorted: &[f64]) -> f64 {
    match sorted.len() {
        0 => 0.0,
        n if n % 2 == 1 => sorted[n / 2],
        n => (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0,
    }
}

/// Run `f` `warmup` times untimed, then `n` times, each returning the
/// milliseconds its measured part took.
fn sample(warmup: usize, n: usize, mut f: impl FnMut(usize) -> f64) -> Vec<f64> {
    for i in 0..warmup {
        f(i);
    }
    (0..n).map(|i| f(warmup + i)).collect()
}

/// What the scenarios draw with (in the browser, the software renderer: its
/// GPU renderers draw into the page's canvas only).
#[derive(Clone, Copy, PartialEq)]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
enum Target {
    Software,
    /// The platform's GPU API, by name.
    Gpu(&'static str),
}

impl Target {
    fn name(self) -> &'static str {
        match self {
            Target::Software => "software",
            Target::Gpu(n) => n,
        }
    }

    fn renderer(self) -> Renderer {
        match self {
            Target::Software => Renderer::Software,
            Target::Gpu(_) => Renderer::Gpu,
        }
    }
}

/// What a run measures.
struct Plan<'a> {
    target: Target,
    font: Font,
    /// Iterations are multiplied by this (`--quick`: a third).
    scale: f64,
    only: &'a [String],
    log: &'a dyn Fn(&str),
}

impl Plan<'_> {
    fn n(&self, n: usize) -> usize {
        ((n as f64 * self.scale).round() as usize).max(3)
    }

    fn wants(&self, name: &str) -> bool {
        self.only.is_empty() || self.only.iter().any(|o| o == name)
    }
}

/// Everything in `plan`, in order.
fn run(plan: &Plan) -> Vec<Outcome> {
    type Scenario = fn(&Plan) -> Option<Outcome>;
    // Scenarios that draw nothing run on the software renderer's run only.
    let all: [(&str, bool, Scenario); 17] = [
        ("calibration", true, calibration),
        ("startup", false, startup),
        ("first_frame_1x", false, |p| {
            first_frame(p, "first_frame_1x", 1.0)
        }),
        ("first_frame_2x", false, |p| {
            first_frame(p, "first_frame_2x", 2.0)
        }),
        ("hover", false, hover),
        ("scroll_list", false, scroll_list),
        ("type_char", false, type_char),
        ("theme_switch", false, theme_switch),
        ("sprites_2000", false, sprites),
        ("gallery_4k", false, gallery_4k),
        ("sprites_10k", false, sprites_10k),
        ("canvas_row", false, canvas_row),
        ("transition", false, transition),
        ("layout_tree", true, layout_tree),
        ("text_layout", true, text_layout),
        ("text_layout_cold", true, text_layout_cold),
        ("text_layout_cjk", true, text_layout_cjk),
    ];
    let mut out = Vec::new();
    for (name, cpu_only, scenario) in all {
        // The calibration loop always runs: the comparison needs it.
        if name != "calibration" && !plan.wants(name) {
            continue;
        }
        if cpu_only && name != "calibration" && plan.target != Target::Software {
            continue;
        }
        (plan.log)(&format!("  {name}…"));
        if let Some(o) = scenario(plan) {
            out.push(o);
        }
    }
    out
}

fn outcome(plan: &Plan, name: &'static str, samples: Vec<f64>, warmup: usize) -> Outcome {
    Outcome {
        name,
        renderer: plan.target.name().into(),
        samples,
        warmup,
        extra: Vec::new(),
        note: String::new(),
    }
}

/// On the GPU, note the draw calls of the scenario's last frame.
fn draw_calls(plan: &Plan, o: &mut Outcome) {
    #[cfg(feature = "gpu")]
    if matches!(plan.target, Target::Gpu(_)) && o.renderer == plan.target.name() {
        o.extra
            .push(("draw_calls", stipple::gpu::last_frame_draw_calls() as f64));
    }
    #[cfg(not(feature = "gpu"))]
    let _ = (plan, o);
}

fn cpu_outcome(name: &'static str, samples: Vec<f64>, warmup: usize) -> Outcome {
    Outcome {
        name,
        renderer: "cpu".into(),
        samples,
        warmup,
        extra: Vec::new(),
        note: String::new(),
    }
}

// --- The scenarios ------------------------------------------------------------

/// A fixed CPU loop (integer mixing over a 256 KiB table, and some floating
/// point), about 10 ms on a desktop: how fast this machine is right now.
fn calibration(plan: &Plan) -> Option<Outcome> {
    let mut table: Vec<u32> = (0..65536u32)
        .map(|i| i.wrapping_mul(2_654_435_761))
        .collect();
    let samples = sample(2, plan.n(15).max(9), |_| {
        let t = Instant::now();
        let mut rng = Rng(SEED);
        let mut acc = 0u64;
        let mut f = 1.0f64;
        for _ in 0..6_000_000 {
            let r = rng.next();
            let i = (r as usize) & 0xffff;
            table[i] = table[i].wrapping_add(r as u32).rotate_left(5);
            acc = acc.wrapping_add(table[(r >> 20) as usize & 0xffff] as u64);
            f = f * 1.000_000_1 + (acc & 7) as f64 * 1e-9;
        }
        std::hint::black_box((acc, f));
        ms_since(t)
    });
    Some(cpu_outcome("calibration", samples, 2))
}

fn startup(plan: &Plan) -> Option<Outcome> {
    let renderer = plan.target.renderer();
    let samples = sample(1, plan.n(8), |_| {
        let t = Instant::now();
        let font = system_font();
        let mut app = make_app(font.as_ref().unwrap_or(&plan.font), 1.0, renderer);
        std::hint::black_box(app.render_once());
        ms_since(t)
    });
    Some(outcome(plan, "startup", samples, 1))
}

fn first_frame(plan: &Plan, name: &'static str, scale: f64) -> Option<Outcome> {
    let renderer = plan.target.renderer();
    let n = if scale > 1.0 { plan.n(12) } else { plan.n(20) };
    let samples = sample(2, n, |_| {
        let mut app = make_app(&plan.font, scale, renderer);
        let t = Instant::now();
        std::hint::black_box(app.render_once());
        ms_since(t)
    });
    let mut o = outcome(plan, name, samples, 2);
    draw_calls(plan, &mut o);
    Some(o)
}

/// A surface that keeps nothing: the frames are presented to it as to a
/// window's.
struct Sink(PhysicalSize);

impl Surface for Sink {
    fn resize(&mut self, size: PhysicalSize) {
        self.0 = size;
    }

    fn size(&self) -> PhysicalSize {
        self.0
    }

    fn present(&mut self, pixmap: &Pixmap, _: &[Rect]) {
        std::hint::black_box(pixmap);
    }
}

/// An app with its first frame presented, and the surface it went to.
fn ready_app(plan: &Plan) -> (App<State>, Sink) {
    let mut app = make_app(&plan.font, 1.0, plan.target.renderer());
    let mut sink = Sink(PhysicalSize::new(0, 0));
    app.present_frame(&mut sink);
    (app, sink)
}

fn center(app: &mut App<State>, role: Role, name: &str) -> Option<Point> {
    app.find(role, name).map(|n| n.bounds.center())
}

fn hover(plan: &Plan) -> Option<Outcome> {
    let (mut app, mut sink) = ready_app(plan);
    let a = center(&mut app, Role::Button, "Action 2")?;
    let b = center(&mut app, Role::Button, "Action 5")?;
    let mut pixels = Vec::new();
    let samples = sample(10, plan.n(60), |i| {
        let at = if i % 2 == 0 { a } else { b };
        RasterStats::take();
        let t = Instant::now();
        app.hover_at(at);
        std::hint::black_box(app.present_frame(&mut sink));
        let ms = ms_since(t);
        pixels.push(RasterStats::take().pixels as f64);
        ms
    });
    let mut o = outcome(plan, "hover", samples, 10);
    draw_calls(plan, &mut o);
    if plan.target == Target::Software {
        pixels.sort_by(f64::total_cmp);
        o.extra.push(("pixels", median(&pixels)));
    }
    Some(o)
}

fn scroll_list(plan: &Plan) -> Option<Outcome> {
    let (mut app, mut sink) = ready_app(plan);
    let list = center(&mut app, Role::ListItem, "Row 3 · item 00021")?;
    app.hover_at(list);
    app.present_frame(&mut sink);
    let mut pixels = Vec::new();
    let samples = sample(10, plan.n(60), |_| {
        RasterStats::take();
        let t = Instant::now();
        app.scroll_by(48.0);
        std::hint::black_box(app.present_frame(&mut sink));
        let ms = ms_since(t);
        pixels.push(RasterStats::take().pixels as f64);
        ms
    });
    let mut o = outcome(plan, "scroll_list", samples, 10);
    if plan.target == Target::Software {
        pixels.sort_by(f64::total_cmp);
        o.extra.push(("pixels", median(&pixels)));
    }
    Some(o)
}

fn type_char(plan: &Plan) -> Option<Outcome> {
    let (mut app, mut sink) = ready_app(plan);
    if !app.click(Role::TextField, "Type here") {
        return None;
    }
    app.present_frame(&mut sink);
    let samples = sample(10, plan.n(60), |i| {
        let ch = ["a", "b", "c", "d", "e"][i % 5];
        let t = Instant::now();
        app.type_text(ch);
        std::hint::black_box(app.present_frame(&mut sink));
        let ms = ms_since(t);
        // Keep the field short: every frame then types into the same text.
        app.press_key(KeyInput::Backspace);
        app.present_frame(&mut sink);
        ms
    });
    Some(outcome(plan, "type_char", samples, 10))
}

fn theme_switch(plan: &Plan) -> Option<Outcome> {
    let (mut app, mut sink) = ready_app(plan);
    let at = center(&mut app, Role::Button, "Theme")?;
    let samples = sample(3, plan.n(20), |_| {
        let t = Instant::now();
        app.click_at(at);
        std::hint::black_box(app.present_frame(&mut sink));
        ms_since(t)
    });
    Some(outcome(plan, "theme_switch", samples, 3))
}

/// A sheet of 64 32×32 round sprites, and a `w`×`h` background.
fn sprite_assets(w: u32, h: u32) -> (SpriteSheet, Arc<Pixmap>) {
    let mut sheet = vec![0u8; 256 * 256 * 4];
    for i in 0..64usize {
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
                let at = ((oy + y) * 256 + ox + x) * 4;
                // Premultiplied, as pixmaps are.
                let pm = |v: u8| (v as u32 * a as u32 / 255) as u8;
                sheet[at..at + 4].copy_from_slice(&[pm(c[0]), pm(c[1]), pm(c[2]), a]);
            }
        }
    }
    let sheet = SpriteSheet::grid(
        Arc::new(Pixmap::from_rgba8(PhysicalSize::new(256, 256), sheet)),
        32,
        32,
    );
    let mut bg = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let at = ((y * w + x) * 4) as usize;
            bg[at..at + 4].copy_from_slice(&[(x / 8) as u8, (y / 5) as u8, 80, 255]);
        }
    }
    (
        sheet,
        Arc::new(Pixmap::from_rgba8(PhysicalSize::new(w, h), bg)),
    )
}

/// A `w`×`h` scene of the background and `n` sprites cut from the sheet,
/// unscaled, at places from the fixed seed.
fn sprite_scene(n: usize, w: u32, h: u32) -> impl Fn() -> Scene {
    let (sheet, bg) = sprite_assets(w, h);
    let mut rng = Rng(SEED);
    let spots: Vec<(f64, f64)> = (0..n)
        .map(|_| {
            (
                (rng.next() % (w as u64 - 32)) as f64,
                (rng.next() % (h as u64 - 32)) as f64,
            )
        })
        .collect();
    move || {
        let mut s = Scene::new(Size::new(w as f64, h as f64));
        let opts = ImageOptions::new();
        s.draw_image_with(Rect::from_xywh(0.0, 0.0, w as f64, h as f64), &bg, &opts);
        for (i, (x, y)) in spots.iter().enumerate() {
            sheet.draw(&mut s, i % 64, Rect::from_xywh(*x, *y, 32.0, 32.0), &opts);
        }
        s
    }
}

/// The `spritedemo --bench` scene: a background and 2000 sprites cut from a
/// sheet, unscaled, at 1920×1080.
fn sprites(plan: &Plan) -> Option<Outcome> {
    let scene = sprite_scene(2000, 1920, 1080);
    let target = plan.target;
    let mut failed = None;
    let samples = sample(3, plan.n(20), |_| {
        let t = Instant::now();
        let s = scene();
        match target {
            Target::Software => {
                std::hint::black_box(SoftwareRenderer::new().render(s, ScaleFactor::IDENTITY));
            }
            Target::Gpu(_) => {
                if let Err(e) = gpu_render(&s) {
                    failed = Some(e);
                }
            }
        }
        ms_since(t)
    });
    let mut o = outcome(plan, "sprites_2000", samples, 3);
    if let Some(e) = failed {
        o.renderer = "software".into();
        o.note = format!("GPU failed: {e}");
    }
    draw_calls(plan, &mut o);
    Some(o)
}

/// 10 000 sprites at 3840×2160, drawn (on a GPU, without a read-back).
fn sprites_10k(plan: &Plan) -> Option<Outcome> {
    let scene = sprite_scene(10_000, 3840, 2160);
    draw_whole(
        plan,
        "sprites_10k",
        plan.n(12),
        &scene,
        ScaleFactor::IDENTITY,
    )
}

/// The widget window at 3840×2160: its scene built once, then drawn whole.
fn gallery_4k(plan: &Plan) -> Option<Outcome> {
    let captured = Rc::new(RefCell::new(None));
    let keep = captured.clone();
    let mut app = make_app(&plan.font, 2.0, Renderer::Software)
        .logical_size(Size::new(1920.0, 1080.0))
        .render_with(move |scene: &Scene, _, scale| {
            *keep.borrow_mut() = Some(scene.clone());
            Pixmap::new(scale.to_physical(scene.logical_size()))
        });
    app.render_once();
    let scene = captured.borrow_mut().take()?;
    // Hundreds of milliseconds a frame on the CPU (seconds in a browser):
    // fewer of those.
    let n = match plan.target {
        Target::Software => plan.n(5),
        Target::Gpu(_) => plan.n(12),
    };
    draw_whole(
        plan,
        "gallery_4k",
        n,
        &|| scene.clone(),
        ScaleFactor::new(2.0),
    )
}

/// Draw the scene `make` builds (not timed) whole at `scale`, `n` times: on
/// the CPU into a pixmap, on a GPU offscreen, waiting for it, not read back.
/// On a GPU, also the texture memory its caches hold after it (MB).
fn draw_whole(
    plan: &Plan,
    name: &'static str,
    n: usize,
    make: &dyn Fn() -> Scene,
    scale: ScaleFactor,
) -> Option<Outcome> {
    let target = plan.target;
    let mut failed = None;
    let bg = Color::rgb(0x18, 0x1b, 0x22);
    let warmup = if target == Target::Software { 1 } else { 2 };
    let samples = sample(warmup, n, |_| {
        let s = make();
        let t = Instant::now();
        match target {
            Target::Software => {
                std::hint::black_box(SoftwareRenderer::new().background(bg).render(s, scale));
            }
            Target::Gpu(_) => {
                if let Err(e) = gpu_draw(&s, bg, scale) {
                    failed = Some(e);
                }
            }
        }
        ms_since(t)
    });
    let mut o = outcome(plan, name, samples, warmup);
    if let Some(e) = failed {
        o.renderer = "software".into();
        o.note = format!("GPU failed: {e}");
    } else if let Target::Gpu(_) = target {
        o.extra
            .push(("texture_mb", gpu_texture_bytes() as f64 / 1e6));
    }
    Some(o)
}

/// The canvas `canvas_row` draws a grid of text into: 120 columns, 40 rows.
const GRID: CanvasId = CanvasId(1);
const GRID_ROWS: usize = 40;
const GRID_ROW_H: f64 = 18.0;

fn grid_line(rng: &mut Rng) -> String {
    (0..120)
        .map(|_| (b'!' + (rng.next() % 90) as u8) as char)
        .collect()
}

fn canvas_row(plan: &Plan) -> Option<Outcome> {
    let mut rng = Rng(SEED);
    let rows = Rc::new(RefCell::new(
        (0..GRID_ROWS)
            .map(|_| grid_line(&mut rng))
            .collect::<Vec<_>>(),
    ));
    let view_rows = rows.clone();
    let font = plan.font.clone();
    let mut app = App::new((), move |_: &(), _: &mut Cx<()>| {
        let rows = view_rows.clone();
        let font = font.clone();
        Element::canvas(GRID, move |c: &mut Canvas| {
            let size = c.size();
            c.fill_rect(
                Rect::from_xywh(0.0, 0.0, size.width, size.height),
                Color::rgb(16, 18, 22),
            );
            // A terminal's way: a character per cell, no shaping.
            let mut run = GlyphRun::new(&font, 13.0, Color::rgb(200, 220, 200));
            for (i, text) in rows.borrow().iter().enumerate() {
                let top = 4.0 + i as f64 * GRID_ROW_H;
                if c.needs_paint(Rect::from_xywh(0.0, top, size.width, GRID_ROW_H)) {
                    run.push_cells(text, Point::new(6.0, top), 8.4);
                }
            }
            c.fill_glyph_run(&run);
        })
        .grow(1.0)
    })
    .logical_size(Size::new(W, H))
    .font(plan.font.clone())
    .renderer(plan.target.renderer());
    let mut sink = Sink(PhysicalSize::new(0, 0));
    app.present_frame(&mut sink);
    let handle = app.handle();
    let mut pixels = Vec::new();
    let mut uploaded = Vec::new();
    let samples = sample(5, plan.n(60), |i| {
        let r = (i * 7) % GRID_ROWS;
        rows.borrow_mut()[r] = grid_line(&mut rng);
        RasterStats::take();
        let t = Instant::now();
        handle.invalidate_canvas_rect(
            GRID,
            Rect::from_xywh(0.0, 4.0 + r as f64 * GRID_ROW_H, W, GRID_ROW_H),
        );
        std::hint::black_box(app.present_frame(&mut sink));
        let ms = ms_since(t);
        pixels.push(RasterStats::take().pixels as f64);
        uploaded.push(gpu_canvas_upload_bytes() as f64 / 1e3);
        ms
    });
    let mut o = outcome(plan, "canvas_row", samples, 5);
    match plan.target {
        Target::Software => {
            pixels.sort_by(f64::total_cmp);
            o.extra.push(("pixels", median(&pixels)));
        }
        Target::Gpu(_) => {
            uploaded.sort_by(f64::total_cmp);
            o.extra.push(("upload_kb", median(&uploaded)));
        }
    }
    Some(o)
}

/// The phone-sized app the transitions run on: a page stack, and a sheet.
struct Phone {
    nav: NavStack<u32>,
    sheet: BottomSheet,
}

fn phone_view(s: &Phone, cx: &mut Cx<Phone>) -> Element {
    let pages = nav_stack(
        cx,
        &s.nav,
        |s| &mut s.nav,
        |cx, page| {
            let page = *page;
            let bar = app_bar(cx, format!("Page {page}"))
                .on_back(|s: &mut Phone| {
                    s.nav.pop();
                })
                .view(cx);
            let rows = (0..24)
                .map(|i| {
                    row(vec![
                        label(cx, format!("Item {i} on page {page}")),
                        spacer(),
                        switch(cx, "", i % 3 == 0, |_| {}),
                    ])
                })
                .collect();
            column(vec![
                bar,
                row(vec![
                    button(cx, "Next").on_tap(cx, move |s: &mut Phone| s.nav.push(page + 1)),
                    button(cx, "Sheet").on_tap(cx, |s: &mut Phone| s.sheet.open()),
                ])
                .gap(8.0),
                scroll(cx).child(column(rows).gap(6.0)).grow(1.0),
            ])
            .gap(8.0)
            .grow(1.0)
        },
    );
    bottom_sheet(cx, &s.sheet, |s| &mut s.sheet)
        .title("Details")
        .children(vec![
            label(cx, "A sheet over the page."),
            button(cx, "Close").on_tap(cx, |s: &mut Phone| s.sheet.dismiss()),
        ])
        .open(cx);
    pages
}

fn transition(plan: &Plan) -> Option<Outcome> {
    let mut app = App::new(
        Phone {
            nav: NavStack::new(0),
            sheet: BottomSheet::new(),
        },
        phone_view,
    )
    .theme(Theme::light())
    .logical_size(Size::new(390.0, 844.0))
    .scale(ScaleFactor::new(3.0))
    .font(plan.font.clone())
    .renderer(plan.target.renderer());
    let mut sink = Sink(PhysicalSize::new(0, 0));
    app.present_frame(&mut sink);
    // Motion on, for this one.
    stipple::anim::set_reduced_motion(Some(false));
    let rounds = plan.n(3).min(6);
    let mut samples = Vec::new();
    let mut missing = None;
    'rounds: for round in 0..=rounds {
        for name in ["Next", "Back", "Sheet", "Close"] {
            let t = Instant::now();
            if !app.click(Role::Button, name) {
                missing = Some(name);
                break 'rounds;
            }
            app.present_frame(&mut sink);
            let mut frames = vec![ms_since(t)];
            // Each frame of the animation, as the display asks for them.
            for _ in 0..600 {
                if !app.animating() {
                    break;
                }
                let t = Instant::now();
                std::hint::black_box(app.present_frame(&mut sink));
                frames.push(ms_since(t));
            }
            // The first round warms up.
            if round > 0 {
                samples.extend(frames);
            }
        }
    }
    stipple::anim::set_reduced_motion(Some(true));
    if let Some(name) = missing {
        (plan.log)(&format!("    (no {name} button: skipped)"));
        return None;
    }
    let mut o = outcome(plan, "transition", samples, 1);
    o.note = "each frame of page push/pop and sheet open/close".into();
    Some(o)
}

/// Draw `scene` on the GPU offscreen and wait for it, not read back.
#[cfg(feature = "gpu")]
fn gpu_draw(scene: &Scene, bg: Color, scale: ScaleFactor) -> Result<(), String> {
    stipple::gpu::draw_scene_scaled(scene, bg, scale)
}

#[cfg(not(feature = "gpu"))]
fn gpu_draw(_: &Scene, _: Color, _: ScaleFactor) -> Result<(), String> {
    Err("built without the `gpu` feature".into())
}

/// Bytes of texture the thread's GPU renderers' caches hold.
#[cfg(feature = "gpu")]
fn gpu_texture_bytes() -> usize {
    stipple::gpu::stats().texture_bytes
}

#[cfg(not(feature = "gpu"))]
fn gpu_texture_bytes() -> usize {
    0
}

/// Bytes of canvas pixels the last GPU frame uploaded.
#[cfg(feature = "gpu")]
fn gpu_canvas_upload_bytes() -> usize {
    stipple::gpu::stats().canvas_upload_bytes
}

#[cfg(not(feature = "gpu"))]
fn gpu_canvas_upload_bytes() -> usize {
    0
}

/// The device the GPU renderer draws on, and whether it's a hardware one
/// (not a CPU implementation: llvmpipe, lavapipe, SwiftShader, WARP, or a
/// virtual machine's paravirtual device).
#[cfg(feature = "gpu")]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn gpu_device() -> (String, bool) {
    let name = stipple::gpu::device_name().unwrap_or_default();
    let hardware = stipple::gpu::is_hardware_device(&name);
    (name, hardware)
}

#[cfg(not(feature = "gpu"))]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn gpu_device() -> (String, bool) {
    (String::new(), false)
}

/// Draw `scene` on the GPU and read it back.
#[cfg(feature = "gpu")]
fn gpu_render(scene: &Scene) -> Result<(), String> {
    stipple::gpu::render_scene_scaled(scene, Color::BLACK, ScaleFactor::IDENTITY)
        .map(std::hint::black_box)
        .map(|_| ())
}

#[cfg(not(feature = "gpu"))]
fn gpu_render(_: &Scene) -> Result<(), String> {
    Err("built without the `gpu` feature".into())
}

/// A tree of ~4,000 elements: 160 rows of a label, a box and a nested row of
/// eight cells, each a column of two labels.
fn layout_tree(plan: &Plan) -> Option<Outcome> {
    let theme = Theme::light();
    let mut cx = Cx::<()>::new(&theme);
    let rows = (0..160)
        .map(|r| {
            let cells = (0..8)
                .map(|c| {
                    column(vec![
                        label(&cx, format!("Cell {r}.{c}")),
                        label(&cx, format!("{}", r * 8 + c)),
                    ])
                    .padding(Insets::uniform(2.0))
                })
                .collect();
            row(vec![
                label(&cx, format!("Row {r}")).width(80.0),
                Element::boxed(BoxStyle::default()).width(12.0).height(12.0),
                row(cells).gap(4.0).grow(1.0),
            ])
            .gap(6.0)
        })
        .collect();
    let tree = scroll(&mut cx).child(column(rows).gap(2.0));
    let bounds = Rect::from_xywh(0.0, 0.0, W, H);
    let font = plan.font.clone();
    let samples = sample(3, plan.n(30), |_| {
        let t = Instant::now();
        std::hint::black_box(stipple::core::layout(&tree, bounds, Some(&font)));
        ms_since(t)
    });
    Some(cpu_outcome("layout_tree", samples, 3))
}

const WORDS: &[&str] = &[
    "the", "quick", "brown", "fox", "jumps", "over", "lazy", "dog", "while", "seven", "wizards",
    "quietly", "pack", "boxes", "of", "liquor", "jugs", "into", "vexing", "frames", "and",
    "zephyrs", "blow", "across", "every", "window", "layout", "glyph", "kerning", "ligature",
];

/// A long paragraph: 400 words drawn from [`WORDS`] with the fixed seed.
fn paragraph_text() -> String {
    let mut rng = Rng(SEED);
    let mut s = String::new();
    for i in 0..400 {
        if i > 0 {
            s.push(if i % 23 == 0 { '\n' } else { ' ' });
        }
        s.push_str(WORDS[(rng.next() % WORDS.len() as u64) as usize]);
    }
    s
}

fn text_layout(plan: &Plan) -> Option<Outcome> {
    let text = paragraph_text();
    let font = plan.font.clone();
    let samples = sample(5, plan.n(50), |_| {
        let t = Instant::now();
        std::hint::black_box(font.layout(&text, 15.0, TextDirection::Auto, Some(560.0)));
        ms_since(t)
    });
    Some(cpu_outcome("text_layout", samples, 5))
}

fn text_layout_cold(plan: &Plan) -> Option<Outcome> {
    let text = paragraph_text();
    let samples = sample(1, plan.n(12), |_| {
        // A font just loaded: nothing shaped yet (loading it isn't timed).
        let font = fresh_font().unwrap_or_else(|| plan.font.clone());
        let t = Instant::now();
        std::hint::black_box(font.layout(&text, 15.0, TextDirection::Auto, Some(560.0)));
        ms_since(t)
    });
    Some(cpu_outcome("text_layout_cold", samples, 1))
}

fn text_layout_cjk(plan: &Plan) -> Option<Outcome> {
    if !plan.font.has_glyph('日') && !cjk_available() {
        (plan.log)("    (no CJK font: skipped)");
        return None;
    }
    // 1,200 characters of Japanese, one paragraph per 60.
    const KANA: &str = "日本語の文章を組むときは漢字とかなが混ざり、句読点で区切られる。東京大阪京都名古屋札幌福岡";
    let chars: Vec<char> = KANA.chars().collect();
    let mut rng = Rng(SEED);
    let text: String = (0..1200)
        .map(|i| {
            if i % 60 == 59 {
                '\n'
            } else {
                chars[(rng.next() % chars.len() as u64) as usize]
            }
        })
        .collect();
    let font = plan.font.clone();
    let samples = sample(3, plan.n(40), |_| {
        let t = Instant::now();
        std::hint::black_box(font.layout(&text, 15.0, TextDirection::Auto, Some(560.0)));
        ms_since(t)
    });
    let mut o = cpu_outcome("text_layout_cjk", samples, 3);
    o.note = "through the system CJK fallback".into();
    Some(o)
}

// --- Fonts and the GPU, per platform ------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
fn system_font() -> Option<Font> {
    Font::system_default()
}

#[cfg(not(target_arch = "wasm32"))]
fn fresh_font() -> Option<Font> {
    Font::system_default()
}

#[cfg(not(target_arch = "wasm32"))]
fn cjk_available() -> bool {
    Font::from_family("cjk", FontStyle::NORMAL).is_ok_and(|f| f.has_glyph('日'))
}

#[cfg(target_arch = "wasm32")]
fn system_font() -> Option<Font> {
    web::font_bytes().and_then(|b| Font::from_bytes(b).ok())
}

#[cfg(target_arch = "wasm32")]
fn fresh_font() -> Option<Font> {
    system_font()
}

#[cfg(target_arch = "wasm32")]
fn cjk_available() -> bool {
    false
}

/// Whether `name` is one of this platform's GPU APIs, and it draws; `gpu`
/// is the platform's preferred one (Vulkan before OpenGL ES on Linux and
/// Android, when both are built in). The API is pinned through
/// `STIPPLE_RENDERER` for the whole run, so a build with both `vk` and `gl`
/// measures the one asked for; it's unset again when the API doesn't draw.
#[cfg(not(target_arch = "wasm32"))]
fn gpu_probe(name: &str) -> Result<&'static str, String> {
    let apis: &[&'static str] = if cfg!(any(target_os = "macos", target_os = "ios")) {
        &["metal"]
    } else if cfg!(target_os = "windows") {
        &["d3d11"]
    } else if cfg!(feature = "vk") {
        &["vulkan", "gl"]
    } else {
        &["gl", "vulkan"]
    };
    let api = match name {
        "gpu" => apis[0],
        "vk" => "vulkan",
        "gles" => "gl",
        name => *apis.iter().find(|a| **a == name).ok_or_else(|| {
            format!(
                "{name} isn't one of this platform's GPU APIs ({})",
                apis.join(", ")
            )
        })?,
    };
    // SAFETY: nothing else runs yet: the bench is single-threaded here and
    // nothing has read the environment.
    unsafe { std::env::set_var("STIPPLE_RENDERER", api) };
    let scene = Scene::new(Size::new(16.0, 16.0));
    gpu_render(&scene).map(|_| api).inspect_err(|_| {
        // SAFETY: as above.
        unsafe { std::env::remove_var("STIPPLE_RENDERER") };
    })
}

// --- Reporting ----------------------------------------------------------------

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn num(v: f64) -> String {
    if v.is_finite() {
        format!("{:.4}", v)
    } else {
        "null".into()
    }
}

/// The run as JSON (the format `tools/bench/compare.py` reads).
fn to_json(meta: &[(&str, String)], outcomes: &[Outcome]) -> String {
    let mut s = String::from("{\n  \"schema\": 1,\n");
    for (k, v) in meta {
        s.push_str(&format!("  {}: {},\n", json_str(k), json_str(v)));
    }
    let cal = outcomes
        .iter()
        .find(|o| o.name == "calibration")
        .map_or(f64::NAN, Outcome::median);
    s.push_str(&format!("  \"calibration_ms\": {},\n", num(cal)));
    s.push_str("  \"scenarios\": [\n");
    for (i, o) in outcomes.iter().enumerate() {
        let samples: Vec<String> = o.samples.iter().map(|v| num(*v)).collect();
        s.push_str(&format!(
            "    {{\"name\": {}, \"renderer\": {}, \"median_ms\": {}, \"p95_ms\": {}, \"min_ms\": {}, \"iterations\": {}, \"warmup\": {}",
            json_str(o.name),
            json_str(&o.renderer),
            num(o.median()),
            num(o.p95()),
            num(o.sorted().first().copied().unwrap_or(f64::NAN)),
            o.samples.len(),
            o.warmup,
        ));
        for (k, v) in &o.extra {
            s.push_str(&format!(", {}: {}", json_str(k), num(*v)));
        }
        if !o.note.is_empty() {
            s.push_str(&format!(", \"note\": {}", json_str(&o.note)));
        }
        s.push_str(&format!(", \"samples_ms\": [{}]}}", samples.join(", ")));
        s.push_str(if i + 1 < outcomes.len() { ",\n" } else { "\n" });
    }
    s.push_str("  ]\n}\n");
    s
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn table(outcomes: &[Outcome]) -> String {
    let mut s = format!(
        "{:<18} {:<9} {:>10} {:>10} {:>6}  {}\n",
        "scenario", "renderer", "median ms", "p95 ms", "n", "other"
    );
    for o in outcomes {
        let extra: Vec<String> = o
            .extra
            .iter()
            .map(|(k, v)| format!("{k} {v:.0}"))
            .chain((!o.note.is_empty()).then(|| o.note.clone()))
            .collect();
        s.push_str(&format!(
            "{:<18} {:<9} {:>10.3} {:>10.3} {:>6}  {}\n",
            o.name,
            o.renderer,
            o.median(),
            o.p95(),
            o.samples.len(),
            extra.join(", ")
        ));
    }
    s
}

// --- The native program -------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .map(|i| args.get(i + 1).cloned().unwrap_or_default())
    };
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "bench [--renderer software|gpu|metal|vulkan|gl|d3d11] [--json FILE] [--only a,b] [--quick] [--runner NAME] [--list]"
        );
        return;
    }
    if args.iter().any(|a| a == "--list") {
        for name in [
            "calibration",
            "startup",
            "first_frame_1x",
            "first_frame_2x",
            "hover",
            "scroll_list",
            "type_char",
            "theme_switch",
            "sprites_2000",
            "gallery_4k",
            "sprites_10k",
            "canvas_row",
            "transition",
            "layout_tree",
            "text_layout",
            "text_layout_cold",
            "text_layout_cjk",
        ] {
            println!("{name}");
        }
        return;
    }
    if std::env::var_os("STIPPLE_RENDERER").is_some() {
        eprintln!(
            "bench: STIPPLE_RENDERER would override the renderer; unset it and use --renderer"
        );
        std::process::exit(2);
    }
    // Frames as they end up: no transitions, no theme cross-fade.
    stipple::anim::set_reduced_motion(Some(true));

    let requested = value("--renderer").unwrap_or_else(|| "software".into());
    let only: Vec<String> = value("--only")
        .map(|s| {
            s.split(',')
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    let (target, gpu_note) = match requested.as_str() {
        "software" | "cpu" => (Target::Software, String::new()),
        name => match gpu_probe(name) {
            Ok(api) => (Target::Gpu(api), String::new()),
            Err(e) => {
                eprintln!("bench: no {name} renderer ({e}); measuring the software renderer");
                (Target::Software, e)
            }
        },
    };
    let Some(font) = Font::system_default() else {
        eprintln!("bench: no system font");
        std::process::exit(2);
    };
    if let Some(path) = value("--png") {
        let mut app = make_app(&font, 1.0, target.renderer());
        app.render_once().save_png(&path).expect("write the PNG");
        eprintln!("bench: wrote {path}");
        return;
    }
    let scale = if args.iter().any(|a| a == "--quick") {
        1.0 / 3.0
    } else {
        1.0
    };
    let log = |s: &str| eprintln!("{s}");
    let plan = Plan {
        target,
        font,
        scale,
        only: &only,
        log: &log,
    };
    eprintln!("bench: {} ({})", target.name(), std::env::consts::OS);
    let started = Instant::now();
    let outcomes = run(&plan);
    print!("{}", table(&outcomes));
    let (device, hardware) = match target {
        Target::Gpu(_) => gpu_device(),
        Target::Software => (String::new(), false),
    };
    if !device.is_empty() {
        eprintln!(
            "bench: on {device} ({})",
            if hardware { "hardware" } else { "software" }
        );
    }
    eprintln!("bench: {:.1} s", started.elapsed().as_secs_f64());

    let meta = [
        ("tool", "stipple-bench".to_string()),
        (
            "platform",
            format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        ),
        ("runner", value("--runner").unwrap_or_default()),
        ("renderer_requested", requested.clone()),
        ("renderer", target.name().to_string()),
        ("gpu_unavailable", gpu_note),
        ("gpu_device", device.clone()),
        (
            "profile",
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
            .into(),
        ),
        ("commit", std::env::var("GITHUB_SHA").unwrap_or_default()),
    ];
    if let Some(path) = value("--json") {
        let json = to_json(&meta, &outcomes).replacen(
            "{\n",
            &format!("{{\n  \"gpu_hardware\": {hardware},\n"),
            1,
        );
        std::fs::write(&path, json).expect("write the JSON");
        eprintln!("bench: wrote {path}");
    }
}

// --- In the browser -------------------------------------------------------------

/// The exports `tools/bench/index.html` calls on a module instance of its
/// own (not the one running the app).
#[cfg(target_arch = "wasm32")]
mod web {
    use super::*;
    use std::cell::RefCell;
    use stipple::platform::backend::web as glue;

    thread_local! {
        static FONT: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
        static OUT: RefCell<String> = const { RefCell::new(String::new()) };
    }

    pub(super) fn font_bytes() -> Option<Vec<u8>> {
        FONT.with(|f| f.borrow().clone())
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn bench_alloc(len: usize) -> *mut u8 {
        glue::alloc(len)
    }

    /// The font the scenarios draw with.
    ///
    /// # Safety
    /// `ptr`/`len` come from one `bench_alloc(len)`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn bench_font(ptr: *mut u8, len: usize) {
        let bytes = unsafe { glue::take_bytes(ptr, len) };
        FONT.with(|f| *f.borrow_mut() = Some(bytes));
    }

    /// Run the scenarios named in `ptr`/`len` (comma-separated; empty: all),
    /// with iterations scaled by `percent`/100. Returns the JSON's length;
    /// [`bench_out`] points at it.
    ///
    /// # Safety
    /// `ptr`/`len` come from one `bench_alloc(len)` (or `len` is 0).
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn bench_run(ptr: *mut u8, len: usize, percent: u32) -> usize {
        let only: Vec<String> = unsafe { glue::take_string(ptr, len) }
            .split(',')
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
        stipple::anim::set_reduced_motion(Some(true));
        let Some(font) = system_font() else {
            return 0;
        };
        let log = |_: &str| {};
        let plan = Plan {
            target: Target::Software,
            font,
            scale: percent.max(1) as f64 / 100.0,
            only: &only,
            log: &log,
        };
        let outcomes = run(&plan);
        let meta = [
            ("tool", "stipple-bench".to_string()),
            ("platform", "web-wasm32".to_string()),
            ("renderer", "software".to_string()),
        ];
        let json = to_json(&meta, &outcomes);
        OUT.with(|o| {
            *o.borrow_mut() = json;
            o.borrow().len()
        })
    }

    /// The JSON the last [`bench_run`] made.
    #[unsafe(no_mangle)]
    pub extern "C" fn bench_out() -> *const u8 {
        OUT.with(|o| o.borrow().as_ptr())
    }
}

// In the browser, `web.rs` builds this file as a library: the page runs
// `main`'s app, and `tools/bench/index.html` the exports above.
stipple::web_app!(main);
