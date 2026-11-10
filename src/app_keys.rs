//! Raw keys in the [`App`]: [`Event::RawKey`](crate::platform::Event::RawKey)
//! goes to the focused element's
//! [`on_raw_key`](crate::core::Element::on_raw_key) handler after the menus'
//! claim on it and before the toolkit's own key handling; Tab for elements
//! that [capture it](crate::core::Element::captures_tab); the input method
//! for elements that [opt into it](crate::core::Element::ime_input); and
//! [Option as Meta](crate::core::Element::option_as_meta). The order is in
//! `docs/keyboard.md`.

use crate::core::LayoutNode;
use crate::core::keys::{Handled, custom_ime_state, focused_config};
use crate::platform::{ImeState, KeyCode, KeyEvent, Modifiers, OptionAsMeta, Window};
use crate::{App, Pane};

/// What the App did with a raw key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RawOutcome {
    /// The element's handler took it: the backend must not translate it.
    pub(crate) consumed: bool,
    /// A handler ran, so the frame may have changed.
    pub(crate) ran: bool,
}

impl<S: 'static> App<S> {
    /// A raw key for pane `idx`. Menus get first claim on a press (an open
    /// menu takes every key; a menu bar shortcut runs its command), then the
    /// focused element's raw key handler. Everything else (Tab traversal,
    /// the menu bar's F10 and Alt keys, editing keys) runs afterwards, on the
    /// backend's translation of the key, unless the handler consumed it.
    pub(crate) fn raw_key(&mut self, idx: usize, event: &KeyEvent) -> RawOutcome {
        let App {
            state,
            theme,
            font,
            panes,
            menu,
            ..
        } = self;
        let pane = &mut panes[idx];
        // A viewport holding input focus gets keys its own way.
        if pane.input_viewport.is_some() {
            return RawOutcome::default();
        }
        pane.ensure_tree(state, theme, font.as_ref());
        let Some(raw) = pane
            .tree
            .as_ref()
            .and_then(|t| focused_config(t, pane.focused))
            .and_then(|k| k.raw)
        else {
            return RawOutcome::default();
        };
        if event.is_press() {
            if pane.menu_ui.borrow().is_active() {
                return RawOutcome::default();
            }
            // Escape cancels an in-app drag.
            if event.logical == KeyCode::Escape && pane.dnd.dragging_internally() {
                return RawOutcome::default();
            }
            if let Some(menu) = menu.as_ref()
                && menu(state)
                    .find_shortcut(event.logical, event.modifiers)
                    .is_some()
            {
                return RawOutcome::default();
            }
        }
        let Some(handled) = pane.handlers.dispatch_raw_key(raw, event, state) else {
            return RawOutcome::default();
        };
        pane.dirty = true;
        let consumed = handled == Handled::Yes;
        if consumed && event.is_press() {
            // Alt then a key the element took: not a menu bar activation.
            self.alt_armed = false;
        }
        RawOutcome {
            consumed,
            ran: true,
        }
    }

    /// Tab was pressed and no menu took it: an element that captures Tab
    /// gets it (as a tab character when it's plain Tab), otherwise focus
    /// moves on. Returns whether anything changed.
    pub(crate) fn tab_key(&mut self, idx: usize, modifiers: Modifiers) -> bool {
        let pane = &self.panes[idx];
        let captures = pane.input_viewport.is_none()
            && pane
                .tree
                .as_ref()
                .and_then(|t| focused_config(t, pane.focused))
                .is_some_and(|k| k.captures_tab);
        if !captures {
            return self.pane_focus_next(idx);
        }
        match crate::core::keys::captured_tab_input(modifiers.shift) {
            Some(input) => self.pane_press_key(idx, input),
            None => false,
        }
    }
}

impl<S: 'static> Pane<S> {
    /// The input method state for an element that isn't a text field but
    /// opted into the IME: the focused element, or the viewport holding
    /// input focus.
    pub(crate) fn custom_ime(&self) -> Option<ImeState> {
        let tree = self.tree.as_ref()?;
        match self.input_viewport {
            Some(id) => find_node(tree, &|n| crate::viewport_is(n, id)).and_then(custom_ime_state),
            None => crate::core::find_focus(tree, self.focused?).and_then(custom_ime_state),
        }
    }

    /// Which Option keys act as Meta for the focused element.
    pub(crate) fn option_as_meta(&self) -> OptionAsMeta {
        self.tree
            .as_ref()
            .and_then(|t| focused_config(t, self.focused))
            .map(|k| k.option_as_meta)
            .unwrap_or_default()
    }

    /// Tell the window which Option keys are Meta, when that changed.
    pub(crate) fn sync_option_as_meta(&mut self, window: &dyn Window) {
        let option = self.option_as_meta();
        if option != self.option_sent {
            window.set_option_as_meta(option);
            self.option_sent = option;
        }
    }
}

/// The first node (in tree order) that `pred` accepts.
fn find_node<'a>(
    node: &'a LayoutNode,
    pred: &dyn Fn(&LayoutNode) -> bool,
) -> Option<&'a LayoutNode> {
    if pred(node) {
        return Some(node);
    }
    node.children.iter().find_map(|c| find_node(c, pred))
}

#[cfg(test)]
mod tests;
