# Rendering on the GPU

Stipple renders on the CPU by default (`SoftwareRenderer`, through
`oxideav-raster`). `stipple::gpu` is an experimental GPU path behind Cargo
features, each linking the OS's own driver through hand-written FFI:

| Feature | Backend | What it does today |
|---|---|---|
| `gl` | OpenGL ES 2 over surfaceless EGL (Linux) | Renders whole scenes (this page), `dma-buf` export and import |
| `vk` | Vulkan (Linux) | Device setup, a clear and a shader-drawn triangle, read back |
| `d3d` | Direct3D 11 (Windows) | A clear and a triangle on WARP, shared-handle export |
| `mtl` | Metal (macOS) | A clear and a triangle, `IOSurface` export |
| (web) | WebGPU in `web/` | A WGSL triangle in the browser |

Only the GL backend draws Stipple scenes yet.

## Rendering an app on the GPU

`App::render_with` swaps the rasterizer for any `Scene → Pixmap` function.
`stipple::gpu::render_scene_scaled` is one:

```rust
use stipple::render::{Scene, SoftwareRenderer};

let app = app.render_with(|scene: &Scene, background, scale| {
    stipple::gpu::render_scene_scaled(scene, background, scale).unwrap_or_else(|_| {
        // No GL device: fall back to the CPU.
        SoftwareRenderer::new().with_background(background).render(scene.clone(), scale)
    })
});
```

Build with `--features stipple/gl`. The frame is read back into a `Pixmap` and
presented through the platform surface, so it works with every Linux backend;
presenting the GPU frame without the read-back is future work. The
`gpuwindow` example does this in a real window.

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
targets and both caches live in a thread-local, kept from frame to frame.

## How close it is to the software renderer

Unit tests (`src/gpu/compare.rs`) draw the same scenes both ways and compare
them pixel by pixel: boxes, paths, images, clips, layers (translated, rotated,
scaled, translucent and nested), text, a canvas, and a whole app through
`App::render_with`, at scales 1, 1.5 and 2. On Mesa's llvmpipe the mean
difference per channel stays under 0.25 of 255, with no pixel off by more than
64 in any of them (the tests allow a mean of 0.5 to 1 and a handful of edge
pixels). Edges can differ by a shade where the two antialias differently, an
upright clip at a fractional pixel is rounded to the nearest pixel instead of
blended, and glyph positions are rounded to a sixteenth of a pixel.

The tests skip when there is no GL device, and fail instead when
`STIPPLE_REQUIRE_GPU` is set. `STIPPLE_GPU_DUMP=<dir>` writes both frames of
each comparison there.

```sh
STIPPLE_REQUIRE_GPU=1 cargo test -p stipple --features gl --lib gpu::
```

CI's GPU job runs them under Mesa's llvmpipe and uploads the frames, and
screenshots `gpuwindow` under Xvfb.

## Not done yet

- Only GLES2 executes the scene. The lowering (`src/gpu/ops.rs`) doesn't depend
  on GL, but Vulkan, Metal, Direct3D and WebGPU only draw a test triangle.
- The frame is read back to the CPU and presented as a bitmap; presenting the
  GL frame directly is future work.
- Glyphs are one texture each rather than packed in an atlas, and every draw
  is its own draw call, so a frame with a lot of text costs more GPU calls
  than it needs.
- Embedded-content viewports draw their placeholder; the app blits registered
  content over the read-back frame, as on the CPU path.
