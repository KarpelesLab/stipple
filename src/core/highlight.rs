//! Hover and press highlights that fade in and out instead of switching.
//!
//! The app paints a translucent overlay on the hovered tappable element, and a
//! stronger one while it's pressed. [`Highlights`] eases each element's overlay
//! toward its target with a [`Tween`], keyed by [`ActionId`], and reports when
//! everything has settled so the app stops asking for frames.

use std::time::Duration;

use crate::anim::{Easing, Tween};
use crate::core::runtime::ActionId;

/// How long the hover highlight takes to fade in or out.
pub const HOVER_FADE: Duration = Duration::from_millis(120);
/// How long the press highlight takes to appear; it fades out over
/// [`HOVER_FADE`], so a quick tap still shows.
pub const PRESS_FADE: Duration = Duration::from_millis(60);

/// One highlight level, easing from `from` to `to` starting at `start`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Channel {
    from: f64,
    to: f64,
    start: Duration,
    length: Duration,
}

impl Channel {
    const OFF: Channel = Channel {
        from: 0.0,
        to: 0.0,
        start: Duration::ZERO,
        length: Duration::ZERO,
    };

    /// Time into the fade at `now`. It counts from one frame before the change,
    /// so the change already shows on the frame that starts it.
    fn progress(&self, now: Duration) -> Duration {
        now.saturating_sub(self.start)
            .saturating_add(crate::platform::frame::interval())
    }

    fn level(&self, now: Duration) -> f64 {
        let t = self.progress(now).as_secs_f64();
        Tween::new(
            self.from,
            self.to,
            self.length.as_secs_f64(),
            Easing::EaseOut,
        )
        .sample(t)
    }

    fn settled(&self, now: Duration) -> bool {
        self.length.is_zero() || self.progress(now) >= self.length
    }

    /// Head for `to` from wherever the level is now. With no clock (`now` is
    /// `None`) the change is immediate.
    fn retarget(&mut self, to: f64, length: Duration, now: Option<Duration>) {
        if self.to == to {
            return;
        }
        match now {
            Some(now) => {
                *self = Channel {
                    from: self.level(now),
                    to,
                    start: now,
                    length,
                };
            }
            None => {
                *self = Channel {
                    to,
                    from: to,
                    ..Channel::OFF
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    id: ActionId,
    hover: Channel,
    press: Channel,
}

/// The fading hover and press highlights of one window.
#[derive(Clone, Debug, Default)]
pub(crate) struct Highlights {
    entries: Vec<Entry>,
}

impl Highlights {
    /// Point the highlights at the currently `hovered` and `pressed` elements,
    /// as of `now` on the animation timeline (`None`: no clock, so no fading).
    pub(crate) fn update(
        &mut self,
        hovered: Option<ActionId>,
        pressed: Option<ActionId>,
        now: Option<Duration>,
    ) {
        for id in [hovered, pressed].into_iter().flatten() {
            if !self.entries.iter().any(|e| e.id == id) {
                self.entries.push(Entry {
                    id,
                    hover: Channel::OFF,
                    press: Channel::OFF,
                });
            }
        }
        for e in &mut self.entries {
            let hover = if Some(e.id) == hovered { 1.0 } else { 0.0 };
            e.hover.retarget(hover, HOVER_FADE, now);
            let (press, length) = if Some(e.id) == pressed {
                (1.0, PRESS_FADE)
            } else {
                (0.0, HOVER_FADE)
            };
            e.press.retarget(press, length, now);
        }
        let t = now.unwrap_or_default();
        self.entries.retain(|e| {
            let off = e.hover.to == 0.0 && e.press.to == 0.0;
            !(off && e.hover.settled(t) && e.press.settled(t))
        });
    }

    /// Whether any highlight is still fading at `now`.
    pub(crate) fn animating(&self, now: Option<Duration>) -> bool {
        let Some(now) = now else {
            return false;
        };
        self.entries
            .iter()
            .any(|e| !e.hover.settled(now) || !e.press.settled(now))
    }

    /// Each highlighted element with its overlay strength at `now`: 0 is none,
    /// 1 is the full hover highlight, 2 is hovered and pressed.
    pub(crate) fn levels(&self, now: Option<Duration>) -> Vec<(ActionId, f64)> {
        let now = now.unwrap_or(Duration::MAX);
        self.entries
            .iter()
            .map(|e| (e.id, e.hover.level(now) + e.press.level(now)))
            .filter(|(_, level)| *level > 0.0)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    fn level(h: &Highlights, id: ActionId, now: Duration) -> f64 {
        h.levels(Some(now))
            .into_iter()
            .find(|(i, _)| *i == id)
            .map_or(0.0, |(_, l)| l)
    }

    #[test]
    fn hover_fades_in_then_settles() {
        let a = ActionId(0);
        let mut h = Highlights::default();
        let t0 = Duration::from_secs(1);
        h.update(Some(a), None, Some(t0));
        let first = level(&h, a, t0);
        assert!(first > 0.0 && first < 1.0, "starts partway: {first}");
        assert!(h.animating(Some(t0)));
        assert!(level(&h, a, t0 + 50 * MS) > first);
        let done = t0 + HOVER_FADE - crate::platform::frame::interval();
        assert!((level(&h, a, done) - 1.0).abs() < 1e-9);
        assert!(!h.animating(Some(done)), "settled: no more frames");
    }

    #[test]
    fn leaving_fades_out_and_forgets_the_element() {
        let (a, b) = (ActionId(0), ActionId(1));
        let mut h = Highlights::default();
        let t0 = Duration::from_secs(1);
        h.update(Some(a), None, Some(t0));
        let t1 = t0 + 200 * MS;
        h.update(Some(b), None, Some(t1));
        assert!(
            level(&h, a, t1) > 0.0 && level(&h, a, t1) < 1.0,
            "a fades out"
        );
        assert!(level(&h, b, t1) > 0.0, "b fades in");
        let t2 = t1 + HOVER_FADE;
        h.update(Some(b), None, Some(t2));
        assert_eq!(level(&h, a, t2), 0.0);
        assert_eq!(h.entries.len(), 1, "a is gone once faded");
    }

    #[test]
    fn press_adds_to_hover_and_interrupts_smoothly() {
        let a = ActionId(3);
        let mut h = Highlights::default();
        let t0 = Duration::from_secs(1);
        h.update(Some(a), None, Some(t0));
        let t1 = t0 + HOVER_FADE;
        h.update(Some(a), Some(a), Some(t1));
        let pressed = level(&h, a, t1 + PRESS_FADE);
        assert!((pressed - 2.0).abs() < 1e-9);
        // Released partway through the fade-out: it continues from there.
        let t2 = t1 + PRESS_FADE;
        h.update(Some(a), None, Some(t2));
        let after = level(&h, a, t2 + 10 * MS);
        assert!(after < 2.0 && after > 1.0, "{after}");
    }

    #[test]
    fn without_a_clock_changes_are_immediate() {
        let a = ActionId(0);
        let mut h = Highlights::default();
        h.update(Some(a), None, None);
        assert_eq!(h.levels(None), vec![(a, 1.0)]);
        assert!(!h.animating(None));
        h.update(None, None, None);
        assert!(h.levels(None).is_empty());
    }
}
