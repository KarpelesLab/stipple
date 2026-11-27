//! The Stipple rendering seam.
//!
//! `stipple::render` is the one crate that knows about `oxideav`. It turns a
//! [`Scene`] (a list of logical-pixel draw primitives) into an `oxideav-core`
//! scene graph, rasterizes it on the CPU with `oxideav-raster`, and hands the
//! result to a [`Surface`] for display.
//!
//! ```
//! use stipple::geometry::{Rect, ScaleFactor, Size};
//! use stipple::render::{Color, Scene, SoftwareRenderer};
//!
//! let mut scene = Scene::new(Size::new(64.0, 64.0));
//! scene.fill_round_rect(Rect::from_xywh(8.0, 8.0, 48.0, 48.0), 12.0, Color::rgb(80, 140, 255));
//!
//! let pixmap = SoftwareRenderer::new().render(scene, ScaleFactor::IDENTITY);
//! assert_eq!(pixmap.size().width, 64);
//! ```
//!
//! The [`Surface`] trait is the GPU-readiness boundary: today the
//! [`SoftwareRenderer`] produces a [`Pixmap`] that the platform layer blits;
//! a future raw-GPU backend (Metal / Vulkan / D3D12 / WebGPU) can implement
//! the same trait without changing any caller. See `ROADMAP.md` §2 / Phase 6.

#![forbid(unsafe_code)]

pub(crate) mod bidi;
mod blit;
mod canvas;
mod color;
mod color_glyph;
mod convert;
mod decoration;
pub(crate) mod emoji_seq;
pub(crate) mod flow;
pub(crate) mod font;
pub(crate) mod glyph_mask;
pub(crate) mod glyphs;
mod image;
pub(crate) mod lower;
mod morx;
pub(crate) mod nine_slice;
mod path;
mod renderer;
pub(crate) mod rich;
pub(crate) mod scene;
mod shaping;
mod software;
pub(crate) mod sprite;
pub(crate) mod surface;
mod text;

pub use bidi::TextDirection;
pub use canvas::{Canvas, CanvasPaint};
pub use color::Color;
pub use decoration::{Decoration, UnderlineStyle};
pub use flow::{
    FloatImage, FlowItem, FlowOptions, FlowPlacement, FlowShape, RelativeWidth, TextFlow, WrapSide,
};
pub use font::{FontFamilyInfo, FontId, FontMetrics, FontStyle, FontWeight};
pub use glyphs::GlyphRun;
pub use image::{ImageError, ImageFit, ImageSource, Svg, image_formats};
pub use nine_slice::{BackgroundImage, NineSlice, SliceFill};
pub use path::{LineCap, LineJoin, Path, StrokeStyle};
pub use renderer::Renderer;
pub use rich::{InlineIcon, RichText, SpanStyle};
pub use scene::{DrawCmd, Scene};

/// Images in another API's memory (a `dma-buf`, an `IOSurface`, a DXGI
/// shared handle, an `AHardwareBuffer`), drawn without a copy: see
/// [`ExternalImage`] and `docs/gpu.md`.
pub mod external {
    #[cfg(any(target_os = "linux", target_os = "fullrust", target_os = "android"))]
    pub use crate::platform::external::DmaBufPlane;
    #[cfg(any(target_os = "linux", target_os = "fullrust"))]
    pub use crate::platform::external::udmabuf;
    pub use crate::platform::external::{
        DRM_FORMAT_MOD_INVALID, DRM_FORMAT_MOD_LINEAR, ExternalError, ExternalFormat,
        ExternalImage, PlaneLayout, YuvColorSpace, imported_natively,
    };
}
pub(crate) use external::{ExternalFormat, ExternalImage};
pub use software::{RasterStats, SoftwareRenderer};
pub use sprite::{Filter, ImageOptions};
pub use surface::{Pixmap, Surface};
/// The looks of plain text in `font` at `size`, for the flow engine.
pub(crate) fn text_styling(font: &Font, size: f64) -> text::Styling {
    text::Styling::plain(font, size)
}
pub use text::{Ellipsis, Font, FontError, TextLayout, TextOutline, TextPaint, TextShadow};

/// Low-level conversions to `oxideav-core` coordinate types. Exposed for
/// crates that build scene-graph nodes directly (e.g. the future text-run
/// shaping bridge) and need to share Stipple's geometry conventions.
pub mod oxideav_bridge {
    pub use crate::render::convert::{to_ox_point, to_transform2d};
    /// The `oxideav` frame, pixel-format and registry types the image API
    /// takes ([`Pixmap::from_video_frame`](crate::render::Pixmap::from_video_frame),
    /// [`Pixmap::decode_with`](crate::render::Pixmap::decode_with)), re-exported
    /// so apps needn't depend on `oxideav-core` / `oxideav-image` directly.
    pub use oxideav_core::{PixelFormat, RuntimeContext, VideoFrame, VideoPlane};
    pub use oxideav_image::Image as OxImage;
}
