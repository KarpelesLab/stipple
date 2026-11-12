//! Touch gestures: a finger dragged inside a scroll container scrolls it
//! (up and down, or sideways in a horizontal one) and keeps going (a fling)
//! when it lifts while moving; a finger held still opens the context menu (a
//! long press); two fingers pinch, pan and rotate an element that takes
//! pinches.
//!
//! [`TouchGestures`] is the recognizer's state machine, kept free of the app
//! so it can be fed synthetic touches in tests and behaves the same on every
//! platform (iOS, Android and the web hand it the same
//! [`Touch`](crate::platform::Touch)es). The app hands it every touch, asks it
//! whether the first finger's pointer events should still reach widgets, ticks
//! it once per frame while it flings, and asks it when a held finger becomes a
//! long press.
//!
//! A press doesn't become a scroll until the finger has moved more than
//! [`SLOP`] logical pixels; the direction it moved picks the innermost
//! container along that axis (a sideways strip inside a vertical list), and
//! with none that way the widget under the finger keeps it (a slider, a
//! swipe). Once it scrolls, the press is cancelled (the app clears the pressed
//! button and any text selection the press started) and the finger's pointer
//! events stop reaching widgets until it lifts. A finger that lands on a
//! fling stops it, and that touch doesn't click either. Under
//! [reduced motion](crate::anim::reduced_motion) there's no fling: the content
//! stops where the finger lifts.
//!
//! A finger that stays within the slop for [`LONG_PRESS`] seconds over an
//! element with a context menu (or a context handler) becomes a long press:
//! the app opens the menu where the finger is, as a right-click does, and the
//! press no longer taps when the finger lifts. Moving past the slop first (a
//! scroll, a drag) or a second finger landing cancels it.
//!
//! A second finger landing while the first is still pressing or scrolling
//! starts a pinch when an element under the two takes pinches
//! ([`Element::on_pinch`](crate::core::Element::on_pinch)): the press or the
//! scroll is cancelled, and every move reports the scale (the distance
//! between the fingers over where it started), the rotation and the
//! centroid's translation since the pinch began. It ends when either finger
//! lifts; the other is then ignored until it lifts too. Without such an
//! element a second finger is ignored, and the first goes on as before.

use crate::geometry::{Point, Vec2};
use crate::platform::{Touch, TouchPhase};

use super::{PinchPhase, PointerId, ScrollId, ScrollTargets};

/// How far (logical pixels) a finger moves before a press becomes a scroll
/// (and stops being a long press).
pub const SLOP: f64 = 8.0;
/// How long (seconds) a finger stays down and still before it is a long press:
/// the delay of iOS's `UILongPressGestureRecognizer`, and close to Android's
/// long-press timeout.
pub const LONG_PRESS: f64 = 0.5;
/// Fling speeds below this (logical px/s) aren't worth animating.
const MIN_FLING: f64 = 50.0;
/// The fastest fling (logical px/s).
const MAX_FLING: f64 = 8000.0;
/// A fling stops once it slows below this (logical px/s).
const STOP_FLING: f64 = 20.0;
/// How quickly a fling slows: its speed falls by `1/e` every `DECAY` seconds,
/// roughly the deceleration of iOS's and Android's scroll views.
const DECAY: f64 = 0.325;
/// The span of finger movement (seconds) the release velocity is measured over.
const VELOCITY_WINDOW: f64 = 0.1;

/// What is under a finger, as far as gestures go: asked when the first finger
/// goes down (at its position) and when a second one does (at the midpoint).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TouchTargets {
    /// The scroll containers a drag can scroll.
    pub scroll: ScrollTargets,
    /// Whether holding the finger opens a context menu (or runs a context
    /// handler) here.
    pub long_press: bool,
    /// The element that takes pinches here.
    pub pinch: Option<PointerId>,
}

/// A pinch's state at one touch, for the app to hand to its element as a
/// [`PinchEvent`](crate::core::PinchEvent).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pinch {
    /// The element taking the pinch.
    pub target: PointerId,
    pub phase: PinchPhase,
    /// The midpoint between the fingers (window coordinates).
    pub centroid: Point,
    /// The distance between the fingers over the distance when it began.
    pub scale: f64,
    /// How far the line between the fingers turned since it began (radians,
    /// clockwise on screen).
    pub rotation: f64,
    /// How far the centroid moved since it began (logical pixels): a
    /// two-finger pan.
    pub translation: Vec2,
}

