//! Choosing what draws a window's frames ([`App::renderer`](crate::App::renderer),
//! `STIPPLE_RENDERER`, or on the web the page's `renderer` option), and drawing
//! them on the GPU.
//!
//! A window that draws on the GPU presents straight into the GPU layer its
//! backend lays over the content ([`Window::gpu_layer`]: a `CAMetalLayer` on
//! macOS and iOS, the window's own DXGI swap chain on Windows, the canvas's
//! WebGPU or WebGL2 context on the web), skipping the `Pixmap` and the
//! [`Surface`](crate::render::Surface).
//! Where the backend has no GPU layer yet, the GPU draws the frame and it is
//! read back and presented as a `Pixmap`, as [`App::render_once`](crate::App::render_once)
//! does everywhere.
//!
//! One renderer (device, pipelines, caches) is shared by every window on the
//! thread. When it can't be had, or the device is lost, frames fall back to
//! the CPU: a lost device is set up anew once, and if that fails as well the
//! app stays on the CPU from then on.

use crate::Pane;
use crate::core::Damage;
use crate::core::ViewportId;
use crate::platform::Window;
use crate::render::{Color, Pixmap, Renderer, Scene};
use crate::style::Theme;
use std::collections::HashMap;

/// Say `msg` where the developer looks: stderr, or the browser's console.
fn say(msg: std::fmt::Arguments<'_>) {
    #[cfg(target_arch = "wasm32")]
    crate::platform::backend::web::log(&msg.to_string(), false);
    #[cfg(not(target_arch = "wasm32"))]
    eprintln!("{msg}");
}

/// Which GPU API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Which {
    /// The platform's own.
    Native,
    Metal,
    Gl,
    D3d11,
    WebGpu,
    WebGl2,
    Vulkan,
}

/// What a window ends up drawing with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Want {
    Software,
    Gpu(Which),
}

/// A `STIPPLE_RENDERER` value: `None` for `auto` (and for anything it
/// doesn't know, which leaves the app's own choice).
pub(crate) fn parse(value: &str) -> Option<Want> {
    match value.trim().to_ascii_lowercase().as_str() {
        "software" | "cpu" => Some(Want::Software),
        "gpu" => Some(Want::Gpu(Which::Native)),
        "metal" | "mtl" => Some(Want::Gpu(Which::Metal)),
        "gl" | "gles" | "opengl" => Some(Want::Gpu(Which::Gl)),
        "d3d11" | "d3d" | "direct3d" => Some(Want::Gpu(Which::D3d11)),
        "webgpu" => Some(Want::Gpu(Which::WebGpu)),
        "webgl2" | "webgl" => Some(Want::Gpu(Which::WebGl2)),
        "vulkan" | "vk" => Some(Want::Gpu(Which::Vulkan)),
        _ => None,
    }
}

/// The page's `renderer` option (`web/stipple.js`), the web's
/// `STIPPLE_RENDERER`.
#[cfg(target_arch = "wasm32")]
mod page {
    use super::Want;
    use std::cell::Cell;

    thread_local! {
        pub(super) static PAGE: Cell<Option<Want>> = const { Cell::new(None) };
        /// The browser draws WebGPU / WebGL2 on a hardware GPU (not
        /// SwiftShader or another software rasterizer), as the page found.
        pub(super) static HARDWARE: Cell<bool> = const { Cell::new(false) };
    }

    /// Set from the page, before the app starts.
    pub(crate) fn set(value: &str) {
        PAGE.with(|p| p.set(super::parse(value)));
    }

