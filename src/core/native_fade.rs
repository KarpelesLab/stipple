//! OS-drawn controls (under [`Theme::native`](crate::style::Theme::native))
//! fading between the OS's pictures of their states.
//!
//! The OS draws each state of a control (hovered, pressed, focused, checked)
//! as a still bitmap, so a change would jump from one picture to the next.
//! Instead, for a while after a change the old picture is drawn over the new
//! one, fading out: hover, press and focus on the theme's `state` motion, a
//! checkbox, radio or switch flipping on its `toggle` motion. A slider's or a
//! progress bar's value and a switch between light and dark don't fade (the
//! first follows the pointer, the second cross-fades the whole window).
//! Reduced motion snaps, as everywhere.
//!
//! Controls are told apart across frames by their tap or focus handle; one
//! without either doesn't fade.

use std::collections::HashMap;
use std::time::Duration;

use crate::anim::{Animated, Motion};
use crate::core::runtime::{ActionId, FocusId, LayoutNode};
use crate::platform::native::{NativeControl, NativeDeco};
use crate::style::MotionTokens;

/// What a control is known by across frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Id {
    Action(ActionId),
    Focus(FocusId),
}

#[derive(Clone, Debug)]
struct Entry {
    /// How it was drawn in the last frame.
    deco: NativeDeco,
    /// The picture fading out, and how much of it shows (1 down to 0).
    from: Option<(NativeDeco, Animated<f64>)>,
    seen: bool,
}

/// The fades of one window's OS-drawn controls.
#[derive(Clone, Debug, Default)]
pub(crate) struct NativeFades {
    entries: HashMap<Id, Entry>,
    /// The last frame's time, and this one's.
    last: Option<Duration>,
    now: Option<Duration>,
}

/// The motion a change from `old` to `new` fades on, if it fades.
fn motion_for(old: &NativeDeco, new: &NativeDeco, m: &MotionTokens) -> Option<Motion> {
    if old.dark != new.dark {
        return None;
    }
    if old.control != new.control {
        return match (old.control, new.control) {
            (NativeControl::Checkbox { .. }, NativeControl::Checkbox { .. })
            | (NativeControl::Radio { .. }, NativeControl::Radio { .. })
            | (NativeControl::Switch { .. }, NativeControl::Switch { .. }) => Some(m.toggle),
            _ => None,
        };
    }
    (old.state != new.state).then_some(m.state)
}

impl NativeFades {
    /// Note how each OS-drawn control in `tree` is drawn this frame (at
    /// `now`; `None`: no clock, so nothing fades), and mark the ones fading
    /// from another picture.
    pub(crate) fn update(
        &mut self,
        tree: &mut LayoutNode,
        m: &MotionTokens,
        now: Option<Duration>,
    ) {
        self.now = now;
        if self.entries.is_empty() && !has_native(tree) {
            self.last = now;
            return;
        }
        // A change counts from one frame before this one (but not before the
        // last frame), so it already shows on the frame that sees it.
        let head = now.map(|now| {
            let frame = now.saturating_sub(crate::platform::frame::interval());
            self.last.map_or(frame, |last| frame.max(last.min(now)))
        });
        for e in self.entries.values_mut() {
            e.seen = false;
        }
        self.walk(tree, m, now, head);
        self.entries.retain(|_, e| e.seen);
        self.last = now;
    }

    fn walk(
        &mut self,
        node: &mut LayoutNode,
        m: &MotionTokens,
        now: Option<Duration>,
        head: Option<Duration>,
    ) {
        node.native_from = None;
        if let Some(deco) = node.native {
            let id = node
                .action
                .map(Id::Action)
                .or(node.focus.map(Id::Focus))
                .filter(|_| !node.ghost);
            if let Some(id) = id {
                let entry = self.entries.entry(id).or_insert(Entry {
                    deco,
                    from: None,
                    seen: false,
                });
                entry.seen = true;
                if let (Some(now), Some(head)) = (now, head) {
                    if entry.deco != deco {
                        entry.from = motion_for(&entry.deco, &deco, m).and_then(|motion| {
                            let mut shows = Animated::new(1.0);
                            shows.animate_to(0.0, motion, head);
                            shows.is_animating(now).then_some((entry.deco, shows))
                        });
                    }
                    if let Some((old, shows)) = &entry.from {
                        if shows.is_animating(now) {
                            let w = shows.value(now).clamp(0.0, 1.0) as f32;
                            node.native_from = Some((*old, w));
                        } else {
                            entry.from = None;
                        }
                    }
                } else {
                    entry.from = None;
                }
                entry.deco = deco;
            }
        }
        for child in &mut node.children {
            self.walk(child, m, now, head);
        }
    }

