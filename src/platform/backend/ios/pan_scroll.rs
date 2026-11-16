//! A trackpad's or a mouse wheel's scrolling on iPad, from the
//! `UIPanGestureRecognizer` that [`pointer`](super::pointer) adds for
//! indirect scroll input: the recognizer reports how far the scroll has
//! gone since it began (`translationInView:`), in points; the app wants
//! each step. UIKit-free, so it's unit-tested on every host.
//!
//! The translation follows the fingers (natural scrolling): fingers moving
//! up drag the content up, which scrolls towards the end, so a step's delta
//! is the translation's change negated. Each gesture starts
//! ([`ScrollPhase::Started`]), moves and ends; a wheel notch is a short
//! gesture of its own.
//!
//! When the fingers lift while moving, the scroll coasts on
//! ([`Momentum`], [`ScrollPhase::Momentum`] steps), slowing down as a
//! `UIScrollView` does at its normal deceleration rate (0.998 of the speed
//! kept per millisecond), until it is slower than [`STOP_SPEED`].

use crate::platform::event::{ScrollDelta, ScrollPhase};

/// `UIGestureRecognizerState`.
pub(crate) const BEGAN: isize = 1;
pub(crate) const CHANGED: isize = 2;
pub(crate) const ENDED: isize = 3;
pub(crate) const CANCELLED: isize = 4;
pub(crate) const FAILED: isize = 5;

/// The scroll gesture in progress.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PanScroll {
    /// The translation last reported, while a gesture is on.
    last: Option<(f64, f64)>,
}

impl PanScroll {
    /// The recognizer's action: its `state` and translation. Returns the
    /// scroll to deliver, if any.
    pub(crate) fn update(&mut self, state: isize, tx: f64, ty: f64) -> Option<ScrollDelta> {
        match state {
            BEGAN => {
                self.last = Some((tx, ty));
                Some(ScrollDelta::pixels(-tx, -ty).phase(ScrollPhase::Started))
            }
            CHANGED => {
                let (lx, ly) = self.last.unwrap_or((0.0, 0.0));
                let started = self.last.is_none();
                self.last = Some((tx, ty));
                let (dx, dy) = (lx - tx, ly - ty);
                let phase = if started {
                    ScrollPhase::Started
                } else {
                    ScrollPhase::Moved
                };
                (dx != 0.0 || dy != 0.0 || started)
                    .then(|| ScrollDelta::pixels(dx, dy).phase(phase))
            }
            ENDED | CANCELLED | FAILED => self
                .last
                .take()
                .map(|_| ScrollDelta::pixels(0.0, 0.0).phase(ScrollPhase::Ended)),
            _ => None,
        }
    }
}

/// `UIScrollViewDecelerationRateNormal`: the share of the speed kept each
/// millisecond.
const DECELERATION: f64 = 0.998;
/// The slowest lift (points per second) that coasts.
pub(crate) const MIN_SPEED: f64 = 50.0;
/// The speed (points per second) at which coasting stops.
pub(crate) const STOP_SPEED: f64 = 10.0;

/// The scroll coasting after the fingers lifted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Momentum {
    /// The fingers' velocity as they lifted (points per second, following
    /// the fingers, as `velocityInView:` reports it).
    vx: f64,
    vy: f64,
    /// Seconds since the lift at the last step.
    last: f64,
    done: bool,
}

impl Momentum {
    /// Coasting from a lift at `(vx, vy)`, if fast enough.
    pub(crate) fn new(vx: f64, vy: f64) -> Option<Self> {
        (vx.hypot(vy) >= MIN_SPEED).then_some(Self {
            vx,
            vy,
            last: 0.0,
            done: false,
        })
    }

    /// How far the fingers' motion carries by `t` seconds after the lift.
    fn travel(&self, t: f64) -> (f64, f64) {
        let k = (DECELERATION.powf(t * 1000.0) - 1.0) / (1000.0 * DECELERATION.ln());
        (self.vx * k, self.vy * k)
    }

    /// The scroll from the last step to `t` seconds after the lift (natural
    /// direction: the travel negated), or `None` once it has stopped. The
    /// step that slows below [`STOP_SPEED`] is the last.
    pub(crate) fn step(&mut self, t: f64) -> Option<ScrollDelta> {
        if self.done {
            return None;
        }
        let t = t.max(self.last);
        let (x0, y0) = self.travel(self.last);
        let (x1, y1) = self.travel(t);
        self.last = t;
        if self.vx.hypot(self.vy) * DECELERATION.powf(t * 1000.0) < STOP_SPEED {
            self.done = true;
        }
        Some(ScrollDelta::pixels(x0 - x1, y0 - y1).phase(ScrollPhase::Momentum))
    }

