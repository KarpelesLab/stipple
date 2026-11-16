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

/// The window number the comparison's frames are drawn as.
const OWNER: u64 = u64::MAX;

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

type Before = Option<(Scene, Vec<crate::geometry::Rect>)>;

fn scene_of(i: usize, bench: bool) -> (Scene, f64, Before) {
    if bench {
        let (scene, scale) = BENCH.with(|b| b.borrow()[i].clone());
        (scene, scale, None)
    } else {
        CASES.with(|c| {
            let c = &c.borrow()[i];
            (c.scene.clone(), c.scale, c.before.clone())
        })
    }
}

/// Case (or with `bench`, timing scene) `i` on the CPU.
pub fn cpu(i: usize, bench: bool) -> Pixmap {
    let (scene, scale, _) = scene_of(i, bench);
    SoftwareRenderer::new()
        .background(corpus::BACKGROUND)
        .render(scene, ScaleFactor::new(scale))
}

/// Case (or timing scene) `i` drawn into the page's canvas on `api` (0 the
/// page's choice, 1 WebGPU, 2 WebGL2). The page sets its GPU up before.
pub fn gpu(i: usize, bench: bool, api: u32) -> Result<String, String> {
    let (scene, scale, before) = scene_of(i, bench);
    let Some((first, changed)) = before else {
        return present(&scene, scale, api, None);
    };
    // A window's frame before this one, then this one with what changed:
    // its canvas texture must be updated in place, not uploaded anew. (The
    // first present sets the renderer up.)
    present(&first, scale, api, None)?;
    let canvases = |damage: Option<Vec<crate::geometry::Rect>>| {
        GPU.with(|g| {
            if let Some(r) = g.borrow_mut().as_mut() {
                r.next_frame(OWNER, damage);
            }
        })
    };
    canvases(None);
    present(&first, scale, api, None)?;
    canvases(Some(changed));
    let name = present(&scene, scale, api, None)?;
    let uploads = GPU.with(|g| g.borrow().as_ref().map_or(0, |r| r.uploads()));
    if uploads != 0 {
        return Err(format!("{name} uploaded the canvas anew"));
    }
    Ok(name)
}

/// Draw `scene` into the canvas, only `dirty` (physical pixels) when the
/// page holds the frame before.
fn present(scene: &Scene, scale: f64, api: u32, dirty: Option<[i32; 4]>) -> Result<String, String> {
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
            scene,
            corpus::BACKGROUND,
            ScaleFactor::new(scale),
            &GpuLayer::Web,
            dirty,
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

/// The partial-redraw case (`corpus::partial`) at `scale` on the CPU: the
/// frame a window should show after it.
pub fn partial_cpu(scale: f64) -> Pixmap {
    let (before, after, region) = corpus::partial();
    let cpu = |s: Scene| {
        SoftwareRenderer::new()
            .background(corpus::BACKGROUND)
            .render(s, ScaleFactor::new(scale))
    };
    corpus::composite(
        &cpu(before),
        &cpu(after),
        corpus::device_region(region, scale),
    )
}

/// The partial-redraw case at `scale` presented into the canvas on `api`:
/// the first frame whole, then the second with only its region changed, as
/// a window's frames are.
pub fn partial_gpu(scale: f64, api: u32) -> Result<String, String> {
    let (before, after, region) = corpus::partial();
    present(&before, scale, api, None)?;
    present(
        &after,
        scale,
        api,
        Some(corpus::device_region(region, scale)),
    )
}

/// A scene drawing the page's external source `id` (`w`×`h`): at its size,
/// then scaled up nearest-neighbor, turned a quarter and mirrored (as the
/// camera's preview draws its `<video>`).
#[cfg(target_arch = "wasm32")]
fn external_scene(id: u32, w: u32, h: u32) -> Scene {
    use crate::geometry::{Rect, Size};
    use crate::render::{ExternalImage, ImageOptions};
    let image = std::sync::Arc::new(ExternalImage::web(
        id,
        crate::geometry::PhysicalSize::new(w, h),
        false,
    ));
    let (fw, fh) = (w as f64, h as f64);
    let mut scene = Scene::new(Size::new(fw * 4.0, fh * 3.0));
    scene.draw_external(Rect::from_xywh(4.0, 4.0, fw, fh), &image);
    let src = Rect::from_xywh(0.0, 0.0, fw, fh);
    let opts = ImageOptions::new()
        .nearest()
        .rotate(std::f64::consts::FRAC_PI_2)
        .flip_y(true);
    let dst = Rect::from_xywh(fw * 1.5, fh * 0.5, fw * 2.0, fh * 2.0);
    scene.draw_external_region(src, dst, &image, &opts);
    scene
}

/// [`external_scene`] on the CPU (the source read back through the page).
#[cfg(target_arch = "wasm32")]
pub fn external_cpu(id: u32, w: u32, h: u32) -> Pixmap {
    SoftwareRenderer::new()
        .background(corpus::BACKGROUND)
        .render(external_scene(id, w, h), ScaleFactor::IDENTITY)
}

/// [`external_scene`] drawn into the canvas on `api`, the source sampled
/// straight from the page's object; how many textures it uploaded too.
#[cfg(target_arch = "wasm32")]
pub fn external_gpu(id: u32, w: u32, h: u32, api: u32) -> Result<String, String> {
    let name = present(&external_scene(id, w, h), 1.0, api, None)?;
    let uploads = GPU.with(|g| g.borrow().as_ref().map_or(0, |r| r.uploads()));
    Ok(format!("{name}, {uploads} uploads"))
}
