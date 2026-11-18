//! Text cut to its box and its line limit, with an ellipsis
//! ([`Element::max_lines`](crate::core::Element::max_lines),
//! [`Element::ellipsis`](crate::core::Element::ellipsis)).
//!
//! The cut is made on the text itself when it's laid out: the text node
//! holds what is shown (rich text's runs cut to match, the "…" in the look
//! of the run before it), and its style the whole text, which is what
//! assistive technologies read.

use crate::core::TextStyle;
use crate::layout::LayoutDirection;
use crate::render::rich::Span;
use crate::render::{Ellipsis, Font, TextDirection};
use std::ops::Range;
use std::sync::Arc;

const ELLIPSIS: &str = "\u{2026}";

/// A piece of the text shown: a range of the original, or the ellipsis.
#[derive(Clone, Debug)]
enum Piece {
    Text(Range<usize>),
    Ellipsis,
}

/// What a text shows, laid out as `layout` would lay it out, when its
/// [`TextStyle`] limits its lines or cuts it to `width` with an ellipsis:
/// the text and the style to draw it with (whose `full` holds the whole
/// text). `None` when all of it shows.
#[allow(clippy::too_many_arguments)]
pub(crate) fn elide(
    style: &TextStyle,
    font: Option<&Font>,
    text: &str,
    size: f64,
    direction: TextDirection,
    wrap: Option<f64>,
    width: f64,
    ui: LayoutDirection,
) -> Option<(String, TextStyle)> {
    if style.cut.is_none() || text.is_empty() || style.flow.is_some() {
        return None;
    }
    let lines: Vec<(Range<usize>, f64)>;
    let dropped;
    {
        let layout = style.layout(font, text, size, direction, wrap, ui)?;
        let n = layout.line_count();
        let keep = style.max_lines().map_or(n, |m| m.clamp(1, n.max(1)));
        dropped = keep < n;
        lines = (0..keep)
            .filter_map(|i| Some((layout.line_range(i)?, layout.line_width(i))))
            .collect();
    }
    let fits = |w: f64| !width.is_finite() || w <= width + 0.01;
    let measure = |pieces: &[Piece]| -> f64 {
        let (s, st) = splice(style, text, pieces);
        st.layout(font, &s, size, direction, None, ui)
            .map_or(0.0, |l| l.size().width)
    };
    let mut pieces = Vec::new();
    let mut cursor = 0;
    let mut changed = false;
    for (i, (line, w)) in lines.iter().enumerate() {
        let last = i + 1 == lines.len();
        let cut_after = last && dropped;
        if fits(*w) && !cut_after {
            continue;
        }
        changed = true;
        // The text this line stands for: itself, or all that's left.
        let rest_end = if cut_after { text.len() } else { line.end };
        let Some(mode) = style.ellipsis() else {
            // A line limit without an ellipsis: the lines after go.
            pieces.push(Piece::Text(cursor..line.end));
            cursor = text.len();
            break;
        };
        match mode {
            Ellipsis::Middle => {
                // The start of the line, then the end of the rest (within
                // its last paragraph).
                let tail_from = text[line.start..rest_end]
                    .rfind('\n')
                    .map_or(line.start, |k| line.start + k + 1);
                let heads = boundaries(text, line.clone());
                let tails = boundaries(text, tail_from..rest_end);
                let (nh, nt) = (heads.len() - 1, tails.len() - 1);
                // Never all of it: the head and tail would meet.
                let total = text[line.start..rest_end].chars().count();
                let most = (nh + nt).min(total.saturating_sub(1));
                let split = |k: usize| {
                    let h = k.div_ceil(2).min(nh);
                    let t = (k - h).min(nt);
                    let h = (k - t).min(nh);
                    (
                        trim_end(text, line.start, heads[h]),
                        tails[tails.len() - 1 - t],
                    )
                };
                let k = largest(most, |k| {
                    let (h, t) = split(k);
                    fits(measure(&[
                        Piece::Text(line.start..h),
                        Piece::Ellipsis,
                        Piece::Text(t..rest_end),
                    ]))
                });
                let (h, t) = split(k);
                pieces.push(Piece::Text(cursor..h));
                pieces.push(Piece::Ellipsis);
                pieces.push(Piece::Text(t..rest_end));
            }
            // `End`, and modes yet to come.
            _ => {
                let heads = boundaries(text, line.clone());
                let k = largest(heads.len() - 1, |k| {
                    let h = trim_end(text, line.start, heads[k]);
                    fits(measure(&[Piece::Text(line.start..h), Piece::Ellipsis]))
                });
                pieces.push(Piece::Text(cursor..trim_end(text, line.start, heads[k])));
                pieces.push(Piece::Ellipsis);
            }
        }
        cursor = rest_end;
        if cut_after {
            break;
        }
    }
    if !changed {
        return None;
    }
    if cursor < text.len() {
        pieces.push(Piece::Text(cursor..text.len()));
    }
    let (shown, mut st) = splice(style, text, &pieces);
    if let Some(cut) = &mut st.cut {
        cut.full = Some(Arc::from(style.plain(text, 0..text.len())));
    }
    Some((shown, st))
}