    /// Set from the page, before the app starts.
    pub(crate) fn set_hardware(on: bool) {
        HARDWARE.with(|h| h.set(on));
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) use page::{set as set_page_renderer, set_hardware as set_page_hardware_gpu};

/// Whether [`Renderer::Auto`] draws on the GPU: on the web, in a module built
/// with the renderer, when the page found a hardware GPU behind WebGPU and
/// WebGL2 (on a software one, SwiftShader, the CPU renderer is faster).
/// Elsewhere not yet (Phase 6h).
fn auto_gpu() -> bool {
    #[cfg(all(target_arch = "wasm32", feature = "gpu"))]
    return page::HARDWARE.with(|h| h.get());
    #[cfg(not(all(target_arch = "wasm32", feature = "gpu")))]
    false
}

/// The page's `renderer` option.
#[cfg(target_arch = "wasm32")]
fn from_env() -> Option<Want> {
    page::PAGE.with(|p| p.get())
}

/// `STIPPLE_RENDERER`, read once.
#[cfg(not(target_arch = "wasm32"))]
fn from_env() -> Option<Want> {
    static ENV: std::sync::OnceLock<Option<Want>> = std::sync::OnceLock::new();
    *ENV.get_or_init(|| {
        let value = std::env::var("STIPPLE_RENDERER").ok()?;
        let want = parse(&value);
        if want.is_none() && !value.trim().eq_ignore_ascii_case("auto") {
            say(format_args!(
                "stipple: STIPPLE_RENDERER={value:?} isn't one of software, gpu, metal, vulkan, gl, d3d11, webgpu, webgl2, auto"
            ));
        }
        want
    })
}

/// What `choice` (with `env`, the environment's override) draws with.
pub(crate) fn resolve(choice: Renderer, env: Option<Want>) -> Want {
    env.unwrap_or(match choice {
        Renderer::Gpu => Want::Gpu(Which::Native),
        // `Auto` is the CPU until the GPU is the better default (Phase 6h),
        // except in a browser with a hardware GPU.
        Renderer::Auto if auto_gpu() => Want::Gpu(Which::Native),
        Renderer::Auto | Renderer::Software => Want::Software,
    })
}

/// A window's renderer choice and GPU presentation state.
#[derive(Debug, Default)]
pub(crate) struct PaneGpu {
    /// What the app asked for.
    pub(crate) choice: Renderer,
    /// The window's GPU layer shows the frames.
    layer: bool,
    /// The next present through the `Surface` must be whole (the GPU layer
    /// was just taken away, or a GPU frame was skipped).
    pub(crate) force: bool,
}

#[cfg(feature = "gpu")]
mod shared {
    use super::{Which, say};
    use crate::gpu::renderer::{Api, GpuError, GpuRenderer};
    use std::cell::RefCell;

    /// Makes a renderer in place of the platform (tests).
    #[cfg(test)]
    pub(crate) type Factory = Box<dyn FnMut() -> Result<GpuRenderer, GpuError>>;

    /// The thread's GPU renderer, shared by its windows.
    #[derive(Default)]
    pub(super) struct Shared {
        renderer: Option<GpuRenderer>,
        /// Gave up on the GPU: draw on the CPU from now on.
        failed: bool,
        /// Devices lost so far.
        losses: u32,
    }

    thread_local! {
        static GPU: RefCell<Shared> = RefCell::new(Shared::default());
        /// Makes renderers instead of the platform (tests).
        #[cfg(test)]
        static FACTORY: RefCell<Option<Factory>> =
            RefCell::new(None);
    }

    fn create(which: Which) -> Result<GpuRenderer, GpuError> {
        #[cfg(test)]
        if let Some(r) = FACTORY.with(|f| f.borrow_mut().as_mut().map(|make| make())) {
            return r;
        }
        GpuRenderer::new(match which {
            Which::Native => None,
            Which::Metal => Some(Api::Metal),
            Which::Gl => Some(Api::Gl),
            Which::D3d11 => Some(Api::D3d11),
            Which::WebGpu => Some(Api::WebGpu),
            Which::WebGl2 => Some(Api::WebGl2),
            Which::Vulkan => Some(Api::Vulkan),
        })
    }

    /// Let go of what the thread's renderer made to present to `layer`
    /// (its window is going away), before the window is.
    pub(crate) fn release(layer: &crate::platform::GpuLayer) {
        GPU.with(|cell| {
            if let Ok(mut gpu) = cell.try_borrow_mut()
                && let Some(r) = gpu.renderer.as_mut()
            {
                r.release(layer);
            }
        });
    }

