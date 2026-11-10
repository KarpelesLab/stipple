//! Input-method support for the focused text field: the underline under its
//! composition (preedit) text, and the [`ImeState`] the window passes to the
//! input method (where the caret is, and the text around it).

use crate::core::runtime::{FocusId, LayoutNode, NodeContent, find_focus, first_text};
use crate::geometry::Rect;
use crate::platform::ImeState;
use crate::render::{Color, Font, Scene};

/// The focused node's editable text leaf, if the focused element is text
/// entry (the same test the focus overlay uses to draw a caret).
fn entry_leaf(tree: &LayoutNode, focused: FocusId) -> Option<&LayoutNode> {
    let node = find_focus(tree, focused)?;
    let entry = node
        .semantics
        .as_ref()
        .and_then(|s| s.role)
        .is_none_or(|r| {
            matches!(
                r,
                crate::core::a11y::Role::TextField | crate::core::a11y::Role::SpinButton
            )
        });
    if node.action.is_some() || !entry {
        return None;
    }
    let leaf = first_text(node)?;
    // Only an editable leaf (one that carries a caret) takes text input.
    leaf.caret.is_some().then_some(leaf)
}

/// Where byte `i` of a text leaf sits: its x and the top of its line, and the
/// line height. Lines break at `\n`, as the caret drawing does.
fn position(leaf: &LayoutNode, font: Option<&Font>, i: usize) -> Option<(f64, f64, f64)> {
    let NodeContent::Text {
        text, size, style, ..
    } = &leaf.content
    else {
        return None;
    };
    let styled = style.resolve(font);
    let font = styled.as_ref();
    let line_h = font.map(|f| f.line_height(*size)).unwrap_or(*size);
    let mut i = i.min(text.len());
    while !text.is_char_boundary(i) {
        i -= 1;
    }
    let ls = text[..i].rfind('\n').map_or(0, |n| n + 1);
    let line = text[..ls].bytes().filter(|&b| b == b'\n').count();
    let w = font.map_or(0.0, |f| f.measure(&text[ls..i], *size).width);
    let b = leaf.bounds;
    let x = (b.min_x() + w).min(b.max_x().max(b.min_x()));
    Some((x, b.min_y() + line as f64 * line_h, line_h))
}

/// Underline the focused field's composition text, one stroke per line it
/// spans. Drawn after the focus overlay, in `color` (the text color).
pub fn paint_preedit(
    tree: &LayoutNode,
    focused: FocusId,
    scene: &mut Scene,
    font: Option<&Font>,
    color: Color,
) {
    let Some(leaf) = entry_leaf(tree, focused) else {
        return;
    };
    let (Some((s, e)), NodeContent::Text { text, .. }) = (leaf.preedit, &leaf.content) else {
        return;
    };
    let e = e.min(text.len());
    let mut start = s.min(e);
    while start < e {
        // The rest of this line, up to a '\n' or the end of the preedit.
        let end = text[start..e].find('\n').map_or(e, |n| start + n);
        if let (Some((x0, y, h)), Some((x1, _, _))) =
            (position(leaf, font, start), position(leaf, font, end))
            && x1 > x0
        {
            scene.fill_rect(Rect::from_xywh(x0, y + h - 2.0, x1 - x0, 1.5), color);
        }
        start = end + 1;
    }
}

/// What the input method should know about the focused field, or `None` when
/// the focused element doesn't take text (which disables the IME).
pub fn ime_state(
    tree: &LayoutNode,
    focused: Option<FocusId>,
    font: Option<&Font>,
) -> Option<ImeState> {
    let leaf = entry_leaf(tree, focused?)?;
    let NodeContent::Text { text, .. } = &leaf.content else {
        return None;
    };
    // The candidate window goes by the start of the composition, else the caret.
    let caret = leaf.caret.unwrap_or(text.len()).min(text.len());
    let at = leaf.preedit.map_or(caret, |(s, _)| s);
    let (x, y, h) = position(leaf, font, at)?;
    // The text without the preedit, and the caret and anchor in it.
    let (surrounding, cursor, anchor) = match leaf.preedit {
        Some((s, e)) if e <= text.len() && s <= e => {
            let mut t = text[..s].to_string();
            t.push_str(&text[e..]);
            (t, s, s)
        }
        _ => {
            let anchor = match leaf.selection {
                Some((a, b)) if b == caret => a,
                Some((a, b)) if a == caret => b,
                _ => caret,
            };
            (text.clone(), caret, anchor)
        }
    };
    Some(ImeState {
        cursor_area: Rect::from_xywh(x, y, 2.0, h),
        surrounding,
        cursor,
        anchor,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Cx, Element, layout};
    use crate::style::Theme;
    use crate::widgets::{EditBuffer, text_editor};

    fn tree_for(buffer: &EditBuffer) -> (LayoutNode, FocusId) {
        let theme = Theme::light();
        let mut cx: Cx<EditBuffer> = Cx::new(&theme);
        let el: Element = text_editor(&mut cx, &theme, buffer, |s: &mut EditBuffer| s);
        let tree = layout(&el, Rect::from_xywh(0.0, 0.0, 400.0, 100.0), None);
        let id = tree_focus(&tree).expect("the field is focusable");
        (tree, id)
    }

    fn tree_focus(node: &LayoutNode) -> Option<FocusId> {
        node.focus
            .or_else(|| node.children.iter().find_map(tree_focus))
    }

    #[test]
    fn ime_state_reports_text_without_the_preedit() {
        let mut b = EditBuffer::from_text("abc");
        b.place_caret(1);
        b.set_preedit("xy", Some((1, 1)));
        let (tree, id) = tree_for(&b);
        let leaf = entry_leaf(&tree, id).unwrap();
        assert_eq!(leaf.preedit, Some((1, 3)));
        let state = ime_state(&tree, Some(id), None).unwrap();
        assert_eq!(state.surrounding, "abc");
        assert_eq!((state.cursor, state.anchor), (1, 1));
        // Without a font the x can't be measured, but the area sits in the field.
        assert!(state.cursor_area.min_x() >= leaf.bounds.min_x());
        // Nothing focused: the IME is off.
        assert_eq!(ime_state(&tree, None, None), None);
    }

    #[test]
    fn ime_state_reports_the_selection_anchor() {
        let mut b = EditBuffer::from_text("hello");
        b.place_caret(1);
        b.extend_to(4);
        let (tree, id) = tree_for(&b);
        let state = ime_state(&tree, Some(id), None).unwrap();
        assert_eq!((state.cursor, state.anchor), (4, 1));
    }
}
