//! Input-method composition (preedit) text in an [`EditBuffer`].
//!
//! While an IME or a compose sequence is composing, its text is shown at the
//! caret, underlined, but isn't part of the buffer's text: it doesn't enter the
//! undo history, and [`EditBuffer::text`] doesn't include it. The commit that
//! ends the composition arrives as [`KeyInput::Text`](crate::core::KeyInput::Text)
//! and is inserted like typing.

use super::{EditBuffer, EditKind};

/// The composition in progress.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Preedit {
    pub(super) text: String,
    /// The byte range within `text` the IME highlights (equal ends for a caret).
    pub(super) cursor: Option<(usize, usize)>,
}

/// What a text field shows for an [`EditBuffer`]: its text with any preedit
/// spliced in at the caret, and where the caret, selection and preedit are in
/// that string. See [`EditBuffer::display`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayText {
    /// The text to draw.
    pub text: String,
    /// The caret, as a byte index into `text`.
    pub caret: usize,
    /// The selected byte range in `text`, if any.
    pub selection: Option<(usize, usize)>,
    /// The preedit's byte range in `text`, if composing.
    pub preedit: Option<(usize, usize)>,
}

/// The largest char boundary of `s` at or below `i`.
fn floor_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

impl EditBuffer {
    /// Show `text` as the composition in progress, with `cursor` (a byte range
    /// within it) highlighted. Empty `text` ends the composition. Starting one
    /// deletes the selection first, as typing would.
    pub fn set_preedit(&mut self, text: &str, cursor: Option<(usize, usize)>) {
        if text.is_empty() {
            self.preedit = None;
            return;
        }
        if self.preedit.is_none() {
            self.delete_selection();
        }
        let cursor = cursor.map(|(s, e)| {
            let (s, e) = (floor_boundary(text, s), floor_boundary(text, e));
            (s.min(e), s.max(e))
        });
        self.preedit = Some(Preedit {
            text: text.to_string(),
            cursor,
        });
        self.seal();
    }

    /// End the composition without committing anything.
    pub fn clear_preedit(&mut self) {
        self.preedit = None;
    }

    /// The composition in progress, if any.
    pub fn preedit(&self) -> Option<&str> {
        self.preedit.as_ref().map(|p| p.text.as_str())
    }

    /// Commit `text` in place of the composition: the preedit goes away and
    /// `text` is inserted at the caret as typing.
    pub(super) fn commit_text(&mut self, text: &str) {
        self.preedit = None;
        self.insert(text);
    }

    /// Delete `before` bytes before the caret and `after` bytes after it
    /// (snapped to char boundaries), as one undo step. An input method asks
    /// for this, for example to replace text it is re-converting.
    pub fn delete_surrounding(&mut self, before: usize, after: usize) {
        self.anchor = None;
        let start = floor_boundary(&self.text, self.caret.saturating_sub(before));
        let mut end = (self.caret + after).min(self.text.len());
        while !self.text.is_char_boundary(end) {
            end += 1;
        }
        self.replace(start..end, "", EditKind::Other);
        self.seal();
    }

    /// What a text field shows: the text with the composition spliced in at
    /// the caret, and where the caret, selection and preedit are in it.
    pub fn display(&self) -> DisplayText {
        let Some(p) = &self.preedit else {
            return DisplayText {
                text: self.text.clone(),
                caret: self.caret,
                selection: self.selection(),
                preedit: None,
            };
        };
        let at = self.caret;
        let mut text = String::with_capacity(self.text.len() + p.text.len());
        text.push_str(&self.text[..at]);
        text.push_str(&p.text);
        text.push_str(&self.text[at..]);
        let (caret, selection) = match p.cursor {
            Some((s, e)) if e > s => (at + e, Some((at + s, at + e))),
            Some((_, e)) => (at + e, None),
            None => (at + p.text.len(), None),
        };
        DisplayText {
            text,
            caret,
            selection,
            preedit: Some((at, at + p.text.len())),
        }
    }

    /// Map a byte index in [`display`](Self::display)'s text back to the
    /// buffer's text: a point inside the composition maps to the caret, one
    /// after it moves back by the composition's length.
    pub fn display_to_text(&self, i: usize) -> usize {
        match &self.preedit {
            Some(p) if i > self.caret + p.text.len() => i - p.text.len(),
            Some(_) if i > self.caret => self.caret,
            _ => i,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::core::KeyInput;
    use crate::widgets::EditBuffer;

    fn preedit(text: &str, cursor: Option<(usize, usize)>) -> KeyInput {
        KeyInput::Preedit {
            text: text.into(),
            cursor,
        }
    }

    #[test]
    fn preedit_shows_at_the_caret_without_editing_the_text() {
        let mut b = EditBuffer::from_text("ab");
        b.apply(&KeyInput::Left);
        b.apply(&preedit("にほ", Some((3, 3))));
        assert_eq!(b.text(), "ab");
        assert_eq!(b.preedit(), Some("にほ"));
        let d = b.display();
        assert_eq!(d.text, "aにほb");
        assert_eq!(d.preedit, Some((1, 7)));
        assert_eq!(d.caret, 4); // after に
        assert_eq!(d.selection, None);
        // Not an edit: nothing to undo yet.
        assert!(!b.can_undo());
        // A highlighted segment shows as the selection.
        b.apply(&preedit("日本", Some((0, 3))));
        assert_eq!(b.display().selection, Some((1, 4)));
        // Clicks map back past the composition.
        assert_eq!(b.display_to_text(0), 0);
        assert_eq!(b.display_to_text(4), 1);
        assert_eq!(b.display_to_text(8), 2);
    }

    #[test]
    fn commit_replaces_the_preedit_as_one_undo_step() {
        let mut b = EditBuffer::from_text("x");
        b.apply(&preedit("´", Some((2, 2))));
        b.apply(&preedit("ni", Some((2, 2))));
        b.apply(&KeyInput::Text("日".into()));
        assert_eq!(b.text(), "x日");
        assert_eq!(b.preedit(), None);
        assert_eq!(b.display().text, "x日");
        assert!(b.undo());
        assert_eq!(b.text(), "x");
        assert_eq!(b.preedit(), None);
        assert!(!b.can_undo());
    }

    #[test]
    fn empty_preedit_cancels_and_a_selection_is_replaced() {
        let mut b = EditBuffer::from_text("hello");
        b.apply(&preedit("k", None));
        b.apply(&preedit("", None));
        assert_eq!(b.display().text, "hello");
        assert!(!b.can_undo());
        // Composing over a selection deletes it first, like typing does.
        b.select_all();
        b.apply(&preedit("か", Some((3, 3))));
        assert_eq!(b.text(), "");
        assert_eq!(b.display().text, "か");
        b.apply(&KeyInput::Text("火".into()));
        assert_eq!(b.text(), "火");
        assert!(b.undo());
        assert!(b.undo());
        assert_eq!(b.text(), "hello");
    }

    #[test]
    fn delete_surrounding_text() {
        let mut b = EditBuffer::from_text("héllo");
        b.place_caret(3); // after "hé"
        // Two bytes before the caret is the whole "é"; one after is an "l".
        b.apply(&KeyInput::DeleteSurrounding {
            before: 2,
            after: 1,
        });
        assert_eq!(b.text(), "hlo");
        assert_eq!(b.caret(), 1);
        assert!(b.undo());
        assert_eq!(b.text(), "héllo");
    }
}
