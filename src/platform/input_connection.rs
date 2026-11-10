//! Android's `InputConnection` model of a text field, on top of Stipple's.
//!
//! An Android input method edits an *editable*: one string with a selection
//! and, while composing, a composing region inside it, addressed in UTF-16
//! units. Stipple's text fields keep their text and a separate preedit at the
//! caret ([`Ime::Preedit`]), addressed in bytes, and tell the backend what they
//! hold through [`ImeState`]. [`Editor`] keeps the editable the input method
//! believes in (the field's last known text with the composition inserted at
//! the selection), answers its queries from it, and turns its edits into
//! [`Ime`] events for the field, updating itself at once so the next query
//! sees the edit before the field has even run.
//!
//! Host-testable logic; the JNI side is `backend::android::ime`.

use crate::platform::event::Ime;
use crate::platform::window::ImeState;

/// The `TextUtils.CAP_MODE_*` bits [`Editor::caps_mode`] answers with.
pub(crate) const CAP_MODE_CHARACTERS: i32 = 0x1000;
pub(crate) const CAP_MODE_WORDS: i32 = 0x2000;
pub(crate) const CAP_MODE_SENTENCES: i32 = 0x4000;

/// What the backend must tell the input method after the field changed (see
/// [`Editor::sync`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sync {
    /// Nothing it doesn't know.
    Unchanged,
    /// The selection moved, or the text changed under the input method:
    /// `InputMethodManager.updateSelection`.
    Selection,
    /// A field gained focus: `InputMethodManager.restartInput`.
    Restart,
}

/// The editable an input method sees. See the [module docs](self).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Editor {
    /// The field's text, without the composition.
    text: String,
    /// The caret and the other end of the selection, as byte indices.
    cursor: usize,
    anchor: usize,
    /// The composition in progress, at the selection.
    composing: String,
    /// Edits sent to the field and not yet handled by it.
    in_flight: u32,
    /// Whether a text field has focus.
    focused: bool,
}

/// The byte index `units` UTF-16 units into `s` (clamped to its end, and kept
/// off the middle of a surrogate pair).
fn byte_at(s: &str, units: usize) -> usize {
    let mut n = 0;
    for (i, c) in s.char_indices() {
        if n >= units {
            return i;
        }
        n += c.len_utf16();
    }
    s.len()
}

/// The UTF-16 length of `s`.
fn len16(s: &str) -> usize {
    s.encode_utf16().count()
}

/// The last `units` UTF-16 units of `s`, whole characters only.
fn tail16(s: &str, units: usize) -> &str {
    let mut n = 0;
    let mut start = s.len();
    for (i, c) in s.char_indices().rev() {
        if n + c.len_utf16() > units {
            break;
        }
        n += c.len_utf16();
        start = i;
    }
    &s[start..]
}

/// The first `units` UTF-16 units of `s`, whole characters only.
fn head16(s: &str, units: usize) -> &str {
    let mut n = 0;
    for (i, c) in s.char_indices() {
        if n + c.len_utf16() > units {
            return &s[..i];
        }
        n += c.len_utf16();
    }
    s
}

