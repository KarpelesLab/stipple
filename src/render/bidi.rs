//! Bidirectional text: the paragraph direction, the Unicode Bidirectional
//! Algorithm (UAX #9) run through `oxideav-scribe`, and visual caret movement.
//!
//! Text is stored and edited in logical order (the order it's typed and read).
//! Before it's drawn, each paragraph goes through scribe's UAX #9 pipeline,
//! which resolves an embedding level per character; each display line is then
//! cut into runs of one level and the runs are put in visual order (rule L2).
//! [`TextLayout`](crate::render::TextLayout) does the shaping and geometry; this
//! module holds the parts that don't need a font.

use oxideav_scribe::bidi::{
    BidiClass, bidi_class, process_paragraph_classes_with_brackets, reorder_line,
    reset_trailing_levels,
};

/// The base direction of a paragraph of text.
///
/// It decides where a line starts (the left edge for left-to-right, the right
/// edge for right-to-left), how neutral characters between runs of different
/// directions resolve, and which way the lines of a paragraph are aligned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TextDirection {
    /// Take each paragraph's direction from its first strong character (UAX #9
    /// rules P2 and P3): Hebrew or Arabic makes it right-to-left, Latin
    /// left-to-right. A paragraph with no strong character is left-to-right.
    #[default]
    Auto,
    /// Left-to-right, whatever the text.
    Ltr,
    /// Right-to-left, whatever the text.
    Rtl,
}

impl TextDirection {
    /// The UAX #9 paragraph level this forces, or `None` to detect it.
    pub(crate) fn base_level(self) -> Option<u8> {
        match self {
            TextDirection::Auto => None,
            TextDirection::Ltr => Some(0),
            TextDirection::Rtl => Some(1),
        }
    }

    /// Whether a paragraph of `text` runs right to left under this direction.
    /// Only the first paragraph (up to the first `\n`) is looked at.
    pub fn is_rtl_for(self, text: &str) -> bool {
        match self {
            TextDirection::Ltr => false,
            TextDirection::Rtl => true,
            TextDirection::Auto => {
                let para = text.split('\n').next().unwrap_or("");
                needs_bidi(para, self) && oxideav_scribe::bidi::paragraph_level(para) == 1
            }
        }
    }
}

/// Whether `para` needs the bidi algorithm at all: true for a right-to-left
/// paragraph or one with a character that can start a right-to-left run.
/// Plain left-to-right text (all of Latin, Greek, Cyrillic, CJK) skips it.
pub(crate) fn needs_bidi(para: &str, direction: TextDirection) -> bool {
    if direction == TextDirection::Rtl {
        return true;
    }
    para.chars().any(|c| {
        // Everything below the Hebrew block is left-to-right or neutral.
        (c as u32) >= 0x0590
            && matches!(
                bidi_class(c),
                BidiClass::R
                    | BidiClass::AL
                    | BidiClass::AN
                    | BidiClass::RLE
                    | BidiClass::RLO
                    | BidiClass::RLI
                    | BidiClass::FSI
            )
    })
}

/// The resolved bidi state of one paragraph: its level and the level of each
/// character, ready to be cut into lines.
#[derive(Clone, Debug)]
pub(crate) struct ParagraphLevels {
    /// 0 for a left-to-right paragraph, 1 for right-to-left.
    pub base: u8,
    classes: Vec<BidiClass>,
    /// Resolved level per character (before the per-line rule L1).
    levels: Vec<u8>,
}

impl ParagraphLevels {
    /// Run UAX #9 (P to I, with bracket pairs) over one paragraph.
    pub fn new(para: &str, direction: TextDirection) -> Self {
        let chars: Vec<char> = para.chars().collect();
        if !needs_bidi(para, direction) {
            return Self {
                base: 0,
                classes: Vec::new(),
                levels: vec![0; chars.len()],
            };
        }
        let classes: Vec<BidiClass> = chars.iter().copied().map(bidi_class).collect();
        let carrier =
            process_paragraph_classes_with_brackets(&classes, &chars, direction.base_level());
        Self {
            base: carrier.paragraph_level & 1,
            classes: carrier.classes,
            levels: carrier.levels,
        }
    }

    /// The levels of the characters in `range` (char indices) as one display
    /// line, with rule L1 applied: trailing whitespace and separators go back to
    /// the paragraph level.
    pub fn line_levels(&self, range: std::ops::Range<usize>) -> Vec<u8> {
        let mut levels = self.levels[range.clone()].to_vec();
        if !self.classes.is_empty() {
            reset_trailing_levels(&self.classes[range], &mut levels, self.base);
        }
        levels
    }
}

