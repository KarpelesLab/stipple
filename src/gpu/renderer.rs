//! The renderer seam: each GPU API is an [`Executor`] of the operation list
//! `ops` lowers a [`Scene`] to, and a [`GpuRenderer`] pairs one with the glyph
//! cache the lowering fills.
//!
//! An executor draws a [`Plan`] either offscreen and reads the frame back
//! (tests, [`App::render_once`](crate::App::render_once), and windows whose
//! backend can only take a `Pixmap`), or straight into a window's GPU layer
//! and presents it ([`GpuLayer`]), with its own texture, glyph and
//! offscreen-target caches behind it.

use super::canvas::CanvasCache;
use super::ops::{self, Plan, RasterCache};
use crate::geometry::{Rect, ScaleFactor};
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
    /// The frame was drawn, but the window can't be seen (a swap chain's
    /// `DXGI_STATUS_OCCLUDED`): stop drawing it until it shows again.
    #[cfg_attr(
        not(any(test, all(target_os = "windows", feature = "d3d"))),
        allow(dead_code)
    )]
    Occluded,
}

impl std::fmt::Display for GpuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GpuError::Unavailable(e) => write!(f, "unavailable: {e}"),
            GpuError::Lost(e) => write!(f, "device lost: {e}"),
            GpuError::Skipped => f.write_str("no drawable this frame"),
            GpuError::Pending => f.write_str("still being set up"),
            GpuError::Occluded => f.write_str("the window can't be seen"),
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
    /// `scale`) and present it. `dirty` is what changed since the frame
    /// presented to `layer` before (physical pixels, `[x0, y0, x1, y1]`), or
    /// `None` for everything: an executor that still holds that frame
    /// redraws only the changed part ([`Plan::limit`], with the region a
    /// [`DamageLog`] gives) and presents only that part where the platform
    /// takes it.
    fn present(
        &mut self,
        plan: &mut Plan,
        layer: &GpuLayer,
        scale: ScaleFactor,
        dirty: Option<[i32; 4]>,
    ) -> Result<(), GpuError> {
        let _ = (plan, layer, scale, dirty);
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

    /// Draw calls in the last frame (0 where it isn't counted).
    fn draws(&self) -> usize {
        0
    }

    /// Bytes of texture the executor's image cache holds (glyph rasters,
    /// images, canvases).
    fn texture_bytes(&self) -> usize {
        0
    }

    /// The device's name, as the driver gives it (`llvmpipe`, `Microsoft
    /// Basic Render Driver`, `Apple M2`), if known.
    fn device(&self) -> String {
        String::new()
    }
}

