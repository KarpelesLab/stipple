//! The AT-SPI events a tree swap raises: what changed between the tree a
//! screen reader last saw and the new one, node by node at the same place.

use super::marshal::Value;
use super::{node_path, text_role};
use crate::platform::access::A11yNode;

pub(super) const OBJECT: &str = "org.a11y.atspi.Event.Object";
pub(super) const WINDOW: &str = "org.a11y.atspi.Event.Window";
pub(super) const FOCUS: &str = "org.a11y.atspi.Event.Focus";

/// One AT-SPI event: a signal on the object at `path`, with the
/// `(kind, detail1, detail2, any_data)` every AT-SPI event carries.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Signal {
    pub(super) path: String,
    pub(super) interface: &'static str,
    pub(super) member: &'static str,
    pub(super) kind: String,
    pub(super) detail1: i32,
    pub(super) detail2: i32,
    /// An object reference here names a path only: the bus name (ours) is
    /// filled in when it is sent.
    pub(super) any: Value,
}

impl Signal {
    pub(super) fn new(path: String, interface: &'static str, member: &'static str) -> Self {
        Self {
            path,
            interface,
            member,
            kind: String::new(),
            detail1: 0,
            detail2: 0,
            any: Value::I32(0),
        }
    }

    pub(super) fn kind(mut self, kind: &str) -> Self {
        self.kind = kind.to_string();
        self
    }

    pub(super) fn details(mut self, d1: i32, d2: i32) -> Self {
        self.detail1 = d1;
        self.detail2 = d2;
        self
    }

    pub(super) fn any(mut self, any: Value) -> Self {
        self.any = any;
        self
    }
}

/// `object:state-changed:{state}` on `path`.
pub(super) fn state(path: String, state: &str, on: bool) -> Signal {
    Signal::new(path, OBJECT, "StateChanged")
        .kind(state)
        .details(i32::from(on), 0)
}

/// The focus moving to the node at `path`: its `state-changed:focused` and
/// the `focus:` event.
pub(super) fn focus(path: String) -> [Signal; 2] {
    [
        state(path.clone(), "focused", true),
        Signal::new(path, FOCUS, "Focus"),
    ]
}

/// The structural path of the focused node, if any.
fn focused_chain(node: &A11yNode, chain: &mut Vec<usize>) -> bool {
    if node.focused {
        return true;
    }
    for (i, c) in node.children.iter().enumerate() {
        chain.push(i);
        if focused_chain(c, chain) {
            return true;
        }
        chain.pop();
    }
    false
}

pub(super) fn focused(node: &A11yNode) -> Option<Vec<usize>> {
    let mut chain = Vec::new();
    focused_chain(node, &mut chain).then_some(chain)
}

/// The events going from `old` to `new` in window `win`. Nodes are matched by
/// their place in the tree (the same child index at every level), as their
/// object paths are.
pub(super) fn diff(win: u64, old: &A11yNode, new: &A11yNode) -> Vec<Signal> {
    let mut out = Vec::new();
    walk(win, old, new, &mut Vec::new(), &mut out);
    // A toast that appeared is announced (`object:announcement`, polite, with
    // its message), as GTK does for `gtk_accessible_announce`.
    for (chain, text) in new.alert_paths_new_since(Some(old)) {
        out.push(announcement(node_path(win, &chain), text));
    }
    let (was, is) = (focused(old), focused(new));
    if was != is {
        if let Some(chain) = was {
            out.push(state(node_path(win, &chain), "focused", false));
        }
        if let Some(chain) = is {
            out.extend(focus(node_path(win, &chain)));
        }
    }
    out
}

