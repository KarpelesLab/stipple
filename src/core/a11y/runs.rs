//! Text attribute runs: the stretches of a text node's text drawn alike, as
//! screen readers read them (a bold word, a link, a search match's
//! highlight, a phrase in another language).
//!
//! A rich text's node keeps its laid-out runs (shared, not copied) and
//! builds the [`AccessTextRun`]s from them only when asked
//! ([`AccessNode::text_runs`]): the platform bridges ask when a screen
//! reader queries text attributes, not on every frame.

use std::ops::Range;
use std::sync::Arc;

use super::{AccessFont, AccessNode, Role};
use crate::core::runtime::{LayoutNode, NodeContent};
use crate::render::rich::Span;
use crate::render::{Color, Decoration};

/// One run of a node's text drawn alike: a byte range of its
/// [`name`](AccessNode::name) and the text attributes over it, as UIA's
/// `GetAttributeValue`, AT-SPI's `GetAttributeRun`, macOS's attributed
/// strings, Android's spans and the web's markup report them. Rich text
/// ([`Element::rich_text`](crate::core::Element::rich_text)) has one per
/// [`SpanStyle`](crate::render::SpanStyle) that differs.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct AccessTextRun {
    /// Its bytes in the node's name.
    pub range: Range<usize>,
    /// Its font (a family of `None` is the app's font's).
    pub font: AccessFont,
    pub underline: bool,
    pub strikethrough: bool,
    /// Its color.
    pub color: Option<Color>,
    /// The color behind it (a highlighted search match), if any.
    pub background: Option<Color>,
    /// Where it links to ([`SpanStyle::link`](crate::render::SpanStyle::link)).
    pub link: Option<Arc<str>>,
    /// Its language, when it isn't the text's
    /// ([`SpanStyle::language`](crate::render::SpanStyle::language)).
    pub language: Option<Arc<str>>,
}

impl AccessTextRun {
    /// A run over `range` in `font`, `color` and `decoration`, with no
    /// background, link or language.
    fn plain(range: Range<usize>, font: AccessFont, color: Color, decoration: Decoration) -> Self {
        AccessTextRun {
            range,
            font,
            underline: decoration.underline.is_some(),
            strikethrough: decoration.strikethrough,
            color: Some(color),
            background: None,
            link: None,
            language: None,
        }
    }

    /// Whether it's drawn as `other` is (all but the range alike).
    fn same_look(&self, other: &AccessTextRun) -> bool {
        AccessTextRun {
            range: other.range.clone(),
            ..self.clone()
        } == *other
    }
}

/// What a rich text node's runs are built from: its spans as laid out (a
/// text view's, each line's, at where the line is in the name), and the
/// text's own look where a span doesn't set one.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RunSource {
    /// `(where in the name, the spans there)`, in order.
    parts: Vec<(usize, Arc<[Span]>)>,
    font: AccessFont,
    color: Color,
    decoration: Decoration,
    /// The name's length, in bytes.
    len: usize,
}

// The bridges serve runs from other threads (AT-SPI's, UIA's).
const _: () = {
    const fn shareable<T: Send + Sync>() {}
    shareable::<RunSource>();
};

