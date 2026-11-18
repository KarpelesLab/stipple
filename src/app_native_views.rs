//! Real OS controls under the native theme (see `core::native_host` and
//! `docs/native-theme.md`, *Real controls*): each frame's plan, handed to
//! the window, and what the user does to them, routed to the handlers of
//! the elements they stand for.

use crate::platform::native::NativeViewEvent;
use crate::{App, Pane};

pub(crate) mod text;

impl<S: 'static> Pane<S> {
    /// Decide which of `tree`'s OS-drawn controls are live this frame and
    /// mark them on it.
    pub(crate) fn plan_native_views(&mut self, tree: &mut crate::core::LayoutNode) {
        let input = crate::core::native_host::PlanInput {
            enabled: self.hosts_native,
            suspend: self.crossfade.active(),
            focused: self.focused,
            focus_visible: self.focus_visible,
        };
        self.native_host.plan(tree, input);
    }

    /// Hand `window` the live controls, when they changed.
    pub(crate) fn sync_native_views(&mut self, window: &dyn crate::platform::Window) {
        if let Some(views) = self.native_host.take_changed() {
            window.set_native_views(views);
        }
    }
}

impl<S: 'static> App<S> {
    /// The user acted on live control `id` in window `idx`: run its
    /// element's handler, as a tap or a drag there would. Returns whether
    /// the window needs a new frame (always, once handled: the control
    /// shows what the state says, even when the handler kept it as it was).
    pub(crate) fn pane_native_view(&mut self, idx: usize, id: u64, event: NativeViewEvent) -> bool {
        // A text control's: its edits, focus and keys.
        match event {
            NativeViewEvent::Edited(edit) => return self.native_text_edited(idx, id, edit),
            NativeViewEvent::Focus(on) => return self.native_text_focus(idx, id, on),
            NativeViewEvent::Key(key) => return self.native_text_key(idx, id, key),
            _ => {}
        }
        let App { state, panes, .. } = self;
        let pane = &mut panes[idx];
        let Some(route) = pane.native_host.route(id) else {
            return false;
        };
        let at = route.frame.center();
        crate::core::set_pointer_position(at, None);
        match event {
            NativeViewEvent::Activated => {
                if let Some(action) = route.action {
                    pane.handlers.dispatch(action, state);
                }
            }
            NativeViewEvent::Value(v) => {
                if let Some(drag) = route.drag {
                    let b = route.bounds;
                    let x = b.min_x() + v.clamp(0.0, 1.0) * b.width();
                    let p = crate::geometry::Point::new(x, b.min_y() + b.height() / 2.0);
                    let motion = crate::core::DragMotion::new(p, p, b, b);
                    pane.handlers.dispatch_drag_motion(drag, &motion, state);
                }
            }
            NativeViewEvent::Picked(i) => {
                if let Some(&pick) = route.picks.get(i) {
                    pane.handlers.dispatch(pick, state);
                }
            }
            _ => {}
        }
        // The control shows what the state says next frame, even where the
        // handler kept it as it was (a pick or a flip refused goes back).
        pane.native_host.resend();
        pane.dirty = true;
        true
    }
}
