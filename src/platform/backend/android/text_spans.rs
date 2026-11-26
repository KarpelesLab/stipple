//! A node's text attribute runs as the spans of the `SpannableString` its
//! `AccessibilityNodeInfo` text is: `StyleSpan` (bold, italic),
//! `UnderlineSpan`, `StrikethroughSpan`, `ForegroundColorSpan` (where a
//! run's color isn't the text's), `BackgroundColorSpan`, `URLSpan` (TalkBack
//! lists it among the node's links), `LocaleSpan` (read in that language's
//! voice) and `AbsoluteSizeSpan` (where a run's size isn't the text's).
//! Text drawn alike stays a plain `String`. The runs are built when TalkBack
//! asks for the node (`createAccessibilityNodeInfo`), not per frame; the
//! JNI side is `a11y::spans`. This is the plan, tested on the host.

use crate::platform::access::{A11yFont, A11yTextRun};

/// `Typeface.BOLD`, `ITALIC`, `BOLD_ITALIC`.
const BOLD: i32 = 1;
const ITALIC: i32 = 2;

/// One span to set, over UTF-16 units `[start, end)` of the text.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Planned {
    pub start: i32,
    pub end: i32,
    pub kind: Kind,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Kind {
    /// `StyleSpan(Typeface.…)`.
    Style(i32),
    Underline,
    Strikethrough,
    /// `ForegroundColorSpan(argb)`.
    Foreground(i32),
    /// `BackgroundColorSpan(argb)`.
    Background(i32),
    /// `URLSpan(url)`.
    Url(String),
    /// `LocaleSpan(Locale.forLanguageTag(tag))`.
    Locale(String),
    /// `AbsoluteSizeSpan(dip, true)`.
    Size(i32),
}

/// `[r, g, b, a]` as Android's ARGB `int`.
fn argb([r, g, b, a]: [u8; 4]) -> i32 {
    ((u32::from(a) << 24) | (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)) as i32
}

/// The UTF-16 offset of byte `at` of `text` (at the char boundary at or
/// before it), counted on from `from` (an earlier `(byte, unit)` pair).
fn utf16_at(text: &str, at: usize, from: (usize, i32)) -> i32 {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    let (b, u) = from;
    if at <= b {
        return text[..at].encode_utf16().count() as i32;
    }
    u + text[b..at].encode_utf16().count() as i32
}

