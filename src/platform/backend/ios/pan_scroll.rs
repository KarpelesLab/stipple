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

#[cfg(test)]
mod tests {
    use super::*;

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