/// What the app should do after a touch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TouchAction {
    /// Nothing changes for the app.
    None,
    /// The press became a scroll: cancel the pressed widget and any selection
    /// it started, then scroll `target` by `delta`.
    Begin { target: ScrollId, delta: f64 },
    /// Scroll `target` by `delta` logical pixels along its axis (positive
    /// reveals content further down, or further towards the end of a sideways
    /// one). A release that moved since the last move sends this instead of
    /// [`End`](Self::End), and may start a fling too.
    Scroll { target: ScrollId, delta: f64 },
    /// The scrolling finger lifted. If it was moving, a fling follows: call
    /// [`TouchGestures::tick`] every frame while [`TouchGestures::flinging`].
    End,
    /// A pinch began, moved or ended. When it begins, cancel the first
    /// finger's press as for [`Begin`](Self::Begin).
    Pinch(Pinch),
}

/// The two fingers of a pinch.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pair {
    target: PointerId,
    a: (u64, Point),
    b: (u64, Point),
    /// The distance, angle and midpoint when it began.
    distance: f64,
    angle: f64,
    centroid: Point,
}

impl Pair {
    fn new(target: PointerId, a: (u64, Point), b: (u64, Point)) -> Self {
        let mut pair = Self {
            target,
            a,
            b,
            distance: 1.0,
            angle: 0.0,
            centroid: Point::new(0.0, 0.0),
        };
        let (distance, angle, centroid) = pair.measure();
        // Two fingers on the same spot: measure from a pixel apart.
        pair.distance = distance.max(1.0);
        pair.angle = angle;
        pair.centroid = centroid;
        pair
    }

    fn measure(&self) -> (f64, f64, Point) {
        let (p, q) = (self.a.1, self.b.1);
        let (dx, dy) = (q.x - p.x, q.y - p.y);
        let centroid = Point::new((p.x + q.x) / 2.0, (p.y + q.y) / 2.0);
        ((dx * dx + dy * dy).sqrt(), dy.atan2(dx), centroid)
    }

