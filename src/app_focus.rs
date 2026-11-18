//! Focus the view and handlers move: [`Element::autofocus`](crate::core::Element::autofocus),
//! [`Cx::focus`](crate::core::Cx::focus) and
//! [`AppHandle::focus`](crate::core::AppHandle::focus), applied after each
//! build's layout; and a dialog's default action on Enter (see
//! [`Dialog::default_action`](crate::widgets::Dialog::default_action)).

use std::collections::HashSet;

use crate::core::{DefaultAction, ElementKey, FocusId, FocusRequest, KeyInput, LayoutNode};
use crate::{App, Pane};

/// A pane's programmatic focus bookkeeping.
pub(crate) struct FocusState<S> {
    /// The autofocus elements laid out last build.
    seen: HashSet<ElementKey>,
    /// The autofocus element that last took focus, and the element focus
    /// goes back to when it leaves.
    back: Option<(ElementKey, Option<ElementKey>)>,
    /// Whether focus was inside that element after the last build.
    inside: bool,
    /// What Enter runs in an open dialog, last on top, and the fields that
    /// take Enter themselves (`on_submit`).
    pub(crate) defaults: Vec<DefaultAction<S>>,
    pub(crate) submitting: Vec<FocusId>,
}

impl<S> Default for FocusState<S> {
    fn default() -> Self {
        Self {
            seen: HashSet::new(),
            back: None,
            inside: false,
            defaults: Vec::new(),
            submitting: Vec::new(),
        }
    }
}

/// The layers keyboard focus may be in: the modal ones on top when one is up
/// (as Tab sees them), else the whole tree.
fn active_layers(tree: &LayoutNode, modal_layers: Option<usize>) -> Vec<&LayoutNode> {
    match modal_layers {
        Some(n) => {
            let layers: Vec<&LayoutNode> = tree.children.iter().filter(|l| !l.ghost).collect();
            layers[layers.len().saturating_sub(n)..].to_vec()
        }
        None => vec![tree],
    }
}

/// The node with identity `key`, if one is laid out (not leaving).
fn find_key(node: &LayoutNode, key: ElementKey) -> Option<&LayoutNode> {
    if node.ghost {
        return None;
    }
    if node.key == Some(key) {
        return Some(node);
    }
    node.children.iter().find_map(|c| find_key(c, key))
}

/// The first focusable at or inside `node`.
fn first_focus(node: &LayoutNode) -> Option<FocusId> {
    if node.ghost {
        return None;
    }
    node.focus
        .or_else(|| node.children.iter().find_map(first_focus))
}

/// The focus `key` names in the active layers: its element's, or the first
/// inside it.
fn target(layers: &[&LayoutNode], key: ElementKey) -> Option<FocusId> {
    layers
        .iter()
        .find_map(|l| find_key(l, key))
        .and_then(first_focus)
}

/// The identity of the innermost keyed node around (or at) focus `id`.
fn key_around(node: &LayoutNode, id: FocusId) -> Option<Option<ElementKey>> {
    if node.ghost {
        return None;
    }
    if node.focus == Some(id) {
        return Some(node.key);
    }
    node.children
        .iter()
        .find_map(|c| key_around(c, id))
        .map(|inner| inner.or(node.key))
}

/// The autofocus elements laid out under `node`, in tree order.
fn autofocus_nodes<'a>(node: &'a LayoutNode, out: &mut Vec<&'a LayoutNode>) {
    if node.ghost {
        return;
    }
    if node.keyboard.as_ref().is_some_and(|k| k.autofocus) && node.key.is_some() {
        out.push(node);
    }
    for child in &node.children {
        autofocus_nodes(child, out);
    }
}