fn walk(win: u64, old: &A11yNode, new: &A11yNode, chain: &mut Vec<usize>, out: &mut Vec<Signal>) {
    let here = node_path(win, chain);
    let path = || here.clone();
    if super::node_role(old) != super::node_role(new) {
        out.push(
            Signal::new(path(), OBJECT, "PropertyChange")
                .kind("accessible-role")
                .any(Value::U32(super::node_role(new))),
        );
    }
    // A scroll container's value is its offset.
    if let Some(s) = new.scroll
        && old
            .scroll
            .is_none_or(|o| (o.offset - s.offset).abs() > 0.5 || (o.max - s.max).abs() > 0.5)
    {
        out.push(
            Signal::new(path(), OBJECT, "PropertyChange")
                .kind("accessible-value")
                .any(Value::F64(s.offset)),
        );
    }
    // A slider's, progress bar's or spin button's value moved.
    if let Some(r) = new.range
        && old.range.is_none_or(|o| o.value != r.value)
    {
        out.push(
            Signal::new(path(), OBJECT, "PropertyChange")
                .kind("accessible-value")
                .any(Value::F64(r.value)),
        );
    }
    if old.name != new.name {
        out.push(
            Signal::new(path(), OBJECT, "PropertyChange")
                .kind("accessible-name")
                .any(Value::Str(new.name.clone())),
        );
        if text_role(new.role) && text_role(old.role) {
            let (at, deleted, inserted) = super::text::change(&old.name, &new.name);
            for (kind, s) in [("delete", deleted), ("insert", inserted)] {
                if !s.is_empty() {
                    let len = super::text::char_count(&s);
                    out.push(
                        Signal::new(path(), OBJECT, "TextChanged")
                            .kind(kind)
                            .details(at, len)
                            .any(Value::Str(s)),
                    );
                }
            }
        }
    }
    if text_role(new.role) {
        out.extend(caret_events(here.clone(), old, new));
    }
    for (name, was, is) in [
        (
            "checked",
            old.checked == Some(true),
            new.checked == Some(true),
        ),
        ("selected", old.selected, new.selected),
        (
            "expanded",
            old.expanded == Some(true),
            new.expanded == Some(true),
        ),
    ] {
        if was != is {
            out.push(state(path(), name, is));
        }
    }
    let common = old.children.len().min(new.children.len());
    for i in 0..common {
        chain.push(i);
        walk(win, &old.children[i], &new.children[i], chain, out);
        chain.pop();
    }
    // Children past the common ones: removed from the end first, or added.
    for i in (common..old.children.len()).rev() {
        chain.push(i);
        let child = node_path(win, chain);
        chain.pop();
        out.push(children_changed(path(), "remove", i, child));
    }
    for i in common..new.children.len() {
        chain.push(i);
        let child = node_path(win, chain);
        chain.pop();
        out.push(children_changed(path(), "add", i, child));
    }
}

/// The caret and selection of a field, as character offsets.
fn text_state(n: &A11yNode) -> (Option<i32>, Option<(i32, i32)>) {
    let at = |b: usize| super::text::char_offset(&n.name, b);
    (
        n.caret.map(at),
        n.text_selection.map(|(s, e)| (at(s), at(e))),
    )
}

/// `object:text-caret-moved` (to the new offset) and
/// `object:text-selection-changed` when a field's caret or selection moved.
fn caret_events(path: String, old: &A11yNode, new: &A11yNode) -> Vec<Signal> {
    let (was_caret, was_sel) = text_state(old);
    let (caret, sel) = text_state(new);
    let mut out = Vec::new();
    if let Some(c) = caret
        && was_caret != caret
    {
        out.push(Signal::new(path.clone(), OBJECT, "TextCaretMoved").details(c, 0));
    }
    if was_sel != sel {
        out.push(Signal::new(path, OBJECT, "TextSelectionChanged"));
    }
    out
}

/// `AtspiLive`: how urgently an announcement is spoken.
const LIVE_POLITE: i32 = 1;

/// `object:announcement` of `text` from the node at `path`.
pub(super) fn announcement(path: String, text: String) -> Signal {
    Signal::new(path, OBJECT, "Announcement")
        .details(LIVE_POLITE, 0)
        .any(Value::Str(text))
}

/// `object:children-changed:{add,remove}` on `parent` for the child at
/// `index`.
pub(super) fn children_changed(parent: String, kind: &str, index: usize, child: String) -> Signal {
    Signal::new(parent, OBJECT, "ChildrenChanged")
        .kind(kind)
        .details(index as i32, 0)
        .any(Value::Obj(String::new(), child))
}