impl Editor {
    /// No field: what [`Default`] gives, as a constant.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) const EMPTY: Editor = Editor {
        text: String::new(),
        cursor: 0,
        anchor: 0,
        composing: String::new(),
        in_flight: 0,
        focused: false,
    };

    /// Whether a text field (one that shares its text) has focus.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn is_focused(&self) -> bool {
        self.focused
    }

    /// The selection, as byte indices `(start, end)` into the field's text.
    fn selection(&self) -> (usize, usize) {
        (self.cursor.min(self.anchor), self.cursor.max(self.anchor))
    }

    /// The editable's text before the selection (the composition included,
    /// which sits there), the selected text, and the text after it.
    fn parts(&self) -> (String, &str, &str) {
        let (s, e) = self.selection();
        let mut before = self.text[..s].to_string();
        if !self.composing.is_empty() {
            before.push_str(&self.composing);
            return (before, "", &self.text[e..]);
        }
        (before, &self.text[s..e], &self.text[e..])
    }

    /// The whole editable, as the input method sees it.
    #[cfg(test)]
    pub(crate) fn editable(&self) -> String {
        let (before, selected, after) = self.parts();
        format!("{before}{selected}{after}")
    }

    /// The selection in the editable, in UTF-16 units: `(start, end)`.
    pub(crate) fn selection16(&self) -> (i32, i32) {
        let (before, selected, _) = self.parts();
        let start = len16(&before) as i32;
        (start, start + len16(selected) as i32)
    }

    /// The composing region in the editable, in UTF-16 units, or `(-1, -1)`.
    pub(crate) fn composing16(&self) -> (i32, i32) {
        if self.composing.is_empty() {
            return (-1, -1);
        }
        let start = len16(&self.text[..self.selection().0]) as i32;
        (start, start + len16(&self.composing) as i32)
    }

    /// Whether a composition is in progress.
    pub(crate) fn is_composing(&self) -> bool {
        !self.composing.is_empty()
    }

    /// `getTextBeforeCursor`: up to `n` UTF-16 units before the selection.
    pub(crate) fn text_before(&self, n: usize) -> String {
        tail16(&self.parts().0, n).to_string()
    }

    /// `getTextAfterCursor`: up to `n` UTF-16 units after the selection.
    pub(crate) fn text_after(&self, n: usize) -> String {
        head16(self.parts().2, n).to_string()
    }

    /// `getSelectedText`: the selection, or `None` when it's empty.
    pub(crate) fn selected(&self) -> Option<String> {
        let (_, selected, _) = self.parts();
        (!selected.is_empty()).then(|| selected.to_string())
    }

    /// `getCursorCapsMode`: which of the `req` modes apply at the caret, as
    /// `TextUtils.getCapsMode` decides: words after a space or at the start,
    /// sentences after `.`, `!` or `?` (and spaces) or at the start.
    pub(crate) fn caps_mode(&self, req: i32) -> i32 {
        let before = self.parts().0;
        let mut mode = req & CAP_MODE_CHARACTERS;
        let last = before.chars().next_back();
        if last.is_none_or(char::is_whitespace) {
            mode |= req & CAP_MODE_WORDS;
        }
        let trimmed = before.trim_end();
        if trimmed.is_empty()
            || (trimmed.ends_with(['.', '!', '?']) && trimmed.len() < before.len())
        {
            mode |= req & CAP_MODE_SENTENCES;
        }
        mode
    }

    /// Note `events` as sent to the field, and return them.
    fn send(&mut self, events: Vec<Ime>) -> Vec<Ime> {
        self.in_flight += events.len() as u32;
        events
    }

    /// The field is handling one of the events sent. Call it before
    /// delivering the event, so a [`sync`](Self::sync) it causes counts it.
    pub(crate) fn handled(&mut self) {
        self.in_flight = self.in_flight.saturating_sub(1);
    }

    /// Replace the selection (or the composition) with `text`, the caret
    /// after it.
    fn replace(&mut self, text: &str) {
        let (s, e) = self.selection();
        self.text.replace_range(s..e, text);
        self.cursor = s + text.len();
        self.anchor = self.cursor;
        self.composing.clear();
    }

    /// `commitText`: `text` replaces the composition, or the selection.
    pub(crate) fn commit(&mut self, text: &str) -> Vec<Ime> {
        self.replace(text);
        self.send(vec![Ime::Commit(text.to_string())])
    }

    /// `setComposingText`: `text` is the composition now (empty ends it).
    pub(crate) fn set_composing(&mut self, text: &str) -> Vec<Ime> {
        if text == self.composing {
            return Vec::new();
        }
        self.composing = text.to_string();
        let cursor = (!text.is_empty()).then_some((text.len(), text.len()));
        self.send(vec![Ime::Preedit {
            text: text.to_string(),
            cursor,
        }])
    }

    /// `finishComposingText`: the composition becomes part of the text.
    pub(crate) fn finish_composing(&mut self) -> Vec<Ime> {
        if self.composing.is_empty() {
            return Vec::new();
        }
        let text = std::mem::take(&mut self.composing);
        self.replace(&text);
        self.send(vec![Ime::Commit(text)])
    }

    /// Delete `before` bytes before the selection and `after` bytes after it.
    fn delete_bytes(&mut self, before: usize, after: usize) -> Vec<Ime> {
        if before == 0 && after == 0 {
            return Vec::new();
        }
        let (s, e) = self.selection();
        self.text.replace_range(e..e + after, "");
        self.text.replace_range(s - before..s, "");
        self.cursor -= before;
        self.anchor -= before;
        self.send(vec![Ime::DeleteSurrounding { before, after }])
    }

    /// `deleteSurroundingText`: delete `before` UTF-16 units before the
    /// selection and `after` after it (the composition stays).
    pub(crate) fn delete_surrounding(&mut self, before: usize, after: usize) -> Vec<Ime> {
        let (s, e) = self.selection();
        let head = &self.text[..s];
        let b = head.len() - tail16(head, before).len();
        let a = head16(&self.text[e..], after).len();
        self.delete_bytes(head.len() - b, a)
    }

    /// `deleteSurroundingTextInCodePoints`: the same, in characters.
    pub(crate) fn delete_surrounding_chars(&mut self, before: usize, after: usize) -> Vec<Ime> {
        let (s, e) = self.selection();
        let b: usize = self.text[..s]
            .chars()
            .rev()
            .take(before)
            .map(char::len_utf8)
            .sum();
        let a: usize = self.text[e..].chars().take(after).map(char::len_utf8).sum();
        self.delete_bytes(b, a)
    }

    /// `setComposingRegion`: make the text from `start` to `end` (UTF-16
    /// units into the editable) the composition, as an input method does when
    /// the caret comes back to a word. Stipple composes only at the caret, so
    /// the region is taken out of the text and handed back as the preedit;
    /// that needs the region to touch the caret. An empty region just ends the
    /// composition.
    pub(crate) fn set_composing_region(&mut self, start: usize, end: usize) -> Vec<Ime> {
        let mut out = self.finish_composing();
        let (start, end) = (start.min(end), start.max(end));
        if start == end || self.cursor != self.anchor {
            return out;
        }
        let (s, e) = (byte_at(&self.text, start), byte_at(&self.text, end));
        if !(s <= self.cursor && self.cursor <= e) {
            return out;
        }
        let word = self.text[s..e].to_string();
        out.extend(self.delete_bytes(self.cursor - s, e - self.cursor));
        out.extend(self.set_composing(&word));
        out
    }

    /// `setSelection`: how far (in characters, negative for left) the caret
    /// must move from where it is to reach `start`, and how many characters
    /// to select from there (Shift+Right), given the editable's text. The
    /// backend sends those as arrow keys. `None` if the selection isn't a
    /// plain caret or a composition is in progress, which arrows can't
    /// reproduce faithfully.
    pub(crate) fn set_selection(&mut self, start: usize, end: usize) -> Option<(i64, usize)> {
        if self.cursor != self.anchor || self.is_composing() {
            return None;
        }
        let (start, end) = (start.min(end), start.max(end));
        let (s, e) = (byte_at(&self.text, start), byte_at(&self.text, end));
        let chars = |a: usize, b: usize| self.text[a..b].chars().count() as i64;
        let moved = if s >= self.cursor {
            chars(self.cursor, s)
        } else {
            -chars(s, self.cursor)
        };
        let select = chars(s, e) as usize;
        self.anchor = s;
        self.cursor = e;
        Some((moved, select))
    }

    /// The field told the backend what it holds now (`None`: no text field
    /// has focus, or one that keeps its text from the input method). Adopts
    /// it, and says what the input method must hear. While edits are on the
    /// way to the field, its state is older than this editor's and is
    /// ignored.
    pub(crate) fn sync(&mut self, state: Option<&ImeState>) -> Sync {
        let Some(state) = state else {
            let had = *self != Editor::default();
            *self = Editor::default();
            return if had { Sync::Restart } else { Sync::Unchanged };
        };
        if self.in_flight > 0 {
            return Sync::Unchanged;
        }
        // A field that gains focus is new to the input method; one whose text
        // changed under it (a key typed, the app's own edit) only needs to
        // hear where the selection is now, as Android's own fields do.
        let gained = !self.focused;
        let text_changed = state.surrounding != self.text;
        self.focused = true;
        let selection_changed = state.cursor != self.cursor || state.anchor != self.anchor;
        self.text = state.surrounding.clone();
        self.cursor = state.cursor.min(self.text.len());
        self.anchor = state.anchor.min(self.text.len());
        if text_changed {
            self.composing.clear();
        }
        if gained {
            Sync::Restart
        } else if text_changed || selection_changed {
            Sync::Selection
        } else {
            Sync::Unchanged
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Rect;

    fn state(text: &str, cursor: usize) -> ImeState {
        ImeState {
            cursor_area: Rect::from_xywh(0.0, 0.0, 2.0, 10.0),
            surrounding: text.to_string(),
            cursor,
            anchor: cursor,
        }
    }

    fn editor(text: &str, cursor: usize) -> Editor {
        let mut e = Editor::default();
        assert_eq!(e.sync(Some(&state(text, cursor))), Sync::Restart);
        e
    }

    #[test]
    fn utf16_helpers_count_surrogate_pairs() {
        let s = "a🎉b";
        assert_eq!(len16(s), 4);
        assert_eq!(byte_at(s, 1), 1);
        assert_eq!(byte_at(s, 2), 5, "never inside the pair");
        assert_eq!(byte_at(s, 3), 5);
        assert_eq!(byte_at(s, 9), s.len());
        assert_eq!(tail16(s, 2), "b", "a pair that doesn't fit is left out");
        assert_eq!(tail16(s, 3), "🎉b");
        assert_eq!(head16(s, 2), "a");
        assert_eq!(head16(s, 3), "a🎉");
    }

    #[test]
    fn composing_then_committing() {
        let mut e = editor("ab", 1);
        assert_eq!(
            e.set_composing("に"),
            [Ime::Preedit {
                text: "に".into(),
                cursor: Some((3, 3))
            }]
        );
        assert_eq!(e.editable(), "aにb");
        assert_eq!(e.composing16(), (1, 2));
        assert_eq!(e.selection16(), (2, 2));
        assert_eq!(e.text_before(10), "aに");
        assert_eq!(e.text_after(10), "b");
        assert_eq!(e.set_composing("に"), [], "no change, nothing sent");
        assert_eq!(e.commit("日"), [Ime::Commit("日".into())]);
        assert_eq!(e.editable(), "a日b");
        assert_eq!(e.composing16(), (-1, -1));
        assert_eq!(e.selection16(), (2, 2));
    }

    #[test]
    fn finishing_commits_the_composition() {
        let mut e = editor("", 0);
        e.set_composing("hel");
        assert_eq!(e.finish_composing(), [Ime::Commit("hel".into())]);
        assert_eq!(e.finish_composing(), [], "nothing left to finish");
        assert_eq!(e.editable(), "hel");
        // An empty composition ends it without committing.
        e.set_composing("x");
        assert_eq!(
            e.set_composing(""),
            [Ime::Preedit {
                text: String::new(),
                cursor: None
            }]
        );
        assert_eq!(e.editable(), "hel");
    }

    #[test]
    fn deleting_around_the_caret_counts_utf16_units() {
        let mut e = editor("x🎉y", 5);
        // The emoji is two UTF-16 units, four bytes.
        assert_eq!(
            e.delete_surrounding(2, 1),
            [Ime::DeleteSurrounding {
                before: 4,
                after: 1
            }]
        );
        assert_eq!(e.editable(), "x");
        assert_eq!(e.selection16(), (1, 1));
        let mut e = editor("é🎉", 6);
        assert_eq!(
            e.delete_surrounding_chars(2, 0),
            [Ime::DeleteSurrounding {
                before: 6,
                after: 0
            }]
        );
        assert_eq!(e.editable(), "");
        assert_eq!(e.delete_surrounding(3, 3), [], "nothing to delete");
    }

    #[test]
    fn a_composing_region_around_the_caret_becomes_the_preedit() {
        let mut e = editor("one two", 7);
        assert_eq!(
            e.set_composing_region(4, 7),
            [
                Ime::DeleteSurrounding {
                    before: 3,
                    after: 0
                },
                Ime::Preedit {
                    text: "two".into(),
                    cursor: Some((3, 3))
                }
            ]
        );
        assert_eq!(e.editable(), "one two");
        assert_eq!(e.composing16(), (4, 7));
        // A region away from the caret can't be composed at it.
        let mut e = editor("one two", 7);
        assert_eq!(e.set_composing_region(0, 3), []);
    }

    #[test]
    fn selection_moves_become_arrow_counts() {
        let mut e = editor("héllo", 6);
        assert_eq!(e.set_selection(1, 1), Some((-4, 0)));
        assert_eq!(e.selection16(), (1, 1));
        assert_eq!(e.set_selection(2, 4), Some((1, 2)));
        assert_eq!(e.selected().as_deref(), Some("ll"));
        assert_eq!(e.set_selection(0, 0), None, "from a selection, no");
    }

    #[test]
    fn caps_mode_follows_sentences_and_words() {
        let all = CAP_MODE_CHARACTERS | CAP_MODE_WORDS | CAP_MODE_SENTENCES;
        assert_eq!(editor("", 0).caps_mode(all), all, "the start is everything");
        assert_eq!(
            editor("Hi. ", 4).caps_mode(CAP_MODE_SENTENCES),
            CAP_MODE_SENTENCES
        );
        assert_eq!(editor("Hi.", 3).caps_mode(CAP_MODE_SENTENCES), 0);
        assert_eq!(editor("Hi there", 8).caps_mode(CAP_MODE_WORDS), 0);
        assert_eq!(
            editor("Hi there ", 9).caps_mode(CAP_MODE_WORDS | CAP_MODE_SENTENCES),
            CAP_MODE_WORDS
        );
    }

    #[test]
    fn the_fields_state_wins_once_its_edits_are_in() {
        let mut e = editor("", 0);
        e.commit("a");
        e.commit("b");
        assert_eq!(e.editable(), "ab");
        // The field reports having handled only the first so far: stale.
        e.handled();
        assert_eq!(e.sync(Some(&state("a", 1))), Sync::Unchanged);
        assert_eq!(e.editable(), "ab");
        e.handled();
        assert_eq!(e.sync(Some(&state("ab", 2))), Sync::Unchanged);
        // A caret the user moved, then text the app replaced.
        assert_eq!(e.sync(Some(&state("ab", 0))), Sync::Selection);
        assert_eq!(e.sync(Some(&state("xyz", 0))), Sync::Selection);
        assert_eq!(e.editable(), "xyz");
        assert_eq!(e.sync(None), Sync::Restart, "focus left the field");
        assert_eq!(e.sync(None), Sync::Unchanged);
    }
}