/// The character boundaries of `text[range]`, from its start to its end.
fn boundaries(text: &str, range: Range<usize>) -> Vec<usize> {
    let mut v: Vec<usize> = text[range.clone()]
        .char_indices()
        .map(|(i, _)| range.start + i)
        .collect();
    v.push(range.end);
    v
}

/// `end`, moved back over the spaces before it (but not before `start`).
fn trim_end(text: &str, start: usize, end: usize) -> usize {
    start + text[start..end].trim_end().len()
}

/// The largest `k` in `0..=max` for which `ok` holds, assuming it holds up
/// to some point and not past it; 0 if it never does.
fn largest(max: usize, ok: impl Fn(usize) -> bool) -> usize {
    let (mut lo, mut hi) = (0, max);
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if ok(mid) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

/// `pieces` put together, with rich text's runs cut to match: the
/// ellipsis takes the look of the run before it (without its image).
fn splice(style: &TextStyle, text: &str, pieces: &[Piece]) -> (String, TextStyle) {
    let mut out = String::new();
    let mut spans: Vec<Span> = Vec::new();
    let source = style.spans.as_deref();
    for piece in pieces {
        let at = out.len();
        match piece {
            Piece::Text(r) => {
                out.push_str(&text[r.clone()]);
                for s in source.unwrap_or_default() {
                    let (a, b) = (s.range.start.max(r.start), s.range.end.min(r.end));
                    if a < b {
                        spans.push(Span {
                            range: a - r.start + at..b - r.start + at,
                            ..s.clone()
                        });
                    }
                }
            }
            Piece::Ellipsis => {
                out.push_str(ELLIPSIS);
                if let Some(source) = source {
                    let look = spans
                        .last()
                        .or(source.first())
                        .map(|s| s.style.clone())
                        .unwrap_or_default();
                    spans.push(Span {
                        range: at..out.len(),
                        style: look,
                        icon: None,
                        float: None,
                    });
                }
            }
        }
    }
    let mut st = style.clone();
    if source.is_some() {
        st.spans = Some(Arc::from(spans));
    }
    (out, st)
}

#[cfg(test)]
mod tests {
    use crate::core::runtime::{LayoutNode, NodeContent, first_text};
    use crate::core::{AccessNode, Element, accessibility_tree, layout, measure};
    use crate::geometry::{Rect, Size};
    use crate::render::{Color, Ellipsis, Font, RichText};
    use crate::widgets::{column, row};

    const LONG: &str = "The quick brown fox jumps over the lazy dog, then naps in the sun";

    fn shown(node: &LayoutNode) -> String {
        match &first_text(node).expect("a text").content {
            NodeContent::Text { text, .. } => text.clone(),
            _ => unreachable!(),
        }
    }

    /// `el` laid out in a column (the root is the window, read by its
    /// children) `w` × `h`.
    fn lay(el: Element, font: &Font, w: f64, h: f64) -> LayoutNode {
        layout(&column(vec![el]), at(w, h), Some(font))
    }

    fn name(node: &LayoutNode) -> String {
        fn find(n: &AccessNode) -> Option<String> {
            (!n.name.is_empty())
                .then(|| n.name.clone())
                .or_else(|| n.children.iter().find_map(find))
        }
        find(&accessibility_tree(node, None)).unwrap_or_default()
    }

    fn text(s: &str) -> Element {
        Element::text(s, 16.0, Color::BLACK)
    }

    fn at(w: f64, h: f64) -> Rect {
        Rect::from_xywh(0.0, 0.0, w, h)
    }

    #[test]
    fn a_line_too_long_ends_in_an_ellipsis() {
        let Some(font) = Font::system_default() else {
            return;
        };
        let el = text(LONG).ellipsis(Ellipsis::End);
        let node = lay(el.clone(), &font, 150.0, 30.0);
        let s = shown(&node);
        assert!(s.ends_with('…'), "{s}");
        let kept = s.trim_end_matches('…');
        assert!(LONG.starts_with(kept) && kept.len() > 5, "{s}");
        assert!(font.measure(&s, 16.0).width <= 150.01);
        // Another word wouldn't fit.
        let more = LONG[kept.len()..].trim_start();
        let word = more.split(' ').next().unwrap();
        let longer = format!("{} {word}…", kept.trim_end());
        assert!(font.measure(&longer, 16.0).width > 150.0, "{longer}");
        assert_eq!(name(&node), LONG, "read in full");
        // Room enough: as it is.
        let node = lay(el, &font, 2000.0, 30.0);
        assert_eq!(shown(&node), LONG);
        assert_eq!(name(&node), LONG);
    }

    #[test]
    fn the_middle_keeps_both_ends() {
        let Some(font) = Font::system_default() else {
            return;
        };
        let path = "/home/someone/projects/stipple/docs/reports/final-report.pdf";
        let el = text(path).ellipsis(Ellipsis::Middle);
        let node = lay(el, &font, 180.0, 30.0);
        let s = shown(&node);
        let (head, tail) = s.split_once('…').expect("an ellipsis");
        assert!(path.starts_with(head) && !head.is_empty(), "{s}");
        assert!(path.ends_with(tail) && tail.ends_with(".pdf"), "{s}");
        assert!(font.measure(&s, 16.0).width <= 180.01);
        assert_eq!(name(&node), path);
    }

    #[test]
    fn max_lines_keeps_the_first_lines() {
        let Some(font) = Font::system_default() else {
            return;
        };
        let line = font.line_height(16.0);
        // Wrapped to 120 px the text takes several lines; two are shown.
        let el = text(LONG).wrap().max_lines(2);
        let room = Size::new(120.0, 1000.0);
        let full = measure(&text(LONG).wrap(), room, Some(&font));
        assert!(full.height > 2.5 * line);
        let size = measure(&el, room, Some(&font));
        assert!((size.height - 2.0 * line).abs() < 0.5, "{size:?}");
        let node = layout(&el, at(120.0, 1000.0), Some(&font));
        let s = shown(&node);
        assert!(LONG.starts_with(&s) && s.len() < LONG.len(), "{s}");
        assert_eq!(font.wrap(&s, 16.0, 120.0).len(), 2, "{s}");
        // With an ellipsis, the second line ends in one.
        let node = lay(el.ellipsis(Ellipsis::End), &font, 120.0, 1000.0);
        let s = shown(&node);
        assert!(s.ends_with('…'), "{s}");
        assert_eq!(font.wrap(&s, 16.0, 120.0).len(), 2, "{s}");
        assert_eq!(name(&node), LONG);
        // The lines of a text that doesn't wrap are its paragraphs.
        let el = text("one\ntwo\nthree").max_lines(2).ellipsis(Ellipsis::End);
        let node = layout(&el, at(500.0, 100.0), Some(&font));
        assert_eq!(shown(&node), "one\ntwo…");
        let el = text("one\ntwo").max_lines(2).ellipsis(Ellipsis::End);
        let node = layout(&el, at(500.0, 100.0), Some(&font));
        assert_eq!(shown(&node), "one\ntwo");
    }

    #[test]
    fn rich_text_is_cut_with_its_runs() {
        let Some(font) = Font::system_default() else {
            return;
        };
        let red = Color::rgb(200, 0, 0);
        let rich = RichText::new()
            .push("Status: ")
            .colored("everything is fine and dandy today", red);
        let el = Element::rich_text(rich, 16.0, Color::BLACK).ellipsis(Ellipsis::End);
        let node = layout(&el, at(140.0, 30.0), Some(&font));
        let NodeContent::Text { text, style, .. } = &first_text(&node).unwrap().content else {
            unreachable!()
        };
        assert!(
            text.starts_with("Status: ") && text.ends_with('…'),
            "{text}"
        );
        let spans = style.spans.as_deref().expect("runs");
        // The runs tile the text shown; the ellipsis is red like the run
        // before it.
        assert_eq!(spans.last().unwrap().range.end, text.len());
        assert_eq!(spans.last().unwrap().style.color, Some(red));
        assert_eq!(spans[0].range.start, 0);
        for w in spans.windows(2) {
            assert_eq!(w[0].range.end, w[1].range.start);
        }
    }

    #[test]
    fn an_ellipsis_shrinks_in_a_crowded_row() {
        let Some(font) = Font::system_default() else {
            return;
        };
        let el = row(vec![text(LONG).ellipsis(Ellipsis::End), text("12 KB")]);
        let node = layout(&el, at(200.0, 30.0), Some(&font));
        let (first, second) = (&node.children[0], &node.children[1]);
        assert!(first.bounds.max_x() <= second.bounds.min_x() + 0.5);
        assert!(second.bounds.max_x() <= 200.5, "{:?}", second.bounds);
        assert!(shown(first).ends_with('…'), "{}", shown(first));
        assert_eq!(shown(second), "12 KB");
    }
}