impl RunSource {
    /// The runs, in order, covering the name: a span's bytes shifted by
    /// the images before it, each read as its alternative text (as the
    /// name reads them); the stretches between spans in the text's look.
    pub(crate) fn runs(&self) -> Vec<AccessTextRun> {
        let mut out: Vec<AccessTextRun> = Vec::with_capacity(self.parts.len() * 2 + 1);
        let base = |range: Range<usize>| {
            AccessTextRun::plain(range, self.font.clone(), self.color, self.decoration)
        };
        let push = |out: &mut Vec<AccessTextRun>, r: AccessTextRun| match out.last_mut() {
            Some(last) if last.same_look(&r) && last.range.end == r.range.start => {
                last.range.end = r.range.end;
            }
            _ => out.push(r),
        };
        let mut at = 0;
        for (offset, spans) in &self.parts {
            // How far the name is ahead of the text (images read as their alt).
            let mut shift = *offset as isize;
            for s in spans.iter() {
                let alt = s
                    .icon
                    .as_ref()
                    .map(|i| i.alt.len())
                    .or(s.float.as_ref().map(|f| f.alt.len()));
                let start = s.range.start.saturating_add_signed(shift);
                let len = alt.unwrap_or(s.range.len());
                if let Some(alt) = alt {
                    // One U+FFFC (three bytes) read as `alt`.
                    shift += alt as isize - s.range.len().min(3) as isize;
                }
                let (start, end) = (start.max(at), (start + len).min(self.len));
                if start >= end {
                    continue;
                }
                if start > at {
                    push(&mut out, base(at..start));
                }
                push(&mut out, self.styled(s, start..end));
                at = end;
            }
        }
        if at < self.len {
            push(&mut out, base(at..self.len));
        }
        out
    }

    /// Span `s`'s run over `range`: its look over the text's.
    fn styled(&self, s: &Span, range: Range<usize>) -> AccessTextRun {
        let st = &s.style;
        let font = AccessFont {
            family: st
                .font
                .as_ref()
                .and_then(|f| f.family_name())
                .map(str::to_string)
                .or_else(|| self.font.family.clone()),
            size: st.size.unwrap_or(self.font.size),
            weight: st.weight.map_or(self.font.weight, |w| w.0),
            italic: st.italic.unwrap_or(self.font.italic),
        };
        let deco = st.decoration.unwrap_or(self.decoration);
        AccessTextRun {
            range,
            font,
            underline: deco.underline.is_some(),
            strikethrough: deco.strikethrough,
            color: Some(st.color.unwrap_or(self.color)),
            background: st.background,
            link: st.link.clone(),
            language: st.language.clone(),
        }
    }
}

/// The source of the runs of `node`'s text (a text, field or number input
/// node whose name `name` is its text in `font`), when it is rich text
/// whose runs differ. Text cut short reads in full, past the runs it
/// shows: it reads as drawn alike.
pub(super) fn source_of(
    node: &LayoutNode,
    role: Role,
    name: &str,
    font: Option<&AccessFont>,
) -> Option<Arc<RunSource>> {
    if name.is_empty() || !matches!(role, Role::Text | Role::TextField | Role::SpinButton) {
        return None;
    }
    let leaf = super::own_text(node).or_else(|| crate::core::runtime::first_text(node))?;
    let NodeContent::Text { color, style, .. } = &leaf.content else {
        return None;
    };
    let spans = style.spans.as_ref()?;
    if style.full().is_some() || style.placeholder {
        return None;
    }
    Some(Arc::new(RunSource {
        parts: vec![(0, spans.clone())],
        font: font?.clone(),
        color: *color,
        decoration: style.decoration,
        len: name.len(),
    }))
}

/// The source of the runs of `node`, whose name `name` is the text of its
/// text leaves `lines` (by order, a text view's rows on screen), each a
/// line: one after the other, a newline between them. `None` when none
/// of them is rich text.
pub(super) fn lines_source_of(
    node: &LayoutNode,
    lines: Range<usize>,
    name: &str,
    font: Option<&AccessFont>,
) -> Option<Arc<RunSource>> {
    fn leaves<'a>(n: &'a LayoutNode, out: &mut Vec<&'a LayoutNode>) {
        if matches!(n.content, NodeContent::Text { .. }) {
            out.push(n);
        }
        for c in n.children.iter().filter(|c| !c.ghost) {
            leaves(c, out);
        }
    }
    let mut all = Vec::new();
    leaves(node, &mut all);
    let mut parts = Vec::new();
    let mut look = None;
    let mut at = 0;
    for leaf in all.get(lines)? {
        let NodeContent::Text {
            text, color, style, ..
        } = &leaf.content
        else {
            continue;
        };
        look.get_or_insert((*color, style.decoration));
        // An empty line shows a space; the name has none.
        let rest = name.get(at..).unwrap_or("");
        let len = if rest.starts_with(text.as_str()) {
            text.len()
        } else {
            0
        };
        if let Some(spans) = style.spans.as_ref().filter(|_| len > 0) {
            parts.push((at, spans.clone()));
        }
        // On to the next line.
        at = match name.get(at + len..).and_then(|r| r.find('\n')) {
            Some(i) => at + len + i + 1,
            None => name.len(),
        };
    }
    let (color, decoration) = look?;
    if parts.is_empty() {
        return None;
    }
    Some(Arc::new(RunSource {
        parts,
        font: font?.clone(),
        color,
        decoration,
        len: name.len(),
    }))
}

