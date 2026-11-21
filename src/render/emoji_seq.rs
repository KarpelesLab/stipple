//! Emoji sequences: the runs of characters a color emoji font draws as one
//! glyph (UTS #51): a pictograph with its presentation selector, skin tone
//! modifier, tag characters and zero-width-joined followers; a pair of
//! regional indicators (a flag); a keycap; and a lone emoji that draws as
//! an emoji by default (U+1F600, not U+263A without its selector).
//!
//! Shaping finds them so a whole sequence goes to one face, the color font
//! that has its first character, rather than each character to the first
//! face that has it (the ZWJ from the text font, the selector from a symbol
//! font), which would keep the font's ligature from forming.

use std::ops::Range;

pub(crate) const ZWJ: char = '\u{200D}';
pub(crate) const VS15: char = '\u{FE0E}';
pub(crate) const VS16: char = '\u{FE0F}';
const KEYCAP: char = '\u{20E3}';

/// A regional indicator letter (two make a flag).
pub(crate) fn is_regional_indicator(c: char) -> bool {
    ('\u{1F1E6}'..='\u{1F1FF}').contains(&c)
}

/// A Fitzpatrick skin tone modifier.
pub(crate) fn is_modifier(c: char) -> bool {
    ('\u{1F3FB}'..='\u{1F3FF}').contains(&c)
}

/// A tag character (subdivision flags: England, Scotland, Wales).
fn is_tag(c: char) -> bool {
    ('\u{E0020}'..='\u{E007F}').contains(&c)
}

/// The characters that join or select within a sequence and draw nothing
/// of their own when the font doesn't fold them into a ligature.
pub(crate) fn is_invisible(c: char) -> bool {
    c == ZWJ || c == VS15 || c == VS16 || is_tag(c)
}

/// Roughly Unicode's `Extended_Pictographic`: what can start a sequence
/// or follow a ZWJ in one.
pub(crate) fn is_pictographic(c: char) -> bool {
    let u = c as u32;
    matches!(u,
        0x00A9 | 0x00AE | 0x203C | 0x2049 | 0x2122 | 0x2139
        | 0x2194..=0x2199 | 0x21A9..=0x21AA | 0x231A..=0x231B | 0x2328 | 0x2388
        | 0x23CF | 0x23E9..=0x23F3 | 0x23F8..=0x23FA | 0x24C2 | 0x25AA..=0x25AB
        | 0x25B6 | 0x25C0 | 0x25FB..=0x25FE | 0x2600..=0x27BF | 0x2934..=0x2935
        | 0x2B05..=0x2B07 | 0x2B1B..=0x2B1C | 0x2B50 | 0x2B55 | 0x3030 | 0x303D
        | 0x3297 | 0x3299 | 0x1F000..=0x1F1E5 | 0x1F200..=0x1F3FA | 0x1F400..=0x1FAFF
        | 0x1FC00..=0x1FFFD)
}

/// Roughly Unicode's `Emoji_Presentation`: pictographs drawn as emoji
/// (in color) without a selector asking for it.
fn is_emoji_presentation(c: char) -> bool {
    let u = c as u32;
    matches!(u,
        0x231A..=0x231B | 0x23E9..=0x23EC | 0x23F0 | 0x23F3 | 0x25FD..=0x25FE
        | 0x2614..=0x2615 | 0x2648..=0x2653 | 0x267F | 0x2693 | 0x26A1 | 0x26AA..=0x26AB
        | 0x26BD..=0x26BE | 0x26C4..=0x26C5 | 0x26CE | 0x26D4 | 0x26EA | 0x26F2..=0x26F3
        | 0x26F5 | 0x26FA | 0x26FD | 0x2705 | 0x270A..=0x270B | 0x2728 | 0x274C | 0x274E
        | 0x2753..=0x2755 | 0x2757 | 0x2795..=0x2797 | 0x27B0 | 0x27BF | 0x2B1B..=0x2B1C
        | 0x2B50 | 0x2B55 | 0x1F004 | 0x1F0CF | 0x1F18E | 0x1F191..=0x1F19A
        | 0x1F300..=0x1F320 | 0x1F32D..=0x1F335 | 0x1F337..=0x1F37C | 0x1F37E..=0x1F393
        | 0x1F3A0..=0x1F3CA | 0x1F3CF..=0x1F3D3 | 0x1F3E0..=0x1F3F0 | 0x1F3F4
        | 0x1F3F8..=0x1F43E | 0x1F440 | 0x1F442..=0x1F4FC | 0x1F4FF..=0x1F53D
        | 0x1F54B..=0x1F54E | 0x1F550..=0x1F567 | 0x1F57A | 0x1F595..=0x1F596 | 0x1F5A4
        | 0x1F5FB..=0x1F64F | 0x1F680..=0x1F6C5 | 0x1F6CC | 0x1F6D0..=0x1F6D2
        | 0x1F6D5..=0x1F6DF | 0x1F6EB..=0x1F6EC | 0x1F6F4..=0x1F6FC | 0x1F7E0..=0x1F7F0
        | 0x1F90C..=0x1F93A | 0x1F93C..=0x1F945 | 0x1F947..=0x1F9FF | 0x1FA70..=0x1FAFF)
}

