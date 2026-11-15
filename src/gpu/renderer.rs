//! The renderer seam: each GPU API is an [`Executor`] of the operation list
//! `ops` lowers a [`Scene`] to, and a [`GpuRenderer`] pairs one with the glyph
//! cache the lowering fills.
//!
//! An executor draws a [`Plan`] either offscreen and reads the frame back
//! (tests, [`App::render_once`](crate::App::render_once), and windows whose
//! backend can only take a `Pixmap`), or straight into a window's GPU layer
//! and presents it ([`GpuLayer`]), with its own texture, glyph and
//! offscreen-target caches behind it.

use super::ops::{self, Plan, RasterCache};
use crate::geometry::ScaleFactor;
use crate::platform::GpuLayer;
use crate::render::{Color, Pixmap, Scene};

/// A GPU API Stipple can render with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Api {
    /// Metal (macOS, iOS), feature `mtl`.
    Metal,
    /// OpenGL ES 2 over EGL (Linux, Android), feature `gl`.
    Gl,
    /// Vulkan (Linux, Android), feature `vk`.
    Vulkan,
    /// Direct3D 11 (Windows), feature `d3d`.
    D3d11,
    /// WebGPU, through `web/gpu.js` (the web, feature `gpu`).
    WebGpu,
    /// WebGL2, through `web/gpu.js` (the web, feature `gpu`).
    WebGl2,
}

impl Api {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Api::Metal => "Metal",
            Api::Gl => "OpenGL ES",
            Api::D3d11 => "Direct3D 11",
            Api::WebGpu => "WebGPU",
            Api::WebGl2 => "WebGL2",
            Api::Vulkan => "Vulkan",
        }
    }

    /// This platform's GPU APIs that were built in, the preferred first:
    /// Metal on macOS and iOS; Vulkan, then GLES where there's no Vulkan, on
    /// Linux and Android; Direct3D 11 on Windows; WebGPU on the web (and
    /// WebGL2 where the browser has no WebGPU: the page decides). Vulkan
    /// also builds on macOS, over MoltenVK, to work on its executor there.
    pub(crate) fn native() -> Vec<Api> {
        let mut apis = Vec::new();
        if cfg!(all(
            any(target_os = "macos", target_os = "ios"),
            feature = "mtl"
        )) {
            apis.push(Api::Metal);
        }
        if cfg!(all(
            any(
                target_os = "linux",
                target_os = "android",
                target_os = "macos"
            ),
            feature = "vk"
        )) {
            apis.push(Api::Vulkan);
        }
        if cfg!(all(
            any(target_os = "linux", target_os = "android"),
            feature = "gl"
        )) {
            apis.push(Api::Gl);
        }
        if cfg!(all(target_os = "windows", feature = "d3d")) {
            apis.push(Api::D3d11);
        }
        if cfg!(all(target_arch = "wasm32", feature = "gpu")) {
            apis.push(Api::WebGpu);
        }
        apis
    }
}

/// What went wrong on the GPU.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GpuError {
    /// No device, the API isn't built in, or setting it up failed: there is
    /// no GPU renderer to be had.
    Unavailable(String),
    /// The device was lost (or a frame failed on it): the renderer is gone,
    /// and a new one may work.
    Lost(String),
    /// Nothing went wrong, but this frame couldn't be shown (no drawable came
    /// in time): try again on the next frame.
    #[cfg_attr(
        not(any(
            all(any(target_os = "macos", target_os = "ios"), feature = "mtl"),
            all(target_os = "windows", feature = "d3d"),
            all(
                any(target_os = "linux", target_os = "android", target_os = "macos"),
                feature = "vk"
            )
        )),
        allow(dead_code)
    )]
    Skipped,
    /// The GPU isn't ready yet (a browser sets it up asynchronously): show
    /// nothing, and wait for the platform to ask for the next frame.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    Pending,
}

impl std::fmt::Display for GpuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GpuError::Unavailable(e) => write!(f, "unavailable: {e}"),
            GpuError::Lost(e) => write!(f, "device lost: {e}"),
            GpuError::Skipped => f.write_str("no drawable this frame"),
            GpuError::Pending => f.write_str("still being set up"),
        }
    }
}

/// One GPU API executing lowered frames.
pub(crate) trait Executor {
    fn api(&self) -> Api;

    /// Draw `plan` offscreen and read the frame back.
    fn read_back(&mut self, plan: &Plan) -> Result<Pixmap, GpuError>;

    /// Whether [`present`](Self::present) can show frames in `layer`.
    fn can_present(&self, layer: &GpuLayer) -> bool {
        let _ = layer;
        false
    }

    /// Draw `plan` into `layer` (a window's GPU layer, `plan.size` pixels at
    /// `scale`) and present it.
    fn present(
        &mut self,
        plan: &Plan,
        layer: &GpuLayer,
        scale: ScaleFactor,
    ) -> Result<(), GpuError> {
        let _ = (plan, layer, scale);
        Err(GpuError::Unavailable(format!(
            "{} can't present to this window",
            self.api().name()
        )))
    }

    /// Let go of what was made to present to `layer` (a swapchain, an EGL
    /// window surface): its window is going away.
    fn release(&mut self, layer: &GpuLayer) {
        let _ = layer;
    }

    /// Draw `plan` offscreen and wait for the GPU, without reading back
    /// (for timing the GPU's part of a frame).
    #[allow(dead_code)] // the comparison tests and benchmark use it
    fn finish(&mut self, plan: &Plan) -> Result<(), GpuError> {
        self.read_back(plan).map(drop)
    }