/// One display line's characters in visual order: `order[v]` is the logical
/// index of the character drawn at visual position `v`.
pub(crate) fn visual_order(levels: &[u8]) -> Vec<usize> {
    if levels.iter().all(|l| l & 1 == 0) {
        return (0..levels.len()).collect();
    }
    reorder_line(levels)
}

/// A character of a display line, listed in visual order: its logical extent
/// (`start..end`, in whatever unit the caller counts: chars or bytes) and its
/// resolved embedding level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct VisualChar {
    pub start: usize,
    pub end: usize,
    pub level: u8,
}

impl VisualChar {
    fn rtl(&self) -> bool {
        self.level & 1 == 1
    }

    /// The caret position at this character's left edge: before it if it
    /// runs left to right, after it if it runs right to left.
    fn at_left(&self) -> usize {
        if self.rtl() { self.end } else { self.start }
    }

    /// The caret position at its right edge.
    fn at_right(&self) -> usize {
        if self.rtl() { self.start } else { self.end }
    }
}

/// The logical caret position at each visual boundary of a line: entry `v` is
/// the boundary left of visual character `v`, entry `n` the line's right end.
/// `empty` is the position for a line with no characters.
///
/// A boundary between two characters belongs to the more deeply embedded one
/// (the higher level), so a number inside right-to-left text, or a Latin word
/// inside a right-to-left paragraph, keeps both of its ends. This makes every
/// logical position appear at one boundary in ordinary mixed text.
pub(crate) fn boundary_carets(vis: &[VisualChar], empty: usize) -> Vec<usize> {
    if vis.is_empty() {
        return vec![empty];
    }
    let n = vis.len();
    (0..=n)
        .map(|v| match (v.checked_sub(1).map(|l| vis[l]), vis.get(v)) {
            (Some(l), Some(r)) if l.level > r.level => l.at_right(),
            (_, Some(r)) => r.at_left(),
            (Some(l), None) => l.at_right(),
            (None, None) => empty,
        })
        .collect()
}

/// The visual boundary a caret at logical position `caret` is drawn at.
///
/// Usually the one boundary [`boundary_carets`] gives that position. If none
/// does (unusual nesting), the caret goes to the leading edge of the character
/// after it (the left side of a left-to-right character, the right side of a
/// right-to-left one), or the trailing edge of the character before it.
pub(crate) fn caret_boundary(vis: &[VisualChar], carets: &[usize], caret: usize) -> usize {
    if let Some(v) = carets.iter().position(|&c| c == caret) {
        return v;
    }
    if let Some(v) = vis.iter().position(|c| c.start == caret) {
        return if vis[v].rtl() { v + 1 } else { v };
    }
    if let Some(v) = vis.iter().position(|c| c.end == caret) {
        return if vis[v].rtl() { v } else { v + 1 };
    }
    0
}

/// One display line's characters in visual order, counted in chars, from
/// their (line-level, after L1) `levels`.
pub(crate) fn visual_chars(levels: &[u8]) -> Vec<VisualChar> {
    visual_order(levels)
        .into_iter()
        .map(|i| VisualChar {
            start: i,
            end: i + 1,
            level: levels[i],
        })
        .collect()
}

/// The caret position one step left (`right == false`) or right of logical
/// char index `caret` on a line, moving visually: through a right-to-left run
/// the Left key moves forward in the text. `None` at the line's visual edge.
pub(crate) fn visual_step(levels: &[u8], caret: usize, right: bool) -> Option<usize> {
    let vis = visual_chars(levels);
    let carets = boundary_carets(&vis, 0);
    let n = vis.len();
    let mut v = caret_boundary(&vis, &carets, caret.min(n));
    // Should two boundaries name the same position, keep stepping until the
    // caret actually moves.
    loop {
        if right {
            if v >= n {
                return None;
            }
            v += 1;
        } else {
            if v == 0 {
                return None;
            }
            v -= 1;
        }
        if carets[v] != caret {
            return Some(carets[v]);
        }
    }
}

