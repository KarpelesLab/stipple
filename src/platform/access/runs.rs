//! Text attribute runs: the stretches of a node's text drawn alike (a bold
//! word, a link, a highlighted match), as the bridges' text attribute APIs
//! report them (UIA's `GetAttributeValue`, AT-SPI's `GetAttributeRun`,
//! `AXAttributedStringForRange`, Android's spans, the web overlay's
//! markup). Mirrors `stipple::core::AccessTextRun`.
//!
//! The runs are built only when a bridge asks for them
//! ([`A11yNode::text_runs`]): the tree carries a [`RunSource`] (the rich
//! text's runs as laid out, shared, not copied) for its rich text.

use std::any::Any;
use std::fmt;
use std::sync::Arc;

use super::{A11yFont, A11yNode};

/// One run of a node's text: a byte range of its name and how it's drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct A11yTextRun {
    /// Its bytes in the node's name, `[start, end)`.
    pub range: (usize, usize),
    /// Its font (the family filled in from the node's, else the app's).
    pub font: A11yFont,
    pub underline: bool,
    pub strikethrough: bool,
    /// Its color `[r, g, b, a]`, when known.
    pub color: Option<[u8; 4]>,
    /// The color behind it (a highlighted match), if any.
    pub background: Option<[u8; 4]>,
    /// Where it links to.
    pub link: Option<String>,
    /// Its language (a BCP 47 tag), when it isn't the text's.
    pub language: Option<String>,
}

impl A11yTextRun {
    /// A run over `range` in `font`, otherwise plain.
    pub fn plain(range: (usize, usize), font: A11yFont) -> Self {
        A11yTextRun {
            range,
            font,
            underline: false,
            strikethrough: false,
            color: None,
            background: None,
            link: None,
            language: None,
        }
    }

    /// Whether it's drawn as `other` is (all but the range alike).
    pub fn same_look(&self, other: &A11yTextRun) -> bool {
        A11yTextRun {
            range: other.range,
            ..self.clone()
        } == *other
    }
}

/// Where a node's runs come from: built on demand.
pub trait RunSource: Send + Sync + fmt::Debug {
    /// The runs, in order, covering the node's name.
    fn runs(&self) -> Vec<A11yTextRun>;
    /// Whether `other` gives the same runs (trees compare equal when their
    /// runs do).
    fn same(&self, other: &dyn RunSource) -> bool;
    fn as_any(&self) -> &dyn Any;
}

/// A node's runs, built when asked for; none for text drawn all alike.
#[derive(Clone, Default)]
pub struct A11yRuns(pub Option<Arc<dyn RunSource>>);

impl A11yRuns {
    /// Fixed runs (for tests and self-tests).
    #[allow(dead_code)]
    pub fn fixed(runs: Vec<A11yTextRun>) -> Self {
        A11yRuns(Some(Arc::new(Fixed(runs))))
    }
}

impl fmt::Debug for A11yRuns {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            None => f.write_str("A11yRuns(none)"),
            Some(s) => write!(f, "A11yRuns({s:?})"),
        }
    }
}

impl PartialEq for A11yRuns {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b) || a.same(b.as_ref()),
            _ => false,
        }
    }
}

/// Runs given as they are.
#[derive(Debug, PartialEq)]
struct Fixed(Vec<A11yTextRun>);