/// The spans `runs` of `text` (in `font`, the node's) make, or `None` when
/// the text reads as plain: one run with nothing beyond its font (a heading
/// all bold reads as a plain heading). A run's color is reported where it
/// isn't the text's (the color most of the text is in).
pub(crate) fn plan(
    text: &str,
    runs: &[A11yTextRun],
    font: Option<&A11yFont>,
) -> Option<Vec<Planned>> {
    let marked = |r: &A11yTextRun| {
        r.underline
            || r.strikethrough
            || r.background.is_some()
            || r.link.is_some()
            || r.language.is_some()
    };
    if runs.is_empty() || (runs.len() == 1 && !marked(&runs[0])) {
        return None;
    }
    // The text's color: the one covering the most of it.
    let mut by_color: Vec<([u8; 4], usize)> = Vec::new();
    for r in runs {
        if let Some(c) = r.color {
            let len = r.range.1.saturating_sub(r.range.0);
            match by_color.iter_mut().find(|(k, _)| *k == c) {
                Some((_, n)) => *n += len,
                None => by_color.push((c, len)),
            }
        }
    }
    let text_color = by_color.iter().max_by_key(|(_, n)| *n).map(|(c, _)| *c);
    let base_size = font.map(|f| f.size);
    let mut out = Vec::new();
    let mut cursor = (0usize, 0i32);
    for r in runs {
        let start = utf16_at(text, r.range.0, cursor);
        cursor = (r.range.0.min(text.len()), start);
        let end = utf16_at(text, r.range.1, cursor);
        if end <= start {
            continue;
        }
        let mut add = |kind| out.push(Planned { start, end, kind });
        let style = (if r.font.weight >= 600 { BOLD } else { 0 })
            | (if r.font.italic { ITALIC } else { 0 });
        if style != 0 {
            add(Kind::Style(style));
        }
        if r.underline {
            add(Kind::Underline);
        }
        if r.strikethrough {
            add(Kind::Strikethrough);
        }
        if let Some(c) = r.color.filter(|c| Some(*c) != text_color) {
            add(Kind::Foreground(argb(c)));
        }
        if let Some(c) = r.background {
            add(Kind::Background(argb(c)));
        }
        if let Some(url) = &r.link {
            add(Kind::Url(url.clone()));
        }
        if let Some(tag) = &r.language {
            add(Kind::Locale(tag.clone()));
        }
        if base_size.is_some_and(|s| (r.font.size - s).abs() >= 0.5) {
            add(Kind::Size(r.font.size.round() as i32));
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font(weight: u16, italic: bool, size: f64) -> A11yFont {
        A11yFont {
            family: None,
            size,
            weight,
            italic,
        }
    }

    fn run(range: (usize, usize), f: A11yFont) -> A11yTextRun {
        A11yTextRun {
            color: Some([0, 0, 0, 255]),
            ..A11yTextRun::plain(range, f)
        }
    }

    #[test]
    fn plain_text_stays_plain() {
        let base = font(400, false, 16.0);
        assert_eq!(plan("hi", &[], Some(&base)), None);
        // One run, even bold all over, reads plain.
        assert_eq!(
            plan("hi", &[run((0, 2), font(700, false, 16.0))], Some(&base)),
            None
        );
        // One run with a link doesn't.
        let mut link = run((0, 2), base.clone());
        link.link = Some("https://stipple.rs".into());
        assert_eq!(
            plan("hi", &[link], Some(&base)),
            Some(vec![Planned {
                start: 0,
                end: 2,
                kind: Kind::Url("https://stipple.rs".into())
            }])
        );
    }

    #[test]
    fn runs_become_spans_in_utf16_units() {
        let base = font(400, false, 16.0);
        // "é" is two bytes, one unit; "😀" four bytes, two units.
        let text = "é bold 😀 link fr big";
        let b = |s: &str| text.find(s).unwrap();
        let mut runs = vec![run((0, b("bold")), base.clone())];
        runs.push(run((b("bold"), b("bold") + 4), font(700, true, 16.0)));
        runs.push(run((b("bold") + 4, b("link")), base.clone()));
        let mut link = run((b("link"), b("link") + 4), base.clone());
        link.link = Some("u".into());
        link.underline = true;
        link.color = Some([0x1a, 0x5f, 0xd0, 0xff]);
        runs.push(link);
        runs.push(run((b("link") + 4, b("fr")), base.clone()));
        let mut fr = run((b("fr"), b("fr") + 2), base.clone());
        fr.language = Some("fr".into());
        fr.strikethrough = true;
        fr.background = Some([0xff, 0xe0, 0x66, 0x80]);
        runs.push(fr);
        runs.push(run((b("fr") + 2, b("big")), base.clone()));
        runs.push(run((b("big"), text.len()), font(400, false, 24.0)));
        let got = plan(text, &runs, Some(&base)).unwrap();
        let units = |s: &str| text[..b(s)].encode_utf16().count() as i32;
        let at = |kind: Kind| {
            got.iter()
                .find(|p| p.kind == kind)
                .map(|p| (p.start, p.end))
        };
        assert_eq!(at(Kind::Style(BOLD | ITALIC)), Some((2, 6)));
        assert_eq!(units("link"), 10);
        assert_eq!(at(Kind::Url("u".into())), Some((10, 14)));
        assert_eq!(at(Kind::Underline), Some((10, 14)));
        assert_eq!(at(Kind::Foreground(0xff1a_5fd0_u32 as i32)), Some((10, 14)));
        assert_eq!(at(Kind::Locale("fr".into())), Some((15, 17)));
        assert_eq!(at(Kind::Strikethrough), Some((15, 17)));
        assert_eq!(at(Kind::Background(0x80ff_e066_u32 as i32)), Some((15, 17)));
        assert_eq!(at(Kind::Size(24)), Some((18, 21)));
        // The text's own color isn't a span.
        assert_eq!(
            got.iter()
                .filter(|p| matches!(p.kind, Kind::Foreground(_)))
                .count(),
            1
        );
    }

    #[test]
    fn a_range_inside_a_character_snaps_back() {
        assert_eq!(utf16_at("é", 1, (0, 0)), 0);
        assert_eq!(utf16_at("aé", 3, (1, 1)), 2);
    }
}
