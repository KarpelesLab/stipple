//! Touch gestures in the [`App`]: every [`Touch`] goes through the shared
//! recognizer ([`TouchGestures`](crate::core::touch::TouchGestures)), which
//! scrolls containers (both axes) and flings them, turns a finger held still
//! into a long press that opens the context menu, and hands two-finger
//! pinches to [`Element::on_pinch`](crate::core::Element::on_pinch)
//! handlers. iOS, Android and the web deliver the same touches, so they all
//! behave alike. See `docs/pointer-events.md`.

use crate::core::touch::{Pinch, TouchAction, TouchTargets};
use crate::core::{ContextId, LayoutNode, PinchEvent, PinchPhase, pointer_at, pointer_local};
use crate::geometry::Point;
use crate::platform::{Touch, TouchPhase};
use crate::{App, Pane, app_dbg, press_layer};

/// The element whose context menu (or context handler) a secondary press at
/// `pos` reaches, and whether the press lands on a non-modal overlay's
/// catcher (outside an open menu, which then closes). A right-click or Menu
/// key inside a menu or dialog looks there; one on a catcher looks in the main
/// tree; a modal scrim blocks it.
pub(crate) fn context_target(
    tree: &LayoutNode,
    overlay_active: bool,
    pos: Point,
) -> (Option<ContextId>, bool) {
    let (layer, main) = press_layer(tree, overlay_active, pos);
    let catcher = !main && layer.bounds == tree.bounds;
    let target = match (catcher, layer.decoration.fill) {
        (true, Some(_)) => None,
        (true, None) => tree.children.first(),
        (false, _) => Some(layer),
    };
    (
        target.and_then(|t| crate::core::context_at(t, pos)),
        catcher,
    )
}

impl<S: 'static> Pane<S> {
    /// What the gestures need to know about the point `p`: the scroll
    /// containers under it, whether a long press opens a context menu there,
    /// and the element taking pinches.
    fn touch_targets(&self, p: Point) -> TouchTargets {
        let Some(tree) = self.tree.as_ref() else {
            return TouchTargets::default();
        };
        let layer = press_layer(tree, self.overlay_active, p).0;
        let pointers = &self.handlers;
        TouchTargets {
            scroll: crate::core::scroll_targets_at(tree, p),
            long_press: context_target(tree, self.overlay_active, p).0.is_some(),
            pinch: pointer_at(layer, p, &|id| pointers.pointers_ref().has_pinch(id)),
        }
    }

    /// The first finger's press no longer taps, focuses or selects: it became
    /// a scroll, a long press or a pinch.
    fn cancel_press(&mut self) {
        self.pressed = None;
        self.press_focus = None;
        self.press_at = None;
        if self.text_selecting.is_some() {
            self.end_text_select();
        }
        self.dirty = true;
    }

    /// Run a pinch's handler. Returns whether it ran.
    fn send_pinch(&mut self, state: &mut S, pinch: Pinch) -> bool {
        let Some((centroid, size)) = self
            .tree
            .as_ref()
            .and_then(|t| pointer_local(t, pinch.target, pinch.centroid))
        else {
            return false;
        };
        let ev = PinchEvent {
            phase: pinch.phase,
            scale: pinch.scale,
            rotation: pinch.rotation,
            translation: pinch.translation,
            centroid,
            window_centroid: pinch.centroid,
            size,
        };
        app_dbg(format_args!("pinch {ev:?} to {:?}", pinch.target));
        let ran = self
            .handlers
            .pointers()
            .dispatch_pinch(pinch.target, &ev, state);
        if ran {
            self.dirty = true;
        }
        ran
    }
}

