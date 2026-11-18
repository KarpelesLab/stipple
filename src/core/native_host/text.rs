//! Text fields as live OS text controls (see `docs/native-theme.md`, *Text
//! fields*): the [`NativeText`] a field's node hands its control, from the
//! state the widget put on it (`Element::native_text_state`) and what the
//! tree says around it (its hint, keyboard hints, label, focus).
//!
//! A field is live while it can be (as any control), and keeps being live
//! while it is edited: its focus ring doesn't make it a picture, nor does
//! an invalid number (whose frame is then Stipple's red one) while it is
//! focused ([`bare_field`]). It still turns into a picture under something
//! drawn over it, a transform, a fade or a clip that isn't a scroll area's, where its editing
//! goes back to Stipple's (the composition in progress, if any, ends).

use std::collections::HashMap;

use crate::core::runtime::LayoutNode;
use crate::platform::native::{NativeControl, NativeDeco, NativeState, NativeText};

use super::PlanInput;

/// `node`'s field state for its live control: the widget's, with the
/// field's hint, keyboard hints and focus. `None` when it has none (a
/// field that isn't one the OS edits).
pub(super) fn spec(node: &LayoutNode, focused: bool) -> Option<Box<NativeText>> {
    let sem = node.semantics.as_ref()?;
    let mut t = sem.native_text.clone()?;
    t.placeholder = sem.placeholder.clone().unwrap_or_default();
    t.hints = sem.input.unwrap_or_default();
    t.multiline = t.hints.multiline;
    t.hints.secure = t.secure;
    t.focused = focused;
    Some(t)
}

/// A field with no OS frame that is edited live all the same: an invalid
/// number input (its frame is Stipple's red one) while it has the focus,
/// so it doesn't turn from an OS field into Stipple's mid-edit.
pub(super) fn bare_field(node: &LayoutNode, input: &PlanInput) -> Option<NativeDeco> {
    let t = node.semantics.as_ref()?.native_text.as_ref()?;
    let focused = node.focus.is_some() && node.focus == input.focused;
    focused.then_some(NativeDeco {
        control: NativeControl::TextField,
        dark: t.dark,
        state: NativeState::default(),
    })
}

/// The name a screen reader reads for a field: its label, else the text
/// of the element it is labelled by, else its hint. (Its text is its
/// value, which the control reports by itself.)
pub(super) fn label(node: &LayoutNode, labels: &HashMap<String, String>) -> String {
    let sem = node.semantics.as_ref();
    sem.and_then(|s| s.label.clone())
        .or_else(|| {
            sem.and_then(|s| s.labelled_by.as_ref())
                .and_then(|id| labels.get(id).cloned())
        })
        .or_else(|| sem.and_then(|s| s.placeholder.clone()))
        .unwrap_or_default()
}

/// The names of the nodes fields may be labelled by, by their ids (only
/// when the tree has a field the OS edits).
pub(super) fn labels(tree: &LayoutNode) -> HashMap<String, String> {
    fn has_field(n: &LayoutNode) -> bool {
        n.semantics
            .as_ref()
            .is_some_and(|s| s.native_text.is_some())
            || n.children.iter().any(has_field)
    }
    fn walk(n: &LayoutNode, out: &mut HashMap<String, String>) {
        if let Some(id) = n.semantics.as_ref().and_then(|s| s.id.clone()) {
            out.insert(id, crate::core::a11y::name_of(n));
        }
        n.children.iter().for_each(|c| walk(c, out));
    }
    let mut out = HashMap::new();
    if has_field(tree) {
        walk(tree, &mut out);
    }
    out
}
