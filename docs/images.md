# Images and SVG

The `image` and `svg` widgets show bitmaps and vector graphics. Everything is
decoded by the [`oxideav`](https://github.com/OxideAV) stack, the same pure-Rust
media stack Stipple renders with; Stipple uses no other image library.

<!-- live: imagedemo -->

```rust
use std::sync::Arc;
use stipple::prelude::*;

struct State {
    photo: Arc<Pixmap>,
    logo: Arc<Svg>,
}

let state = State {
    photo: Arc::new(Pixmap::decode(include_bytes!("photo.png"))?),
    logo: Arc::new(Svg::parse(include_bytes!("logo.svg"))?),
};

// In the view:
row(vec![
    image(&state.photo).width(128.0),       // height follows the aspect ratio
    svg(&state.logo).width(64.0).height(64.0),
    svg(&state.logo).fit(ImageFit::Cover).width(160.0).height(64.0).clip(),
])
```

`examples/imagedemo` shows both, and every fit mode. It reads them as
packaged files rather than compiling them in:

```rust
let sun = stipple::platform::assets().image("sun.png")?; // assets/sun.png
let logo = stipple::platform::assets().svg("logo.svg")?;
```

`include_bytes!` is fine for a few small images. For more, keep them as
files the app is packaged with (the APK's `assets/`, the bundle's, next to
the executable, on the web server) and load each when it's needed, off the UI
thread with `AppHandle::load_asset`; see
[Packaged assets](os-integration.md#packaged-assets).

## Formats

| Format | Crate | Notes |
|---|---|---|
| PNG, APNG | `oxideav-png` | APNG shows its first frame |
| JPEG | `oxideav-mjpeg` | baseline, progressive, lossless, arithmetic |
| GIF | `oxideav-gif` | first frame |
| WebP | `oxideav-webp` | lossy and lossless; first frame |
| BMP | `oxideav-bmp` | |
| SVG, SVGZ | `oxideav-svg` | rasterized by `oxideav-raster` |

`Pixmap::decode` sniffs the format through the `oxideav-image` gateway, over a
registry the codec crates above register into (`stipple::render::image_formats`).
For a format the stack has that Stipple doesn't register (HEIF, AVIF, TIFF, …),
build an `oxideav_core::RuntimeContext`, register that crate into it, and call
`Pixmap::decode_with(&ctx, bytes)`.

SVG support is what `oxideav-svg` parses: shapes, paths, gradients, transforms,
`<use>`, clip paths, masks and CSS styling. `<text>` is skipped unless a font
resolver is installed in `oxideav-svg`. In the current release a gradient in
`objectBoundingBox` units (the default) draws as a flat color; give gradients
`gradientUnits="userSpaceOnUse"` and absolute coordinates.

## Frames from the rest of the stack

Pixels from elsewhere in `oxideav` (a decoded video frame, a filter's output)
come in as a `VideoFrame`:

- `Pixmap::from_video_frame(&frame, size, format)` converts any pixel layout
  the stack converts from (YUV, RGB, gray, palettes, 16-bit) to RGBA. Packed
  RGBA is a row copy.
- `ImageSource::from_video_frame(...)` does the same and wraps the result for
  `image(...)`.
- `Pixmap::to_video_frame()` goes the other way: a single packed-RGBA plane.
- `Pixmap::from_oxideav_image(&img)` takes an `oxideav_image::Image`.

The `oxideav` types are re-exported in `stipple::render::oxideav_bridge`.

## Size and fit

An image sizes to its natural size in logical pixels: one logical pixel per
bitmap pixel, or the SVG's `width`/`height`. Set one of `.width()` / `.height()`
and the other follows the aspect ratio; set both to fix the box. `.fit(...)`
places the image in its box:

- `ImageFit::Contain` (default): scaled to fit, centered, letterboxed.
- `ImageFit::Cover`: scaled to cover, centered, clipped to the box.
- `ImageFit::Fill`: stretched to the box.

A bitmap is scaled when drawn, so it softens at high DPI unless it was decoded
at the higher resolution. An SVG is rasterized at the physical size of its box,
so it stays sharp at any scale; the last raster is cached.

## Backgrounds and nine-slices

Any element can have an image behind its content, drawn over its fill and
under its border: `.background_image(...)`.

- `BackgroundImage::image(source, fit)` places a bitmap or SVG in the box per
  an `ImageFit`: a painted screen behind a menu. (`ImageFit::Cover` clips what
  overflows.)
- A `NineSlice` is a frame drawn once and stretched to any size: the bitmap
  cut into nine by `insets` (in its pixels), the four corners drawn at their
  own size, the edges stretched along their length, the center filling what
  is left. Panels, buttons, text fields, dialogue boxes and bars are skinned
  this way.

```rust
use stipple::render::{NineSlice, SliceFill};

let frame = NineSlice::new(atlas.clone(), Insets::uniform(6.0))
    .region(Rect::from_xywh(0.0, 32.0, 24.0, 24.0)) // a part of an atlas
    .edges(SliceFill::Tile)    // or Stretch (the default), Round
    .center(SliceFill::Hidden) // a hollow frame
    .scale(2.0)                // logical pixels per source pixel
    .nearest();                // hard pixels, for pixel art
panel(cx).background_image(&frame).children(vec![...]);
```

- **Edges and center**: `SliceFill::Stretch` stretches, `Tile` repeats at the
  slice's size from the start and cuts the last copy short, `Round` repeats a
  whole number of copies each stretched a little to fit (CSS's `round`), and
  `Hidden` leaves the part out.
- **Scale**: `scale` is logical pixels per source pixel. Small pixel art
  wants 2 or 3 with `.nearest()`; art drawn at twice the size for high-density
  screens wants 0.5. The corners keep their size; when the box is smaller
  than two of them they shrink together, keeping their shape.
- **Padding**: the element's padding grows, side by side, to at least the
  slice's scaled insets, so the content stays off the frame. `.padding(...)`
  on the slice sets another content padding (a frame whose drawn border is
  thinner than its cuts); `.padding(...)` on the element afterwards wins.
- **Tint and opacity**: `.tint(color)` multiplies the pixels (one gray frame,
  tinted per use), `.opacity(o)` fades it.
- **Per state**: overrides swap it like any style:
  `.hovered(|e| e.background_image(&lit))`. A tappable element with a
  background image doesn't get the theme's hover highlight (a rectangle over
  a shaped frame would show its corners): give it a hovered frame instead.
  `StateSkin::apply` sets all the states at once. For the built-in widgets,
  put the frames in a `Skin` on the theme; see
  [games.md](games.md#skins-painted-frames).

`gameui`'s "Painted" look (choose "Look" on its title menu) dresses a whole
game UI in nine-slices drawn at start-up:

<!-- live: gameui -->

`Scene::draw_nine_slice(dst, &slice)` draws one directly (in a canvas, a
game's own drawing). Each piece is an image region whose edges land on whole
physical pixels, so the pieces meet without seams at any display scale: on the
software renderer an unscaled or whole-number-scaled slice takes the region
blit's fast path, and on the GPU each piece is a textured quad from the
bitmap's one texture. A tiled edge is cut into at most 1024 pieces, a tiled
center into about 16 000; past that the tiles grow.

## Keep the `Arc`

Images compare by identity, not by pixels. Decode once and keep the `Arc` in
your state (or a `static`); building the view from the same `Arc` each frame
costs nothing, and swapping in a different one repaints just the image's area.
Decoding inside the view would decode, and repaint, every frame.

## Memory

A decoded bitmap is RGBA, 4 bytes a pixel, whatever its file's size: a
1170×2532 phone background is 11.9 MB once decoded though its JPEG was a few
hundred kilobytes, and a 4096×4096 sprite sheet 64 MB. An SVG costs its parsed
tree plus its last raster (the box's physical size, again 4 bytes a pixel).
The GPU path then holds each bitmap a second time, as an RGBA texture; it
doesn't keep compressed (ASTC, ETC2, BC) textures yet.

So decode what the screen shows, at the size it's shown at, and let the rest
go: an image is freed when the last `Arc` to it is dropped, which a game does
as it leaves a scene. Loading the next scene's art with
`AppHandle::load_asset` decodes it on a worker thread, behind a loading
screen.

## Limits

- On the GPU path (`App::render_with` with `stipple::gpu`), each bitmap is a
  texture cached by its `Arc`, so keeping the `Arc` also saves the upload. See
  [gpu.md](gpu.md).
- Animated GIF / APNG / WebP show their first frame.
