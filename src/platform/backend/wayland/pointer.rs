//! `wl_pointer` decoding for the Wayland backend: buttons, scrolls with
//! their units (wheel notches or pixels) and gesture phase, and the pointer
//! lock (`zwp_pointer_constraints_v1` holds the pointer still,
//! `zwp_relative_pointer_manager_v1` reports its motion).

use crate::platform::event::{PointerButton, ScrollDelta, ScrollPhase};

pub(super) const WL_POINTER_SET_CURSOR: u16 = 0;
pub(super) const WL_POINTER_LEAVE: u16 = 1;
/// Since `wl_seat` v5: the end of a group of pointer events.
pub(super) const WL_POINTER_FRAME: u16 = 5;
/// Since v5: what is scrolling (wheel 0, finger 1, continuous 2, wheel tilt 3).
pub(super) const WL_POINTER_AXIS_SOURCE: u16 = 6;
/// Since v5: a finger scroll on an axis stopped.
pub(super) const WL_POINTER_AXIS_STOP: u16 = 7;

/// Convert a `wl_fixed` (signed 24.8 fixed-point) to logical pixels.
pub(super) fn fixed_to_f64(raw: u32) -> f64 {
    (raw as i32) as f64 / 256.0
}

/// Translate an evdev pointer button code into a [`PointerButton`].
pub(super) fn pointer_button(code: u32) -> PointerButton {
    match code {
        0x110 => PointerButton::Left,  // BTN_LEFT
        0x111 => PointerButton::Right, // BTN_RIGHT
        0x112 => PointerButton::Middle,
        0x113 | 0x116 => PointerButton::Back, // BTN_SIDE, BTN_BACK
        0x114 | 0x115 => PointerButton::Forward, // BTN_EXTRA, BTN_FORWARD
        n => PointerButton::Other(n as u16),
    }
}

/// The scroll state of a `wl_pointer` between events: the wheel notches
/// (`axis_discrete`) and source (`axis_source`) of the frame being read, and
/// whether a finger scroll is under way (for its phase).
#[derive(Debug, Default)]
pub(super) struct Axes {
    discrete: [i32; 2],
    finger: bool,
    scrolling: bool,
}

impl Axes {
    /// `axis_source`: fingers and continuous devices scroll in gestures.
    pub(super) fn source(&mut self, source: u32) {
        self.finger = matches!(source, 1 | 2);
    }

    /// `axis_discrete`: the notches of the `axis` event that follows.
    pub(super) fn discrete(&mut self, axis: u32, steps: i32) {
        if let Some(slot) = self.discrete.get_mut(axis as usize) {
            *slot = steps;
        }
    }

    /// `axis`: `axis` 0 is vertical, 1 horizontal, and `value` (a `wl_fixed`)
    /// is in surface pixels, positive down or right. After `axis_discrete`,
    /// the scroll is that many lines rather than the compositor's per-notch
    /// distance (often 10 or 15 px).
    pub(super) fn axis(&mut self, axis: u32, value: u32) -> Option<ScrollDelta> {
        let steps = self
            .discrete
            .get_mut(axis as usize)
            .map_or(0, std::mem::take);
        let (dx, dy) = match axis {
            0 => (0.0, 1.0),
            1 => (1.0, 0.0),
            _ => return None,
        };
        if steps != 0 {
            self.scrolling = false;
            let n = f64::from(steps);
            return Some(ScrollDelta::lines(dx * n, dy * n));
        }
        let px = fixed_to_f64(value);
        let mut delta = ScrollDelta::pixels(dx * px, dy * px);
        if self.finger {
            delta.phase = if self.scrolling {
                ScrollPhase::Moved
            } else {
                ScrollPhase::Started
            };
            self.scrolling = true;
        }
        Some(delta)
    }

    /// `axis_stop`: the fingers lifted. An empty delta ends the gesture (once,
    /// though each axis stops).
    pub(super) fn stop(&mut self) -> Option<ScrollDelta> {
        std::mem::take(&mut self.scrolling)
            .then(|| ScrollDelta::pixels(0.0, 0.0).with_phase(ScrollPhase::Ended))
    }

    /// `frame`: the source applied to this frame only.
    pub(super) fn frame(&mut self) {
        self.finger = false;
    }
}

/// The globals the pointer lock needs.
pub(super) const CONSTRAINTS: &str = "zwp_pointer_constraints_v1";
pub(super) const RELATIVE: &str = "zwp_relative_pointer_manager_v1";

// `zwp_pointer_constraints_v1.lock_pointer`, `zwp_locked_pointer_v1.destroy`
// and its `unlocked` event; `zwp_relative_pointer_manager_v1.get_relative_pointer`,
// `zwp_relative_pointer_v1.destroy` and its `relative_motion` event.
pub(super) const CONSTRAINTS_LOCK_POINTER: u16 = 1;
pub(super) const LOCKED_POINTER_DESTROY: u16 = 0;
const LOCKED_POINTER_UNLOCKED: u16 = 1;
pub(super) const RELATIVE_GET_RELATIVE_POINTER: u16 = 1;
pub(super) const RELATIVE_POINTER_DESTROY: u16 = 0;
const RELATIVE_POINTER_RELATIVE_MOTION: u16 = 0;
/// `zwp_pointer_constraints_v1.lifetime.oneshot`: the lock ends for good
/// once the compositor lifts it.
const LIFETIME_ONESHOT: u32 = 1;

/// The pointer lock's protocol objects.
#[derive(Debug, Default)]
pub(super) struct Lock {
    /// The bound globals, when the compositor has them.
    pub(super) constraints: Option<u32>,
    pub(super) relative_manager: Option<u32>,
    /// While locked: the `zwp_locked_pointer_v1`, the
    /// `zwp_relative_pointer_v1` and the surface it's locked to.
    pub(super) active: Option<(u32, u32, u32)>,
}

