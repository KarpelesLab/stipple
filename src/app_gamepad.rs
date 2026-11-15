//! Gamepads in the [`App`]: [`App::on_gamepad`]. See `docs/gamepads.md`.

use crate::App;
use crate::platform::gamepad::GamepadEvent;

/// Hears gamepad events (see [`App::on_gamepad`]).
pub(crate) type GamepadFn<S> = Box<dyn FnMut(&mut S, &GamepadEvent)>;

impl<S: 'static> App<S> {
    /// Hear every gamepad event: a pad connecting or disconnecting, a button
    /// going down or up, an axis moving. `hook` gets the state and the
    /// [`GamepadEvent`], and every window re-renders after it.
    ///
    /// This starts gamepad support (see [`gamepad`](crate::gamepad)); pads
    /// already connected arrive as [`GamepadEvent::Connected`]. A game that
    /// polls instead reads [`gamepad::state`](crate::gamepad::state) each
    /// frame.
    ///
    /// ```no_run
    /// # use stipple::prelude::*;
    /// use stipple::gamepad::{GamepadButton, GamepadEvent};
    /// #[derive(Default)]
    /// struct State { jumps: u32 }
    /// # fn view(_: &State, _: &mut Cx<State>) -> Element { spacer() }
    /// App::new(State::default(), view)
    ///     .on_gamepad(|s, event| {
    ///         if let GamepadEvent::Button { button: GamepadButton::South, pressed: true, .. } = event {
    ///             s.jumps += 1;
    ///         }
    ///     })
    ///     .run();
    /// ```
    pub fn on_gamepad(mut self, hook: impl FnMut(&mut S, &GamepadEvent) + 'static) -> Self {
        self.gamepad = Some(Box::new(hook));
        crate::platform::gamepad::start();
        self
    }

    /// Hand a gamepad event to the hook. Returns whether there was one (the
    /// windows then need a rebuild).
    pub(crate) fn run_gamepad(&mut self, event: &GamepadEvent) -> bool {
        let Some(hook) = self.gamepad.as_mut() else {
            return false;
        };
        hook(&mut self.state, event);
        for pane in &mut self.panes {
            pane.dirty = true;
        }
        true
    }
}
