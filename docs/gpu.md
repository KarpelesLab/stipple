# Rendering on the GPU

Stipple renders on the CPU by default (`SoftwareRenderer`, through
`oxideav-raster`). `stipple::gpu` is an experimental GPU path behind Cargo
features, each linking the OS's own driver through hand-written FFI:

| Feature | Backend | What it does today |
|---|---|---|
| `mtl` | Metal (macOS, iOS) | Renders whole scenes and presents them straight to the window through a `CAMetalLayer`; `IOSurface` export and import |
| `vk` | Vulkan (Linux, Android) | Renders whole scenes and presents them straight to the window through a swapchain (X11, Android); read back elsewhere; `dma-buf` import (Linux) |
| `gl` | OpenGL ES 2 over EGL (Linux, Android) | Renders whole scenes and presents them straight to an EGL window surface (X11, Android) where there's no Vulkan; read back elsewhere; `dma-buf` export and import (Linux) |
| `d3d` | Direct3D 11 (Windows) | Renders whole scenes and presents them straight to the window through a flip-model DXGI swap chain; shared-handle export and import |
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
instant on the GPU), and embedded viewport content given as a `Pixmap` is
drawn as an image uploaded every frame (content given as an external image
is imported without a copy: see [External images](#external-images-without-a-copy)).

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
  tree), are shaded analytically: the exact pixel coverage of a sharp box or
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
- **Native controls.** Under `Theme::native()` the OS draws each control
  (uxtheme on Windows, AppKit on macOS, UIKit on iOS) into a bitmap, cached
  by control, value, state, light or dark, physical size and scale
  (`platform::native`), and the scene holds that bitmap as an image. The
  same `Arc` comes back every frame the control looks the same, so its
  texture is uploaded once and the GPU frame shows exactly the pixels the
  software path copies. When the system's look or a window's scale
  changes, the bitmaps are dropped and their textures freed on the next
  frame (see [the native theme](native-theme.md#on-the-gpu)).
- **Image regions** (`Scene::draw_image_region`: sprites, tiles) are textured
  quads from the whole bitmap's texture: the region's texture coordinates
  (swapped ends for a flip), kept half a texel inside the region, `GL_NEAREST`
  when sampled nearest, the tint and opacity applied in the shader.
- **Text** is drawn from the scene's own glyph outlines, whatever font set
  them: each glyph is rasterized once on the CPU per size, color, transform and
  sixteenth of a pixel of position, kept across frames (and dropped after 120
  frames unused), packed into the glyph atlas, and drawn as a region of it
  (see [Batching and the glyph atlas](#batching-and-the-glyph-atlas)).
- **Layers.** Transforms are applied to the geometry. A layer with an opacity
  below 1 is drawn into an offscreen target and composited once at that
  opacity, so overlapping content inside it doesn't show through itself.
- **Clips** are scissor rectangles when upright, stencil polygons when rotated.
- **Canvases** (`Element::canvas`) in a window's frames are kept as
  textures: rasterized on the CPU once, into exactly the pixels the canvas
  covers, then on each later frame only the frame's damage over it (the
  areas `invalidate_canvas_rect` named, whatever moved over it) is
  rasterized again and uploaded into the texture it already has
  (`replaceRegion`-style sub-uploads: a blit from a buffer on Metal,
  `UpdateSubresource` with a box on Direct3D 11, buffer-to-image copy
  regions on Vulkan, `glTexSubImage2D` on GLES2 and WebGL2, `writeTexture`
  with an origin on WebGPU). A terminal that changes a row a tick uploads a
  row. A canvas that changes over half its area three frames in a row (a
  game drawn into it, one that moves), one drawn rotated or skewed, partly
  off the window, or sharing its id with another canvas is instead drawn
  straight on the GPU: its paint callback runs for the part on screen and
  what it paints is drawn like the rest (a canvas back to changing little
  for ten frames becomes a texture again). `src/gpu/canvas.rs`.
- **Offscreen scenes** (`Scene::draw_offscreen`: a game's `PixelViewport`
  frame at 320×180, say, scaled up nearest-neighbor) are *resampled
  layers* (`Op::PushResampled`): the scene is lowered in the layer's own
  pixels, drawn into the top-left of a layer target (its scissor starts at
  those pixels, not the enclosing clip, and a partial frame keeps all of
  it), and that part of the target is composited into the destination
  rectangle with the scene's filter, nearest or bilinear. So a pixel-art
  game's frame never leaves the GPU and its sprite sheets stay textures,
  where the software renderer's bitmap of it would be uploaded anew every
  frame. Metal, Vulkan, GLES2 and Direct3D 11 execute them; on the web, drawn
  rotated or flipped, or bigger than the frame, the scene is drawn as the
  software renderer's bitmap instead. See [games.md](games.md#on-the-gpu).
- Gradients, soft masks, raw `oxideav` image nodes and concave clips (none of
  which Stipple's own widgets make) are rasterized on the CPU for just their
  bounds and drawn as an image.

Everything is blended in premultiplied alpha. The context, shaders, render
targets and both caches are kept from frame to frame.

The lowering (`src/gpu/ops.rs`), the glyph atlas (`src/gpu/atlas.rs`) and
batching (`src/gpu/batch.rs`) are shared; each GPU API is an executor of the
batched operation list (`src/gpu/renderer.rs`): GLES2 in `src/gpu/gl/scene.rs`, Metal
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
On an Apple M5 Max the widget window of [How fast](#how-fast) took 1.27 ms
on Metal drawn whole and 0.45 ms with a button's worth (120×40 logical) of
change; with batching and the glyph atlas (below), 0.55 and 0.37 ms. In Chrome on the same machine the work of a hover in the benchmark
app (`tools/bench/run_web.py`, `web_hover`'s `work_ms`) went from about
3.8–4.8 ms to 3.0–4.2 ms on WebGL2 and from 4.8–5.4 ms to 3.1–4.8 ms on
WebGPU; what's left is building the frame on the CPU.

## Batching and the glyph atlas

Boxes and textured quads (images, image regions and sprites, nine-slice
pieces, glyphs, layers composited) are all drawn by one *quad* shader whose
every input is a vertex attribute: a box's rectangle, color, radius and
stroke, or a textured quad's texture coordinates, clamp rectangle, tint and
distances to its antialiased edges, five `vec4`s a vertex (the format is at
the top of `src/gpu/batch.rs`). So the lowered operations, still in paint
order, are batched: a run of boxes and textured quads that samples one
texture (boxes sample none) and isn't interrupted by a path, a clip or a
layer is a single draw call, the quads' vertices one range of the frame's
vertex buffer. Nothing is reordered, so the frame is the same, just drawn
with fewer calls. A batch of boxes alone is drawn by a variant of the shader
without a texture.

Glyph rasters go into the **glyph atlas**: square pages of straight-alpha
RGBA, 512 pixels on a side at first, packed on shelves (rows of one height,
filled left to right, freed spans and emptied shelves reused). A full page is
doubled, up to 2048 pixels (every raster stays where it is); then a second
page opens; once both are full, the least recently used rasters (never one
the frame being drawn uses) are evicted to make room. Rasters keep the
transparent pixel their bounds were padded with and are sampled no closer
than half a texel to their edges, so bilinear filtering never reaches a
neighbor. A glyph too big for the atlas (over 256 pixels) is an image of
its own, as before. The pages live on the CPU in the lowering's glyph
cache; each frame carries a snapshot of them, and an executor keeps one
texture per page, uploading a page whole when it first sees it or it has
grown, and otherwise only the area written since the version it has (Metal
blits it in the frame's command buffer, Vulkan copies it from its staging
buffer, GLES uses `glTexSubImage2D`, Direct3D 11 `UpdateSubresource`, the web
`writeTexture` and `texSubImage2D`).

All of a frame's text is then one texture, so text batches with the boxes
around it: the busy widget window (865 boxes and glyphs) is one draw call
where it was 865, the 2000-sprite scene one where it was 2001, and an app's
widget gallery a handful. `stipple::gpu::last_frame_draw_calls()` says how
many the thread's last GPU frame took; the benchmarks report it
(`draw_calls`).

## External images, without a copy

A buffer that lives in another API's memory is drawn as an
`ExternalImage` (`stipple::render::external`): a `dma-buf` (Linux, Android), an `IOSurface`
(macOS, iOS), a DXGI shared handle (Windows) or an `AHardwareBuffer`
(Android), with its size, format (`Rgba8`, `Bgra8`, their `X` forms, and
`Nv12`: two-plane YUV 4:2:0, full range BT.601, as the camera delivers),
each plane's offset and stride and, for a `dma-buf`, the DRM format
modifier. `Scene::draw_external(rect, &image)` and
`draw_external_region(src, dst, &image, &options)` draw it where a
`Pixmap` would go (rotated, mirrored, faded, as `draw_image_region`); an
embedded viewport shows one with `App::set_viewport_external` /
`AppHandle::set_viewport_external`, and the camera's preview hands its
frames over this way (below).

Each GPU executor imports the buffer as a texture, cached by the image's
identity as a `Pixmap`'s texture is:

| Handle | Executor | How | Formats |
|---|---|---|---|
| `dma-buf` | GLES (Linux) | `EGL_EXT_image_dma_buf_import`, an `EGLImage` per plane (the modifier echoed), `glEGLImageTargetTexture2DOES` | all; NV12 as an `R8` and a `GR88` plane, converted in the shader |
| `dma-buf` | Vulkan (Linux) | `VK_EXT_external_memory_dma_buf` and `VK_KHR_external_memory_fd`, laid out with `VK_EXT_image_drm_format_modifier` (modifier, offset, pitch), a dedicated allocation | single-plane |
| `IOSurface` | Metal | `newTextureWithDescriptor:iosurface:plane:`, a texture per plane | BGRA, RGBA, NV12 (`R8Unorm` + `RG8Unorm`, converted in the shader) |
| DXGI shared handle | Direct3D 11 | `ID3D11Device1::OpenSharedResource1` (NT handle) or `OpenSharedResource`, a view in the texture's format | single-plane |
| `AHardwareBuffer` | GLES (Android) | `eglGetNativeClientBufferANDROID`, `EGL_NATIVE_BUFFER_ANDROID` | `R8G8B8A8`, `R8G8B8X8` |

Where an executor has no import for the handle or the format (NV12 on
Vulkan and Direct3D 11, any buffer on the web), or the driver refuses it,
the image is read on the CPU instead, once, and drawn as its pixels: a
linear `dma-buf` is mapped (`mmap`, inside `DMA_BUF_IOCTL_SYNC`), an
`IOSurface` locked, an `AHardwareBuffer` locked, NV12 converted as the
camera converts it. The software renderer draws external images the same
way. A tiled `dma-buf` or a DXGI texture can't be read there: a producer
that may meet the software renderer gives pixels with
`ExternalImage::cpu_pixels`.

**Lifetime and fences.** The image owns its handle (the descriptors are
closed, the surface and hardware buffer released, an NT handle closed when
the last `Arc` goes, after the `on_release` callback that hands the buffer
back to its producer). Executors hold the images a frame drew until the GPU
is done with it (the next frame on GLES, Vulkan and Direct3D 11; three
frames on Metal), so dropping an image right after handing a scene over is
safe. A producer still writing on its GPU attaches a `sync_file`
(`acquire_fence`, Linux and Android), waited on (up to 32 ms) before the
import or the read; past that, `dma-buf`'s implicit synchronization and
`IOSurface`'s order the writes. An image is one frame: a renderer keeps what
it imported for an image as long as the image lives, so a new frame is a
new `ExternalImage`, even of the same buffer.

**What tells the camera.** Each executor reports, every frame, the formats
it imports natively (`stipple::render::external::imported_natively`).
While the window's renderer imports NV12 (Metal), the camera's preview
gets the frame's `CVPixelBuffer` (retained, so the capture pool doesn't
reuse it while drawn) instead of converting it on the capture thread; on
the CPU renderer, or before the first GPU frame, it converts as before. A
canvas that draws an external image is always walked on the GPU rather than
kept as a CPU-rasterized texture (`gpu::canvas`), so the buffer is
imported, not read back.

`stipple::render::external::udmabuf(bytes)` makes a `dma-buf` from bytes
(the kernel's `udmabuf`, from a sealed `memfd`): for a CPU producer (a
content process drawing in software) and for the tests.

### Checking it

`src/gpu/external_tests.rs` draws real `dma-buf`s (from `udmabuf`: a BGRA
image with translucent pixels and padded rows, and an NV12 frame with its
chroma plane at an offset, drawn rotated and mirrored as the camera's
preview draws a phone's frames) on the GPU and on the CPU, which maps them,
and compares; on GLES it also checks nothing was uploaded. A third test
renders a buffer on the GPU itself, exports it through GBM (tiled, with a
vendor modifier) and imports it back. `STIPPLE_REQUIRE_UDMABUF` makes the
first two fail instead of skipping without `/dev/udmabuf`.

```sh
cargo test --features gl --lib external   # GLES
cargo test --features vk --lib external   # Vulkan
```

Run so far:

- GLES on Mesa's llvmpipe (locally with Mesa 26.1, and on CI's Ubuntu
  runner): the `udmabuf` BGRA and NV12 buffers are imported (no upload) and
  match the software renderer (BGRA exactly; NV12 within a mean of 0.33 of
  255, where the GPU samples the half-size chroma plane bilinearly and the
  CPU per pixel).
- Vulkan on Mesa's lavapipe (CI's Ubuntu runner): the BGRA buffer is
  imported (no upload, through `VK_EXT_image_drm_format_modifier` with the
  linear modifier) and matches exactly; the NV12 one is read on the CPU and
  matches.
- GLES on an NVIDIA GPU (driver 595): a buffer the GPU rendered and
  exported (modifier `0x0300000000606011`, block-linear) is imported and
  drawn. NVIDIA's driver accepts a `udmabuf` (system memory) but samples
  zeros from it, so the tests skip those there.
- Vulkan on the same NVIDIA GPU: the import of the GLES-exported,
  block-linear buffer goes through (image, memory, view) but samples
  nothing, which needs looking into.
- CI's `visual-gpu` job runs these on llvmpipe and lavapipe with
  `/dev/udmabuf` opened up (the runner's kernel has it built in), not
  blocking yet.
- Metal, Direct3D 11 and the Android paths are compile-checked only; the
  camera's zero-copy preview hasn't run on a Mac or an iPhone yet.

## Metal

The Metal executor is the GLES2 one translated: the same shaders in MSL
(batched boxes and textured quads, flat stencil fills, the path resolve), compiled from
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
all. Frames are paced by the display link as before, and the layer is
display-synced (`displaySyncEnabled`, macOS); `nextDrawable` blocks when
three frames are in flight. See [Frame pacing](#frame-pacing).

### How fast

On an Apple M5 Max (`cargo test --release --features mtl --lib --
--ignored bench_cpu_against_gpu --nocapture`), per frame:

| Scene | CPU | Metal | Metal, read back |
|---|---|---|---|
| A widget window at 2560×1600 (text, checkboxes, buttons) | 38.4 ms | 0.56 ms | 3.3 ms |
| 2000 sprites over a background at 1920×1080 | 33.3 ms | 0.7 ms | 2.2 ms |

(Before batching and the atlas, the same frames took 1.16 and 1.51 ms on
Metal, in 865 and 2001 draw calls; they now take one each.)

The Metal times include lowering the scene on the CPU and waiting for the
GPU to finish. In a window, `spritedemo` (2560×1504 pixels, redrawn every
frame) runs at the display's 120 fps either way, but takes 84% of a CPU core
on the software renderer and 4% on Metal.

Canvases kept as textures: the benchmark's `canvas_row` (one row of a
120×40 grid of characters in a canvas filling a 1024×768 window changed and
invalidated, to the frame read back) takes 1.4 ms on Metal, uploading
74 kB; drawing that canvas straight every frame instead took 171 ms (every
glyph of the grid rasterized and drawn again).

## Direct3D 11

The Direct3D 11 executor is the Metal one translated again: the same
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
waitable object (`SetMaximumFrameLatency(1)`) keeps at most one frame
queued: the renderer waits on it before drawing, so what it draws is the next
frame shown, and it paces the frame clock (see
[Frame pacing](#frame-pacing)). While the swap chain shows the frames the window stops
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
vertices in device pixels (path fills as fan triangles; boxes and image
quads batched, each batch one draw with its quad vertices). Images are
textures the page keeps by id, uploaded the first time an image is drawn and
freed when it's dropped or unused for 120 frames, exactly as the other
executors cache theirs; each glyph atlas page is a texture too, updated with
only what changed in it (`writeTexture`, `texSubImage2D`).

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
| The busy widget window at 2560×1600 (the harness) | 37 ms | 1.4 ms | 1.1 ms |
| 2000 sprites at 1920×1080 (the harness) | 58–65 ms | 1.4 ms | 1.0 ms |
| `themegallery` at 2560×1600, a whole new frame | 35 ms | 2.9 ms | 3.8 ms |
| `spritedemo` at 2560×1600, a whole new frame | 6.9 ms | 1.7 ms | 3.6 ms |
| `widgetsdemo` at 2560×1600, a whole new frame | 426 ms | 3.5 ms | 4.5 ms |

A scene of many long paths gains least: `canvasdemo` takes 300 ms on the
CPU and about 220 ms on WebGL2, most of it moving its vertices (a 34 MB op
buffer a frame); smaller path vertices are 6f's. (Before batching and the
glyph atlas, the harness's widget window took 2.9 ms on WebGPU and 3.3 ms on
WebGL2, its sprites 3.6 and 3.7 ms.) On SwiftShader
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
memory blocks rather than an allocation each), as are the glyph atlas
pages (whole the first time, then only what changed), every vertex of the
frame in one host-visible buffer (the paths' and clips', then the batched
quads'), the remaining uniforms as push constants, one descriptor set per
texture. Render passes load and store; a pass that starts cleared clears
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

The shaders are GLSL 4.50 in `src/gpu/shaders/` (`scene.vert`, `quad.vert`,
`quad.frag`, `quadbox.frag`, `flat.frag`, `resolve.frag`, the GLSL ES and MSL ones
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
With `VK_GOOGLE_display_timing`, a new swapchain's refresh
(`vkGetRefreshCycleDurationGOOGLE`) is reported to the frame clock.

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
  [How fast](#how-fast) takes 0.56 ms a frame for the widget window (2.4 ms read back) and
  0.66 ms for the sprites (1.6 ms), about Metal's own times.

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

## Frame pacing

Animation frames come from the frame clock (`stipple::platform::frame`, see
[Animation](animation.md#where-frames-come-from)): a view that animates asks
for a frame, the display source ticks at the next refresh, and the app
builds and presents. When nothing asks, no source is armed, nothing is
presented and the event loop doesn't wake: an idle app does no GPU work at
all (a frame with no damage isn't drawn either). What paces each frame:

| Platform | CPU frames | GPU frames |
|---|---|---|
| X11 | Present's `PresentNotifyMSC`, a `CompleteNotify` at the window's next vblank, on a connection of the clock's own | the same, the swapchain FIFO (Vulkan) or EGL's swap interval behind it |
| Wayland | `wl_surface.frame`, requested with each commit | the same (frames are read back and committed as `wl_shm` buffers) |
| Windows | `DwmFlush`, DWM's composition pass | the swap chain's frame-latency waitable object, after a present |
| macOS | `CVDisplayLink` | the same, into a display-synced `CAMetalLayer` |
| iOS | `CADisplayLink` | the same |
| Android | `AChoreographer` | the same |
| Web | `requestAnimationFrame` | the same, into the canvas |

On X11 the Present clock runs on a thread with its own connection, so the
backend's connection never sees Present's generic events: armed, it asks for
the vblank after the last one it saw (at once if that has passed) and ticks
when the notify comes. Without Present it isn't installed and the timer
paces frames at RandR's refresh, as before (`STIPPLE_X11_PRESENT_CLOCK=0`
forces that). On Windows, once a window has a swap chain, the clock's thread
waits on (a duplicate of) its waitable object after each present instead of
calling `DwmFlush`, and hands the signal it consumed to the renderer, which
then draws without waiting again: the frame is drawn exactly when the swap
chain has room for it. A frame with nothing presented since falls back to
`DwmFlush`.

The display's refresh comes from what the platform reports, each logged and
used to pace the fallback timer: RandR's mode and Present's UST/MSC over 60
vblanks (X11), `wl_output.mode`, `VK_GOOGLE_display_timing`, `CVDisplayLink`'s
nominal period, `UIScreen.maximumFramesPerSecond`. The clock also measures
the refresh it sees: the median gap between back-to-back ticks over the last
16 (`FrameStats::measured_interval`). A source that doesn't deliver a
requested frame within two refreshes is backed up by the timer, and one that
misses three in a row is no longer waited for, as before.

With `STIPPLE_DEBUG_FRAMES=1` the clock logs the reported and measured
refresh and, once a second while frames are presented, how many there were
and where they came from:

```text
stipple: frame pacing: Present reports a 16.66 ms refresh (60.0 Hz)
stipple: frame pacing: Present ticks every 16.71 ms (59.8 Hz), measured
stipple: frames: 60 presents in 1.00 s (60.0/s): 60 from the display (Present), 0 from the timer, 60 wakeups
```

Games are paced the same way: `App::game_loop`'s update runs when the
clock delivers a frame, and the frame it repaints goes through the same
present (`spritedemo --selftest` prints the presents' mean and longest
times and where the frames came from). On Xvfb, 60 frames a second from
Present with the CPU and with GLES on llvmpipe; with Vulkan on NVIDIA's
driver (which has no DRI3 on Xvfb) a 1280×720 present takes about 20 ms
and the game runs at about 50.

An idle app logs nothing. The `framepacing` example animates for three
seconds, then idles for three, and checks the presents came at roughly the
display's rate and that the idle app presented nothing and never woke. CI
runs it on Xvfb (CPU, and in `visual-gpu-present` with Vulkan on lavapipe
and GLES on llvmpipe), under headless sway, and, not blocking yet, on
Windows (CPU and Direct3D 11) and macOS (CPU and Metal). Run locally on
Xvfb with the CPU, GLES and Vulkan: 60 presents a second from Present, none
and no wakeups idle.

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
scaled, translucent and nested), text, a canvas, a whole app through
`App::render_with`, and a screen of controls under `Theme::native()`, at
scales 1, 1.5 and 2. On Windows that screen has the controls uxtheme draws,
and the test also checks that they differ from the self-drawn ones, that a
second frame uploads none of them again, and that dropping the OS-drawn
bitmaps uploads them anew. AppKit and UIKit draw only on the main thread,
which a test isn't on: there `nativegallery --gpu-check` (built with a GPU
feature) renders the native gallery both ways on the main thread, at the
same scales and tolerance, and prints `nativegallery gpu: ok`. On Mesa's llvmpipe the mean
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
the executor was written and compile-checked on a Mac). Both also run
`nativegallery --gpu-check`, the native theme on Metal and on Direct3D 11
against the CPU (non-blocking). They all upload the frames. The
`visual-gpu-present` jobs and the macOS and Windows visual jobs also run
`spritedemo --selftest` drawn with their GPU (it fails if pixels were
uploaded each frame) and `spritedemo --bench` on that GPU and on the CPU,
not blocking yet.

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
- Embedded-content viewports: on a read-back frame the app blits registered
  content over the placeholder, as on the CPU path; presenting straight to
  the window, a `Pixmap` is drawn as an image, uploaded every frame, and an
  external image is imported (see
  [External images](#external-images-without-a-copy), with what of it has
  run where).
