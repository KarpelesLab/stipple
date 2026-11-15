//! Games: a per-frame update loop, fixed time steps, polled input and
//! rebindable actions. See `docs/games.md`.

mod actions;
mod fixed_step;
mod game_loop;
mod input;

#[cfg(test)]
mod app_tests;

pub use actions::{Actions, Binding, ParseBindingError};
pub use fixed_step::FixedStep;
pub(crate) use game_loop::Runtime;
pub use game_loop::{Frame, GameLoop, LoopStats, PausedFn, UpdateFn};
pub use input::InputState;
