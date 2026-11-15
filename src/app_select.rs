//! Selectable read-only text in the [`App`]
//! ([`Element::selectable`](crate::core::Element::selectable)): the window's
//! one selection, which element it is in (by its identity across builds),
//! pressing, dragging, double- and triple-clicking to select, the keys that
//! extend and copy it, and its context menu (Copy, Select All), which a long
//! press opens on a touch screen. The selection itself is an [`EditBuffer`],
//! so words, lines and Shift+arrows behave as in a text field.

use std::cell::RefCell;
use std::rc::Rc;

use crate::core::{
    ContextId, Cx, ElementKey, KeyInput, LayoutNode, Menu, MenuItem, NodeContent, focus_at,
    local_point,
};
use crate::geometry::Point;
use crate::platform::clipboard::{ClipboardContent, ClipboardKind};
use crate::render::Font;
use crate::style::Theme;
use crate::widgets::EditBuffer;
use crate::{Pane, press_layer};

/// The selection in a window's read-only text.
#[derive(Debug)]
pub(crate) struct ReadSelection {
    /// The text element it is in.
    pub key: ElementKey,
    /// Its text, caret and anchor.
    pub buffer: EditBuffer,
}

/// A window's selection, shared with its context menu's actions.
pub(crate) type SharedSelection = Rc<RefCell<Option<ReadSelection>>>;

/// The selectable text leaf under `point` (the topmost), and the point in
/// its layout coordinates.
fn selectable_at(node: &LayoutNode, point: Point) -> Option<(&LayoutNode, Point)> {
    let point = node.to_local(point)?;
    if crate::core::runtime::clipped_out(node, point) {
        return None;
    }
    for child in node.children.iter().rev() {
        if let Some(hit) = selectable_at(child, point) {
            return Some(hit);
        }
    }
    match &node.content {
        NodeContent::Text { style, .. }
            if style.selectable && node.key.is_some() && node.bounds.contains(point) =>
        {
            Some((node, point))
        }
        _ => None,
    }
}

/// The selectable text leaf with identity `key`.
fn find(node: &LayoutNode, key: ElementKey) -> Option<&LayoutNode> {
    if node.key == Some(key) && matches!(node.content, NodeContent::Text { .. }) {
        return Some(node);
    }
    node.children.iter().find_map(|c| find(c, key))
}

/// The caret position in `leaf`'s text nearest `local` (in its layout
/// coordinates).
fn index_in(leaf: &LayoutNode, local: Point, font: Option<&Font>) -> Option<usize> {
    let layout = crate::core::leaf_layout(leaf, font)?;
    let b = leaf.text_rect();
    Some(layout.index_at(Point::new(local.x - b.min_x(), local.y - b.min_y())))
}

/// `leaf`'s text, if it is text.
fn text_of(leaf: &LayoutNode) -> Option<&str> {
    match &leaf.content {
        NodeContent::Text { text, .. } => Some(text),
        _ => None,
    }
}

/// Mark the selection's leaf in `tree` with it (highlighted in `theme`'s
/// selection color), and give every selectable leaf without a context menu
/// of its own the selection's, `menu`. Forget the selection when its text is
/// gone or changed.
pub(crate) fn stamp(
    tree: &mut LayoutNode,
    selection: &SharedSelection,
    menu: Option<ContextId>,
    theme: &Theme,
) {
    fn walk(
        n: &mut LayoutNode,
        sel: Option<&ReadSelection>,
        menu: Option<ContextId>,
        theme: &Theme,
        found: &mut bool,
    ) {
        if let NodeContent::Text { text, style, .. } = &mut n.content
            && style.selectable
        {
            if n.context.is_none() {
                n.context = menu;
            }
            if let Some(s) = sel
                && n.key == Some(s.key)
                && s.buffer.text() == text.as_str()
            {
                *found = true;
                n.selection = s.buffer.selection();
                style.highlight = Some(theme.palette.selection);
            }
        }
        for c in &mut n.children {
            walk(c, sel, menu, theme, found);
        }
    }
    let mut sel = selection.borrow_mut();
    let mut found = false;
    walk(tree, sel.as_ref(), menu, theme, &mut found);
    if !found {
        *sel = None;
    }
}

