//! The `org.a11y.atspi.Text` reading of a node's text: offsets in characters,
//! and the character / word / sentence / line around an offset, as a screen
//! reader asks for them while it reads by word or line.

/// `AtspiTextBoundaryType`, the older `Get…AtOffset` calls' unit.
const BOUNDARY_CHAR: u32 = 0;
const BOUNDARY_WORD_START: u32 = 1;
const BOUNDARY_WORD_END: u32 = 2;
const BOUNDARY_SENTENCE_START: u32 = 3;
const BOUNDARY_SENTENCE_END: u32 = 4;

/// `AtspiTextGranularity`, `GetStringAtOffset`'s unit.
const GRANULARITY_CHAR: u32 = 0;
const GRANULARITY_WORD: u32 = 1;
const GRANULARITY_SENTENCE: u32 = 2;

/// The number of characters in `text`.
pub(super) fn char_count(text: &str) -> i32 {
    text.chars().count() as i32
}

/// The characters `[start, end)` of `text`; an `end` of -1 (or past the end)
/// means to the end.
pub(super) fn slice(text: &str, start: i32, end: i32) -> String {
    let n = char_count(text);
    let start = start.clamp(0, n);
    let end = if end < 0 { n } else { end.clamp(start, n) };
    text.chars()
        .skip(start as usize)
        .take((end - start) as usize)
        .collect()
}

/// The character at `offset` (its code point), or 0 past the end.
pub(super) fn char_at(text: &str, offset: i32) -> i32 {
    usize::try_from(offset)
        .ok()
        .and_then(|o| text.chars().nth(o))
        .map_or(0, |c| c as i32)
}

/// What a unit of text is made of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unit {
    Char,
    /// A word and the space after it (the `WORD_START` boundary).
    WordAndSpace,
    /// A word alone.
    Word,
    Sentence,
    Line,
}

/// The `[start, end)` character range of the `unit` that holds `offset`.
fn range_at(chars: &[char], offset: usize, unit: Unit) -> (usize, usize) {
    let n = chars.len();
    let offset = offset.min(n);
    match unit {
        Unit::Char => (offset, (offset + 1).min(n)),
        Unit::Word | Unit::WordAndSpace => {
            let word = |c: char| c.is_alphanumeric() || c == '\'' || c == '_';
            // Back to the start of the word (or of the spaces before it).
            let mut start = offset;
            if start < n && !word(chars[start]) && unit == Unit::WordAndSpace {
                // In the spaces after a word: that word's unit.
                while start > 0 && !word(chars[start - 1]) {
                    start -= 1;
                }
            }
            while start > 0 && word(chars[start - 1]) {
                start -= 1;
            }
            let mut end = start;
            while end < n && word(chars[end]) {
                end += 1;
            }
            if end == start {
                // Not in a word: the run of other characters.
                while end < n && !word(chars[end]) {
                    end += 1;
                }
                return (start, end.max(offset.min(n)));
            }
            if unit == Unit::WordAndSpace {
                while end < n && !word(chars[end]) {
                    end += 1;
                }
            }
            (start, end)
        }
        Unit::Sentence => {
            let ends = |i: usize| matches!(chars[i], '.' | '!' | '?' | '\n');
            let mut start = offset;
            while start > 0 && !ends(start - 1) {
                start -= 1;
            }
            // The space after the previous sentence's stop belongs to it.
            while start < offset && chars[start] == ' ' {
                start += 1;
            }
            let mut end = offset;
            while end < n && !ends(end) {
                end += 1;
            }
            // The stop and the spaces after it.
            if end < n {
                end += 1;
            }
            while end < n && chars[end] == ' ' {
                end += 1;
            }
            (start, end)
        }
        Unit::Line => {
            let mut start = offset;
            while start > 0 && chars[start - 1] != '\n' {
                start -= 1;
            }
            let mut end = offset;
            while end < n && chars[end] != '\n' {
                end += 1;
            }
            if end < n {
                end += 1; // the newline ends the line
            }
            (start, end)
        }
    }
}

fn boundary_unit(boundary: u32) -> Unit {
    match boundary {
        BOUNDARY_CHAR => Unit::Char,
        BOUNDARY_WORD_START | BOUNDARY_WORD_END => Unit::WordAndSpace,
        BOUNDARY_SENTENCE_START | BOUNDARY_SENTENCE_END => Unit::Sentence,
        _ => Unit::Line,
    }
}

fn granularity_unit(granularity: u32) -> Unit {
    match granularity {
        GRANULARITY_CHAR => Unit::Char,
        GRANULARITY_WORD => Unit::Word,
        GRANULARITY_SENTENCE => Unit::Sentence,
        _ => Unit::Line, // line and paragraph
    }
}

/// Which unit relative to the one at the offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Which {
    Before,
    At,
    After,
}

