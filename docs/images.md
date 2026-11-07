# Images and SVG

The `image` and `svg` widgets show bitmaps and vector graphics. Everything is
decoded by the [`oxideav`](https://github.com/OxideAV) stack, the same pure-Rust
media stack Stipple renders with; Stipple uses no other image library.

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

`examples/imagedemo` shows both, and every fit mode.

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

## Keep the `Arc`

Images compare by identity, not by pixels. Decode once and keep the `Arc` in
your state (or a `static`); building the view from the same `Arc` each frame
costs nothing, and swapping in a different one repaints just the image's area.
Decoding inside the view would decode, and repaint, every frame.

## Limits

- The GPU path (`App::render_with`, `stipple::gpu`) doesn't draw images yet;
  they render on the CPU path. See ROADMAP Phase 7f.
- Animated GIF / APNG / WebP show their first frame.
