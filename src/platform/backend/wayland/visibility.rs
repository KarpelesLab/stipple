//! Whether a window can be seen, for `Event::Occluded`: from
//! `xdg_toplevel`'s `suspended` state where the compositor sends it, and
//! from its frame callbacks where it doesn't.
//!
//! A compositor answers a `wl_surface.frame` callback when it draws the
//! surface, which it doesn't while the surface can't be seen. Some say so
//! with `suspended` (Mutter, KWin); others just stop answering (sway 1.11 on
//! a workspace switch, which has no `suspended` at all). So each window keeps
//! the callback of one commit outstanding at a time, and one that goes
//! [`STALL`] without an answer while the window isn't suspended means the
//! window is hidden; the answer, which comes once it's drawn again, means it
//! shows. A commit's callback is timed from the commit, so a frame the app
//! is slow to draw doesn't count, only a compositor that doesn't draw the
//! surface for that long; and if a compositor that does send `suspended`
//! takes it back (resumes the window) while a callback is outstanding, it's
//! believed: the callback is timed again from there.
//!
//! The frame clock's own callback (`FRAME_CALLBACK`) is one of these: with
//! one window, the same; with several, the one of whichever window asked
//! first.

use std::time::{Duration, Instant};

/// How long a commit's frame callback may go unanswered before the window
/// counts as hidden: well over any compositor's refresh, and the odd frame
/// it drops or takes long to draw.
pub(super) const STALL: Duration = Duration::from_millis(500);

/// A window's visibility, as last reported.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Visibility {
    /// `xdg_toplevel`'s `suspended` state, as applied.
    suspended: bool,
    /// Its frame callback has gone unanswered for [`STALL`].
    stalled: bool,
}

impl Visibility {
    /// Whether the window is hidden.
    pub(super) fn hidden(self) -> bool {
        self.suspended || self.stalled
    }

    /// Apply the `suspended` state. Returns whether the window is now
    /// hidden, if that changed. Resumed also ends a stall: the compositor
    /// says it shows (a configure that merely isn't suspended says nothing
    /// from a compositor without the state).
    pub(super) fn set_suspended(&mut self, suspended: bool) -> Option<bool> {
        let was = self.hidden();
        if self.suspended && !suspended {
            self.stalled = false;
        }
        self.suspended = suspended;
        (self.hidden() != was).then_some(self.hidden())
    }

    /// Whether applying `suspended` resumes the window.
    pub(super) fn resumes(self, suspended: bool) -> bool {
        self.suspended && !suspended
    }

    /// Whether the frame callback is stalled. Returns whether the window is
    /// now hidden, if that changed.
    pub(super) fn set_stalled(&mut self, stalled: bool) -> Option<bool> {
        let was = self.hidden();
        self.stalled = stalled;
        (self.hidden() != was).then_some(self.hidden())
    }
}

/// A window's outstanding frame callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pending {
    /// The window's surface (its main surface, whichever surface the commit
    /// went to).
    window: u32,
    callback: u32,
    /// When the commit went out, or the compositor last said the window
    /// shows.
    since: Instant,
    /// Already reported stalled.
    stalled: bool,
}

/// The windows' outstanding frame callbacks, kept on the connection.
#[derive(Debug, Default)]
pub(super) struct Callbacks {
    pending: Vec<Pending>,
}

impl Callbacks {
    /// Whether `window` has a frame callback outstanding.
    pub(super) fn outstanding(&self, window: u32) -> bool {
        self.pending.iter().any(|p| p.window == window)
    }

    /// `callback` was asked for with a commit of `window` at `now`.
    pub(super) fn sent(&mut self, window: u32, callback: u32, now: Instant) {
        self.pending.retain(|p| p.window != window);
        self.pending.push(Pending {
            window,
            callback,
            since: now,
            stalled: false,
        });
    }

    /// `callback` was answered: the window it was for.
    pub(super) fn done(&mut self, callback: u32) -> Option<u32> {
        let i = self.pending.iter().position(|p| p.callback == callback)?;
        Some(self.pending.swap_remove(i).window)
    }

    /// The compositor says `window` shows: time its callback from `now`.
    pub(super) fn shown(&mut self, window: u32, now: Instant) {
        for p in self.pending.iter_mut().filter(|p| p.window == window) {
            p.since = now;
            p.stalled = false;
        }
    }

