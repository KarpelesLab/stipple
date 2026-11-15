//! Games: a per-frame update loop, fixed time steps, polled input and
//! rebindable actions, and 2D drawing helpers. See `docs/games.md`
//! (Phase 9 in `ROADMAP.md`).
//!
//! - Drawing: [`SpriteSheet`] and [`SpriteAnimation`] for sprites, [`Tilemap`]
//!   for tiled levels, [`Camera2D`] for a world-to-screen transform with
//!   follow and shake, and [`PixelViewport`] for a fixed low resolution scaled
//!   by whole numbers and letterboxed. They draw through
//!   [`Scene::draw_image_region`](crate::render::Scene::draw_image_region).

mod actions;
mod camera;
mod fixed_step;
mod game_loop;
mod input;
mod pixel;
mod sprite;
mod tilemap;

#[cfg(test)]
mod app_tests;
#[cfg(test)]
mod draw_tests;

pub use actions::{Actions, Binding, ParseBindingError};
pub use camera::Camera2D;
pub use fixed_step::FixedStep;
pub(crate) use game_loop::Runtime;
pub use game_loop::{Frame, GameLoop, LoopStats, PausedFn, UpdateFn};
pub use input::InputState;
pub use pixel::{PixelLayout, PixelViewport};
pub use sprite::{Playback, SpriteAnimation, SpriteSheet};
pub use tilemap::Tilemap;
