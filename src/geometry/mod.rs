//! 2D geometry primitives for the Stipple UI toolkit.
//!
//! Stipple works in **logical pixels** (`f64`): device-independent units that
//! the platform layer converts to physical device pixels using a
//! [`ScaleFactor`]. All layout, hit-testing, and the public widget API speak
//! logical pixels; physical pixels appear only at the `stipple::platform` /
//! `stipple::render` boundary (see [`PhysicalSize`]).
//!
//! This crate is a dependency-free leaf. Interop with `oxideav-core`'s
//! `Transform2D` lives in `stipple::render`, at the render boundary, to keep the
//! geometry types free of any rendering dependency.

#![forbid(unsafe_code)]

mod affine;
mod insets;
mod physical;
mod point;
mod rect;
mod size;
mod transform;
mod vec2;

pub use affine::Affine;
pub use insets::Insets;
pub use physical::{PhysicalSize, ScaleFactor};
pub use point::Point;
pub use rect::Rect;
pub use size::Size;
pub use transform::Transform;
pub use vec2::Vec2;
