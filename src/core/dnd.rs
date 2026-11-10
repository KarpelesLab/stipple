//! The drag-over state of drop targets, for views to read and style.
//!
//! While a drag hovers a drop target, the app keeps a [`DropState`] for it:
//! whether the drag is over it, whether it takes what the drag carries, where
//! the pointer is within it, and a `level` that eases from 0 to 1 on enter and
//! back on leave or drop (like the hover highlight in
//! [`highlight`](super::highlight)). A view reads it with
//! [`Cx::drop_state`](crate::core::Cx::drop_state) and builds its drop zone
//! from it; the app rebuilds on enter, leave and drop, on every move over a
//! target whose view reads the state, and on each frame while a level eases.

use std::time::Duration;

use crate::core::highlight::Channel;
use crate::core::runtime::DropId;
use crate::geometry::Point;

/// How long a drop target's [`level`](DropState::level) takes to ease in or
/// out (no time at all under [reduced motion](crate::anim::reduced_motion)).
pub const DROP_FADE: Duration = Duration::from_millis(180);

/// What a drag is doing to one drop target, as of the frame being built. See
/// [`Cx::drop_state`](crate::core::Cx::drop_state).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DropState {
    /// A drag is over the target now (it is the top-most drop target under
    /// the pointer).
    pub over: bool,
    /// The target accepts what the drag carries. Kept from the last drag over
    /// it while `level` eases back to 0, so a fading tint keeps its color.
    pub accepted: bool,
    /// The pointer within the target (from its top-left corner, in logical
    /// pixels) while the drag is over it.
    pub position: Option<Point>,
    /// Eases from 0 to 1 over [`DROP_FADE`] when a drag enters, and back to 0
    /// when it leaves or drops. Drive a grow, a glow or a tint with it.
    pub level: f64,
}

impl DropState {
    /// Whether the state shows anything: a drag over the target, or a level
    /// still easing.
    pub fn is_active(&self) -> bool {
        self.over || self.level > 0.0
    }
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    id: DropId,
    channel: Channel,
    accepted: bool,
}

/// The easing drag-over levels of one window's drop targets.
#[derive(Clone, Debug, Default)]
pub(crate) struct DropFades {
    entries: Vec<Entry>,
    /// The target under the drag, whether it accepts, and the pointer in it.
    over: Option<(DropId, bool, Point)>,
}

impl DropFades {
    /// Point the fades at the target the drag is `over` now (with whether it
    /// accepts and the pointer within it), as of `now` on the animation
    /// timeline (`None`: no clock, so changes are immediate).
    pub(crate) fn update(&mut self, over: Option<(DropId, bool, Point)>, now: Option<Duration>) {
        if let Some((id, _, _)) = over
            && !self.entries.iter().any(|e| e.id == id)
        {
            self.entries.push(Entry {
                id,
                channel: Channel::OFF,
                accepted: false,
            });
        }
        for e in &mut self.entries {
            match over {
                Some((id, accepted, _)) if id == e.id => {
                    e.accepted = accepted;
                    e.channel.retarget(1.0, DROP_FADE, now);
                }
                _ => e.channel.retarget(0.0, DROP_FADE, now),
            }
        }
        let t = now.unwrap_or_default();
        self.entries
            .retain(|e| !(e.channel.to == 0.0 && e.channel.settled(t)));
        self.over = over;
    }

    /// Whether any level is still easing at `now`.
    pub(crate) fn animating(&self, now: Option<Duration>) -> bool {
        now.is_some_and(|now| self.entries.iter().any(|e| !e.channel.settled(now)))
    }

    /// Target `id`'s state at `now`.
    pub(crate) fn state(&self, id: DropId, now: Option<Duration>) -> DropState {
        let now = now.unwrap_or(Duration::MAX);
        let Some(e) = self.entries.iter().find(|e| e.id == id) else {
            return DropState::default();
        };
        let over = self.over.filter(|(o, _, _)| *o == id);
        DropState {
            over: over.is_some(),
            accepted: e.accepted,
            position: over.map(|(_, _, p)| p),
            level: e.channel.level(now),
        }
    }