impl RunSource for Fixed {
    fn runs(&self) -> Vec<A11yTextRun> {
        self.0.clone()
    }
    fn same(&self, other: &dyn RunSource) -> bool {
        other.as_any().downcast_ref::<Fixed>() == Some(self)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// Not every bridge (nor every target) uses every helper; the tests do.
#[allow(dead_code)]
impl A11yNode {
    /// The runs of this node's text, in order, covering its name without
    /// gaps: one per stretch drawn alike (one for text drawn all alike;
    /// none without text, or without a font). Built now: a bridge calls it
    /// when a screen reader asks, not per frame.
    pub fn text_runs(&self) -> Vec<A11yTextRun> {
        let len = self.name.len();
        let Some(font) = self.font.as_ref() else {
            return Vec::new();
        };
        if len == 0 || self.role == super::A11yRole::PasswordField {
            return Vec::new();
        }
        let built = self.runs.0.as_ref().map(|s| s.runs());
        let Some(mut runs) = built.filter(|r| !r.is_empty()) else {
            return vec![A11yTextRun::plain((0, len), font.clone())];
        };
        for r in &mut runs {
            if r.font.family.is_none() {
                r.font.family = font.family.clone();
            }
        }
        fill_gaps(runs, len, font)
    }

    /// The run holding byte `at` of the name (the last one at its end).
    pub fn text_run_at(&self, at: usize) -> Option<A11yTextRun> {
        let runs = self.text_runs();
        let i = runs
            .iter()
            .position(|r| at < r.range.1)
            .unwrap_or(runs.len().checked_sub(1)?);
        runs.into_iter().nth(i)
    }
}

/// `runs` clamped to `len` bytes, the gaps between them filled with plain
/// runs in `font`, and neighbors drawn alike joined.
fn fill_gaps(runs: Vec<A11yTextRun>, len: usize, font: &A11yFont) -> Vec<A11yTextRun> {
    let mut out: Vec<A11yTextRun> = Vec::with_capacity(runs.len() + 2);
    let mut at = 0;
    let push = |out: &mut Vec<A11yTextRun>, r: A11yTextRun| match out.last_mut() {
        Some(last) if last.same_look(&r) && last.range.1 == r.range.0 => last.range.1 = r.range.1,
        _ => out.push(r),
    };
    for mut r in runs {
        let (s, e) = (r.range.0.max(at), r.range.1.min(len));
        if s >= e {
            continue;
        }
        if s > at {
            push(&mut out, A11yTextRun::plain((at, s), font.clone()));
        }
        r.range = (s, e);
        push(&mut out, r);
        at = e;
    }
    if at < len {
        push(&mut out, A11yTextRun::plain((at, len), font.clone()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::access::A11yRole;

    fn font(weight: u16) -> A11yFont {
        A11yFont {
            family: None,
            size: 16.0,
            weight,
            italic: false,
        }
    }

    fn node(name: &str, runs: Vec<A11yTextRun>) -> A11yNode {
        let mut n = A11yNode::new(A11yRole::Text, name, (0.0, 0.0, 100.0, 20.0));
        n.font = Some(A11yFont {
            family: Some("Inter".into()),
            ..font(400)
        });
        n.runs = A11yRuns::fixed(runs);
        n
    }

    #[test]
    fn text_drawn_alike_is_one_run() {
        let n = node("hello", Vec::new());
        let runs = n.text_runs();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].range, (0, 5));
        assert_eq!(runs[0].font.family.as_deref(), Some("Inter"));
        // No font, no runs.
        let bare = A11yNode::new(A11yRole::Text, "x", (0.0, 0.0, 1.0, 1.0));
        assert!(bare.text_runs().is_empty());
    }

    #[test]
    fn gaps_fill_and_alike_neighbors_join() {
        let bold = A11yTextRun::plain((6, 11), font(700));
        let n = node("hello world again", vec![bold]);
        let runs = n.text_runs();
        let ranges: Vec<_> = runs.iter().map(|r| r.range).collect();
        assert_eq!(ranges, [(0, 6), (6, 11), (11, 17)]);
        assert_eq!(runs[1].font.weight, 700);
        assert_eq!(runs[1].font.family.as_deref(), Some("Inter"));
        assert_eq!(n.text_run_at(7).unwrap().font.weight, 700);
        assert_eq!(n.text_run_at(17).unwrap().range, (11, 17));
        // Two plain runs side by side read as one.
        let n = node(
            "abcd",
            vec![
                A11yTextRun::plain((0, 2), font(400)),
                A11yTextRun::plain((2, 9), font(400)),
            ],
        );
        assert_eq!(n.text_runs().len(), 1);
        assert_eq!(n.text_runs()[0].range, (0, 4));
    }

    #[test]
    fn runs_compare_by_content() {
        let a = A11yRuns::fixed(vec![A11yTextRun::plain((0, 1), font(700))]);
        let b = A11yRuns::fixed(vec![A11yTextRun::plain((0, 1), font(700))]);
        let c = A11yRuns::fixed(vec![A11yTextRun::plain((0, 1), font(400))]);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, A11yRuns::default());
    }
}
