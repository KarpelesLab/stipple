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
mod canvas;
mod color;
mod convert;
mod decoration;
pub(crate) mod font;
mod glyphs;
mod image;
mod lower;
mod path;
mod scene;
mod software;
mod surface;
mod text;

pub use bidi::TextDirection;
pub use canvas::{Canvas, CanvasPaint};
pub use color::Color;
pub use decoration::{Decoration, UnderlineStyle};
pub use font::{FontFamilyInfo, FontId, FontMetrics, FontStyle, FontWeight};
pub use glyphs::GlyphRun;
pub use image::{ImageError, ImageFit, ImageSource, Svg, image_formats};
pub use path::{LineCap, LineJoin, Path, StrokeStyle};
pub use scene::{DrawCmd, Scene};
pub use software::{RasterStats, SoftwareRenderer};
pub use surface::{Pixmap, Surface};
pub use text::{Font, FontError, TextLayout};

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