    /// Whether it has stopped.
    pub(crate) fn done(&self) -> bool {
        self.done
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lift_coasts_on_slowing_down_and_stops() {
        assert_eq!(Momentum::new(0.0, 30.0), None, "too slow to coast");
        // Fingers flicking up at 1000 pt/s scroll on towards the end.
        let mut m = Momentum::new(0.0, -1000.0).unwrap();
        let first = m.step(1.0 / 60.0).unwrap();
        assert_eq!(first.phase, ScrollPhase::Momentum);
        assert_eq!(first.dx, 0.0);
        assert!(first.dy > 15.0 && first.dy < 17.0, "{first:?}");
        let mut total = first.dy;
        let mut last = first.dy;
        let mut frames = 1;
        while let Some(d) = m.step(f64::from(frames + 1) / 60.0) {
            assert!(d.dy <= last + 1e-9, "slows down");
            last = d.dy;
            total += d.dy;
            frames += 1;
            assert!(frames < 1000, "stops");
        }
        assert!(m.done());
        // The whole coast: v / (1000 ln(1/0.998)) ~ 499.5 pt, less the
        // tail below the stopping speed (~5 pt).
        assert!(total > 490.0 && total < 500.0, "{total}");
        // About 2.3 s at 60 Hz.
        assert!((130..150).contains(&frames), "{frames}");
        assert_eq!(m.step(10.0), None);
    }

    #[test]
    fn momentum_follows_both_axes_and_late_frames_catch_up() {
        let mut a = Momentum::new(300.0, 400.0).unwrap();
        let d = a.step(0.1).unwrap();
        assert!(d.dx < 0.0 && d.dy < 0.0, "against the fingers");
        assert!((d.dy / d.dx - 4.0 / 3.0).abs() < 1e-9);
        // One late step covers what two would.
        let mut b = Momentum::new(300.0, 400.0).unwrap();
        let (p, q) = (b.step(0.05).unwrap(), b.step(0.1).unwrap());
        assert!((p.dy + q.dy - d.dy).abs() < 1e-9);
        // A step back in time moves nothing.
        assert_eq!(b.step(0.02).unwrap().dy, 0.0);
    }

    #[test]
    fn steps_are_the_translations_change_negated() {
        let mut p = PanScroll::default();
        let d = p.update(BEGAN, 0.0, -3.0).unwrap();
        assert_eq!((d.dx, d.dy, d.phase), (0.0, 3.0, ScrollPhase::Started));
        // Fingers going up 10 more points scroll 10 towards the end.
        let d = p.update(CHANGED, 0.0, -13.0).unwrap();
        assert_eq!((d.dx, d.dy, d.phase), (0.0, 10.0, ScrollPhase::Moved));
        // Sideways, and back down.
        let d = p.update(CHANGED, 4.0, -8.0).unwrap();
        assert_eq!((d.dx, d.dy), (-4.0, -5.0));
        // No change, nothing to say.
        assert_eq!(p.update(CHANGED, 4.0, -8.0), None);
        let d = p.update(ENDED, 4.0, -8.0).unwrap();
        assert_eq!((d.dx, d.dy, d.phase), (0.0, 0.0, ScrollPhase::Ended));
        // Ended once.
        assert_eq!(p.update(ENDED, 0.0, 0.0), None);
        assert_eq!(p.update(CANCELLED, 0.0, 0.0), None);
    }

    #[test]
    fn a_change_without_a_beginning_starts_the_gesture() {
        let mut p = PanScroll::default();
        let d = p.update(CHANGED, 0.0, 5.0).unwrap();
        assert_eq!((d.dy, d.phase), (-5.0, ScrollPhase::Started));
        assert_eq!(p.update(0, 0.0, 0.0), None);
        assert!(p.update(CANCELLED, 0.0, 0.0).is_some());
    }

    #[test]
    fn deltas_are_pixels() {
        let mut p = PanScroll::default();
        let d = p.update(BEGAN, 0.0, -1.0).unwrap();
        assert_eq!(d.in_pixels(), (0.0, 1.0));
    }
}