    fn event(&self, phase: PinchPhase) -> Pinch {
        let (distance, angle, centroid) = self.measure();
        let mut rotation = angle - self.angle;
        // The shorter way round.
        if rotation > std::f64::consts::PI {
            rotation -= std::f64::consts::TAU;
        } else if rotation < -std::f64::consts::PI {
            rotation += std::f64::consts::TAU;
        }
        Pinch {
            target: self.target,
            phase,
            centroid,
            scale: distance.max(1.0) / self.distance,
            rotation,
            translation: Vec2::new(centroid.x - self.centroid.x, centroid.y - self.centroid.y),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    Idle,
    /// The first finger is down and hasn't moved past the slop; it went down
    /// at `start` at time `since`, and is at `at`. `caught` when it landed on
    /// a fling.
    Pending {
        id: u64,
        start: Point,
        at: Point,
        since: f64,
        targets: TouchTargets,
        caught: bool,
    },
    /// The first finger is down and its gesture belongs to a widget (a slider,
    /// a sideways swipe): its pointer events pass through untouched.
    Yielded {
        id: u64,
    },
    /// The first finger is scrolling `target` (sideways when `horizontal`);
    /// it is at `at`.
    Dragging {
        id: u64,
        target: ScrollId,
        horizontal: bool,
        at: Point,
    },
    /// The finger lifted while moving; `target` keeps scrolling at `velocity`
    /// (logical px/s, in offset terms), slowing down.
    Flinging {
        target: ScrollId,
        velocity: f64,
        last: f64,
    },
    /// The first finger became a long press: it is spent until it lifts.
    LongPressed {
        id: u64,
    },
    /// Two fingers are pinching.
    Pinching(Pair),
    /// A pinch ended with finger `id` still down: it is ignored until it lifts.
    Settling {
        id: u64,
    },
}

/// The touch gesture recognizer. See the [module docs](self).
#[derive(Clone, Debug)]
pub struct TouchGestures {
    state: State,
    /// The first finger down, which is also the pointer until it lifts.
    primary: Option<u64>,
    /// Recent finger positions along the scroll axis while dragging: (time,
    /// coordinate).
    samples: Vec<(f64, f64)>,
}

impl Default for TouchGestures {
    fn default() -> Self {
        Self {
            state: State::Idle,
            primary: None,
            samples: Vec::new(),
        }
    }
}

/// The coordinate of `p` along a container's axis.
fn along(p: Point, horizontal: bool) -> f64 {
    if horizontal { p.x } else { p.y }
}

impl TouchGestures {
    /// Feed one touch at time `now` (seconds, any fixed origin). `targets_at`
    /// says what is under a point: asked when the first finger goes down, and
    /// at the midpoint when a second one lands while the first presses or
    /// scrolls. Further fingers are ignored.
    pub fn touch(
        &mut self,
        touch: &Touch,
        now: f64,
        targets_at: impl FnOnce(Point) -> TouchTargets,
    ) -> TouchAction {
        match touch.phase {
            TouchPhase::Started if self.primary.is_none() => self.primary = Some(touch.id),
            TouchPhase::Ended | TouchPhase::Cancelled if self.primary == Some(touch.id) => {
                self.primary = None;
            }
            _ => {}
        }
        self.recognize(touch, now, targets_at)
    }

    /// The first finger down, which is also the pointer, while it is down.
    pub fn primary(&self) -> Option<u64> {
        self.primary
    }

    fn recognize(
        &mut self,
        touch: &Touch,
        now: f64,
        targets_at: impl FnOnce(Point) -> TouchTargets,
    ) -> TouchAction {
        let p = touch.position;
        match (self.state, touch.phase) {
            (State::Idle | State::Flinging { .. }, TouchPhase::Started) => {
                let caught = matches!(self.state, State::Flinging { .. });
                self.state = State::Pending {
                    id: touch.id,
                    start: p,
                    at: p,
                    since: now,
                    targets: targets_at(p),
                    caught,
                };
                TouchAction::None
            }
            // A second finger: a pinch, if something under the two takes it.
            // The first may have caught a fling (stopping it) on its way
            // down: two fingers landing on a list still gliding pinch too.
            (
                State::Pending { id, at, .. } | State::Dragging { id, at, .. },
                TouchPhase::Started,
            ) if id != touch.id => {
                let mid = Point::new((at.x + p.x) / 2.0, (at.y + p.y) / 2.0);
                let Some(target) = targets_at(mid).pinch else {
                    return TouchAction::None;
                };
                let pair = Pair::new(target, (id, at), (touch.id, p));
                self.state = State::Pinching(pair);
                self.samples.clear();
                TouchAction::Pinch(pair.event(PinchPhase::Began))
            }
            (State::Pinching(mut pair), TouchPhase::Moved) => {
                if pair.a.0 == touch.id {
                    pair.a.1 = p;
                } else if pair.b.0 == touch.id {
                    pair.b.1 = p;
                } else {
                    return TouchAction::None;
                }
                self.state = State::Pinching(pair);
                TouchAction::Pinch(pair.event(PinchPhase::Changed))
            }
            (State::Pinching(mut pair), TouchPhase::Ended | TouchPhase::Cancelled)
                if pair.a.0 == touch.id || pair.b.0 == touch.id =>
            {
                let other = if pair.a.0 == touch.id {
                    pair.a.1 = p;
                    pair.b.0
                } else {
                    pair.b.1 = p;
                    pair.a.0
                };
                self.state = State::Settling { id: other };
                let phase = if touch.phase == TouchPhase::Cancelled {
                    PinchPhase::Cancelled
                } else {
                    PinchPhase::Ended
                };
                TouchAction::Pinch(pair.event(phase))
            }
            (
                State::Settling { id } | State::LongPressed { id },
                TouchPhase::Ended | TouchPhase::Cancelled,
            ) if id == touch.id => {
                self.state = State::Idle;
                TouchAction::None
            }
            (
                State::Pending {
                    id,
                    start,
                    since,
                    targets,
                    caught,
                    ..
                },
                TouchPhase::Moved,
            ) if id == touch.id => {
                let still = |at| State::Pending {
                    id,
                    start,
                    at,
                    since,
                    targets,
                    caught,
                };
                let (dx, dy) = (p.x - start.x, p.y - start.y);
                if dx.abs() <= SLOP && dy.abs() <= SLOP {
                    self.state = still(p);
                    return TouchAction::None;
                }
                // The direction picks the container along it.
                let horizontal = dx.abs() > dy.abs();
                let target = if horizontal {
                    targets.scroll.horizontal
                } else {
                    targets.scroll.vertical
                };
                match target {
                    // Scroll, from where the finger crossed the slop so the
                    // content doesn't jump.
                    Some(target) => {
                        let moved = if horizontal { dx } else { dy };
                        let crossed = along(start, horizontal) + SLOP.copysign(moved);
                        self.state = State::Dragging {
                            id,
                            target,
                            horizontal,
                            at: p,
                        };
                        self.samples.clear();
                        self.samples.push((now, along(p, horizontal)));
                        TouchAction::Begin {
                            target,
                            delta: crossed - along(p, horizontal),
                        }
                    }
                    // Nothing to scroll that way: the widget under the finger
                    // has it (a caught fling's touch stays inert).
                    None if !caught => {
                        self.state = State::Yielded { id };
                        TouchAction::None
                    }
                    None => {
                        self.state = still(p);
                        TouchAction::None
                    }
                }
            }
            (
                State::Dragging {
                    id,
                    target,
                    horizontal,
                    at,
                },
                TouchPhase::Moved,
            ) if id == touch.id => {
                self.state = State::Dragging {
                    id,
                    target,
                    horizontal,
                    at: p,
                };
                self.sample(now, along(p, horizontal));
                let delta = along(at, horizontal) - along(p, horizontal);
                if delta == 0.0 {
                    TouchAction::None
                } else {
                    TouchAction::Scroll { target, delta }
                }
            }
            (
                State::Dragging {
                    id,
                    target,
                    horizontal,
                    at,
                },
                TouchPhase::Ended,
            ) if id == touch.id => {
                self.sample(now, along(p, horizontal));
                let velocity = self.release_velocity(now);
                // Under reduced motion the content stops with the finger.
                self.state = if velocity.abs() >= MIN_FLING && !crate::anim::reduced_motion() {
                    State::Flinging {
                        target,
                        velocity: velocity.clamp(-MAX_FLING, MAX_FLING),
                        last: now,
                    }
                } else {
                    State::Idle
                };
                self.samples.clear();
                // A last move folded into the release still scrolls.
                let delta = along(at, horizontal) - along(p, horizontal);
                if delta != 0.0 {
                    return TouchAction::Scroll { target, delta };
                }
                TouchAction::End
            }
            // A release far from where the finger went down with no move in
            // between: a busy app can have its moves folded into the release.
            // It scrolls the whole way, without a fling.
            (
                State::Pending {
                    id, start, targets, ..
                },
                TouchPhase::Ended,
            ) if id == touch.id => {
                self.state = State::Idle;
                let (dx, dy) = (p.x - start.x, p.y - start.y);
                let horizontal = dx.abs() > dy.abs();
                let (moved, target) = if horizontal {
                    (dx, targets.scroll.horizontal)
                } else {
                    (dy, targets.scroll.vertical)
                };
                match target {
                    Some(target) if moved.abs() > SLOP => TouchAction::Begin {
                        target,
                        delta: -moved,
                    },
                    _ => TouchAction::None,
                }
            }
            (
                State::Pending { id, .. } | State::Yielded { id } | State::Dragging { id, .. },
                TouchPhase::Ended | TouchPhase::Cancelled,
            ) if id == touch.id => {
                let was_dragging = matches!(self.state, State::Dragging { .. });
                self.state = State::Idle;
                self.samples.clear();
                if was_dragging {
                    TouchAction::End
                } else {
                    TouchAction::None
                }
            }
            _ => TouchAction::None,
        }
    }

    /// When (on [`touch`](Self::touch)'s clock) the finger held down becomes a
    /// long press, if it may: it is over something with a context menu and
    /// hasn't moved past the slop.
    pub fn long_press_deadline(&self) -> Option<f64> {
        match self.state {
            State::Pending {
                since,
                targets,
                caught: false,
                ..
            } if targets.long_press => Some(since + LONG_PRESS),
            _ => None,
        }
    }

    /// Whether the finger held down became a long press by `now`: then it
    /// returns where the finger is (the app opens the context menu there and
    /// cancels the press), and the finger is spent until it lifts.
    pub fn long_press_due(&mut self, now: f64) -> Option<Point> {
        let due = self.long_press_deadline()?;
        if now < due {
            return None;
        }
        let State::Pending { id, at, .. } = self.state else {
            return None;
        };
        self.state = State::LongPressed { id };
        Some(at)
    }

    /// Advance a fling to `now`: how far its container scrolls this frame, or
    /// `None` when there's no fling (it stops on its own once slow enough).
    pub fn tick(&mut self, now: f64) -> Option<(ScrollId, f64)> {
        let State::Flinging {
            target,
            velocity,
            last,
        } = self.state
        else {
            return None;
        };
        let dt = (now - last).max(0.0);
        let decay = (-dt / DECAY).exp();
        // The distance covered while the speed decays exponentially over dt.
        let delta = velocity * DECAY * (1.0 - decay);
        let velocity = velocity * decay;
        self.state = if velocity.abs() < STOP_FLING {
            State::Idle
        } else {
            State::Flinging {
                target,
                velocity,
                last: now,
            }
        };
        Some((target, delta))
    }

    /// Stop a fling (it reached the end of its content).
    pub fn stop_fling(&mut self) {
        if matches!(self.state, State::Flinging { .. }) {
            self.state = State::Idle;
        }
    }

    /// Hand the first finger's gesture to the widget it pressed (a slider or
    /// other draggable latched it), so it neither scrolls nor long-presses.
    pub fn yield_to_widget(&mut self) {
        if let State::Pending { id, caught, .. } = self.state
            && !caught
        {
            self.state = State::Yielded { id };
        }
    }

    /// Whether a fling is running (the app keeps asking for frames).
    pub fn flinging(&self) -> bool {
        matches!(self.state, State::Flinging { .. })
    }

    /// Whether the first finger is scrolling.
    #[cfg(test)]
    pub fn dragging(&self) -> bool {
        matches!(self.state, State::Dragging { .. })
    }

    /// Whether two fingers are pinching.
    #[cfg(test)]
    pub fn pinching(&self) -> bool {
        matches!(self.state, State::Pinching(_))
    }

    /// Whether the first finger's press should be ignored: it landed on a
    /// fling to stop it.
    pub fn swallows_press(&self) -> bool {
        matches!(self.state, State::Pending { caught: true, .. })
    }

    /// Whether the first finger's pointer moves should be kept from widgets:
    /// it is scrolling, or may start to (it's inside a scroll container and
    /// hasn't left the slop), caught a fling, became a long press, or is part
    /// of a pinch.
    pub fn holds_moves(&self) -> bool {
        match self.state {
            State::Pending {
                targets, caught, ..
            } => !targets.scroll.is_empty() || caught,
            State::Dragging { .. }
            | State::LongPressed { .. }
            | State::Pinching(_)
            | State::Settling { .. } => true,
            _ => false,
        }
    }

    fn sample(&mut self, now: f64, at: f64) {
        self.samples.push((now, at));
        // Keep a little more than the window, for a release after a pause.
        let keep = now - VELOCITY_WINDOW * 2.0;
        if let Some(first) = self.samples.iter().position(|&(t, _)| t >= keep) {
            self.samples
                .drain(..first.min(self.samples.len().saturating_sub(2)));
        }
    }

    /// The finger's speed at release, in offset terms (a finger moving up or
    /// left scrolls towards the end: positive). Zero if it paused before
    /// lifting.
    fn release_velocity(&self, now: f64) -> f64 {
        let recent: Vec<_> = self
            .samples
            .iter()
            .filter(|&&(t, _)| t >= now - VELOCITY_WINDOW)
            .copied()
            .collect();
        let (Some(&(t0, a0)), Some(&(t1, a1))) = (recent.first(), recent.last()) else {
            return 0.0;
        };
        // Too short a span to measure: a single sample, or one burst of
        // events with no time between them.
        if t1 - t0 < 0.005 {
            return 0.0;
        }
        (a0 - a1) / (t1 - t0)
    }
}

#[cfg(test)]
mod tests;