    /// Texture uploads in the last frame.
    fn uploads(&self) -> usize;
}

/// A GPU renderer: an executor and the glyph rasters it draws.
pub(crate) struct GpuRenderer {
    exec: Box<dyn Executor>,
    rasters: RasterCache,
    /// The next frame fails as if the device were lost (tests).
    #[cfg(test)]
    lose: bool,
}

impl std::fmt::Debug for GpuRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuRenderer")
            .field("api", &self.exec.api())
            .finish_non_exhaustive()
    }
}

impl GpuRenderer {
    /// A renderer on `api`, or on this platform's own API for `None` (the
    /// first of [`Api::native`] that can be set up).
    pub(crate) fn new(api: Option<Api>) -> Result<Self, GpuError> {
        // On the web the page picks WebGPU or WebGL2 for `None`.
        #[cfg(all(target_arch = "wasm32", feature = "gpu"))]
        return super::web::WebExecutor::new(api).map(|e| Self::with_executor(Box::new(e)));
        #[allow(unreachable_code)]
        let apis = api.map_or_else(Api::native, |a| vec![a]);
        let mut why = GpuError::Unavailable(
            "built without a GPU backend for this platform (features `mtl`, `vk`, `gl`, `d3d`, or `gpu` on the web)"
                .into(),
        );
        for api in apis {
            match Self::executor(api) {
                Ok(exec) => return Ok(Self::with_executor(exec)),
                Err(e) => why = e,
            }
        }
        Err(why)
    }

    #[cfg_attr(all(target_arch = "wasm32", feature = "gpu"), allow(dead_code))]
    fn executor(api: Api) -> Result<Box<dyn Executor>, GpuError> {
        match api {
            #[cfg(all(any(target_os = "macos", target_os = "ios"), feature = "mtl"))]
            Api::Metal => super::metal_scene::MetalExecutor::new().map(|e| Box::new(e) as _),
            #[cfg(all(any(target_os = "linux", target_os = "android"), feature = "gl"))]
            Api::Gl => super::gl::GlExecutor::new().map(|e| Box::new(e) as _),
            #[cfg(all(
                any(target_os = "linux", target_os = "android", target_os = "macos"),
                feature = "vk"
            ))]
            Api::Vulkan => super::vk::VkExecutor::new().map(|e| Box::new(e) as _),
            #[cfg(all(target_os = "windows", feature = "d3d"))]
            Api::D3d11 => super::d3d::D3dExecutor::new().map(|e| Box::new(e) as _),
            #[allow(unreachable_patterns)]
            other => Err(GpuError::Unavailable(format!(
                "built without {} for this platform",
                other.name()
            ))),
        }
    }

    /// A renderer on `exec`.
    pub(crate) fn with_executor(exec: Box<dyn Executor>) -> Self {
        Self {
            exec,
            rasters: RasterCache::default(),
            #[cfg(test)]
            lose: false,
        }
    }

    pub(crate) fn api(&self) -> Api {
        self.exec.api()
    }

    fn plan(&mut self, scene: &Scene, background: Color, scale: ScaleFactor) -> Plan {
        let physical = scale.to_physical(scene.logical_size());
        ops::plan(scene, background, physical, &mut self.rasters)
    }

    #[cfg(test)]
    fn lost(&mut self) -> Result<(), GpuError> {
        if std::mem::take(&mut self.lose) {
            return Err(GpuError::Lost("simulated".into()));
        }
        Ok(())
    }
    #[cfg(not(test))]
    fn lost(&mut self) -> Result<(), GpuError> {
        Ok(())
    }

    /// Render `scene` at `scale` on a target cleared to `background`, and
    /// read it back.
    pub(crate) fn render(
        &mut self,
        scene: &Scene,
        background: Color,
        scale: ScaleFactor,
    ) -> Result<Pixmap, GpuError> {
        self.lost()?;
        let plan = self.plan(scene, background, scale);
        self.exec.read_back(&plan)
    }

    /// Whether frames can go straight to `layer`.
    pub(crate) fn can_present(&self, layer: &GpuLayer) -> bool {
        self.exec.can_present(layer)
    }

    /// Render `scene` at `scale` into `layer` and present it.
    pub(crate) fn present(
        &mut self,
        scene: &Scene,
        background: Color,
        scale: ScaleFactor,
        layer: &GpuLayer,
    ) -> Result<(), GpuError> {
        self.lost()?;
        let plan = self.plan(scene, background, scale);
        self.exec.present(&plan, layer, scale)
    }

    /// Let go of what presents to `layer`: its window is going away.
    pub(crate) fn release(&mut self, layer: &GpuLayer) {
        self.exec.release(layer);
    }

    /// Render `scene` offscreen and wait for the GPU, without a read-back.
    #[allow(dead_code)] // the comparison tests and benchmark use it
    pub(crate) fn finish(
        &mut self,
        scene: &Scene,
        background: Color,
        scale: ScaleFactor,
    ) -> Result<(), GpuError> {
        let plan = self.plan(scene, background, scale);
        self.exec.finish(&plan)
    }

    /// Texture uploads in the last frame.
    #[allow(dead_code)] // the comparison tests and benchmark use it
    pub(crate) fn uploads(&self) -> usize {
        self.exec.uploads()
    }

    /// Make the next frame fail as if the device were lost.
    #[cfg(test)]
    pub(crate) fn lose_device(&mut self) {
        self.lose = true;
    }
}