    /// Run `f` on the thread's renderer for `which`, setting it up first.
    /// `None` when there is no GPU to draw with (and never will be: the
    /// caller draws on the CPU); a lost device is dropped, to be set up
    /// again on the next frame, once.
    pub(super) fn with<T>(
        which: Which,
        f: impl FnOnce(&mut GpuRenderer) -> Result<T, GpuError>,
    ) -> Option<Result<T, GpuError>> {
        GPU.with(|cell| {
            let mut gpu = cell.borrow_mut();
            if gpu.failed {
                return None;
            }
            if gpu.renderer.is_none() {
                match create(which) {
                    Ok(r) => {
                        say(format_args!("stipple: drawing with {}", r.api().name()));
                        gpu.renderer = Some(r);
                    }
                    // Not ready yet (a browser's GPU comes up asynchronously):
                    // nothing to show until the platform asks again.
                    Err(GpuError::Pending) => return Some(Err(GpuError::Pending)),
                    Err(e) => {
                        say(format_args!(
                            "stipple: the GPU renderer is {e}; drawing on the CPU"
                        ));
                        gpu.failed = true;
                        return None;
                    }
                }
            }
            let r = f(gpu.renderer.as_mut().unwrap());
            match &r {
                Err(GpuError::Lost(e)) => {
                    gpu.renderer = None;
                    gpu.losses += 1;
                    if gpu.losses > 1 {
                        say(format_args!(
                            "stipple: GPU device lost again ({e}); drawing on the CPU"
                        ));
                        gpu.failed = true;
                    } else {
                        say(format_args!(
                            "stipple: GPU device lost ({e}); setting it up again"
                        ));
                    }
                }
                Err(GpuError::Unavailable(e)) => {
                    say(format_args!(
                        "stipple: the GPU renderer is unavailable: {e}; drawing on the CPU"
                    ));
                    gpu.renderer = None;
                    gpu.failed = true;
                }
                _ => {}
            }
            Some(r)
        })
    }

    /// Start over: no renderer, nothing given up (tests).
    #[cfg(test)]
    pub(crate) fn reset(factory: Option<Factory>) {
        GPU.with(|g| *g.borrow_mut() = Shared::default());
        FACTORY.with(|f| *f.borrow_mut() = factory);
    }

    /// Run `f` on the thread's renderer, if there is one (tests).
    #[cfg(test)]
    pub(crate) fn current<T>(f: impl FnOnce(Option<&mut GpuRenderer>) -> T) -> T {
        GPU.with(|g| f(g.borrow_mut().renderer.as_mut()))
    }

    /// Whether the thread gave up on the GPU (tests).
    #[cfg(test)]
    pub(crate) fn given_up() -> bool {
        GPU.with(|g| g.borrow().failed)
    }
}

#[cfg(all(test, feature = "gpu"))]
pub(crate) use shared::{current, given_up, reset};

/// A backend's window (and its GPU layer) is going away: the renderer lets
/// go of the swapchain or EGL surface it made for it, now, while the native
/// window is still valid.
#[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
pub(crate) fn release_gpu_layer(layer: crate::platform::GpuLayer) {
    #[cfg(feature = "gpu")]
    shared::release(&layer);
    #[cfg(not(feature = "gpu"))]
    let _ = layer;
}

/// What `damage` (logical pixels) changes of a `size` frame drawn at
/// `scale`, for the GPU: the physical pixel rectangle around it, a pixel
/// wider all round (the GPU's antialiasing and filtering reach a little
/// further than the CPU's), or `None` for the whole frame.
#[cfg_attr(not(feature = "gpu"), allow(dead_code))]
fn gpu_dirty(
    damage: &Damage,
    scale: crate::geometry::ScaleFactor,
    size: crate::geometry::PhysicalSize,
) -> Option<[i32; 4]> {
    let bounds = crate::geometry::Rect::from_xywh(0.0, 0.0, size.width as f64, size.height as f64);
    let r = damage
        .to_physical(scale, bounds)
        .into_iter()
        .reduce(|a, b| a.union(b))?;
    let (w, h) = (size.width as i32, size.height as i32);
    let r = [
        (r.min_x() as i32 - 1).max(0),
        (r.min_y() as i32 - 1).max(0),
        (r.max_x().ceil() as i32 + 1).min(w),
        (r.max_y().ceil() as i32 + 1).min(h),
    ];
    (r[2] > r[0] && r[3] > r[1]).then_some(r)
}

/// Without a GPU backend built in there's nothing to draw with: say so once.
#[cfg(not(feature = "gpu"))]
fn no_gpu() {
    static SAID: std::sync::Once = std::sync::Once::new();
    SAID.call_once(|| {
        say(format_args!(
            "stipple: built without the GPU renderer (feature `gpu`); drawing on the CPU"
        ))
    });
}

impl<S: 'static> Pane<S> {
    /// The GPU API this window draws with, if it draws on the GPU: it asked
    /// to, and has no custom renderer ([`App::render_with`](crate::App::render_with)).
    pub(crate) fn gpu_wanted(&self) -> Option<Which> {
        if self.frame_renderer.is_some() {
            return None;
        }
        match resolve(self.gpu.choice, from_env()) {
            Want::Software => None,
            Want::Gpu(which) => Some(which),
        }
    }