/// The byte ranges of `text`'s emoji sequences, in order: sequences of
/// two characters or more, and lone emoji that draw as emoji by default
/// (other lone pictographs shape as any character).
pub(crate) fn sequences(text: &str) -> Vec<Range<usize>> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let end_of = |k: usize| chars.get(k).map_or(text.len(), |&(i, _)| i);
    let at = |k: usize| chars.get(k).map(|&(_, c)| c);
    let mut out = Vec::new();
    let mut k = 0;
    while k < chars.len() {
        let c = chars[k].1;
        let mut j = k + 1;
        if is_regional_indicator(c) {
            if at(j).is_some_and(is_regional_indicator) {
                j += 1;
            }
        } else if c.is_ascii_digit() || c == '#' || c == '*' {
            if at(j) == Some(VS16) && at(j + 1) == Some(KEYCAP) {
                j += 2;
            } else if at(j) == Some(KEYCAP) {
                j += 1;
            }
        } else if is_pictographic(c) {
            loop {
                while at(j).is_some_and(|n| n == VS16 || is_modifier(n) || is_tag(n)) {
                    j += 1;
                }
                if at(j) == Some(ZWJ) && at(j + 1).is_some_and(is_pictographic) {
                    j += 2;
                    continue;
                }
                break;
            }
        }
        let lone_emoji = j == k + 1 && is_emoji_presentation(c) && at(j) != Some(VS15);
        if j > k + 1 || lone_emoji {
            out.push(chars[k].0..end_of(j));
        }
        k = j;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seqs(s: &str) -> Vec<&str> {
        sequences(s).into_iter().map(|r| &s[r]).collect()
    }

    #[test]
    fn finds_each_kind_of_sequence() {
        // Skin tone, flag, keycap (with and without its selector), ZWJ
        // family, the rainbow flag, England's tag flag, a red heart.
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        let rainbow = "\u{1F3F3}\u{FE0F}\u{200D}\u{1F308}";
        let england = "\u{1F3F4}\u{E0067}\u{E0062}\u{E0065}\u{E006E}\u{E0067}\u{E007F}";
        let text = format!(
            "a\u{1F44D}\u{1F3FD} \u{1F1EF}\u{1F1F5}1\u{FE0F}\u{20E3}#\u{20E3}{family}x{rainbow}{england}\u{2764}\u{FE0F}"
        );
        assert_eq!(
            seqs(&text),
            [
                "\u{1F44D}\u{1F3FD}",
                "\u{1F1EF}\u{1F1F5}",
                "1\u{FE0F}\u{20E3}",
                "#\u{20E3}",
                family,
                rainbow,
                england,
                "\u{2764}\u{FE0F}",
            ]
        );
    }

    #[test]
    fn leaves_plain_text_alone() {
        // Digits, a lone emoji, an odd regional indicator, a ZWJ between
        // Devanagari letters, a text-presentation selector.
        assert!(seqs("123 #* \u{263A} \u{1F1EF} \u{1F600}\u{FE0E}").is_empty());
        assert!(seqs("\u{0915}\u{094D}\u{200D}\u{0937}").is_empty());
        assert!(seqs("\u{2764}\u{FE0E}").is_empty());
        // A lone emoji that is one by default.
        assert_eq!(seqs("a\u{1F600}b"), ["\u{1F600}"]);
        // Three regional indicators: one flag, then a lone letter.
        assert_eq!(seqs("\u{1F1EF}\u{1F1F5}\u{1F1FA}"), ["\u{1F1EF}\u{1F1F5}"]);
        // A trailing ZWJ isn't part of the sequence.
        assert_eq!(seqs("\u{1F468}\u{1F3FB}\u{200D}"), ["\u{1F468}\u{1F3FB}"]);
    }
}
