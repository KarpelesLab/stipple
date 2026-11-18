//! Windows that belong to others ([`WindowAttributes::parent`]): the
//! backend is told the parent by its own id; a modal window
//! ([`WindowAttributes::modal`], [`WindowAttributes::sheet`]) blocks its
//! parent's input in the app as well as natively; and a parent closing takes
//! its windows with it.

use crate::App;
use crate::platform::{Event, WindowAttributes, WindowId};

impl<S: 'static> App<S> {
    /// `attrs` as the backend takes them: the parent named by the backend's
    /// id for it (`None` if it isn't open).
    pub(crate) fn native_attrs(&self, mut attrs: WindowAttributes) -> WindowAttributes {
        attrs.parent = attrs.parent.and_then(|p| {
            self.panes
                .iter()
                .find(|pane| pane.id == p)
                .and_then(|pane| pane.native)
        });
        attrs
    }

    /// Whether pane `idx` has a modal window open over it.
    pub(crate) fn blocked(&self, idx: usize) -> bool {
        let id = self.panes[idx].id;
        self.panes
            .iter()
            .any(|p| p.attrs.parent == Some(id) && p.attrs.blocks_parent())
    }

    /// Whether `event` for pane `idx` is the user's input that a modal
    /// window over it holds back: pointer, keys, text, drops, closing it.
    pub(crate) fn input_blocked(&self, idx: usize, event: &Event) -> bool {
        let input = matches!(
            event,
            Event::PointerMoved { .. }
                | Event::PointerButton { .. }
                | Event::Scroll { .. }
                | Event::PointerDelta { .. }
                | Event::Touch(_)
                | Event::Key { .. }
                | Event::RawKey(_)
                | Event::Text(_)
                | Event::Ime(_)
                | Event::EditAction(_)
                | Event::DragEnter { .. }
                | Event::DragOver { .. }
                | Event::Drop { .. }
                | Event::DragLift { .. }
                | Event::NativeView { .. }
                | Event::BackRequested
                | Event::CloseRequested
        );
        input && self.blocked(idx)
    }

    /// The windows that belong to window `id`, which is closing: they close
    /// with it.
    pub(crate) fn close_children(&self, id: WindowId) {
        for pane in self.panes.iter().filter(|p| p.attrs.parent == Some(id)) {
            self.handle.close_window(pane.id);
        }
    }
}