impl<S: 'static> App<S> {
    /// Deliver a finger to the primary window the way a touch-screen backend
    /// does: the [`Touch`], then, for the first finger, the pointer press, move
    /// or release it maps to. Returns whether anything changed. A drag in a
    /// scroll container scrolls it (see [`advance_fling`](Self::advance_fling)
    /// for what follows a quick release), a finger held still on an element
    /// with a context menu opens it (see
    /// [`advance_long_press`](Self::advance_long_press)), and two fingers over
    /// an [`on_pinch`](crate::core::Element::on_pinch) element pinch it.
    pub fn touch(&mut self, touch: Touch) -> bool {
        let position = touch.position;
        let primary = match self.panes[0].touch.primary() {
            None => touch.phase == TouchPhase::Started,
            Some(id) => id == touch.id,
        };
        let mut changed = self.pane_touch(0, touch);
        if !primary {
            return changed;
        }
        changed |= match touch.phase {
            TouchPhase::Started => {
                self.panes[0].last_pointer = position;
                !self.panes[0].touch.swallows_press() && self.pane_pointer_down(0, position)
            }
            TouchPhase::Moved => {
                self.panes[0].last_pointer = position;
                !self.panes[0].touch.holds_moves() && self.pane_pointer_move(0, position)
            }
            TouchPhase::Ended => self.pane_pointer_up(0, position),
            // Released far away, so the press doesn't click.
            TouchPhase::Cancelled => self.pane_pointer_up(0, Point::new(-1.0e9, -1.0e9)),
        };
        changed
    }

    /// Advance a touch fling in the primary window by one frame (the loop does
    /// this on every frame while one runs). Returns whether it scrolled.
    pub fn advance_fling(&mut self) -> bool {
        self.pane_fling(0)
    }

    /// Turn a finger held down in the primary window into a long press, if it
    /// has been held long enough (the loop does this when the long-press timer
    /// wakes it): the press is cancelled and the context menu under the
    /// finger opens. Returns whether one did.
    pub fn advance_long_press(&mut self) -> bool {
        self.pane_long_press(0)
    }

    /// A finger touched, moved or lifted: drive the gestures. Returns whether
    /// to repaint.
    pub(crate) fn pane_touch(&mut self, idx: usize, touch: Touch) -> bool {
        let App {
            state,
            theme,
            font,
            panes,
            ..
        } = self;
        let pane = &mut panes[idx];
        if touch.phase == TouchPhase::Started {
            pane.ensure_tree(state, theme, font.as_ref());
        }
        let now = pane.touch_now();
        let mut gestures = std::mem::take(&mut pane.touch);
        let action = gestures.touch(&touch, now, |p| pane.touch_targets(p));
        pane.touch = gestures;
        app_dbg(format_args!("touch {touch:?}: {action:?}"));
        if pane.touch.flinging() && !pane.animating {
            // The finger lifted while moving: frames drive the fling, through
            // `pane_fling`.
            pane.animating = true;
            crate::platform::frame::request();
        }
        let redraw = match action {
            TouchAction::None => false,
            TouchAction::Begin { target, delta } => {
                // The press becomes a scroll: it no longer clicks, focuses or
                // selects.
                pane.cancel_press();
                pane.scroll_container_by(state, theme, font.as_ref(), target, delta);
                true
            }
            TouchAction::Scroll { target, delta } => {
                pane.scroll_container_by(state, theme, font.as_ref(), target, delta)
                    .0
            }
            TouchAction::End => false,
            TouchAction::Pinch(pinch) => {
                if pinch.phase == PinchPhase::Began {
                    pane.cancel_press();
                }
                pane.send_pinch(state, pinch) || pinch.phase == PinchPhase::Began
            }
        };
        self.sync_long_press_timer(idx);
        redraw
    }

    /// Keep a timer running while a held finger may become a long press, so
    /// the loop wakes when it does; drop it once it can't.
    fn sync_long_press_timer(&mut self, idx: usize) {
        let pane = &mut self.panes[idx];
        match pane.touch.long_press_deadline() {
            Some(due) if pane.long_press_timer.is_none() => {
                // A millisecond late rather than early.
                let wait = (due - pane.touch_now()).max(0.0) + 0.001;
                let timer = self
                    .handle
                    .after(std::time::Duration::from_secs_f64(wait), |_: &mut S| {});
                pane.long_press_timer = Some(timer);
            }
            Some(_) => {}
            None => {
                if let Some(timer) = pane.long_press_timer.take() {
                    self.handle.cancel(timer);
                }
            }
        }
    }

    /// A finger held down long enough becomes a long press: cancel its press
    /// and open the context menu (or run the context handler) under it.
    /// Returns whether to repaint.
    pub(crate) fn pane_long_press(&mut self, idx: usize) -> bool {
        let pane = &mut self.panes[idx];
        let now = pane.touch_now();
        let Some(at) = pane.touch.long_press_due(now) else {
            return false;
        };
        app_dbg(format_args!("long press at {at:?}"));
        pane.cancel_press();
        self.sync_long_press_timer(idx);
        self.pane_context_at(idx, at);
        true
    }

    /// Advance a running fling by a frame. Returns whether it scrolled.
    pub(crate) fn pane_fling(&mut self, idx: usize) -> bool {
        let App {
            state,
            theme,
            font,
            panes,
            ..
        } = self;
        let pane = &mut panes[idx];
        let now = pane.touch_now();
        let Some((target, delta)) = pane.touch.tick(now) else {
            return false;
        };
        let (moved, clamped) = pane.scroll_container_by(state, theme, font.as_ref(), target, delta);
        if clamped {
            // It reached the end of the content.
            pane.touch.stop_fling();
        }
        moved
    }
}
