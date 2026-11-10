//! The document `UITextInput` shows the keyboard, kept free of UIKit so it is
//! unit-tested on every host: the focused field's text from
//! [`Window::set_ime`](crate::platform::Window::set_ime) with the composition
//! spliced in at the caret, in the UTF-16 offsets UIKit counts in.

use crate::platform::ImeState;

/// The document as the keyboard sees it: the text, the selection and the
/// marked range, in UTF-16 offsets.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Doc {
    pub(crate) text: String,
    pub(crate) sel: (usize, usize),
    pub(crate) marked: Option<(usize, usize)>,
}

pub(crate) fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// The largest char boundary of `s` at or before byte `at`.
pub(crate) fn floor_boundary(s: &str, at: usize) -> usize {
    let mut at = at.min(s.len());
    while !s.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// The document `ime` and the composition make.
pub(crate) fn document(ime: Option<&ImeState>, marked: &str, marked_sel: (usize, usize)) -> Doc {
    let Some(ime) = ime else {
        let len = utf16_len(marked);
        return Doc {
            text: marked.to_string(),
            sel: (marked_sel.0.min(len), marked_sel.1.min(len)),
            marked: (!marked.is_empty()).then_some((0, len)),
        };
    };
    let base = &ime.surrounding;
    let cursor = floor_boundary(base, ime.cursor);
    let anchor = floor_boundary(base, ime.anchor);
    if marked.is_empty() {
        let (lo, hi) = (cursor.min(anchor), cursor.max(anchor));
        return Doc {
            text: base.clone(),
            sel: (utf16_len(&base[..lo]), utf16_len(&base[..hi])),
            marked: None,
        };
    }
    let start = utf16_len(&base[..cursor]);
    let len = utf16_len(marked);
    let mut text = base[..cursor].to_string();
    text.push_str(marked);
    text.push_str(&base[cursor..]);
    Doc {
        text,
        sel: (start + marked_sel.0.min(len), start + marked_sel.1.min(len)),
        marked: Some((start, start + len)),
    }
}

/// `ime` after replacing the bytes `lo..hi` of its text with `text`, the
/// caret after it: how the keyboard's own edits show until the app reports
/// the field again.
pub(crate) fn spliced(ime: &ImeState, lo: usize, hi: usize, text: &str) -> ImeState {
    let base = &ime.surrounding;
    let lo = floor_boundary(base, lo);
    let hi = floor_boundary(base, hi).max(lo);
    let mut surrounding = base[..lo].to_string();
    surrounding.push_str(text);
    surrounding.push_str(&base[hi..]);
    let caret = lo + text.len();
    ImeState {
        surrounding,
        cursor: caret,
        anchor: caret,
        ..ime.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Rect;

    fn field(text: &str, cursor: usize, anchor: usize) -> ImeState {
        ImeState {
            cursor_area: Rect::from_xywh(10.0, 20.0, 2.0, 16.0),
            surrounding: text.into(),
            cursor,
            anchor,
        }
    }

    #[test]
    fn the_document_counts_utf16_and_splices_the_composition_at_the_caret() {
        // "é" is 2 bytes, 1 unit; "😀" 4 bytes, 2 units.
        let ime = field("é😀b", 6, 6);
        let doc = document(Some(&ime), "", (0, 0));
        assert_eq!(doc.sel, (3, 3));
        assert_eq!(doc.marked, None);
        let doc = document(Some(&ime), "にほん", (3, 3));
        assert_eq!(doc.text, "é😀にほんb");
        assert_eq!(doc.marked, Some((3, 6)));
        assert_eq!(doc.sel, (6, 6));
        // A selection, either way round.
        let doc = document(Some(&field("abcd", 1, 3)), "", (0, 0));
        assert_eq!(doc.sel, (1, 3));
        // No field: only the composition.
        let doc = document(None, "ab", (1, 1));
        assert_eq!(
            (doc.text.as_str(), doc.sel, doc.marked),
            ("ab", (1, 1), Some((0, 2)))
        );
    }

    #[test]
    fn keyboard_edits_show_until_the_app_reports_back() {
        let ime = field("Hi", 2, 2);
        let typed = spliced(&ime, 2, 2, "日本");
        assert_eq!((typed.surrounding.as_str(), typed.cursor), ("Hi日本", 8));
        let replaced = spliced(&typed, 2, 8, "JP");
        assert_eq!(
            (replaced.surrounding.as_str(), replaced.cursor),
            ("HiJP", 4)
        );
        // Offsets inside a character snap back to its start.
        assert_eq!(spliced(&typed, 3, 3, "x").surrounding, "Hix日本");
    }
}
