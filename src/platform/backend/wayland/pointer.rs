//! `wl_pointer` decoding for the Wayland backend: buttons, and scrolls with
//! their units (wheel notches or pixels) and gesture phase.

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

#[cfg(test)]
mod tests {
    use super::*;

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
