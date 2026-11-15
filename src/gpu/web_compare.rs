//! The web's GPU comparison, for `examples/webdemo/gpucompare.rs`: the
//! corpus `compare` checks Metal and GLES2 with, drawn by the software
//! renderer and by the page's WebGPU or WebGL2 into its canvas, for the page
//! to diff. Not a public API.

use super::corpus::{self, Case};
use super::renderer::{Api, GpuError, GpuRenderer};
use crate::geometry::ScaleFactor;
use crate::platform::GpuLayer;
use crate::render::{Font, Pixmap, Scene, SoftwareRenderer};
use std::cell::RefCell;

thread_local! {
    static CASES: RefCell<Vec<Case>> = const { RefCell::new(Vec::new()) };
    static BENCH: RefCell<Vec<(Scene, f64)>> = const { RefCell::new(Vec::new()) };
    static GPU: RefCell<Option<GpuRenderer>> = const { RefCell::new(None) };
}

/// Build the corpus (the text cases with `font`, and the timing scenes);
/// returns how many cases there are.
pub fn load(font: Option<Font>) -> usize {
    let cases = corpus::cases(font.as_ref());
    let n = cases.len();
    CASES.with(|c| *c.borrow_mut() = cases);
    let mut bench = vec![(corpus::sprites(), 1.0)];
    if let Some(font) = font {
        bench.insert(0, (corpus::widgets(font), 2.0));
    }
    BENCH.with(|b| *b.borrow_mut() = bench);
    n
}

/// Case `i`'s name and its tolerances (mean, share of bad pixels).
pub fn case(i: usize) -> (String, f64, f64) {
    CASES.with(|c| {
        let c = &c.borrow()[i];
        (c.name.clone(), c.mean, c.bad)
    })
}

/// How many timing scenes there are (a widget window at 2560x1600 when
/// there is a font, 2000 sprites at 1920x1080).
pub fn bench_count() -> usize {
    BENCH.with(|b| b.borrow().len())
}

fn scene_of(i: usize, bench: bool) -> (Scene, f64) {
    if bench {
        BENCH.with(|b| b.borrow()[i].clone())
    } else {
        CASES.with(|c| {
            let c = &c.borrow()[i];
            (c.scene.clone(), c.scale)
        })
    }
}

/// Case (or with `bench`, timing scene) `i` on the CPU.
pub fn cpu(i: usize, bench: bool) -> Pixmap {
    let (scene, scale) = scene_of(i, bench);
    SoftwareRenderer::new()
        .background(corpus::BACKGROUND)
        .render(scene, ScaleFactor::new(scale))
}

/// Case (or timing scene) `i` drawn into the page's canvas on `api` (0 the
/// page's choice, 1 WebGPU, 2 WebGL2). The page sets its GPU up before.
pub fn gpu(i: usize, bench: bool, api: u32) -> Result<String, String> {
    let (scene, scale) = scene_of(i, bench);
    GPU.with(|g| {
        let mut g = g.borrow_mut();
        if g.is_none() {
            let want = match api {
                1 => Some(Api::WebGpu),
                2 => Some(Api::WebGl2),
                _ => None,
            };
            *g = Some(GpuRenderer::new(want).map_err(|e| e.to_string())?);
        }
        let r = g.as_mut().unwrap();
        let name = r.api().name().to_string();
        match r.present(
            &scene,
            corpus::BACKGROUND,
            ScaleFactor::new(scale),
            &GpuLayer::Web,
        ) {
            Ok(()) => Ok(name),
            Err(e) => {
                if matches!(e, GpuError::Lost(_) | GpuError::Unavailable(_)) {
                    *g = None;
                }
                Err(e.to_string())
            }
        }
    })
}
