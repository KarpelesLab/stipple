//! Live OS text controls in the App (see `core::native_host::text` and
//! `platform::native::text`): what the user does in one reaches the field's
//! handlers as the keys it amounts to, so the field's limits, filters and
//! its `on_change` and `on_submit` handlers apply as when typing into
//! Stipple's own field.

use crate::core::KeyInput;
use crate::platform::Modifiers;
use crate::platform::native::{NativeKey, NativeTextEdit};
use crate::{App, Pane};

/// The keys an edit of a live text control amounts to: the composition
/// put away, the replaced range selected and typed over (or deleted), the
/// new selection, and the new composition.
pub(crate) fn keys(edit: &NativeTextEdit) -> Vec<KeyInput> {
    let mut keys = vec![KeyInput::Preedit {
        text: String::new(),
        cursor: None,
    }];
    let (start, end) = edit.replaced;
    if start < end || !edit.inserted.is_empty() {
        keys.push(KeyInput::SetSelection {
            anchor: start,
            caret: end,
        });
        keys.push(if edit.inserted.is_empty() {
            KeyInput::Backspace
        } else {
            KeyInput::Text(edit.inserted.clone())
        });
    }
    keys.push(KeyInput::SetSelection {
        anchor: edit.anchor,
        caret: edit.caret,
    });
    if let Some(c) = &edit.composing {
        keys.push(KeyInput::Preedit {
            text: c.text.clone(),
            cursor: c.cursor,
        });
    }
    keys
}

impl<S: 'static> Pane<S> {
    /// Give the field control `id` stands for the keyboard focus (the user
    /// is in it). Returns its key handler, if it has one.
    fn focus_native_field(&mut self, id: u64) -> Option<crate::core::FocusId> {
        let focus = self.native_host.route(id)?.focus?;
        if self.focused != Some(focus) {
            self.focused = Some(focus);
            self.focus_visible = false;
            self.dirty = true;
        }
        Some(focus)
    }
}

impl<S: 'static> App<S> {
    /// The user edited live text control `id` in window `idx`: apply it to
    /// the field, as the keys it amounts to.
    pub(crate) fn native_text_edited(&mut self, idx: usize, id: u64, edit: NativeTextEdit) -> bool {
        let App { state, panes, .. } = self;
        let pane = &mut panes[idx];
        pane.native_host.note_seen(id, edit.seq);
        crate::platform::native::text::trace(|| format!("app takes {edit:?}"));
        // The next frame tells the control the app took the edit in.
        pane.dirty = true;
        if let Some(focus) = pane.focus_native_field(id) {
            for key in keys(&edit) {
                pane.handlers.dispatch_key(focus, &key, state);
            }
        }
        true
    }

    /// The keyboard focus came to live text control `id` (`true`), or left
    /// it for outside the app's view (`false`).
    pub(crate) fn native_text_focus(&mut self, idx: usize, id: u64, on: bool) -> bool {
        crate::platform::native::text::trace(|| format!("app focus {on}"));
        let pane = &mut self.panes[idx];
        if on {
            let was = pane.focused;
            pane.focus_native_field(id);
            return pane.focused != was;
        }
        let focus = pane.native_host.route(id).and_then(|r| r.focus);
        if focus.is_some() && pane.focused == focus {
            pane.focused = None;
            pane.dirty = true;
            return true;
        }
        false
    }

    /// The user pressed a key live text control `id` leaves to the app:
    /// it goes to the field (Return submits it, Up and Down step a number)
    /// or moves focus on (Tab), as in Stipple's own field.
    pub(crate) fn native_text_key(&mut self, idx: usize, id: u64, key: NativeKey) -> bool {
        crate::platform::native::text::trace(|| format!("app key {key:?}"));
        let focused = self.panes[idx].focus_native_field(id).is_some();
        let changed = match key {
            NativeKey::Tab | NativeKey::BackTab => {
                let shift = key == NativeKey::BackTab;
                self.tab_key(
                    idx,
                    Modifiers {
                        shift,
                        ..Modifiers::NONE
                    },
                )
            }
            NativeKey::Enter => self.pane_press_key(idx, KeyInput::Enter),
            NativeKey::Escape => self.pane_press_key(idx, KeyInput::Escape),
            NativeKey::Up => self.pane_press_key(idx, KeyInput::Up),
            NativeKey::Down => self.pane_press_key(idx, KeyInput::Down),
        };
        // Whatever happened, the control shows the state next frame.
        self.panes[idx].dirty = true;
        changed || focused
    }
}
