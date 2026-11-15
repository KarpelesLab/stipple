/// What draws an app's frames, set with [`App::renderer`](crate::App::renderer).
///
/// The environment variable `STIPPLE_RENDERER` overrides it, for trying an app
/// one way or the other without rebuilding it: `software` (or `cpu`), `gpu`
/// (this platform's GPU API), `metal`, `gl`, `d3d11`, `webgpu`, `webgl2`, or
/// `auto`.
/// On the web the page's `renderer` option (`web/stipple.js`) does.
///
/// Whatever is asked for, frames fall back to the [`SoftwareRenderer`] when
/// there is no GPU to draw with: the app was built without a GPU backend for
/// the platform (Cargo features `mtl`, `gl`, `d3d`, `gpu` on the web), there's
/// no device, setting it up fails, or the device is lost while the app runs (a lost device is set up
/// again once; when that fails too, the app stays on the CPU). See
/// `docs/gpu.md`.
///
/// [`SoftwareRenderer`]: crate::render::SoftwareRenderer
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Renderer {
    /// Stipple's choice. For now that is [`Software`](Self::Software)
    /// everywhere: the GPU becomes the default platform by platform once it
    /// matches the CPU closely enough on each (Phase 6 of the roadmap).
    #[default]
    Auto,
    /// The CPU rasterizer, always.
    Software,
    /// The platform's own GPU API (Metal on macOS and iOS, OpenGL ES on
    /// Linux, Direct3D 11 on Windows, WebGPU or else WebGL2 in a browser),
    /// presenting straight to the window where the platform layer can (macOS
    /// and iOS: a `CAMetalLayer`; Windows: a DXGI swap chain; the web: the
    /// canvas) and reading the frame back where it can't yet.
    Gpu,
}