/// Register the selection's context menu in `cx`: Copy (the selected
/// `text`, if any) and Select All.
pub(crate) fn register_menu<S: 'static>(
    cx: &mut Cx<S>,
    selection: &SharedSelection,
    text: Option<String>,
) -> ContextId {
    let enabled = text.is_some();
    let all = selection.clone();
    let menu = Menu::new("")
        .item(
            MenuItem::new("Copy", move |_: &mut S| {
                if let Some(t) = &text {
                    crate::core::write_clipboard(
                        ClipboardKind::Clipboard,
                        ClipboardContent::from_text(t.clone()),
                    );
                }
            })
            .enabled(enabled),
        )
        .item(MenuItem::new("Select All", move |_: &mut S| {
            if let Some(s) = all.borrow_mut().as_mut() {
                s.buffer.select_all();
            }
        }));
    cx.register_context_menu(menu)
}

impl<S: 'static> Pane<S> {
    /// The selected text as copied (rich text's images as their alternative
    /// text), if any is selected.
    pub(crate) fn selected_text(&self) -> Option<String> {
        let sel = self.read_sel.borrow();
        let s = sel.as_ref()?;
        let (a, b) = s.buffer.selection()?;
        let leaf = self.tree.as_ref().and_then(|t| find(t, s.key));
        Some(match leaf.map(|l| &l.content) {
            Some(NodeContent::Text { text, style, .. }) if text.as_str() == s.buffer.text() => {
                style.plain(text, a..b)
            }
            _ => s.buffer.text()[a..b].to_string(),
        })
    }

    /// Forget the selection. Returns whether there was one.
    fn clear_selection(&mut self) -> bool {
        let had = self.read_sel.borrow_mut().take().is_some();
        if had {
            self.dirty = true;
        }
        had
    }

    /// A press at `pos` at time `now`. On selectable text (and not on a tap
    /// target, `tapping`) a mouse press puts the selection's caret there and
    /// starts a drag-selection; a double-click selects the word and a
    /// triple-click the line. A finger doesn't select (it scrolls; a long
    /// press selects, see [`select_for_menu`](Self::select_for_menu)). A
    /// press anywhere else in the window's main tree clears the selection.
    /// Returns whether a selection started.
    pub(crate) fn select_press_at(
        &mut self,
        font: Option<&Font>,
        pos: Point,
        now: crate::platform::time::Instant,
        tapping: bool,
    ) -> bool {
        let touch = self.touch.primary().is_some();
        let Some(tree) = self.tree.as_ref() else {
            return false;
        };
        let (layer, main) = press_layer(tree, self.overlay_active, pos);
        let hit = selectable_at(layer, pos)
            .filter(|_| !tapping)
            .and_then(|(leaf, local)| {
                Some((
                    leaf.key?,
                    text_of(leaf)?.to_string(),
                    index_in(leaf, local, font)?,
                ))
            });
        let Some((key, text, index)) = hit else {
            // A press in a menu (its Copy, its Select All) keeps it.
            if main {
                self.clear_selection();
            }
            return false;
        };
        if touch {
            // A tap on the selection keeps it, anywhere else clears it.
            let inside = self.read_sel.borrow().as_ref().is_some_and(|s| {
                s.key == key
                    && s.buffer
                        .selection()
                        .is_some_and(|(a, b)| a <= index && index <= b)
            });
            if !inside {
                self.clear_selection();
            }
            return false;
        }
        let foc = focus_at(layer, pos);
        {
            let mut sel = self.read_sel.borrow_mut();
            let same = sel
                .as_ref()
                .is_some_and(|s| s.key == key && s.buffer.text() == text);
            if !same {
                *sel = Some(ReadSelection {
                    key,
                    buffer: EditBuffer::from_text(text),
                });
            }
            let b = &mut sel.as_mut().expect("set above").buffer;
            b.place_caret(index);
            match self.clicks.press(now, pos) {
                2 => b.select_word(),
                3 => b.select_line(),
                _ => {}
            }
        }
        // Clicking text takes the focus from a field, as on the web, so
        // Ctrl/Cmd+C copies this selection.
        if foc != self.focused {
            self.focused = foc;
            self.focus_visible = false;
        }
        self.read_selecting = true;
        self.dirty = true;
        true
    }

    /// Extend the selection being dragged to `pos`. Returns whether it did.
    pub(crate) fn select_drag_at(&mut self, font: Option<&Font>, pos: Point) -> bool {
        if !self.read_selecting {
            return false;
        }
        let mut sel = self.read_sel.borrow_mut();
        let Some(s) = sel.as_mut() else {
            return false;
        };
        let key = s.key;
        let Some(index) = self.tree.as_ref().and_then(|t| {
            let at = local_point(t, &|n| n.key == Some(key), pos)?;
            index_in(find(t, key)?, at, font)
        }) else {
            return false;
        };
        let before = s.buffer.selection();
        s.buffer.extend_to(index);
        let moved = s.buffer.selection() != before;
        drop(sel);
        if moved {
            self.dirty = true;
        }
        moved
    }

    /// End a drag-selection (the button was released).
    pub(crate) fn end_select(&mut self) {
        self.read_selecting = false;
    }

    /// A right-click or long press at `pos`: on selectable text outside the
    /// selection, select the word there, so the context menu acts on it.
    pub(crate) fn select_for_menu(&mut self, font: Option<&Font>, pos: Point) {
        let Some(tree) = self.tree.as_ref() else {
            return;
        };
        let layer = press_layer(tree, self.overlay_active, pos).0;
        let Some((key, text, index)) = selectable_at(layer, pos).and_then(|(leaf, local)| {
            Some((
                leaf.key?,
                text_of(leaf)?.to_string(),
                index_in(leaf, local, font)?,
            ))
        }) else {
            return;
        };
        let mut sel = self.read_sel.borrow_mut();
        let inside = sel.as_ref().is_some_and(|s| {
            s.key == key
                && s.buffer.text() == text
                && s.buffer
                    .selection()
                    .is_some_and(|(a, b)| a <= index && index <= b)
        });
        if inside {
            return;
        }
        let mut buffer = EditBuffer::from_text(text);
        buffer.place_caret(index);
        buffer.select_word();
        *sel = Some(ReadSelection { key, buffer });
        self.dirty = true;
    }

    /// Whether the focused element edits text itself (a text field).
    pub(crate) fn focus_edits(&self) -> bool {
        fn edits(n: &LayoutNode) -> bool {
            n.text_pos.is_some() || n.children.iter().any(edits)
        }
        match (self.focused, self.tree.as_ref()) {
            (Some(id), Some(tree)) => crate::core::find_focus(tree, id).is_some_and(edits),
            _ => false,
        }
    }

    /// Whether there is a selection in read-only text that editing keys
    /// (Ctrl/Cmd+C, Shift+arrows) go to.
    pub(crate) fn has_read_selection(&self) -> bool {
        self.read_sel.borrow().is_some() && !self.focus_edits()
    }

    /// Apply an editing key to the selection in read-only text, unless a
    /// text field has the focus: copying, selecting all, and extending it.
    /// Escape clears it. Returns whether the key was taken.
    pub(crate) fn select_key(&mut self, input: &KeyInput) -> bool {
        if !self.has_read_selection() {
            return false;
        }
        match input {
            KeyInput::Copy | KeyInput::Cut => {
                if let Some(text) = self.selected_text() {
                    crate::core::set_clipboard_text(&text);
                }
                true
            }
            KeyInput::Escape => self.clear_selection(),
            KeyInput::SelectAll
            | KeyInput::SelectLeft
            | KeyInput::SelectRight
            | KeyInput::SelectUp
            | KeyInput::SelectDown
            | KeyInput::SelectHome
            | KeyInput::SelectEnd
            | KeyInput::SelectWordLeft
            | KeyInput::SelectWordRight
            | KeyInput::SelectWord
            | KeyInput::SelectLine => {
                if let Some(s) = self.read_sel.borrow_mut().as_mut() {
                    s.buffer.apply(input);
                }
                self.dirty = true;
                true
            }
            _ => false,
        }
    }
}
