# Rendering on the GPU

Stipple renders on the CPU by default (`SoftwareRenderer`, through
`oxideav-raster`). `stipple::gpu` is an experimental GPU path behind Cargo
features, each linking the OS's own driver through hand-written FFI:

| Feature | Backend | What it does today |
|---|---|---|
| `mtl` | Metal (macOS, iOS) | Renders whole scenes and presents them straight to the window through a `CAMetalLayer`; `IOSurface` export |
| `gl` | OpenGL ES 2 over surfaceless EGL (Linux) | Renders whole scenes, read back and presented as a bitmap; `dma-buf` export and import |
| `vk` | Vulkan (Linux) | Device setup, a clear and a shader-drawn triangle, read back |
| `d3d` | Direct3D 11 (Windows) | A clear and a triangle on WARP, shared-handle export |
| (web) | WebGPU in `web/` | A WGSL triangle in the browser |

## Choosing the renderer

```rust
use stipple::render::Renderer;

let app = app.renderer(Renderer::Gpu);
```

- `Renderer::Software` draws on the CPU.
- `Renderer::Gpu` draws with the platform's GPU API: Metal on macOS and iOS
  (build with `--features stipple/mtl`), OpenGL ES 2 on Linux
  (`--features stipple/gl`). On macOS and iOS the frame goes straight into a
  `CAMetalLayer` over the window's view; on Linux it is read back and
  presented as a bitmap, until 6c presents GL and Vulkan frames directly.
- `Renderer::Auto`, the default, is the CPU for now. The GPU becomes the
  default platform by platform once it has been shown to match the CPU there
  and to be faster (Phase 6h).

The environment variable `STIPPLE_RENDERER` overrides the app's choice, for
trying an app either way without rebuilding it: `software` (or `cpu`), `gpu`,
`metal`, `gl`, or `auto`. With it set, Stipple says on stderr what it draws
with (`stipple: drawing with Metal`).

```sh
STIPPLE_RENDERER=metal cargo run --release --features mtl --example widgetsdemo
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
in `src/gpu/metal_scene.rs`.

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

Every presented frame is drawn whole: drawables don't keep the previous
frame, so redrawing only the damage would need a retained texture and a
copy, which is 6f's. An idle window still draws nothing: a frame with no
damage isn't drawn at all. Frames are paced by the display link as before;
`nextDrawable` blocks when three frames are in flight.

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

## How close it is to the software renderer

Unit tests (`src/gpu/compare.rs`) draw the same scenes both ways, on GLES2
or on Metal and compare
them pixel by pixel: boxes, paths, images, clips, layers (translated, rotated,
scaled, translucent and nested), text, a canvas, and a whole app through
`App::render_with`, at scales 1, 1.5 and 2. On Mesa's llvmpipe the mean
difference per channel stays under 0.25 of 255, with no pixel off by more than
64 in any of them (the tests allow a mean of 0.5 to 1 and a handful of edge
pixels). On Metal the mean stays under 0.23, with at most one such pixel in
the corpus. Edges can differ by a shade where the two antialias differently, an
upright clip at a fractional pixel is rounded to the nearest pixel instead of
blended, and glyph positions are rounded to a sixteenth of a pixel.

Further tests draw an app with `App::renderer(Renderer::Gpu)`, lose the
device mid-run and check the frame falls back to the CPU and the GPU comes
back, and (on a stand-in device) that a second loss leaves the app on the CPU.

The tests skip when there is no device, and fail instead when
`STIPPLE_REQUIRE_GPU` is set. `STIPPLE_GPU_DUMP=<dir>` writes both frames of
each comparison there.

```sh
STIPPLE_REQUIRE_GPU=1 cargo test --features gl --lib gpu::   # Linux
STIPPLE_REQUIRE_GPU=1 cargo test --features mtl --lib gpu::  # macOS
```

CI's GPU job runs them under Mesa's llvmpipe and screenshots `gpuwindow`
under Xvfb; the macOS visual job runs them on the runner's Metal device,
prints the frame times, and screenshots `widgetsdemo` with
`STIPPLE_RENDERER=metal`. Both upload the frames.

## Not done yet

The plan for hardware rendering on every platform, presented straight to the
window, is Phase 6 in the [roadmap](../ROADMAP.md#phase-6--hardware-rendering).
Today:

- GLES2 and Metal execute the scene; Vulkan, Direct3D and WebGPU only draw
  a test triangle.
- On Linux the GL frame is read back to the CPU and presented as a bitmap;
  only Metal presents straight to the window.
- `Renderer::Auto` is still the CPU everywhere.
- The Metal path on iOS is compile-checked; it hasn't run on a device or the
  simulator yet.
- Glyphs are one texture each rather than packed in an atlas, and every draw
  is its own draw call, so a frame with a lot of text costs more GPU calls
  than it needs.
- Embedded-content viewports: on a read-back frame the app blits registered
  content over the placeholder, as on the CPU path; presenting straight to
  the window it is drawn as an image, uploaded every frame.