impl<S> Pane<S> {
    /// Move focus as the build asked (`requests`, then the handle's for this
    /// window) and as autofocus elements that just appeared do, on the
    /// freshly laid-out `tree`. Returns whether focus changed.
    pub(crate) fn apply_focus(&mut self, tree: &LayoutNode, requests: Vec<FocusRequest>) -> bool {
        let before = self.focused;
        let layers = active_layers(tree, self.modal_layers);
        let mut found = Vec::new();
        autofocus_nodes(tree, &mut found);
        let keys: HashSet<ElementKey> = found.iter().filter_map(|n| n.key).collect();

        // An autofocus element that took focus left, with focus in it: focus
        // goes back to where it was.
        if let Some((key, back)) = self.focus_state.back
            && !keys.contains(&key)
        {
            if self.focus_state.inside {
                self.focused = back.and_then(|b| target(&layers, b));
            }
            self.focus_state.back = None;
        }

        // The last autofocus element new to this build, in the active
        // layers, takes focus.
        let fresh = found
            .iter()
            .rev()
            .filter_map(|n| n.key)
            .filter(|k| !self.focus_state.seen.contains(k))
            .find_map(|k| target(&layers, k).map(|f| (k, f)));
        if let Some((key, focus)) = fresh {
            let back = self.focused.and_then(|f| key_around(tree, f)).flatten();
            self.focus_state.back = Some((key, back));
            self.focused = Some(focus);
        }
        self.focus_state.seen = keys;

        // Then what the view and handlers asked for.
        let queued = self.handle.take_focus_requests(self.id);
        for request in requests.into_iter().chain(queued) {
            match request {
                FocusRequest::Key(key) => {
                    if let Some(focus) = target(&layers, key) {
                        self.focused = Some(focus);
                        self.focus_state.back = None;
                    }
                }
                FocusRequest::Blur => {
                    self.focused = None;
                    self.focus_state.back = None;
                }
            }
        }

        self.focus_state.inside = match (self.focus_state.back, self.focused) {
            (Some((key, _)), Some(f)) => {
                find_key(tree, key).is_some_and(|n| crate::core::find_focus(n, f).is_some())
            }
            _ => false,
        };
        self.focused != before
    }
}

impl<S: 'static> App<S> {
    /// Enter in pane `idx` runs the open dialog's default action, if it has
    /// one and the focused element leaves Enter alone: nothing focused, or a
    /// single-line field without `on_submit`, or anything else in the dialog
    /// that isn't a button. Returns whether it ran.
    pub(crate) fn default_enter(&mut self, idx: usize, input: &KeyInput) -> bool {
        if !matches!(input, KeyInput::Enter) {
            return false;
        }
        let App { panes, state, .. } = self;
        let pane = &panes[idx];
        let (Some(action), Some(tree)) = (pane.focus_state.defaults.last(), pane.tree.as_ref())
        else {
            return false;
        };
        if pane.input_viewport.is_some() {
            return false;
        }
        let Some(scope) = find_key(tree, action.scope) else {
            return false;
        };
        if let Some(f) = pane.focused {
            let layers = active_layers(tree, pane.modal_layers);
            let reachable = layers
                .iter()
                .any(|l| crate::core::find_focus(l, f).is_some());
            if reachable {
                if crate::core::find_focus(scope, f).is_none() {
                    // Focus is in something above the dialog (a message box).
                    return false;
                }
                let button = crate::core::tab_focus::tap_activation(tree, f).is_some();
                let multiline =
                    crate::core::input_hints(tree, Some(f)).is_some_and(|h| h.multiline);
                if button || multiline || pane.focus_state.submitting.contains(&f) {
                    return false;
                }
            }
        }
        let run = action.run.clone();
        (run.borrow_mut())(state);
        for pane in panes.iter_mut() {
            pane.dirty = true;
        }
        true
    }

    /// Escape in pane `idx` with a text field or a button focused closes
    /// what the view said Escape closes (a dialog, a sheet) before the field
    /// sees it: neither does anything with Escape itself. Returns whether it
    /// closed something.
    pub(crate) fn escape_first(&mut self, idx: usize, input: &KeyInput) -> bool {
        if !matches!(input, KeyInput::Escape) {
            return false;
        }
        let pane = &self.panes[idx];
        let (Some(f), Some(tree)) = (pane.focused, pane.tree.as_ref()) else {
            return false;
        };
        // A drawn message box takes its own Escape.
        if pane.input_viewport.is_some() || pane.alert_return.is_some() {
            return false;
        }
        let field = crate::core::input_hints(tree, Some(f)).is_some();
        let button = crate::core::tab_focus::tap_activation(tree, f).is_some();
        (field || button) && self.view_back(idx, true)
    }
}
