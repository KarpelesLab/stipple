# Rendering on the GPU

Stipple renders on the CPU by default (`SoftwareRenderer`, through
`oxideav-raster`). `stipple::gpu` is an experimental GPU path behind Cargo
features, each linking the OS's own driver through hand-written FFI:

| Feature | Backend | What it does today |
|---|---|---|
| `mtl` | Metal (macOS, iOS) | Renders whole scenes and presents them straight to the window through a `CAMetalLayer`; `IOSurface` export |
| `vk` | Vulkan (Linux, Android) | Renders whole scenes and presents them straight to the window through a swapchain (X11, Android); read back elsewhere |
| `gl` | OpenGL ES 2 over EGL (Linux, Android) | Renders whole scenes and presents them straight to an EGL window surface (X11, Android) where there's no Vulkan; read back elsewhere; `dma-buf` export and import (Linux) |
| `d3d` | Direct3D 11 (Windows) | Renders whole scenes and presents them straight to the window through a flip-model DXGI swap chain; shared-handle export |
| `gpu` on the web | WebGPU, else WebGL2, through `web/gpu.js` | Renders whole scenes straight into the page's canvas |

## Choosing the renderer

```rust
use stipple::render::Renderer;

let app = app.renderer(Renderer::Gpu);
```

- `Renderer::Software` draws on the CPU.
- `Renderer::Gpu` draws with the platform's GPU API: Metal on macOS and iOS
  (build with `--features stipple/mtl`); on Linux and Android Vulkan
  (`--features stipple/vk`), or OpenGL ES 2 where Vulkan can't be set up
  (`--features stipple/gl`; build with both for the fallback); Direct3D 11 on
  Windows (`--features stipple/d3d`); WebGPU or WebGL2 in a browser
  (`--features stipple/gpu`; see [the web](#the-web)). The frame goes
  straight to the window: into a `CAMetalLayer` over the view on macOS and
  iOS, a Vulkan swapchain or an EGL window surface on X11 and Android, a DXGI
  swap chain on Windows, the canvas on the web. On Wayland it is drawn on the
  GPU, read back and presented as a bitmap (see [Wayland](#wayland) for why).
- `Renderer::Auto`, the default, is the CPU for now. The GPU becomes the
  default platform by platform once it has been shown to match the CPU there
  and to be faster (Phase 6h).

The environment variable `STIPPLE_RENDERER` overrides the app's choice, for
trying an app either way without rebuilding it: `software` (or `cpu`), `gpu`,
`metal`, `vulkan` (or `vk`), `gl`, `d3d11` (or `d3d`), `webgpu`, `webgl2`, or
`auto`. On the web the page's `renderer` option (or `?renderer=` in its URL)
does the same. Stipple says on stderr what it draws with (`stipple: drawing
with Vulkan`), and once a frame has gone straight to a window, that it does
(`stipple: presenting with Vulkan straight to the window`).

```sh
STIPPLE_RENDERER=metal cargo run --release --features mtl --example widgetsdemo
STIPPLE_RENDERER=vulkan cargo run --release --features vk,gl --example widgetsdemo
STIPPLE_RENDERER=gl cargo run --release --features vk,gl --example widgetsdemo
STIPPLE_RENDERER=d3d11 cargo run --release --features d3d --example widgetsdemo
```

Frames fall back to the CPU whenever there's no GPU to draw with:

- the app was built without a GPU backend for this platform, or asks for one
  the platform doesn't have (`STIPPLE_RENDERER=gl` on a Mac);
- there's no device, or setting it up fails (the shaders don't compile, a
  pipeline can't be made);
- the device is lost while the app runs (a command buffer fails, a target
  can't be allocated). That frame is drawn on the CPU and the renderer is set
  up again on the next one; if the device is lost a second time, the app
  stays on the CPU from then on.

A window the GPU can't present to (Wayland; X11 without libxcb; an EGL
without its XCB platform) is still drawn on the GPU: the frame is read back
and presented as a bitmap. With `Renderer::Gpu` on Linux and Android, Vulkan
is tried first and OpenGL ES next, when both are built in.

Each of these is said once on stderr. When a window falls back, its GPU layer
is taken away and the next frame is presented whole through the window's
ordinary bitmap path. One renderer (device, pipelines, texture and glyph
caches) is shared by every window on the thread.

A renderer set with `App::render_with` (below) takes precedence over both
`App::renderer` and `STIPPLE_RENDERER`.

What the GPU path doesn't do yet: the theme cross-fade (a theme switch is
instant on the GPU), and embedded viewport content is drawn as an image
uploaded every frame.

## Rendering through `render_with`

`App::render_with` swaps the rasterizer for any `Scene → Pixmap` function.
`stipple::gpu::render_scene_scaled` (the platform's GPU API, read back) is one:

```rust
use stipple::render::{Scene, SoftwareRenderer};

let app = app.render_with(|scene: &Scene, background, scale| {
    stipple::gpu::render_scene_scaled(scene, background, scale).unwrap_or_else(|_| {
        // No GL device: fall back to the CPU.
        SoftwareRenderer::new().background(background).render(scene.clone(), scale)
    })
});
```

The frame is read back into a `Pixmap` and presented through the platform
surface. The `gpuwindow` example does this in a real window on Linux.

(`render_scene(scene, background, font)` is the same at scale 1; its `font` is
no longer used.)

## What is drawn how

The renderer walks the scene's node tree in paint order, as the software
renderer does, and turns it into device-space draw operations:

- **Boxes.** Upright rects and rounded rects, filled or stroked (what
  `fill_rect`, `fill_round_rect` and their strokes draw, so most of a widget
  tree), go through one shader: the exact pixel coverage of a sharp box or
  ring, a signed distance for rounded corners.
- **Paths.** Every other solid fill or stroke (curves, `fill_path`,
  `stroke_path` with joins, caps and dashes, rotated boxes) is flattened and
  stroked on the CPU exactly as `oxideav-raster` does it, then filled on the
  GPU by stencil-and-cover: the winding (non-zero or even-odd) is counted in a
  stencil buffer at 4×4 supersampling, and the 16 samples per pixel are
  averaged into the frame.
- **Images.** Bitmaps (`Scene::draw_image`: the controls the OS draws under
  `Theme::native()`, decoded images, SVG rasters) are textured quads with
  bilinear filtering and antialiased edges. Each texture is cached by the
  image's identity, its `Arc<Pixmap>`: an image that stays the same is
  uploaded once and reused every frame, a new `Arc` is uploaded anew, and a
  texture is freed once its image is dropped or hasn't been drawn for 120
  frames. The images stay plain `Pixmap`s, so `Pixmap::from_oxideav_image`,
  `from_video_frame` and `to_video_frame` work as before.
- **Image regions** (`Scene::draw_image_region`: sprites, tiles) are textured
  quads from the whole bitmap's texture: the region's texture coordinates
  (swapped ends for a flip), kept half a texel inside the region, `GL_NEAREST`
  when sampled nearest, the tint and opacity applied in the shader.
- **Text** is drawn from the scene's own glyph outlines, whatever font set
  them: each glyph is rasterized once on the CPU per size, color, transform and
  sixteenth of a pixel of position, kept across frames (and dropped after 120
  frames unused), and drawn as an image.
- **Layers.** Transforms are applied to the geometry. A layer with an opacity
  below 1 is drawn into an offscreen target and composited once at that
  opacity, so overlapping content inside it doesn't show through itself.
- **Clips** are scissor rectangles when upright, stencil polygons when rotated.
- **Canvases** run their paint callback for the part on screen, and what they
  paint is drawn like the rest.
- Gradients, soft masks, raw `oxideav` image nodes and concave clips (none of
  which Stipple's own widgets make) are rasterized on the CPU for just their
  bounds and drawn as an image.

Everything is blended in premultiplied alpha. The context, shaders, render
targets and both caches are kept from frame to frame.

The lowering (`src/gpu/ops.rs`) is shared; each GPU API is an executor of its
operation list (`src/gpu/renderer.rs`): GLES2 in `src/gpu/gl/scene.rs`, Metal
in `src/gpu/metal_scene.rs`, Vulkan in `src/gpu/vk/`, Direct3D 11 in
`src/gpu/d3d/scene.rs`, WebGPU and WebGL2 in `web/gpu.js` (fed by
`src/gpu/web.rs`).

## Redrawing only what changed

A window's frame on the GPU is redrawn only where it changed, the way the
software renderer repaints only the damage. The app's damage for the frame
(the tree diff, the hover and focus overlays, invalidated canvases, all in
logical pixels) becomes one rectangle of physical pixels around all of it, a
pixel wider all round. When the GPU still holds the frame before, the frame
is *limited* to that rectangle (`Plan::limit` in `src/gpu/ops.rs`): every
draw is clipped to it (a scissor, intersected with the frame's own clips),
the background is filled in it in place of the clear, the draws wholly
outside it are dropped before the executor sees them, and the target keeps
the rest of the previous frame (its colors are loaded, only its stencil is
cleared). A hover over one button then costs a few draws of a few thousand
pixels instead of the whole window.

What the GPU still holds is per backend:

| Backend | The previous frame is in | Presented |
|---|---|---|
| Metal | the executor's frame target, copied whole into each drawable (a `CAMetalLayer`'s drawables don't keep their contents, and it has no partial present) | whole |
| Vulkan | the executor's frame target, copied whole into the acquired image | with `VK_KHR_incremental_present` where the driver has it: the changed rectangle |
| GLES on EGL | the executor's frame target, drawn whole into the window surface | with `eglSwapBuffersWithDamageKHR`/`EXT` where EGL has it: the changed rectangle |
| Direct3D 11 | the swap chain's own buffers (flip-sequential keeps them): buffer 0 holds the frame before last, so the change of the last two frames is redrawn | `IDXGISwapChain1::Present1` with the redrawn rectangle as the dirty rectangle |
| WebGPU | a texture of the renderer's own (the canvas's is new each frame), copied into the canvas's | whole |
| WebGL2 | the canvas's drawing buffer, `preserveDrawingBuffer: true` | whole |

The executors that keep a frame target share it between their windows and
their read-backs, so it remembers whose frame it holds (`KeptFrame` in
`src/gpu/renderer.rs`); a swap chain's back buffers are tracked by their
age, with the change of each presented frame logged (`DamageLog`). The
frame is drawn whole whenever that's unknown or not worth it: the first
frame, a resize (a new target or new buffers), an expose or any forced
frame, a frame that failed or was skipped, another window's frame in the
shared target, a back buffer older than the log, damage the app can't
localize (`Damage::Full`), damage over more than 60% of the window, or a
background that isn't opaque (filling the rectangle with it wouldn't
replace what was there). Flip-discard swap chains (the fallback when
flip-sequential can't be made) and drivers without the partial-present
extensions still redraw only the change; they just present whole.

`a_partial_redraw_matches_the_software_renderer` (in `src/gpu/compare.rs`,
on every backend the comparison runs on) draws one frame whole and a second,
different everywhere, only in a rectangle over it, and checks the result
against the software renderer's two frames put together the same way;
`gpucompare.html` does the same on WebGPU and WebGL2 through the canvas, and
`a_window_frame_redraws_only_what_changed` presents to a `CAMetalLayer`.
On an Apple M5 Max the widget window of [How fast](#how-fast) takes 1.27 ms
on Metal drawn whole and 0.45 ms with a button's worth (120×40 logical) of
change. In Chrome on the same machine the work of a hover in the benchmark
app (`tools/bench/run_web.py`, `web_hover`'s `work_ms`) went from about
3.8–4.8 ms to 3.0–4.2 ms on WebGL2 and from 4.8–5.4 ms to 3.1–4.8 ms on
WebGPU; what's left is building the frame on the CPU.

## Metal

The Metal executor is the GLES2 one translated: the same four shaders in MSL
(boxes, textured quads, flat stencil fills, the path resolve), compiled from
source when the renderer is created (`newLibraryWithSource:`; no offline
toolchain and no precompiled `.metallib`), the same premultiplied blending,
paths by stencil-and-cover at 4×4 into a scratch target, layers in pooled
offscreen targets, scissor rectangles and stencil polygons for clips, and
images and glyphs as textures cached by identity. Every target is
`BGRA8Unorm` with a `Stencil8` texture beside it.

A frame is one command buffer; a change of target (a layer pushed or popped,
a path's scratch pass) starts a new render pass. Colors are blended on the
encoded sRGB values, as the software renderer blends them, and the window's
`CAMetalLayer` is tagged sRGB, so the window server matches the colors to the
display exactly as it does for the CPU frames. The layer's drawable is sized
to the window's physical pixels (its `contentsScale` is the display's scale).

A window's frames are drawn into a target the executor keeps, then copied
whole into the drawable (so the layer isn't `framebufferOnly`): drawables
don't keep the previous frame, and the kept one lets a frame redraw only what
changed (see [Redrawing only what changed](#redrawing-only-what-changed)).
An idle window still draws nothing: a frame with no damage isn't drawn at
all. Frames are paced by the display link as before; `nextDrawable` blocks
when three frames are in flight.

### How fast

On an Apple M5 Max (`cargo test --release --features mtl --lib --
--ignored bench_cpu_against_gpu --nocapture`), per frame:

| Scene | CPU | Metal | Metal, read back |
|---|---|---|---|
| A widget window at 2560×1600 (text, checkboxes, buttons) | 38.4 ms | 1.1 ms | 4.0 ms |
| 2000 sprites over a background at 1920×1080 | 33.3 ms | 1.8 ms | 3.3 ms |

The Metal times include lowering the scene on the CPU and waiting for the
GPU to finish. In a window, `spritedemo` (2560×1504 pixels, redrawn every
frame) runs at the display's 120 fps either way, but takes 84% of a CPU core
on the software renderer and 4% on Metal.

## Direct3D 11

The Direct3D 11 executor is the Metal one translated again: the same four
shaders in HLSL (`src/gpu/shaders/scene.hlsl`), the same premultiplied
blending, stencil-and-cover paths at 4×4 into a scratch target, pooled layer
targets, scissor rectangles and stencil polygons. Every target is
`B8G8R8A8_UNORM` with a `D24_UNORM_S8_UINT` buffer beside it (D3D11 has no
stencil-only format). It runs on a hardware device when there is one and on
WARP, the software rasterizer every Windows ships, otherwise. All of it goes
through hand-written COM vtables (`src/gpu/d3d/abi.rs` lists every slot it
calls and the descriptor layouts, which are unit-tested against the SDK's
sizes on any host).

The shaders are compiled when the renderer is created, with `D3DCompile`
from `d3dcompiler_47.dll`, loaded at run time. That DLL is part of Windows
8.1 and later, so nothing ships with the app; checked-in DXBC would need
Microsoft's compiler on every machine that changes a shader (it doesn't run
off Windows, and no shader compiler is allowed as a dependency), for a
compile that takes a few milliseconds once. Without the DLL the renderer is
unavailable and the app draws on the CPU.

A window drawn on the GPU gets a swap chain from
`IDXGIFactory2::CreateSwapChainForHwnd`: two `B8G8R8A8_UNORM` buffers,
flip-sequential (its buffers keep their frames, so a frame redraws only what
changed since the one before last and presents only that with `Present1`;
flip-discard where flip-sequential can't be made, drawn and presented
whole), stretched while the window is resized ahead of its next frame. Its buffers are resized to each frame's
physical size, so a resize or a move to a monitor with another DPI
(`WM_DPICHANGED` resizes the window, and the next frame is drawn at the new
scale) reaches the swap chain with the frame drawn for it. A frame-latency
waitable object keeps at most one frame queued: the renderer waits on it
before drawing, so what it draws is the next frame shown. It doesn't drive
the frame clock: frames are still paced by the compositor (`DwmFlush`), as
on the CPU path. While the swap chain shows the frames the window stops
painting its bitmap; when the renderer falls back, the swap chain is
released and the bitmap is painted again.

A removed or reset device (`DXGI_ERROR_DEVICE_REMOVED` from `Present`, a
failed `Map`, `GetDeviceRemovedReason`), or any call that fails mid-frame, is
a lost device: that frame is drawn on the CPU and the renderer is set up
again, once, as on Metal.

## The web

In a browser the module can't reach a GPU API itself, so the lowered frame
crosses into the page: `src/gpu/web.rs` encodes it as a compact buffer of
32-bit words (the format is at the top of that file) and `web/gpu.js` draws
it. Everything geometric is done in Rust, once: each draw carries its
vertices in device pixels (path fills as fan triangles, image quads already
through their transform, boxes padded for their antialiased edge). Images
and glyph rasters are textures the page keeps by id, uploaded the first time
an image is drawn and freed when it's dropped or unused for 120 frames,
exactly as the other executors cache theirs.

`gpu.js` first *records* the frame (the clip, layer and path logic, shared by
both APIs) into a list of render-target switches and draws, one array of
vertices and one of uniforms, then *replays* it with one upload of each:

- **WebGPU** through `canvas.getContext("webgpu")` in the browser's preferred
  format, premultiplied: the Metal executor's four shaders in WGSL, a
  `stencil8` texture beside each target, stencil-and-cover paths at 4×4 in an
  `r8unorm` scratch target, layers in offscreen targets, uniforms at dynamic
  offsets in one buffer.
- **WebGL2** where there's no WebGPU (or it can't draw into the canvas): the
  same in GLSL ES 3.0, with a `DEPTH24_STENCIL8` renderbuffer beside each
  target, one uniform block per draw (`bindBufferRange`) and sampler objects
  for nearest and bilinear sampling. The canvas is drawn y-down and
  offscreen targets top row first, so all of them sample alike.

The frame goes straight into the canvas: nothing is read back. On WebGPU it
is drawn into a texture of the renderer's own and copied into the canvas's,
and on WebGL2 the drawing buffer is preserved, so a frame redraws only what
changed over the one before (the op buffer's header says the frame is
partial; a page with no frame to draw over answers so, and the next frame is
drawn whole). A canvas holds
one kind of context, so the GPU's is taken on the app's first GPU frame; if
the app ends up on the CPU, `stipple.js` puts a fresh canvas in its place for
the 2D context.

Choosing: `App::renderer` and the page's `renderer` option (`auto`,
`software`, `gpu`, `webgpu`, `webgl2`; `?renderer=` in the URL overrides
both). `auto` is the app's choice, and on the web `Renderer::Auto` is the
GPU when the browser has a hardware one: it is much faster there (below),
while a browser without one hands out WebGPU and WebGL2 on SwiftShader,
where the software renderer is faster. `stipple.js` tells them apart once
per page: WebGL2 refuses a context asked for with
`failIfMajorPerformanceCaveat` on a software rasterizer, and the renderer's
name (`WEBGL_debug_renderer_info`) catches the rest (SwiftShader, llvmpipe,
lavapipe, Microsoft Basic Render). Only a module built with the renderer
(one that imports `gpu_frame`) is told (`stipple_gpu_hardware`). Natively
`Auto` stays the CPU until 6h.

Setting up is asynchronous (`requestAdapter`, `requestDevice`): the first
frame waits for it, then the page asks for a whole frame. WebGPU is tried
with a test clear of the canvas, since a browser without GPU compositing can
hand out a device and still fail to give the canvas a texture; then WebGL2
gets a fresh canvas. A lost device (`GPUDevice.lost`, `webglcontextlost`) is
set up again once, as natively, and a second loss leaves the app on the CPU.
So does a WebGPU validation error, which also asks for a new frame right
away so the one it spoiled is drawn on the CPU.

### Checking it

`examples/webdemo/gpucompare.html` loads `gpucompare.wasm` (the cdylib
example `gpucompare`, built with `--features gpu`), which holds the
comparison corpus of `src/gpu/corpus.rs`, draws each scene with the software
renderer and on WebGPU and WebGL2 into a canvas, and diffs the frames with
the same tolerances as the native tests. It loses each device and checks a
new one comes up, times both renderers, and with `?app=webdemo.wasm` runs the
web demo with `renderer: "gpu"` through two lost devices (back on the GPU
after the first, on the CPU after the second). CI's `visual-web-gpu` job runs
it in headless Chrome on SwiftShader; the site check (`tools/site/check.py`)
runs six of the examples a second time with `?renderer=gpu`. It also checks
a partial redraw: a frame presented whole, then another redrawn only in a
rectangle over it.

```sh
cargo build --release --target wasm32-unknown-unknown --features gpu --example gpucompare --example webdemo
# serve gpucompare.html, stipple.js, gpu.js, the two .wasm and font.ttf, open gpucompare.html?app=webdemo.wasm
```

In Chrome on an Apple M5 Max, the corpus stays within a mean of 0.33 of 255
per channel on both APIs (0.24 on SwiftShader), with at most 0.32% of the
pixels off by more than 64 (the 1.5× nine-slice, as natively).

Per frame, including lowering the scene and waiting for the GPU, against
rasterizing in the module and `putImageData` (the software path):

| Scene (Chrome, M5 Max) | Software | WebGPU | WebGL2 |
|---|---|---|---|
| The busy widget window at 2560×1600 (the harness) | 37 ms | 2.8 ms | 3.2 ms |
| 2000 sprites at 1920×1080 (the harness) | 58–65 ms | 4.0 ms | 4.2 ms |
| `themegallery` at 2560×1600, a whole new frame | 35 ms | 2.9 ms | 3.8 ms |
| `spritedemo` at 2560×1600, a whole new frame | 6.9 ms | 1.7 ms | 3.6 ms |
| `widgetsdemo` at 2560×1600, a whole new frame | 426 ms | 3.5 ms | 4.5 ms |

A scene of many long paths gains least: `canvasdemo` takes 300 ms on the
CPU and about 220 ms on WebGL2, most of it moving its vertices (a 34 MB op
buffer a frame); batching and smaller path vertices are 6f's. On SwiftShader
the GPU path is no faster than the CPU (27 to 150 ms against 38 to 41 for
the widget window, 63 against 56 for the sprites).
## Vulkan

The Vulkan executor (`src/gpu/vk/`, feature `vk`) runs the same operation
list in two steps. `record.rs` turns the plan into render passes and draws
(which pipeline, scissor, stencil reference, vertex range, push constants and
texture), the Metal executor's clip, layer and stencil-and-cover bookkeeping
without any Vulkan in it, so it is unit-tested on every host. `exec.rs` then
turns that into one command buffer: the frame's new images uploaded through a
staging buffer (textures cached by the image's identity, in shared 16 MB
memory blocks rather than an allocation each), every vertex of the frame in
one host-visible buffer, every uniform as push constants, one descriptor set
per texture. Render passes load and store; a pass that starts cleared clears
inside it; between passes one barrier makes the last pass's writes visible
and moves images that change roles (a layer drawn, then composited) to their
new layout. Targets are `R8G8B8A8_UNORM` (the frame, layers) and `R8_UNORM`
(the path scratch), each with the first stencil format the device can attach
(`S8_UINT`, else a depth-stencil one). One frame is in flight.

`libvulkan` is loaded at run time (`dlopen`, then `vkGetInstanceProcAddr`),
so a build with `vk` still starts where there's no Vulkan, and falls back to
GLES or the CPU. The device picked is a discrete GPU, else an integrated
one, else whatever there is (lavapipe).

### The shaders

The shaders are GLSL 4.50 in `src/gpu/shaders/` (`scene.vert`, `box.frag`,
`tex.frag`, `flat.frag`, `resolve.frag`, the GLSL ES and MSL ones
translated), with their SPIR-V checked in next to them and included with
`include_bytes!`: no shader compiler is a dependency. `tools/shaders.sh`
compiles them with glslang's `glslang`, at the release it pins (built without
the optimizer, so the bytes are the same on every machine), and with
`--check` fails if the checked-in SPIR-V differs; CI's `shaders` job builds
that glslang and runs the check. Unit tests read the modules back and check
their push-constant offsets and descriptor bindings against the ones the
recorder writes.

```sh
GLSLANG=path/to/glslang tools/shaders.sh          # after editing a shader
GLSLANG=path/to/glslang tools/shaders.sh --check  # what CI runs
```

### Presenting

The frame is drawn into its offscreen target exactly as for a read-back, then
copied into the acquired swapchain image on the GPU (`vkCmdBlitImage`, or a
copy when the formats match): a pass more than drawing into the swapchain
image, but one set of pipelines and one path for both. The swapchain is
FIFO, in an 8-bit UNORM format (the frame's encoded sRGB values copied as
they are, as the CPU frames are shown), re-made when the window's size
changes or it is out of date. Since the frame target persists, a window's
frame redraws only what changed in it, and with `VK_KHR_incremental_present`
the present names that rectangle (a new swapchain's first frame names none).

- **X11** (`VK_KHR_xcb_surface`). The backend speaks X11 over its own
  socket and has no `xcb_connection_t`, but a window's id is the same on
  every connection to the server: the GPU opens one of its own with libxcb
  (`libxcb.so.1`, loaded with `dlopen` when a window is first presented to;
  every X11 desktop has it) and makes the surface on the backend's window.
  The driver's presentation (DRI3 and Present, or `PutImage` under a software
  driver) goes over that connection, everything else over the backend's.
  Without libxcb, frames are read back.
- **Android** (`VK_KHR_android_surface`) on the activity's `ANativeWindow`,
  sized to the whole window, the app drawn at its content rectangle and the
  rest filled with the background. When the window goes away the backend has
  the swapchain destroyed before it hands the window back.
- **Wayland**: read back; see below.
- **macOS**, for working on the executor: `vk` also builds there and runs on
  MoltenVK (the Vulkan SDK's, or Homebrew's `molten-vk` and `vulkan-loader`),
  presenting to the window's `CAMetalLayer` through `VK_EXT_metal_surface`.
  Metal stays the platform's renderer. On an Apple M5 Max the benchmark of
  [How fast](#how-fast) takes 1.3 ms a frame for the widget window (3.3 ms read back) and
  1.4 ms for the sprites (2.5 ms), about Metal's own times.

### Wayland

On Wayland neither Vulkan nor GLES presents straight to the window yet.
`VK_KHR_wayland_surface` and `wl_egl_window` both take libwayland-client's
`wl_display` and the window's `wl_surface` proxy; the backend speaks the
protocol itself, and its objects can't be lent to a second connection (a
client's object ids are its own, and a subsurface must belong to the same
client as its parent). Loading libwayland just for the surface would mean
moving the whole backend onto it. The way that fits the backend is a
`dma-buf`: draw into an exportable image (Vulkan's
`VK_EXT_external_memory_dma_buf`, or GLES through GBM, as the `dma-buf` seam
below already does) and attach it with `zwp_linux_dmabuf_v1` on the
backend's own connection. That is planned with 6g's zero-copy work; until
then a Wayland window's frames are drawn on the GPU, read back and presented
through `wl_shm`.

## OpenGL ES on a window

Where there's no Vulkan, the GLES2 executor presents too: on the first frame
for a window it moves its context to an EGL display on the window system
(EGL's XCB platform, `EGL_EXT_platform_xcb` in Mesa 21 and later, on the
GPU's own libxcb connection, on X11; the one display on Android), picks the
first window config the window's surface takes, and from then on draws each
frame into its offscreen target, then that target into the window surface
as one quad flipped to the window's rows, and swaps (with
`eglSwapBuffersWithDamageKHR`/`EXT` naming what changed, where EGL has it).
No read-back. Read-backs
(tests, `render_scene_scaled`) keep working on the same context without a
surface.

## How close it is to the software renderer

Unit tests (`src/gpu/compare.rs`) draw the same scenes both ways, on GLES2,
Vulkan, Metal or Direct3D 11, and compare them pixel by pixel: boxes, paths, images, clips, layers (translated, rotated,
scaled, translucent and nested), text, a canvas, and a whole app through
`App::render_with`, at scales 1, 1.5 and 2. On Mesa's llvmpipe the mean
difference per channel stays under 0.25 of 255, with no pixel off by more than
64 in any of them (the tests allow a mean of 0.5 to 1 and a handful of edge
pixels). On Metal the mean stays under 0.23, with at most one such pixel in
the corpus. Vulkan through MoltenVK on the same Mac gives Metal's numbers to
the last digit; CI runs the corpus on Vulkan under Mesa's lavapipe. Edges can differ by a shade where the two antialias differently, an
upright clip at a fractional pixel is rounded to the nearest pixel instead of
blended, and glyph positions are rounded to a sixteenth of a pixel.

Further tests draw an app with `App::renderer(Renderer::Gpu)`, lose the
device mid-run and check the frame falls back to the CPU and the GPU comes
back, and (on a stand-in device) that a second loss leaves the app on the CPU.

The tests skip when there is no device, and fail instead when
`STIPPLE_REQUIRE_GPU` is set. `STIPPLE_GPU_DUMP=<dir>` writes both frames of
each comparison there.

```sh
STIPPLE_REQUIRE_GPU=1 cargo test --features gl --lib gpu::   # Linux, GLES
STIPPLE_REQUIRE_GPU=1 cargo test --features vk --lib gpu::   # Linux, Vulkan
STIPPLE_REQUIRE_GPU=1 cargo test --features mtl --lib gpu::  # macOS
STIPPLE_REQUIRE_GPU=1 cargo test --features vk --lib gpu::   # macOS, MoltenVK
STIPPLE_REQUIRE_GPU=1 cargo test --features d3d --lib gpu::  # Windows
```

(With `STIPPLE_RENDERER` set, the app tests compare the GPU with itself: it
overrides the CPU baseline's renderer too. Build with only the API to test.)

CI's GPU job runs them under Mesa's llvmpipe (GLES) and lavapipe (Vulkan) and
screenshots `gpuwindow` under Xvfb; the `visual-gpu-present` jobs screenshot
`widgetsdemo` presented straight to an Xvfb window with each of Vulkan and
GLES; the Android emulator run (API 34) starts the demo built with `vk` and
checks it presents with Vulkan (those two not blocking yet); the macOS visual
job runs the corpus on the runner's Metal device, prints the frame times, and
screenshots `widgetsdemo` with `STIPPLE_RENDERER=metal`; the Windows visual
job runs them on WARP and screenshots `widgetsdemo` with
`STIPPLE_RENDERER=d3d11` (both non-blocking until they have passed there:
the executor was written and compile-checked on a Mac). They all upload the
frames.

## Not done yet

The plan for hardware rendering on every platform, presented straight to the
window, is Phase 6 in the [roadmap](../ROADMAP.md#phase-6--hardware-rendering).
Today:

- GLES2, Vulkan, Metal, Direct3D 11, WebGPU and WebGL2 execute the scene.
- On Wayland the GPU frame is read back and presented as a bitmap (see
  [Wayland](#wayland)); Metal, Vulkan and GLES (X11, Android), Direct3D 11
  and the web (into the canvas) present straight to the window.
- The X11 and Android present paths (Vulkan and GLES) have been
  compile-checked and run in CI only, under Mesa's software drivers and the
  emulator; the Vulkan swapchain code has run on macOS through MoltenVK.
- Direct3D 11 is exercised by CI on WARP; it hasn't been tried on a
  hardware GPU yet.
- `Renderer::Auto` is still the CPU everywhere.
- The Metal path on iOS is compile-checked; it hasn't run on a device or the
  simulator yet.
- Only frames presented to a window are redrawn in part; a frame that is
  read back (Wayland, `render_once`, the native benchmark's GPU runs) is
  drawn whole. The partial presents of Direct3D 11, Vulkan and GLES (X11,
  Android) have been compile-checked and run in CI only; the partial
  redraw itself is checked on every backend by the comparison tests.
- Glyphs are one texture each rather than packed in an atlas, and every draw
  is its own draw call, so a frame with a lot of text costs more GPU calls
  than it needs.
- Embedded-content viewports: on a read-back frame the app blits registered
  content over the placeholder, as on the CPU path; presenting straight to
  the window it is drawn as an image, uploaded every frame.