/// The byte index one visual step left or right of `caret` (a byte index into
/// `line`, which holds no `\n`), with the paragraph direction `direction`.
/// `None` at the line's visual edge.
pub(crate) fn visual_step_in(
    line: &str,
    caret: usize,
    direction: TextDirection,
    right: bool,
) -> Option<usize> {
    let offsets: Vec<usize> = line.char_indices().map(|(i, _)| i).collect();
    let n = offsets.len();
    let byte_of = |c: usize| offsets.get(c).copied().unwrap_or(line.len());
    let ci = offsets.partition_point(|&o| o < caret);
    let para = ParagraphLevels::new(line, direction);
    let levels = para.line_levels(0..n);
    visual_step(&levels, ci, right).map(byte_of)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn levels(s: &str, dir: TextDirection) -> Vec<u8> {
        let p = ParagraphLevels::new(s, dir);
        p.line_levels(0..s.chars().count())
    }

    fn visual(s: &str, dir: TextDirection) -> String {
        let chars: Vec<char> = s.chars().collect();
        let l = levels(s, dir);
        visual_order(&l).iter().map(|&i| chars[i]).collect()
    }

    #[test]
    fn mixed_line_reverses_only_the_hebrew() {
        // The number after the Hebrew joins its run (UAX #9 W7 keeps it a
        // European number, at level 2 inside the right-to-left run).
        assert_eq!(
            visual("abc אבג 123 def", TextDirection::Auto),
            "abc 123 גבא def"
        );
        // In a right-to-left paragraph the runs swap sides, and the number
        // keeps its own left-to-right order.
        assert_eq!(visual("אבג 123 def", TextDirection::Auto), "def 123 גבא");
        assert_eq!(visual("abc", TextDirection::Rtl), "abc");
    }

    #[test]
    fn auto_direction_follows_the_first_strong_character() {
        assert!(!TextDirection::Auto.is_rtl_for("abc אבג"));
        assert!(TextDirection::Auto.is_rtl_for("123 אבג abc"));
        assert!(TextDirection::Auto.is_rtl_for("مرحبا"));
        assert!(!TextDirection::Auto.is_rtl_for("123"));
        assert!(TextDirection::Rtl.is_rtl_for("abc"));
        assert!(!TextDirection::Ltr.is_rtl_for("אבג"));
    }

    /// Every caret position is reachable exactly once walking right from the
    /// left edge, and the walk ends at the right edge.
    #[test]
    fn visual_steps_visit_every_position() {
        for (s, dir) in [
            ("abc אבג 123 def", TextDirection::Auto),
            ("אבג abc", TextDirection::Auto),
            ("abc", TextDirection::Rtl),
            ("שלום", TextDirection::Auto),
        ] {
            let l = levels(s, dir);
            let n = l.len();
            let vis = visual_chars(&l);
            let mut seen = vec![boundary_carets(&vis, 0)[0]];
            let mut caret = seen[0];
            while let Some(next) = visual_step(&l, caret, true) {
                caret = next;
                seen.push(caret);
                assert!(seen.len() <= n + 1, "{s}: cycle {seen:?}");
            }
            let mut sorted = seen.clone();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(sorted.len(), n + 1, "{s}: {seen:?}");
            // And walking back left retraces the same path.
            let mut back = vec![caret];
            while let Some(prev) = visual_step(&l, caret, false) {
                caret = prev;
                back.push(caret);
                assert!(back.len() <= n + 1, "{s}: cycle {back:?}");
            }
            back.reverse();
            assert_eq!(back, seen, "{s}");
        }
    }

    #[test]
    fn left_moves_forward_through_hebrew() {
        // "abc אבג": a0 b1 c2 ' '3 א4 ב5 ג6. Right from after the space enters
        // the Hebrew at its visual left end, which is its logical end.
        let l = levels("abc אבג", TextDirection::Auto);
        assert_eq!(visual_step(&l, 3, true), Some(7));
        assert_eq!(visual_step(&l, 7, true), Some(6));
        assert_eq!(visual_step(&l, 5, true), Some(4));
        assert_eq!(visual_step(&l, 4, true), None);
        // A right-to-left paragraph: Left from the start moves forward.
        let l = levels("שלום", TextDirection::Auto);
        assert_eq!(visual_step(&l, 0, false), Some(1));
        assert_eq!(visual_step(&l, 0, true), None);
    }

    #[test]
    fn visual_step_in_works_on_bytes() {
        // Hebrew letters are two bytes each.
        assert_eq!(
            visual_step_in("שלום", 0, TextDirection::Auto, false),
            Some(2)
        );
        assert_eq!(visual_step_in("ab", 1, TextDirection::Auto, true), Some(2));
        assert_eq!(visual_step_in("", 0, TextDirection::Auto, true), None);
    }
}