    /// Whether a control is still fading (as of the last frame).
    pub(crate) fn animating(&self) -> bool {
        let Some(now) = self.now else {
            return false;
        };
        self.entries
            .values()
            .any(|e| e.from.as_ref().is_some_and(|(_, a)| a.is_animating(now)))
    }
}

fn has_native(node: &LayoutNode) -> bool {
    node.native.is_some() || node.children.iter().any(has_native)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::{Easing, set_reduced_motion};
    use crate::geometry::Rect;
    use crate::platform::native::NativeState;

    const MS: Duration = Duration::from_millis(1);
    const T0: Duration = Duration::from_secs(1);

    fn frame() -> Duration {
        crate::platform::frame::interval()
    }

    fn tokens() -> MotionTokens {
        MotionTokens {
            state: Motion::tween(0.1, Easing::Linear),
            toggle: Motion::tween(0.2, Easing::Linear),
            ..MotionTokens::standard()
        }
    }

    fn deco(control: NativeControl, hovered: bool) -> NativeDeco {
        NativeDeco {
            control,
            dark: false,
            state: NativeState {
                hovered,
                ..NativeState::default()
            },
        }
    }

    /// A window holding one OS-drawn control with tap handle 7.
    fn tree(d: NativeDeco) -> LayoutNode {
        let mut c = LayoutNode::container(Rect::from_xywh(0.0, 0.0, 80.0, 24.0), vec![]);
        c.native = Some(d);
        c.action = Some(ActionId(7));
        LayoutNode::container(Rect::from_xywh(0.0, 0.0, 200.0, 200.0), vec![c])
    }

    fn frame_at(f: &mut NativeFades, d: NativeDeco, now: Duration) -> Option<(NativeDeco, f32)> {
        let mut t = tree(d);
        f.update(&mut t, &tokens(), Some(now));
        t.children[0].native_from
    }

    const BUTTON: NativeControl = NativeControl::Button { primary: false };

    #[test]
    fn hover_fades_from_the_old_picture_on_the_state_motion() {
        set_reduced_motion(Some(false));
        let mut f = NativeFades::default();
        assert_eq!(frame_at(&mut f, deco(BUTTON, false), T0), None, "at rest");
        assert!(!f.animating());
        let t1 = T0 + 500 * MS;
        let hovered = deco(BUTTON, true);
        let (old, w) = frame_at(&mut f, hovered, t1).expect("fading");
        assert_eq!(old, deco(BUTTON, false));
        let want = 1.0 - frame().as_secs_f64() / 0.1;
        assert!((f64::from(w) - want).abs() < 1e-4, "one frame in: {w}");
        assert!(f.animating());
        let (_, w) = frame_at(&mut f, hovered, t1 + 50 * MS - frame()).unwrap();
        assert!((w - 0.5).abs() < 1e-4, "half-way: {w}");
        assert_eq!(frame_at(&mut f, hovered, t1 + 100 * MS), None, "done");
        assert!(!f.animating(), "no more frames");
    }

    #[test]
    fn a_check_fades_on_the_toggle_motion_but_a_slider_value_snaps() {
        set_reduced_motion(Some(false));
        let mut f = NativeFades::default();
        let off = deco(NativeControl::Checkbox { checked: false }, false);
        let on = deco(NativeControl::Checkbox { checked: true }, false);
        frame_at(&mut f, off, T0);
        let t1 = T0 + 500 * MS;
        frame_at(&mut f, on, t1);
        let (_, w) = frame_at(&mut f, on, t1 + 100 * MS - frame()).unwrap();
        assert!((w - 0.5).abs() < 1e-4, "200 ms toggle, half-way: {w}");

        let mut f = NativeFades::default();
        frame_at(
            &mut f,
            deco(NativeControl::Slider { value: 100 }, false),
            T0,
        );
        let moved = frame_at(
            &mut f,
            deco(NativeControl::Slider { value: 300 }, false),
            t1,
        );
        assert_eq!(moved, None);
        assert!(!f.animating());
    }

    #[test]
    fn reduced_motion_and_dark_mode_snap_and_the_first_frame_rests() {
        set_reduced_motion(Some(true));
        let mut f = NativeFades::default();
        frame_at(&mut f, deco(BUTTON, false), T0);
        assert_eq!(frame_at(&mut f, deco(BUTTON, true), T0 + 500 * MS), None);
        assert!(!f.animating());
        set_reduced_motion(Some(false));

        let mut f = NativeFades::default();
        frame_at(&mut f, deco(BUTTON, false), T0);
        let dark = NativeDeco {
            dark: true,
            ..deco(BUTTON, false)
        };
        assert_eq!(frame_at(&mut f, dark, T0 + 500 * MS), None);
    }
}