impl AccessNode {
    /// The runs of this node's text, in order, covering its
    /// [`name`](Self::name) without gaps: one per stretch drawn alike (a
    /// bold word, a link, a highlighted match), one for text drawn all
    /// alike, none for a node without text of its own (a container, a
    /// control named by its label, a password field). Built when called.
    ///
    /// ```
    /// use stipple::core::{Element, accessibility_tree, layout};
    /// use stipple::layout::Axis;
    /// use stipple::geometry::Rect;
    /// use stipple::render::{Color, RichText};
    ///
    /// let text = RichText::new().push("a ").bold("bold").push(" word");
    /// let line = Element::rich_text(text, 14.0, Color::BLACK);
    /// let el = Element::stack(Axis::Vertical, vec![line]);
    /// let laid = layout(&el, Rect::from_xywh(0.0, 0.0, 300.0, 40.0), None);
    /// let tree = accessibility_tree(&laid, None);
    /// let runs = tree.children[0].text_runs();
    /// assert_eq!(runs.len(), 3);
    /// assert_eq!(runs[1].range, 2..6);
    /// assert_eq!(runs[1].font.weight, 700);
    /// ```
    pub fn text_runs(&self) -> Vec<AccessTextRun> {
        if let Some(src) = &self.runs {
            return src.runs();
        }
        match &self.font {
            Some(font) if !self.name.is_empty() && self.role != Role::PasswordField => {
                vec![AccessTextRun {
                    range: 0..self.name.len(),
                    font: font.clone(),
                    underline: false,
                    strikethrough: false,
                    color: None,
                    background: None,
                    link: None,
                    language: None,
                }]
            }
            _ => Vec::new(),
        }
    }

    /// The source [`text_runs`](Self::text_runs) builds from, for the
    /// platform's tree.
    pub(crate) fn run_source(&self) -> Option<&Arc<RunSource>> {
        self.runs.as_ref()
    }
}

/// A rich text's runs as the platform's tree carries them, built when a
/// bridge asks.
#[derive(Debug)]
pub(crate) struct PlatformRuns(pub Arc<RunSource>);

impl crate::platform::access::RunSource for PlatformRuns {
    fn runs(&self) -> Vec<crate::platform::access::A11yTextRun> {
        let rgba = |c: Color| [c.r, c.g, c.b, c.a];
        self.0
            .runs()
            .into_iter()
            .map(|r| crate::platform::access::A11yTextRun {
                range: (r.range.start, r.range.end),
                font: crate::platform::access::A11yFont {
                    family: r.font.family,
                    size: r.font.size,
                    weight: r.font.weight,
                    italic: r.font.italic,
                },
                underline: r.underline,
                strikethrough: r.strikethrough,
                color: r.color.map(rgba),
                background: r.background.map(rgba),
                link: r.link.map(|l| l.to_string()),
                language: r.language.map(|l| l.to_string()),
            })
            .collect()
    }

    fn same(&self, other: &dyn crate::platform::access::RunSource) -> bool {
        other
            .as_any()
            .downcast_ref::<PlatformRuns>()
            .is_some_and(|o| Arc::ptr_eq(&o.0, &self.0) || *o.0 == *self.0)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
