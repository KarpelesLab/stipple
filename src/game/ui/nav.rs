//! Gamepads driving a menu: the d-pad and the left stick move focus (with a
//! held direction repeating), South activates, East goes back. The App
//! feeds every [`GamepadEvent`] through a [`PadNav`] when
//! [`App::gamepad_navigation`](crate::App::gamepad_navigation) is on, and
//! turns what comes out into the keys a keyboard would press.

use std::time::Duration;

use crate::core::tab_focus::Direction;
use crate::platform::gamepad::{GamepadAxis, GamepadButton, GamepadEvent, GamepadId};
use crate::platform::time::Instant;

/// How far a stick has to lean (0 to 1) to press a direction.
pub const STICK_PRESS: f32 = 0.5;
/// How far back it has to come to let go of it.
pub const STICK_RELEASE: f32 = 0.3;
/// How long a direction is held before it repeats.
pub const REPEAT_DELAY: Duration = Duration::from_millis(400);
/// How often it repeats after that.
pub const REPEAT_EVERY: Duration = Duration::from_millis(120);

/// What a gamepad asks a menu for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NavInput {
    /// Move focus that way (an arrow key).
    Move(Direction),
    /// Press the focused control (Enter).
    Activate,
    /// Go back (Escape, the app's `on_back`).
    Back,
}

/// Where a held direction comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    DPad,
    Stick,
}

/// The direction held, repeating.
#[derive(Clone, Copy, Debug)]
struct Held {
    pad: GamepadId,
    source: Source,
    dir: Direction,
    next: Instant,
}

/// A pad's left stick, and the direction it holds.
#[derive(Clone, Copy, Debug)]
struct Stick {
    pad: GamepadId,
    x: f32,
    y: f32,
    dir: Option<Direction>,
}

/// Turns gamepad events into menu navigation (see the module docs).
#[derive(Debug, Default)]
pub(crate) struct PadNav {
    held: Option<Held>,
    sticks: Vec<Stick>,
}

/// The direction a stick at `(x, y)` (+y down) holds, given the one it held:
/// past [`STICK_PRESS`] along its main axis to take one, and it keeps it
/// until it comes back within [`STICK_RELEASE`] or leans further another way.
pub(crate) fn stick_direction(x: f32, y: f32, held: Option<Direction>) -> Option<Direction> {
    let (ax, ay) = (x.abs(), y.abs());
    let lean = (ax.max(ay) > STICK_PRESS).then_some({
        if ax >= ay {
            if x > 0.0 {
                Direction::Right
            } else {
                Direction::Left
            }
        } else if y > 0.0 {
            Direction::Down
        } else {
            Direction::Up
        }
    });
    if let Some(dir) = held {
        let along = match dir {
            Direction::Right => x,
            Direction::Left => -x,
            Direction::Down => y,
            Direction::Up => -y,
        };
        if along > STICK_RELEASE && lean.is_none_or(|l| l == dir) {
            return Some(dir);
        }
    }
    lean
}

impl PadNav {
    /// Take a gamepad event at `now`: what the menu should do, if anything.
    pub(crate) fn event(&mut self, event: &GamepadEvent, now: Instant) -> Option<NavInput> {
        match *event {
            GamepadEvent::Button {
                id,
                button,
                pressed,
                ..
            } => {
                let dir = match button {
                    GamepadButton::DPadUp => Some(Direction::Up),
                    GamepadButton::DPadDown => Some(Direction::Down),
                    GamepadButton::DPadLeft => Some(Direction::Left),
                    GamepadButton::DPadRight => Some(Direction::Right),
                    GamepadButton::South if pressed => return Some(NavInput::Activate),
                    GamepadButton::East if pressed => return Some(NavInput::Back),
                    _ => None,
                }?;
                if pressed {
                    Some(self.hold(id, Source::DPad, dir, now))
                } else {
                    self.release(id, Source::DPad, Some(dir));
                    None
                }
            }
            GamepadEvent::Axis {
                id, axis, value, ..
            } => {
                if !matches!(axis, GamepadAxis::LeftX | GamepadAxis::LeftY) {
                    return None;
                }
                let at = match self.sticks.iter().position(|s| s.pad == id) {
                    Some(i) => i,
                    None => {
                        self.sticks.push(Stick {
                            pad: id,
                            x: 0.0,
                            y: 0.0,
                            dir: None,
                        });
                        self.sticks.len() - 1
                    }
                };
                let stick = &mut self.sticks[at];
                if axis == GamepadAxis::LeftX {
                    stick.x = value;
                } else {
                    stick.y = value;
                }
                let was = stick.dir;
                let dir = stick_direction(stick.x, stick.y, was);
                stick.dir = dir;
                match dir {
                    Some(d) if Some(d) != was => Some(self.hold(id, Source::Stick, d, now)),
                    Some(_) => None,
                    None => {
                        if was.is_some() {
                            self.release(id, Source::Stick, None);
                        }
                        None
                    }
                }
            }
            GamepadEvent::Disconnected { id } => {
                self.sticks.retain(|s| s.pad != id);
                if self.held.is_some_and(|h| h.pad == id) {
                    self.held = None;
                }
                None
            }
            _ => None,
        }
    }

    /// A direction goes down: move once now, and repeat while it's held.
    fn hold(&mut self, pad: GamepadId, source: Source, dir: Direction, now: Instant) -> NavInput {
        self.held = Some(Held {
            pad,
            source,
            dir,
            next: now.checked_add(REPEAT_DELAY).unwrap_or(now),
        });
        NavInput::Move(dir)
    }

    /// A direction comes up (any, for the stick): stop repeating it.
    fn release(&mut self, pad: GamepadId, source: Source, dir: Option<Direction>) {
        if self
            .held
            .is_some_and(|h| h.pad == pad && h.source == source && dir.is_none_or(|d| d == h.dir))
        {
            self.held = None;
        }
    }

    /// A repeat of the held direction, if one is due at `now`.
    pub(crate) fn due(&mut self, now: Instant) -> Option<NavInput> {
        let held = self.held.as_mut()?;
        if now < held.next {
            return None;
        }
        let next = held.next.checked_add(REPEAT_EVERY).unwrap_or(now);
        // After a stall, don't catch up: one move, then the usual pace.
        held.next = if next <= now {
            now.checked_add(REPEAT_EVERY).unwrap_or(now)
        } else {
            next
        };
        Some(NavInput::Move(held.dir))
    }

    /// When the next repeat is due, while a direction is held.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.held.map(|h| h.next)
    }

    /// Let go of everything (navigation turned off for now).
    pub(crate) fn reset(&mut self) {
        self.held = None;
        for s in &mut self.sticks {
            s.dir = None;
        }
    }
}