/// `GetText{Before,At,After}Offset`: the text, start and end of the unit.
pub(super) fn text_at(text: &str, offset: i32, boundary: u32, which: Which) -> (String, i32, i32) {
    unit_at(text, offset, boundary_unit(boundary), which)
}

/// `GetStringAtOffset`.
pub(super) fn string_at(text: &str, offset: i32, granularity: u32) -> (String, i32, i32) {
    unit_at(text, offset, granularity_unit(granularity), Which::At)
}

fn unit_at(text: &str, offset: i32, unit: Unit, which: Which) -> (String, i32, i32) {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let offset = usize::try_from(offset).unwrap_or(0).min(n);
    let (mut start, mut end) = range_at(&chars, offset, unit);
    match which {
        Which::At => {}
        Which::Before if start == 0 => (start, end) = (0, 0),
        Which::Before => (start, end) = range_at(&chars, start - 1, unit),
        Which::After if end >= n => (start, end) = (n, n),
        Which::After => (start, end) = range_at(&chars, end, unit),
    }
    let s: String = chars[start..end].iter().collect();
    (s, start as i32, end as i32)
}

/// The change from `old` to `new` as one deletion and one insertion between
/// their common start and end: `(offset, deleted, inserted)` in characters.
pub(super) fn change(old: &str, new: &str) -> (i32, String, String) {
    let a: Vec<char> = old.chars().collect();
    let b: Vec<char> = new.chars().collect();
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let deleted = a[prefix..a.len() - suffix].iter().collect();
    let inserted = b[prefix..b.len() - suffix].iter().collect();
    (prefix as i32, deleted, inserted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slices_by_character() {
        assert_eq!(slice("héllo", 1, 3), "él");
        assert_eq!(slice("héllo", 2, -1), "llo");
        assert_eq!(slice("héllo", -4, 99), "héllo");
        assert_eq!(slice("abc", 2, 1), "");
        assert_eq!(char_count("héllo"), 5);
        assert_eq!(char_at("héllo", 1), 'é' as i32);
        assert_eq!(char_at("héllo", 9), 0);
    }

    #[test]
    fn words_lines_and_sentences_around_an_offset() {
        let t = "Rows tapped: 12";
        assert_eq!(
            text_at(t, 2, BOUNDARY_WORD_START, Which::At),
            ("Rows ".into(), 0, 5)
        );
        assert_eq!(
            text_at(t, 4, BOUNDARY_WORD_START, Which::At),
            ("Rows ".into(), 0, 5)
        );
        assert_eq!(
            text_at(t, 6, BOUNDARY_WORD_START, Which::At),
            ("tapped: ".into(), 5, 13)
        );
        assert_eq!(
            text_at(t, 6, BOUNDARY_WORD_START, Which::After),
            ("12".into(), 13, 15)
        );
        assert_eq!(
            text_at(t, 6, BOUNDARY_WORD_START, Which::Before),
            ("Rows ".into(), 0, 5)
        );
        assert_eq!(
            text_at(t, 0, BOUNDARY_WORD_START, Which::Before),
            ("".into(), 0, 0)
        );
        assert_eq!(string_at(t, 7, GRANULARITY_WORD), ("tapped".into(), 5, 11));
        assert_eq!(text_at(t, 3, BOUNDARY_CHAR, Which::At), ("s".into(), 3, 4));
        assert_eq!(
            text_at(t, 3, BOUNDARY_CHAR, Which::After),
            (" ".into(), 4, 5)
        );
        let lines = "one\ntwo\nthree";
        assert_eq!(text_at(lines, 5, 5, Which::At), ("two\n".into(), 4, 8));
        assert_eq!(text_at(lines, 5, 5, Which::After), ("three".into(), 8, 13));
        assert_eq!(string_at(lines, 0, 3), ("one\n".into(), 0, 4));
        let s = "Hi there. Bye now!";
        assert_eq!(
            string_at(s, 12, GRANULARITY_SENTENCE),
            ("Bye now!".into(), 10, 18)
        );
        assert_eq!(
            string_at(s, 2, GRANULARITY_SENTENCE),
            ("Hi there. ".into(), 0, 10)
        );
        // An empty text has an empty unit everywhere.
        assert_eq!(
            text_at("", 0, BOUNDARY_WORD_START, Which::At),
            ("".into(), 0, 0)
        );
    }

    #[test]
    fn a_change_is_one_deletion_and_one_insertion() {
        assert_eq!(change("abc", "abXc"), (2, "".into(), "X".into()));
        assert_eq!(change("abXc", "abc"), (2, "X".into(), "".into()));
        assert_eq!(change("héllo", "hallo"), (1, "é".into(), "a".into()));
        assert_eq!(change("", "hi"), (0, "".into(), "hi".into()));
        assert_eq!(change("aa", "aaa"), (2, "".into(), "a".into()));
    }
}