    /// `window` is gone.
    pub(super) fn forget(&mut self, window: u32) {
        self.pending.retain(|p| p.window != window);
    }

    /// When the next callback stalls, if one is outstanding.
    pub(super) fn deadline(&self) -> Option<Instant> {
        self.pending
            .iter()
            .filter(|p| !p.stalled)
            .map(|p| p.since + STALL)
            .min()
    }

    /// The windows whose callback has stalled by `now`, each once.
    pub(super) fn take_stalled(&mut self, now: Instant) -> Vec<u32> {
        let mut out = Vec::new();
        for p in &mut self.pending {
            if !p.stalled && now.saturating_duration_since(p.since) >= STALL {
                p.stalled = true;
                out.push(p.window);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn a_callback_answered_in_time_never_stalls() {
        let t0 = Instant::now();
        let mut c = Callbacks::default();
        c.sent(10, 100, t0);
        assert!(c.outstanding(10));
        assert_eq!(c.deadline(), Some(t0 + STALL));
        // A slow compositor frame, under the limit.
        assert!(c.take_stalled(t0 + STALL - MS).is_empty());
        assert_eq!(c.done(100), Some(10));
        assert!(!c.outstanding(10));
        assert_eq!(c.deadline(), None);
        assert!(c.take_stalled(t0 + STALL * 4).is_empty());
    }

    #[test]
    fn an_unanswered_callback_stalls_once_and_the_answer_ends_it() {
        let t0 = Instant::now();
        let mut c = Callbacks::default();
        c.sent(10, 100, t0);
        assert_eq!(c.take_stalled(t0 + STALL), vec![10]);
        // Reported once; nothing more to wait for.
        assert!(c.take_stalled(t0 + STALL * 3).is_empty());
        assert_eq!(c.deadline(), None);
        // Still outstanding: no second callback is asked for meanwhile.
        assert!(c.outstanding(10));
        assert_eq!(c.done(100), Some(10));
        // Someone else's callback isn't ours.
        assert_eq!(c.done(100), None);
    }

    #[test]
    fn windows_stall_on_their_own() {
        let t0 = Instant::now();
        let mut c = Callbacks::default();
        c.sent(10, 100, t0);
        c.sent(20, 200, t0 + STALL / 2);
        assert_eq!(c.deadline(), Some(t0 + STALL));
        assert_eq!(c.take_stalled(t0 + STALL), vec![10]);
        assert_eq!(c.deadline(), Some(t0 + STALL / 2 + STALL));
        assert_eq!(c.done(200), Some(20));
        assert!(c.take_stalled(t0 + STALL * 2).is_empty());
        c.forget(10);
        assert!(!c.outstanding(10));
    }

    #[test]
    fn being_told_the_window_shows_restarts_the_wait() {
        let t0 = Instant::now();
        let mut c = Callbacks::default();
        c.sent(10, 100, t0);
        assert_eq!(c.take_stalled(t0 + STALL), vec![10]);
        c.shown(10, t0 + STALL * 2);
        assert_eq!(c.deadline(), Some(t0 + STALL * 3));
        assert!(c.take_stalled(t0 + STALL * 3 - MS).is_empty());
        assert_eq!(c.take_stalled(t0 + STALL * 3), vec![10]);
    }

    #[test]
    fn hidden_while_suspended_or_stalled() {
        let mut v = Visibility::default();
        assert!(!v.hidden());
        assert_eq!(v.set_stalled(true), Some(true));
        assert_eq!(v.set_stalled(true), None);
        assert_eq!(v.set_stalled(false), Some(false));
        // Suspended, then stalled too: hidden once.
        assert_eq!(v.set_suspended(true), Some(true));
        assert_eq!(v.set_stalled(true), None);
        // Resumed: the compositor says it shows, so the stall is over too.
        assert_eq!(v.set_suspended(false), Some(false));
        assert!(!v.hidden());
        // Stalled, and a configure that doesn't suspend it: a compositor
        // without `suspended` says nothing by that.
        assert_eq!(v.set_stalled(true), Some(true));
        assert!(!v.resumes(false));
        assert_eq!(v.set_suspended(false), None);
        assert!(v.hidden());
        // Suspended and resumed: shown.
        assert_eq!(v.set_suspended(true), None);
        assert!(v.resumes(false));
        assert_eq!(v.set_suspended(false), Some(false));
    }
}