    /// Draw `scene` on the GPU and read it back, or `None` to draw it on
    /// the CPU.
    pub(crate) fn gpu_rasterize(
        &mut self,
        scene: &Scene,
        background: Color,
        scale: crate::geometry::ScaleFactor,
    ) -> Option<Pixmap> {
        // A browser's GPU draws into the canvas only, never read back: a
        // frame that comes here is the CPU's.
        if cfg!(target_arch = "wasm32") {
            return None;
        }
        let which = self.gpu_wanted()?;
        #[cfg(feature = "gpu")]
        {
            shared::with(which, |g| g.render(scene, background, scale))?.ok()
        }
        #[cfg(not(feature = "gpu"))]
        {
            let _ = (which, scene, background, scale);
            no_gpu();
            None
        }
    }

    /// Present `scene` straight into the window's GPU layer. Gives the scene
    /// back when it has to go through the `Surface` instead: the window
    /// draws on the CPU, its backend has no GPU layer, or the GPU failed
    /// (then [`PaneGpu::force`] asks for a whole present).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn present_gpu(
        &mut self,
        scene: Scene,
        damage: &Damage,
        theme: &Theme,
        viewport_content: &HashMap<ViewportId, Pixmap>,
        window: &dyn Window,
        force: bool,
    ) -> Option<Scene> {
        let Some(which) = self.gpu_wanted() else {
            return self.leave_gpu(window, scene);
        };
        #[cfg(feature = "gpu")]
        {
            // A layer that shows the last frame needs nothing new.
            if self.gpu.layer && !force && damage.is_empty() {
                return None;
            }
            let Some(layer) = window.gpu_layer() else {
                return Some(scene);
            };
            let mut scene = scene;
            self.draw_viewport_content(&mut scene, viewport_content);
            let background = theme.palette.background;
            let scale = self.scale;
            let dirty = if force {
                None
            } else {
                gpu_dirty(damage, scale, scale.to_physical(scene.logical_size()))
            };
            let shown = shared::with(which, |g| {
                if !g.can_present(&layer) {
                    return Ok(false);
                }
                g.present(&scene, background, scale, &layer, dirty)
                    .map(|()| true)
                    .inspect(|_| {
                        static SAID: std::sync::Once = std::sync::Once::new();
                        SAID.call_once(|| {
                            say(format_args!(
                                "stipple: presenting with {} straight to the window",
                                g.api().name()
                            ))
                        });
                    })
            });
            match shown {
                Some(Ok(true)) => {
                    self.gpu.layer = true;
                    // The CPU frame is gone: no partial repaint or fade from it.
                    self.last_pixmap = None;
                    None
                }
                Some(Err(crate::gpu::renderer::GpuError::Skipped)) => {
                    self.gpu.layer = true;
                    self.gpu.force = true;
                    window.request_redraw();
                    None
                }
                // The platform asks for a redraw itself once the GPU is
                // ready (on the web a redraw is delivered at once, so asking
                // here would loop).
                Some(Err(crate::gpu::renderer::GpuError::Pending)) => {
                    self.gpu.layer = true;
                    self.gpu.force = true;
                    None
                }
                _ => self.leave_gpu(window, scene),
            }
        }
        #[cfg(not(feature = "gpu"))]
        {
            let _ = (which, damage, theme, viewport_content, force);
            no_gpu();
            self.leave_gpu(window, scene)
        }
    }

    /// Stop presenting through the GPU layer, if it was, so the `Surface`'s
    /// whole frame shows; give `scene` back for it.
    fn leave_gpu(&mut self, window: &dyn Window, scene: Scene) -> Option<Scene> {
        if std::mem::take(&mut self.gpu.layer) {
            window.drop_gpu_layer();
            self.gpu.force = true;
        }
        Some(scene)
    }

    /// Draw registered viewport content into `scene` over the viewports'
    /// placeholders, as images (the GPU analog of `composite_viewports`).
    #[cfg(feature = "gpu")]
    fn draw_viewport_content(&self, scene: &mut Scene, content: &HashMap<ViewportId, Pixmap>) {
        if content.is_empty() {
            return;
        }
        let Some(tree) = self.tree.as_ref() else {
            return;
        };
        let mut views = Vec::new();
        crate::collect_viewports(tree, &mut views);
        let s = self.scale.get();
        for (id, bounds) in views {
            if let Some(src) = content.get(&id) {
                let at = crate::core::node_to_window(tree, &|n| crate::viewport_is(n, id))
                    .map_or(bounds.origin, |(_, m, _)| m.apply(bounds.origin));
                // Whole pixels, unscaled, as the CPU path blits it.
                let (x, y) = ((at.x * s).round() / s, (at.y * s).round() / s);
                let size = src.size();
                let rect = crate::geometry::Rect::from_xywh(
                    x,
                    y,
                    size.width as f64 / s,
                    size.height as f64 / s,
                );
                scene.draw_image(rect, std::sync::Arc::new(src.clone()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_damage_is_one_padded_physical_rectangle() {
        use crate::geometry::{PhysicalSize, Rect, ScaleFactor};
        let size = PhysicalSize::new(200, 100);
        let two = ScaleFactor::new(2.0);
        assert_eq!(gpu_dirty(&Damage::Full, two, size), None);
        assert_eq!(gpu_dirty(&Damage::None, two, size), None);
        let d = Damage::Regions(vec![
            Rect::from_xywh(10.0, 10.0, 5.0, 5.0),
            Rect::from_xywh(20.0, 5.0, 5.5, 5.0),
        ]);
        assert_eq!(gpu_dirty(&d, two, size), Some([19, 9, 52, 31]));
        // Clipped to the frame.
        let d = Damage::Regions(vec![Rect::from_xywh(95.0, 45.0, 20.0, 20.0)]);
        assert_eq!(gpu_dirty(&d, two, size), Some([189, 89, 200, 100]));
    }

    #[test]
    fn the_environment_overrides_the_app() {
        assert_eq!(parse("software"), Some(Want::Software));
        assert_eq!(parse(" CPU "), Some(Want::Software));
        assert_eq!(parse("gpu"), Some(Want::Gpu(Which::Native)));
        assert_eq!(parse("metal"), Some(Want::Gpu(Which::Metal)));
        assert_eq!(parse("gl"), Some(Want::Gpu(Which::Gl)));
        assert_eq!(parse("d3d11"), Some(Want::Gpu(Which::D3d11)));
        assert_eq!(parse("D3D"), Some(Want::Gpu(Which::D3d11)));
        assert_eq!(parse("WebGPU"), Some(Want::Gpu(Which::WebGpu)));
        assert_eq!(parse("webgl2"), Some(Want::Gpu(Which::WebGl2)));
        assert_eq!(parse("auto"), None);
        assert_eq!(parse("vulkan"), Some(Want::Gpu(Which::Vulkan)));
        assert_eq!(parse("vulkan?"), None);
        assert_eq!(resolve(Renderer::Auto, None), Want::Software);
        assert_eq!(resolve(Renderer::Gpu, None), Want::Gpu(Which::Native));
        assert_eq!(
            resolve(Renderer::Software, Some(Want::Gpu(Which::Metal))),
            Want::Gpu(Which::Metal)
        );
        assert_eq!(resolve(Renderer::Gpu, Some(Want::Software)), Want::Software);
    }
}

/// The App's side of the fallback, on a stand-in GPU that paints frames
/// magenta and loses its device on command.
#[cfg(all(test, feature = "gpu"))]
mod fallback_tests {
    use crate::gpu::ops::Plan;
    use crate::gpu::renderer::{Api, Executor, GpuError, GpuRenderer};
    use crate::prelude::*;
    use crate::render::Renderer;
    use std::cell::Cell;
    use std::rc::Rc;

    const MAGENTA: [u8; 4] = [255, 0, 255, 255];

    struct Fake;

    impl Executor for Fake {
        fn api(&self) -> Api {
            Api::Metal
        }
        fn read_back(&mut self, plan: &Plan) -> Result<Pixmap, GpuError> {
            let mut p = Pixmap::new(plan.size);
            for px in p.as_bytes_mut().as_chunks_mut::<4>().0 {
                *px = MAGENTA;
            }
            Ok(p)
        }
        fn uploads(&self) -> usize {
            0
        }
    }

    fn app() -> App<()> {
        App::new((), |_: &(), cx: &mut Cx<()>| label(cx, "hello"))
            .no_font()
            .logical_size(Size::new(40.0, 30.0))
    }

    fn on_gpu(p: &Pixmap) -> bool {
        p.pixel(0, 0) == Some(MAGENTA)
    }

    fn lose() {
        super::current(|r| r.expect("a renderer").lose_device());
    }

    #[test]
    fn a_lost_device_is_set_up_again_once_then_the_cpu_draws() {
        let made = Rc::new(Cell::new(0));
        let count = made.clone();
        super::reset(Some(Box::new(move || {
            count.set(count.get() + 1);
            Ok(GpuRenderer::with_executor(Box::new(Fake)))
        })));
        let mut app = app().renderer(Renderer::Gpu);
        assert!(on_gpu(&app.render_once()), "drawn on the GPU");
        assert_eq!(made.get(), 1);

        lose();
        let frame = app.render_once();
        assert!(!on_gpu(&frame), "the frame the device was lost in: the CPU");
        assert_eq!(frame, self::app().render_once());
        assert!(super::current(|r| r.is_none()), "the renderer is dropped");

        assert!(on_gpu(&app.render_once()), "set up again");
        assert_eq!(made.get(), 2);

        lose();
        assert!(!on_gpu(&app.render_once()));
        assert!(super::given_up(), "lost twice: the CPU from now on");
        assert!(!on_gpu(&app.render_once()));
        assert_eq!(made.get(), 2, "no more tries");
        super::reset(None);
    }

    #[test]
    fn no_device_means_the_cpu_and_software_never_asks() {
        let made = Rc::new(Cell::new(0));
        let count = made.clone();
        super::reset(Some(Box::new(move || {
            count.set(count.get() + 1);
            Err(GpuError::Unavailable("no device".into()))
        })));
        let mut software = app().renderer(Renderer::Software);
        let cpu = software.render_once();
        assert_eq!(made.get(), 0, "software doesn't touch the GPU");
        let mut app = app().renderer(Renderer::Gpu);
        assert_eq!(app.render_once(), cpu, "no device: drawn on the CPU");
        assert!(super::given_up());
        app.render_once();
        assert_eq!(made.get(), 1, "tried once");
        super::reset(None);
    }

    #[test]
    fn a_custom_renderer_wins() {
        super::reset(Some(Box::new(|| {
            Ok(GpuRenderer::with_executor(Box::new(Fake)))
        })));
        let mut app =
            app()
                .renderer(Renderer::Gpu)
                .render_with(|scene: &crate::render::Scene, bg, scale| {
                    crate::render::SoftwareRenderer::new()
                        .background(bg)
                        .render(scene.clone(), scale)
                });
        assert!(!on_gpu(&app.render_once()));
        super::reset(None);
    }
}