    /// Every target with a state worth showing at `now`.
    pub(crate) fn states(&self, now: Option<Duration>) -> Vec<(DropId, DropState)> {
        self.entries
            .iter()
            .map(|e| (e.id, self.state(e.id, now)))
            .filter(|(_, s)| s.is_active())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    fn frame() -> Duration {
        crate::platform::frame::interval()
    }

    #[test]
    fn enter_eases_in_then_settles() {
        let a = DropId(0);
        let mut f = DropFades::default();
        let t0 = Duration::from_secs(1);
        f.update(Some((a, true, Point::new(5.0, 6.0))), Some(t0));
        let s = f.state(a, Some(t0));
        assert!(s.over && s.accepted);
        assert_eq!(s.position, Some(Point::new(5.0, 6.0)));
        assert!(
            s.level > 0.0 && s.level < 1.0,
            "starts partway: {}",
            s.level
        );
        assert!(f.animating(Some(t0)));
        let mid = f.state(a, Some(t0 + 60 * MS)).level;
        assert!(mid > s.level && mid < 1.0, "{mid}");
        let done = t0 + DROP_FADE - frame();
        assert!((f.state(a, Some(done)).level - 1.0).abs() < 1e-9);
        assert!(!f.animating(Some(done)), "settled: no more frames");
    }

    #[test]
    fn leave_eases_out_keeping_the_verdict_then_forgets() {
        let a = DropId(2);
        let mut f = DropFades::default();
        let t0 = Duration::from_secs(1);
        f.update(Some((a, false, Point::new(1.0, 1.0))), Some(t0));
        let t1 = t0 + DROP_FADE;
        f.update(None, Some(t1));
        let s = f.state(a, Some(t1));
        assert!(!s.over && s.position.is_none());
        assert!(!s.accepted, "the refusal tint fades out with it");
        assert!(s.level > 0.0 && s.level < 1.0, "{}", s.level);
        assert!(f.animating(Some(t1)));
        let t2 = t1 + DROP_FADE;
        f.update(None, Some(t2));
        assert_eq!(f.state(a, Some(t2)), DropState::default());
        assert!(f.states(Some(t2)).is_empty());
        assert!(!f.animating(Some(t2)));
    }

    #[test]
    fn moving_between_targets_crossfades() {
        let (a, b) = (DropId(0), DropId(1));
        let mut f = DropFades::default();
        let t0 = Duration::from_secs(1);
        f.update(Some((a, true, Point::new(0.0, 0.0))), Some(t0));
        let t1 = t0 + 90 * MS;
        let a_mid = f.state(a, Some(t1)).level;
        f.update(Some((b, true, Point::new(0.0, 0.0))), Some(t1));
        // `a` turns back from where it was, smoothly.
        let a_next = f.state(a, Some(t1)).level;
        assert!(
            a_next <= a_mid + 1e-9 && a_next > 0.0,
            "{a_mid} -> {a_next}"
        );
        assert!(!f.state(a, Some(t1)).over && f.state(b, Some(t1)).over);
        assert_eq!(f.states(Some(t1)).len(), 2);
    }

    #[test]
    fn position_follows_the_pointer() {
        let a = DropId(0);
        let mut f = DropFades::default();
        let t0 = Duration::from_secs(1);
        f.update(Some((a, true, Point::new(1.0, 2.0))), Some(t0));
        f.update(Some((a, true, Point::new(30.0, 40.0))), Some(t0 + 16 * MS));
        assert_eq!(
            f.state(a, Some(t0 + 16 * MS)).position,
            Some(Point::new(30.0, 40.0))
        );
    }

    #[test]
    fn without_a_clock_changes_are_immediate() {
        let a = DropId(0);
        let mut f = DropFades::default();
        f.update(Some((a, true, Point::new(0.0, 0.0))), None);
        assert_eq!(f.state(a, None).level, 1.0);
        assert!(!f.animating(None));
        f.update(None, None);
        assert_eq!(f.state(a, None), DropState::default());
    }
}