/// What the compositor said about the pointer lock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum LockEvent {
    /// The pointer moved this far (unaccelerated, surface units).
    Motion(f64, f64),
    /// The compositor lifted the lock.
    Unlocked,
}

impl Lock {
    /// Whether the cursor should be hidden: the pointer is locked to a
    /// surface.
    pub(super) fn hides_cursor(&self) -> bool {
        self.active.is_some()
    }

    /// Read a message for one of the lock's objects: the surface it is about
    /// and what happened. `None` for other objects' messages.
    pub(super) fn event(&self, object: u32, opcode: u16, args: &[u8]) -> Option<(u32, LockEvent)> {
        let (locked, relative, surface) = self.active?;
        if object == relative && opcode == RELATIVE_POINTER_RELATIVE_MOTION {
            let (dx, dy) = relative_motion(args)?;
            return Some((surface, LockEvent::Motion(dx, dy)));
        }
        (object == locked && opcode == LOCKED_POINTER_UNLOCKED)
            .then_some((surface, LockEvent::Unlocked))
    }
}

/// `lock_pointer`'s arguments: the new `zwp_locked_pointer_v1`, the surface
/// and pointer, no region (the whole surface), a one-shot lifetime.
pub(super) fn lock_pointer_args(id: u32, surface: u32, pointer: u32) -> Vec<u8> {
    [id, surface, pointer, 0, LIFETIME_ONESHOT]
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect()
}

/// `get_relative_pointer`'s arguments: the new `zwp_relative_pointer_v1`, and
/// the pointer.
pub(super) fn relative_pointer_args(id: u32, pointer: u32) -> Vec<u8> {
    [id, pointer].iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// The unaccelerated motion in a `relative_motion` event (`utime_hi`,
/// `utime_lo`, then `dx`, `dy`, `dx_unaccel` and `dy_unaccel` as `wl_fixed`).
pub(super) fn relative_motion(args: &[u8]) -> Option<(f64, f64)> {
    let word = |i: usize| {
        args.get(i..i + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    Some((fixed_to_f64(word(16)?), fixed_to_f64(word(20)?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_lock_requests_and_events() {
        let words = |b: Vec<u8>| -> Vec<u32> {
            b.chunks(4)
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect()
        };
        assert_eq!(words(lock_pointer_args(40, 3, 9)), [40, 3, 9, 0, 1]);
        assert_eq!(words(relative_pointer_args(41, 9)), [41, 9]);
        // utime, then accelerated and unaccelerated motion.
        let motion: Vec<u8> = [0u32, 1000, 512, 256, (-3i32 * 256) as u32, 128]
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect();
        assert_eq!(relative_motion(&motion), Some((-3.0, 0.5)));
        assert_eq!(relative_motion(&motion[..20]), None);
        let mut lock = Lock::default();
        assert_eq!(lock.event(41, 0, &motion), None, "not locked");
        assert!(!lock.hides_cursor());
        lock.active = Some((40, 41, 3));
        assert!(lock.hides_cursor());
        assert_eq!(
            lock.event(41, 0, &motion),
            Some((3, LockEvent::Motion(-3.0, 0.5)))
        );
        assert_eq!(lock.event(40, 1, &[]), Some((3, LockEvent::Unlocked)));
        assert_eq!(lock.event(40, 0, &[]), None, "locked: nothing to do");
        assert_eq!(lock.event(12, 0, &motion), None);
    }

    #[test]
    fn wheels_scroll_lines_and_fingers_pixels_in_phases() {
        let mut axes = Axes::default();
        // A wheel notch up: one line, whatever distance the compositor sent.
        axes.discrete(0, -1);
        assert_eq!(
            axes.axis(0, (-15i32 * 256) as u32),
            Some(ScrollDelta::lines(0.0, -1.0))
        );
        axes.discrete(1, 1);
        assert_eq!(
            axes.axis(1, (15 * 256) as u32),
            Some(ScrollDelta::lines(1.0, 0.0))
        );
        axes.frame();
        // A touchpad without a source: pixels, no phase.
        assert_eq!(
            axes.axis(0, (7 * 256) as u32),
            Some(ScrollDelta::pixels(0.0, 7.0))
        );
        axes.frame();
        // A finger scroll: started, moved, then stopped.
        axes.source(1);
        let first = axes.axis(0, 256).unwrap();
        assert_eq!(first.phase, ScrollPhase::Started);
        axes.frame();
        axes.source(1);
        assert_eq!(axes.axis(1, 512).unwrap().phase, ScrollPhase::Moved);
        axes.frame();
        let end = axes.stop().unwrap();
        assert_eq!((end.dy, end.phase), (0.0, ScrollPhase::Ended));
        assert_eq!(axes.stop(), None, "the other axis stopping adds nothing");
        assert_eq!(axes.axis(2, 0), None);
    }

    #[test]
    fn fixed_point_and_buttons() {
        // wl_fixed is signed 24.8: 256 → 1.0, 128 → 0.5, negatives wrap.
        assert_eq!(fixed_to_f64(256), 1.0);
        assert_eq!(fixed_to_f64(128), 0.5);
        assert_eq!(fixed_to_f64((-256i32) as u32), -1.0);
        assert_eq!(pointer_button(0x110), PointerButton::Left);
        assert_eq!(pointer_button(0x111), PointerButton::Right);
        assert_eq!(pointer_button(0x112), PointerButton::Middle);
        assert_eq!(pointer_button(0x113), PointerButton::Back);
        assert_eq!(pointer_button(0x114), PointerButton::Forward);
        assert_eq!(pointer_button(0x999), PointerButton::Other(0x999));
    }
}