thread_local! {
    /// The draw calls of the thread's last frame (`gpu::last_frame_draw_calls`).
    pub(crate) static LAST_DRAWS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The changes of the last frames presented to a window, so that a back
/// buffer holding an older frame can be brought up to date by redrawing just
/// what changed since: the union of the changes of the frames it missed.
/// Whenever that isn't known (the first frame, a resize, a frame that changed
/// everything, a buffer older than the log) the whole frame is redrawn.
#[derive(Debug, Default)]
pub(crate) struct DamageLog {
    /// The frame size the log is for.
    size: (u32, u32),
    /// Each presented frame's change from the one before, newest first.
    past: Vec<Option<[i32; 4]>>,
}

/// How many presented frames a [`DamageLog`] remembers (back buffers older
/// than that are redrawn whole).
const DAMAGE_LOG_FRAMES: usize = 4;

/// Past this share of the frame, a partial redraw saves too little over a
/// full one (which needn't load the old frame) to be worth it.
const PARTIAL_MAX_SHARE: f64 = 0.6;

/// The union of two pixel rectangles.
fn union(a: [i32; 4], b: [i32; 4]) -> [i32; 4] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

impl DamageLog {
    /// What to redraw of a back buffer `age` frames old (1: it holds the
    /// frame presented last; 0: its contents are unknown) for a `size`
    /// frame whose own change is `dirty`: a pixel rectangle, or `None` to
    /// redraw everything.
    pub(crate) fn region(
        &self,
        age: u32,
        size: (u32, u32),
        dirty: Option<[i32; 4]>,
    ) -> Option<[i32; 4]> {
        let age = age as usize;
        if age == 0 || size != self.size || age - 1 > self.past.len() {
            return None;
        }
        let mut r = dirty?;
        for d in &self.past[..age - 1] {
            r = union(r, (*d)?);
        }
        let (w, h) = (size.0 as i32, size.1 as i32);
        let r = [r[0].max(0), r[1].max(0), r[2].min(w), r[3].min(h)];
        if r[2] <= r[0] || r[3] <= r[1] {
            return None;
        }
        let area = (r[2] - r[0]) as f64 * (r[3] - r[1]) as f64;
        if area > PARTIAL_MAX_SHARE * w as f64 * h as f64 {
            return None;
        }
        Some(r)
    }

    /// A `size` frame whose change was `dirty` was presented.
    pub(crate) fn presented(&mut self, size: (u32, u32), dirty: Option<[i32; 4]>) {
        if size != self.size {
            self.past.clear();
            self.size = size;
        }
        self.past.insert(0, dirty);
        self.past.truncate(DAMAGE_LOG_FRAMES);
    }

    /// Forget everything: the next frame is redrawn whole.
    pub(crate) fn clear(&mut self) {
        self.past.clear();
    }
}

/// A frame target that persists from frame to frame, shared by an
/// executor's windows (and its read-backs): which window the frame it holds
/// was presented to, and that window's [`DamageLog`]. A window whose frame
/// the target still holds redraws only what changed.
#[derive(Debug, Default)]
#[cfg_attr(target_os = "windows", allow(dead_code))] // D3D's swap chain keeps its frames
pub(crate) struct KeptFrame {
    owner: Option<usize>,
    log: DamageLog,
}

#[cfg_attr(target_os = "windows", allow(dead_code))]
impl KeptFrame {
    /// What to redraw of the target for window `key`'s next `size` frame,
    /// whose change is `dirty`; `None` for everything. The frame is only
    /// redrawn in part when the background is opaque.
    pub(crate) fn region(
        &self,
        key: usize,
        plan: &Plan,
        dirty: Option<[i32; 4]>,
    ) -> Option<[i32; 4]> {
        if self.owner != Some(key) || plan.background.a != 255 {
            return None;
        }
        self.log
            .region(1, (plan.size.width, plan.size.height), dirty)
    }

    /// Limit `plan` to what window `key` has to redraw (see [`region`](Self::region));
    /// the region, or `None` when it's redrawn whole.
    pub(crate) fn limit(
        &self,
        key: usize,
        plan: &mut Plan,
        dirty: Option<[i32; 4]>,
    ) -> Option<[i32; 4]> {
        let r = self.region(key, plan, dirty)?;
        plan.limit(r);
        Some(r)
    }

    /// The target now holds window `key`'s `size` frame, which changed `dirty`.
    pub(crate) fn presented(&mut self, key: usize, size: (u32, u32), dirty: Option<[i32; 4]>) {
        if self.owner != Some(key) {
            self.log.clear();
            self.owner = Some(key);
        }
        self.log.presented(size, dirty);
    }

    /// Something else was drawn into the target (a read-back, a failed
    /// frame): the next frame is drawn whole.
    pub(crate) fn forget(&mut self) {
        self.owner = None;
        self.log.clear();
    }
}

/// A GPU renderer: an executor and the glyph rasters it draws.
pub(crate) struct GpuRenderer {
    exec: Box<dyn Executor>,
    rasters: RasterCache,
    /// The canvases each window keeps as textures.
    canvases: CanvasCache,
    /// Whose frame the next one is, and its damage ([`next_frame`](Self::next_frame)).
    next: Option<(u64, Option<Vec<Rect>>)>,
    /// Whose frame is being drawn.
    drawing: Option<u64>,
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
            canvases: CanvasCache::default(),
            next: None,
            drawing: None,
            #[cfg(test)]
            lose: false,
        }
    }

    pub(crate) fn api(&self) -> Api {
        self.exec.api()
    }

    /// The next frame is window `owner`'s, and `damage` (logical rects;
    /// `None` for all of it) is what changed since its last one: its
    /// canvases are kept as textures and updated only there. Frames drawn
    /// without this draw canvases straight.
    pub(crate) fn next_frame(&mut self, owner: u64, damage: Option<Vec<Rect>>) {
        self.next = Some((owner, damage));
    }

    fn plan(&mut self, scene: &Scene, background: Color, scale: ScaleFactor) -> Plan {
        let physical = scale.to_physical(scene.logical_size());
        let next = self.next.take();
        self.drawing = next.as_ref().map(|n| n.0);
        let canvases = next.map(|(owner, damage)| ops::CanvasArgs {
            cache: &mut self.canvases,
            owner,
            damage,
        });
        ops::plan_with(scene, background, physical, &mut self.rasters, canvases)
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
        let frame = self.exec.read_back(&plan);
        self.counted(frame)
    }

    /// After drawing a frame: note its draw calls; one that failed leaves
    /// its window's canvas textures unknown, so they start over.
    fn counted<T>(&mut self, frame: Result<T, GpuError>) -> Result<T, GpuError> {
        let owner = self.drawing.take();
        if frame.is_ok() {
            LAST_DRAWS.with(|d| d.set(self.exec.draws()));
        } else if let Some(owner) = owner {
            self.canvases.forget(owner);
        }
        frame
    }

    /// Whether frames can go straight to `layer`.
    pub(crate) fn can_present(&self, layer: &GpuLayer) -> bool {
        self.exec.can_present(layer)
    }

    /// Render `scene` at `scale` into `layer` and present it. `dirty` is
    /// what changed since the frame presented to `layer` before (physical
    /// pixels), `None` for everything (see [`Executor::present`]).
    pub(crate) fn present(
        &mut self,
        scene: &Scene,
        background: Color,
        scale: ScaleFactor,
        layer: &GpuLayer,
        dirty: Option<[i32; 4]>,
    ) -> Result<(), GpuError> {
        self.lost()?;
        let mut plan = self.plan(scene, background, scale);
        let frame = self.exec.present(&mut plan, layer, scale, dirty);
        self.counted(frame)
    }

    /// Render only the pixel rectangle `region` of `scene` over the frame
    /// the executor's offscreen target holds (the last one rendered), and
    /// read it back: the partial redraw a window gets, checked against a
    /// whole frame.
    #[allow(dead_code)] // the comparison tests use it
    pub(crate) fn render_partial(
        &mut self,
        scene: &Scene,
        background: Color,
        scale: ScaleFactor,
        region: [i32; 4],
    ) -> Result<Pixmap, GpuError> {
        let mut plan = self.plan(scene, background, scale);
        plan.limit(region);
        let frame = self.exec.read_back(&plan);
        self.counted(frame)
    }

    /// Let go of what presents to `layer`: its window is going away.
    pub(crate) fn release(&mut self, layer: &GpuLayer) {
        self.exec.release(layer);
    }

    /// Render `scene` offscreen and wait for the GPU, without a read-back;
    /// with `region`, only that pixel rectangle over the last frame (see
    /// [`render_partial`](Self::render_partial)).
    pub(crate) fn finish(
        &mut self,
        scene: &Scene,
        background: Color,
        scale: ScaleFactor,
        region: Option<[i32; 4]>,
    ) -> Result<(), GpuError> {
        let mut plan = self.plan(scene, background, scale);
        if let Some(r) = region {
            plan.limit(r);
        }
        let frame = self.exec.finish(&plan);
        self.counted(frame)
    }

    /// Texture uploads in the last frame.
    pub(crate) fn uploads(&self) -> usize {
        self.exec.uploads()
    }

    /// Draw calls in the last frame.
    #[allow(dead_code)] // the comparison tests and benchmark use it
    pub(crate) fn draws(&self) -> usize {
        self.exec.draws()
    }

    /// The device's name, if known.
    pub(crate) fn device(&self) -> String {
        self.exec.device()
    }

    /// Whether the device is a hardware GPU, by its name (see
    /// [`is_hardware_device`](super::is_hardware_device)).
    pub(crate) fn hardware(&self) -> bool {
        super::is_hardware_device(&self.device())
    }

    /// Bytes of texture the executor's caches hold.
    pub(crate) fn texture_bytes(&self) -> usize {
        self.exec.texture_bytes()
    }

    /// Bytes of canvas pixels the last frame rasterized and uploaded, and
    /// bytes of canvas pixels kept on the CPU.
    pub(crate) fn canvas_bytes(&self) -> (usize, usize) {
        (self.canvases.uploaded(), self.canvases.bytes())
    }

    /// Make the next frame fail as if the device were lost.
    #[cfg(test)]
    pub(crate) fn lose_device(&mut self) {
        self.lose = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::PhysicalSize;

    const SIZE: (u32, u32) = (100, 100);

    #[test]
    fn a_back_buffer_catches_up_on_the_frames_it_missed() {
        let mut log = DamageLog::default();
        let a = Some([10, 10, 20, 20]);
        let b = Some([50, 50, 60, 60]);
        // Nothing presented yet: everything.
        assert_eq!(log.region(1, SIZE, a), None);
        log.presented(SIZE, None);
        assert_eq!(log.region(1, SIZE, a), a);
        // Unknown contents: everything.
        assert_eq!(log.region(0, SIZE, a), None);
        // Two frames old, and the frame before changed everything.
        assert_eq!(log.region(2, SIZE, a), None);
        log.presented(SIZE, a);
        assert_eq!(log.region(2, SIZE, b), Some([10, 10, 60, 60]));
        assert_eq!(log.region(1, SIZE, b), b);
        // Older than the log.
        assert_eq!(log.region(9, SIZE, b), None);
        // A new size, or a change of the whole frame: everything.
        assert_eq!(log.region(1, (100, 90), b), None);
        assert_eq!(log.region(1, SIZE, None), None);
        // Most of the frame: drawn whole.
        assert_eq!(log.region(1, SIZE, Some([0, 0, 90, 90])), None);
        // Clipped to the frame.
        assert_eq!(
            log.region(1, SIZE, Some([-5, 90, 10, 120])),
            Some([0, 90, 10, 100])
        );
    }

    #[test]
    fn a_kept_frame_is_its_windows_only() {
        let mut kept = KeptFrame::default();
        let mut plan = Plan {
            updates: Vec::new(),
            size: PhysicalSize::new(SIZE.0, SIZE.1),
            background: Color::rgb(1, 2, 3),
            ops: Vec::new(),
            partial: None,
            atlas: Vec::new(),
            cmds: Vec::new(),
            quads: Vec::new(),
        };
        let dirty = Some([10, 10, 20, 20]);
        assert_eq!(kept.region(1, &plan, dirty), None);
        kept.presented(1, SIZE, None);
        assert_eq!(kept.region(1, &plan, dirty), dirty);
        assert_eq!(kept.region(2, &plan, dirty), None, "another window's frame");
        assert_eq!(kept.limit(1, &mut plan, dirty), dirty);
        assert_eq!(plan.partial, dirty);
        kept.forget();
        assert_eq!(kept.region(1, &plan, dirty), None);
        // A see-through background can't be filled over the old frame.
        kept.presented(1, SIZE, None);
        plan.background = Color::rgba(1, 2, 3, 128);
        assert_eq!(kept.region(1, &plan, dirty), None);
    }
}
