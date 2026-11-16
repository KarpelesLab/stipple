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
    /// Stipple's choice: the GPU where its path has met Phase 6's exit
    /// criteria and the device is a hardware GPU, the CPU everywhere else.
    /// That is, in a build with the platform's GPU feature: Metal on macOS
    /// and Vulkan on Linux, for windows the GPU presents to straight (X11's;
    /// not Wayland's yet), and in a browser WebGPU / WebGL2 on a hardware
    /// GPU. A software GPU (llvmpipe, lavapipe, WARP, SwiftShader, a virtual
    /// machine's) is slower than the CPU renderer, so `Auto` stays on the
    /// CPU there, as it does for headless rendering
    /// ([`App::render_once`](crate::App::render_once)) and on iOS, Windows,
    /// Android and Wayland for now. A GPU that fails to start or is lost
    /// falls back to the CPU. See `docs/gpu.md`, "The default".
    #[default]
    Auto,
    /// The CPU rasterizer, always.
    Software,
    /// The platform's own GPU API (Metal on macOS and iOS, Vulkan or else
    /// OpenGL ES on Linux and Android, Direct3D 11 on Windows, WebGPU or
    /// else WebGL2 in a browser), on whatever device it has,
    /// presenting straight to the window where the platform layer can and
    /// reading the frame back where it can't.
    Gpu,
}
