//! Gamepads in the [`App`]: [`App::on_gamepad`], and menus driven by a
//! gamepad ([`App::gamepad_navigation`]). See `docs/gamepads.md` and
//! `docs/games.md`.

use crate::App;
use crate::game::ui::nav::{NavInput, PadNav};
use crate::platform::KeyCode;
use crate::platform::gamepad::GamepadEvent;
use crate::platform::time::Instant;

/// Hears gamepad events (see [`App::on_gamepad`]).
pub(crate) type GamepadFn<S> = Box<dyn FnMut(&mut S, &GamepadEvent)>;

/// When gamepad navigation is on (see [`App::gamepad_navigation_when`]).
type NavWhen<S> = Box<dyn Fn(&S) -> bool>;

/// Menus driven by gamepads and Escape going back (see
/// [`App::gamepad_navigation`]).
pub(crate) struct GamepadNav<S> {
    pad: PadNav,
    /// When it is on (all the time without one).
    when: Option<NavWhen<S>>,
}

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

    /// Drive the menus with gamepads, as a game's are: the d-pad and the
    /// left stick move focus as the arrow keys do (a held direction repeats
    /// after 400 ms, every 120 ms), South (A on an Xbox pad, ✕ on a
    /// PlayStation one) presses the focused control as Enter does, and East
    /// (B, ○) goes back: an open menu closes, else the
    /// [`on_back`](Self::on_back) hook runs. Escape on the keyboard goes back
    /// the same way when nothing focused uses it.
    ///
    /// The arrows move focus by where the controls are inside an element
    /// made [`spatial_nav`](crate::core::Element::spatial_nav), which also
    /// gives its first control focus when nothing has it, so the pad has
    /// somewhere to start. This starts gamepad support (see
    /// [`gamepad`](crate::gamepad)); [`on_gamepad`](Self::on_gamepad) still
    /// hears every event first. See `docs/games.md`.
    ///
    /// ```no_run
    /// # use stipple::prelude::*;
    /// struct State { screens: Vec<&'static str> }
    /// # fn view(_: &State, _: &mut Cx<State>) -> Element { spacer() }
    /// App::new(State { screens: vec!["title"] }, view)
    ///     .gamepad_navigation()
    ///     .on_back(|s| {
    ///         if s.screens.len() > 1 {
    ///             s.screens.pop();
    ///             Handled::Yes
    ///         } else {
    ///             Handled::No
    ///         }
    ///     })
    ///     .run();
    /// ```
    pub fn gamepad_navigation(mut self) -> Self {
        self.gamepad_nav = Some(GamepadNav {
            pad: PadNav::default(),
            when: None,
        });
        // (Tests feed events by hand, without opening the devices.)
        #[cfg(not(test))]
        crate::platform::gamepad::start();
        self
    }

    /// [`gamepad_navigation`](Self::gamepad_navigation), while `when` says
    /// so: off while the game itself plays (the pad steers the ship, not the
    /// menu), or while a "press a button…" row waits for the pad's next
    /// button. Escape still goes back only while it is on.
    pub fn gamepad_navigation_when(mut self, when: impl Fn(&S) -> bool + 'static) -> Self {
        self = self.gamepad_navigation();
        if let Some(nav) = self.gamepad_nav.as_mut() {
            nav.when = Some(Box::new(when));
        }
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

    /// Whether gamepad navigation is on now.
    fn nav_on(&self) -> bool {
        self.gamepad_nav
            .as_ref()
            .is_some_and(|n| n.when.as_ref().is_none_or(|w| w(&self.state)))
    }

    /// The window a gamepad drives: the one with keyboard focus, else
    /// `fallback` (the window the event came to).
    fn nav_pane(&self, fallback: usize) -> usize {
        self.panes
            .iter()
            .position(|p| p.window_focused)
            .unwrap_or(fallback)
    }

    /// A gamepad event, for the menus: move focus, press, go back. Returns
    /// whether anything changed. Arranges the wake for a held direction's
    /// repeat.
    pub(crate) fn gamepad_nav_event(
        &mut self,
        idx: usize,
        event: &GamepadEvent,
        now: Instant,
    ) -> bool {
        let on = self.nav_on();
        let Some(nav) = self.gamepad_nav.as_mut() else {
            return false;
        };
        if !on {
            nav.pad.reset();
            return false;
        }
        let input = nav.pad.event(event, now);
        let deadline = nav.pad.deadline();
        if let Some(at) = deadline {
            let due = crate::platform::wake::deadline().map_or(at, |d| d.min(at));
            crate::platform::wake::set_deadline(Some(due));
        }
        match input {
            Some(input) => {
                let idx = self.nav_pane(idx);
                self.nav_apply(idx, input)
            }
            None => false,
        }
    }

    /// On a wake: the held direction's repeat, if one is due. Returns
    /// whether it changed anything.
    pub(crate) fn gamepad_nav_wake(&mut self, idx: usize, now: Instant) -> bool {
        let on = self.nav_on();
        let Some(nav) = self.gamepad_nav.as_mut() else {
            return false;
        };
        if !on {
            nav.pad.reset();
            return false;
        }
        let input = nav.pad.due(now);
        match input {
            Some(input) => {
                let idx = self.nav_pane(idx);
                self.nav_apply(idx, input)
            }
            None => false,
        }
    }

    /// When the held direction repeats next, if one is held.
    pub(crate) fn gamepad_nav_deadline(&self) -> Option<Instant> {
        self.gamepad_nav.as_ref().and_then(|n| n.pad.deadline())
    }

    /// Escape that nothing focused used: go back, when gamepad navigation
    /// is on. Returns whether it went back.
    pub(crate) fn escape_back(&mut self, idx: usize) -> bool {
        self.nav_on() && self.go_back(idx)
    }

    /// Do what a gamepad asked in window `idx`, as the keys would.
    fn nav_apply(&mut self, idx: usize, input: NavInput) -> bool {
        let rtl = self.theme.is_rtl();
        let (code, key) = match input {
            NavInput::Back => return self.go_back(idx),
            NavInput::Activate => (KeyCode::Enter, crate::core::KeyInput::Enter),
            NavInput::Move(dir) => {
                use crate::core::tab_focus::Direction;
                let code = match dir {
                    Direction::Left => KeyCode::ArrowLeft,
                    Direction::Right => KeyCode::ArrowRight,
                    Direction::Up => KeyCode::ArrowUp,
                    Direction::Down => KeyCode::ArrowDown,
                };
                (code, dir.key())
            }
        };
        // An open menu (a select's list, a context menu) first.
        if let Some(used) = self.menu_nav(idx, crate::menu_key_of(code, rtl)) {
            return used;
        }
        self.pane_press_key(idx, key)
    }

    /// Hand the primary window a gamepad event as a running app would: to
    /// [`on_gamepad`](Self::on_gamepad)'s hook, then to
    /// [`gamepad_navigation`](Self::gamepad_navigation), at `now`. Returns
    /// whether anything changed. For tests, and for gamepad events an app
    /// routes itself.
    pub fn gamepad_event(&mut self, event: &GamepadEvent, now: Instant) -> bool {
        let heard = self.run_gamepad(event);
        self.gamepad_nav_event(0, event, now) || heard
    }

    /// Run a held gamepad direction's repeat due at `now` in the primary
    /// window, as the running app does when it wakes. Returns whether it
    /// moved focus. For tests.
    pub fn gamepad_repeat(&mut self, now: Instant) -> bool {
        self.gamepad_nav_wake(0, now)
    }
}
