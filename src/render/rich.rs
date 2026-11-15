//! Rich text: one text made of runs, each in its own color, weight, slant,
//! size and decoration, with images in the line. It wraps and runs through
//! the Unicode Bidirectional Algorithm as one text.

use std::ops::Range;
use std::sync::Arc;

use crate::geometry::{Point, Size};
use crate::layout::LayoutDirection;
use crate::render::text::{SpanFace, Styling};
use crate::render::{
    Color, Decoration, Font, FontWeight, ImageSource, Pixmap, Scene, Svg, TextDirection,
    TextLayout, TextPaint, UnderlineStyle,
};

/// The character an inline image takes the place of in a [`RichText`]'s
/// text: U+FFFC OBJECT REPLACEMENT CHARACTER.
pub(crate) const OBJECT: char = '\u{FFFC}';

/// How one run of a [`RichText`] differs from the text around it. Every
/// property left `None` is the text's own (an element's color, font, weight,
/// size and decoration, or a [`TextPaint`]'s).
///
/// ```
/// use stipple::render::{Color, SpanStyle};
/// let gold = SpanStyle::new().color(Color::rgb(0xd4, 0xa0, 0x17)).bold();
/// assert_eq!(gold.color, Some(Color::rgb(0xd4, 0xa0, 0x17)));
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct SpanStyle {
    /// The run's color.
    pub color: Option<Color>,
    /// The run's font (its weight and slant still follow the text's unless
    /// set here).
    pub font: Option<Font>,
    /// The run's weight.
    pub weight: Option<FontWeight>,
    /// Whether the run is italic.
    pub italic: Option<bool>,
    /// The run's size, in logical pixels.
    pub size: Option<f64>,
    /// The run's underline, strikethrough and overline (`Decoration::NONE`
    /// for none where the text has some).
    pub decoration: Option<Decoration>,
}

impl SpanStyle {
    /// A run like the text around it; set what differs.
    pub fn new() -> Self {
        Self::default()
    }

    /// In `color`.
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    /// In `font`.
    pub fn font(mut self, font: Font) -> Self {
        self.font = Some(font);
        self
    }

    /// At `weight`.
    pub fn weight(mut self, weight: FontWeight) -> Self {
        self.weight = Some(weight);
        self
    }

    /// Bold.
    pub fn bold(self) -> Self {
        self.weight(FontWeight::BOLD)
    }

    /// Italic.
    pub fn italic(mut self) -> Self {
        self.italic = Some(true);
        self
    }

    /// At `size` logical pixels.
    pub fn size(mut self, size: f64) -> Self {
        self.size = Some(size);
        self
    }

    /// With `decoration`'s lines.
    pub fn decorate(mut self, decoration: Decoration) -> Self {
        self.decoration = Some(decoration);
        self
    }

    /// Underlined in `style`.
    pub fn underline(self, style: UnderlineStyle) -> Self {
        self.decorate(Decoration::NONE.underline(style))
    }

    /// Struck through.
    pub fn strikethrough(self) -> Self {
        self.decorate(Decoration::NONE.strikethrough())
    }
}

/// An image in a line of [`RichText`]: a bitmap or an SVG, as tall as the
/// text's size unless given a height, as wide as its aspect ratio makes it,
/// and centered on the middle of the capital letters. `alt` is what copying
/// the text and screen readers get in its place.
///
/// ```
/// use std::sync::Arc;
/// use stipple::geometry::PhysicalSize;
/// use stipple::render::{InlineIcon, Pixmap};
/// let coin = Arc::new(Pixmap::new(PhysicalSize::new(16, 16)));
/// let icon = InlineIcon::new(coin).height(18.0).alt("coins");
/// assert_eq!(icon.alt, "coins");
/// ```
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct InlineIcon {
    /// The image.
    pub source: ImageSource,
    /// Its height in logical pixels; `None` is the text's size.
    pub height: Option<f64>,
    /// The text standing for it when copied or read aloud.
    pub alt: String,
}

impl InlineIcon {
    /// `source`, as tall as the text, with no alternative text.
    pub fn new(source: impl Into<ImageSource>) -> Self {
        Self {
            source: source.into(),
            height: None,
            alt: String::new(),
        }
    }

    /// `height` logical pixels tall.
    pub fn height(mut self, height: f64) -> Self {
        self.height = Some(height);
        self
    }

    /// Copied and read aloud as `alt`.
    pub fn alt(mut self, alt: impl Into<String>) -> Self {
        self.alt = alt.into();
        self
    }

    /// Its size where the text is `text_size` tall.
    fn size_at(&self, text_size: f64) -> Size {
        let h = self.height.unwrap_or(text_size).max(0.0);
        let natural = self.source.size();
        let w = if natural.height > 0.0 {
            h * natural.width / natural.height
        } else {
            h
        };
        Size::new(w, h)
    }
}

impl From<ImageSource> for InlineIcon {
    fn from(source: ImageSource) -> Self {
        Self::new(source)
    }
}

impl From<Arc<Pixmap>> for InlineIcon {
    fn from(image: Arc<Pixmap>) -> Self {
        Self::new(image)
    }
}

impl From<Arc<Svg>> for InlineIcon {
    fn from(svg: Arc<Svg>) -> Self {
        Self::new(svg)
    }
}

/// One run of a [`RichText`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Span {
    /// Its bytes in the text.
    pub range: Range<usize>,
    pub style: SpanStyle,
    pub icon: Option<InlineIcon>,
}

/// Text made of runs, each with its own [`SpanStyle`] (color, weight,
/// slant, size, decoration), and images in the line ([`InlineIcon`]),
/// laid out as one text: it wraps across runs, and right-to-left and mixed
/// text is reordered as a whole.
///
/// [`label`](crate::widgets::label), [`heading`](crate::widgets::heading)
/// and [`paragraph`](crate::widgets::paragraph) take one (or a plain string);
/// [`Scene::fill_rich_text`] draws one.
///
/// ```
/// use stipple::render::{Color, RichText, SpanStyle};
/// let red = Color::rgb(0xd0, 0x30, 0x30);
/// let line = RichText::new()
///     .push("Sold 3 × ")
///     .bold("Healing Potion")
///     .push(" for ")
///     .styled("120", SpanStyle::new().color(red));
/// assert_eq!(line.text(), "Sold 3 × Healing Potion for 120");
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RichText {
    text: String,
    spans: Vec<Span>,
}

impl RichText {
    /// Empty text.
    pub fn new() -> Self {
        Self::default()
    }

    /// This text and `text` after it, styled `style`.
    pub fn styled(mut self, text: impl AsRef<str>, style: SpanStyle) -> Self {
        let text = text.as_ref();
        if text.is_empty() {
            return self;
        }
        let start = self.text.len();
        self.text.push_str(text);
        match self.spans.last_mut() {
            // Joined to the run before when it looks the same.
            Some(last) if last.icon.is_none() && last.style == style => {
                last.range.end = self.text.len();
            }
            _ => self.spans.push(Span {
                range: start..self.text.len(),
                style,
                icon: None,
            }),
        }
        self
    }

    /// This text and `text` after it, like the text around it.
    pub fn push(self, text: impl AsRef<str>) -> Self {
        self.styled(text, SpanStyle::new())
    }

    /// This text and `text` after it, bold.
    pub fn bold(self, text: impl AsRef<str>) -> Self {
        self.styled(text, SpanStyle::new().bold())
    }

    /// This text and `text` after it, italic.
    pub fn italic(self, text: impl AsRef<str>) -> Self {
        self.styled(text, SpanStyle::new().italic())
    }

    /// This text and `text` after it, in `color`.
    pub fn colored(self, text: impl AsRef<str>, color: Color) -> Self {
        self.styled(text, SpanStyle::new().color(color))
    }

    /// This text and an image after it, as tall as the text around it (see
    /// [`InlineIcon`] for another height and alternative text).
    pub fn icon(self, icon: impl Into<InlineIcon>) -> Self {
        self.icon_styled(icon, SpanStyle::new())
    }

    /// [`icon`](Self::icon) in a run styled `style`: its size sets the
    /// image's default height.
    pub fn icon_styled(mut self, icon: impl Into<InlineIcon>, style: SpanStyle) -> Self {
        let start = self.text.len();
        self.text.push(OBJECT);
        self.spans.push(Span {
            range: start..self.text.len(),
            style,
            icon: Some(icon.into()),
        });
        self
    }

    /// The text laid out, each image standing as one U+FFFC (OBJECT
    /// REPLACEMENT CHARACTER): what byte indices (a selection, a caret) are
    /// into.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The text as copied and read aloud: each image as its alternative
    /// text.
    pub fn plain_text(&self) -> String {
        plain_of(&self.text, &self.spans, 0..self.text.len())
    }

    /// Whether there is no text (nor image).
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Lay the text out in `font` at `size` (each run's own font and size
    /// where it has one), for drawing with [`Scene::fill_layout`] and for
    /// hit-testing; see [`Font::layout`].
    pub fn layout(
        &self,
        font: &Font,
        size: f64,
        direction: TextDirection,
        max_width: Option<f64>,
    ) -> TextLayout<'_> {
        styling(&self.spans, font, size).layout(
            &self.text,
            direction,
            max_width,
            LayoutDirection::Ltr,
        )
    }

    /// The text and, unless every run looks like the text around it, its
    /// runs.
    pub(crate) fn into_parts(self) -> (String, Option<Arc<[Span]>>) {
        let plain = self
            .spans
            .iter()
            .all(|s| s.icon.is_none() && s.style == SpanStyle::default());
        let spans = (!plain).then(|| Arc::from(self.spans));
        (self.text, spans)
    }
}

/// The looks of `spans` over text in `font` at `size`.
pub(crate) fn styling(spans: &[Span], font: &Font, size: f64) -> Styling {
    let base = SpanFace::plain(font, size);
    let faces = spans
        .iter()
        .map(|s| {
            let st = &s.style;
            let size = st.size.unwrap_or(size);
            let mut style = font.style();
            if let Some(w) = st.weight {
                style.weight = w;
            }
            if let Some(i) = st.italic {
                style.italic = i;
            }
            let font = st.font.as_ref().unwrap_or(font).styled(style);
            SpanFace {
                icon: s.icon.as_ref().map(|i| (i.source.clone(), i.size_at(size))),
                font,
                size,
                color: st.color,
                decoration: st.decoration,
            }
        })
        .collect();
    let starts = spans
        .iter()
        .enumerate()
        .map(|(i, s)| (s.range.start, i))
        .collect();
    Styling::new(base, faces, starts)
}

/// `text[range]` with each image in `spans` as its alternative text.
pub(crate) fn plain_of(text: &str, spans: &[Span], range: Range<usize>) -> String {
    let mut out = String::with_capacity(range.len());
    for (i, c) in text[range.clone()].char_indices() {
        let at = range.start + i;
        if c == OBJECT
            && let Some(icon) = spans
                .iter()
                .find(|s| s.range.start == at)
                .and_then(|s| s.icon.as_ref())
        {
            out.push_str(&icon.alt);
        } else {
            out.push(c);
        }
    }
    out
}

impl From<&str> for RichText {
    fn from(text: &str) -> Self {
        Self::new().push(text)
    }
}

impl From<String> for RichText {
    fn from(text: String) -> Self {
        Self::new().push(text)
    }
}

impl From<&String> for RichText {
    fn from(text: &String) -> Self {
        Self::new().push(text)
    }
}

impl From<std::borrow::Cow<'_, str>> for RichText {
    fn from(text: std::borrow::Cow<'_, str>) -> Self {
        Self::new().push(text)
    }
}

impl Scene {
    /// Lay out and paint `text` at `origin` (the top-left of its box), in
    /// `paint`'s font, size, color, decoration, outline and shadow wherever
    /// a run doesn't set its own. See [`fill_text`](Self::fill_text).
    pub fn fill_rich_text(&mut self, text: &RichText, origin: Point, paint: &TextPaint) {
        if text.is_empty() || paint.size <= 0.0 {
            return;
        }
        let layout = text.layout(&paint.font, paint.size, TextDirection::Auto, None);
        self.paint_layout(&layout, origin, paint);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{PhysicalSize, Rect, ScaleFactor, Vec2};
    use crate::render::{SoftwareRenderer, TextOutline, TextShadow};

    fn font() -> Option<Font> {
        let f = Font::system_default();
        if f.is_none() {
            eprintln!("skipping: no system font found");
        }
        f
    }

    fn solid(rgba: [u8; 4], side: u32) -> Arc<Pixmap> {
        let data = (0..side * side).flat_map(|_| rgba).collect();
        Arc::new(Pixmap::from_rgba8(PhysicalSize::new(side, side), data))
    }

    /// Pixels of `pm` within `area` matching `f`.
    fn count(pm: &Pixmap, area: Rect, f: impl Fn([u8; 4]) -> bool) -> usize {
        let mut n = 0;
        for y in area.min_y().max(0.0) as u32..area.max_y() as u32 {
            for x in area.min_x().max(0.0) as u32..area.max_x() as u32 {
                if pm.pixel(x, y).is_some_and(&f) {
                    n += 1;
                }
            }
        }
        n
    }

    fn render(size: Size, draw: impl FnOnce(&mut Scene)) -> Pixmap {
        let mut scene = Scene::new(size);
        scene.fill_rect(
            crate::geometry::Rect::from_xywh(0.0, 0.0, size.width, size.height),
            Color::WHITE,
        );
        draw(&mut scene);
        SoftwareRenderer::new().render(scene, ScaleFactor::IDENTITY)
    }

    const RED: Color = Color::rgb(255, 0, 0);

    fn reddish(p: [u8; 4]) -> bool {
        p[0] > 160 && p[1] < 100 && p[2] < 100
    }

    fn dark(p: [u8; 4]) -> bool {
        p[0] < 100 && p[1] < 100 && p[2] < 100
    }

    #[test]
    fn runs_take_their_own_colors() {
        let Some(font) = font() else { return };
        let text = RichText::new().push("HHHH").colored("HHHH", RED);
        let split = font.measure("HHHH", 24.0).width + 4.0;
        let pm = render(Size::new(220.0, 40.0), |s| {
            let paint = TextPaint::new(&font, 24.0, Color::BLACK);
            s.fill_rich_text(&text, Point::new(4.0, 4.0), &paint);
        });
        let left = Rect::from_xywh(0.0, 0.0, split, 40.0);
        let right = Rect::from_xywh(split, 0.0, 220.0 - split, 40.0);
        assert!(count(&pm, left, dark) > 20);
        assert_eq!(count(&pm, left, reddish), 0);
        assert!(count(&pm, right, reddish) > 20);
        assert_eq!(count(&pm, right, dark), 0);
    }

    #[test]
    fn a_bigger_run_makes_its_line_taller() {
        let Some(font) = font() else { return };
        let plain = font.layout("Hp Hp", 14.0, TextDirection::Auto, None);
        let text = RichText::new()
            .push("Hp ")
            .styled("Hp", SpanStyle::new().size(28.0));
        let rich = text.layout(&font, 14.0, TextDirection::Auto, None);
        assert!(rich.size().height > plain.size().height + 5.0);
        assert!((rich.line_height(0) - rich.size().height).abs() < 0.01);
        assert!(rich.size().width > plain.size().width);
        // A plain layout is as tall as the font's lines.
        assert!((plain.size().height - font.line_height(14.0)).abs() < 0.01);
    }

    #[test]
    fn an_icon_sits_in_the_line_centered_on_the_capitals() {
        let Some(font) = font() else { return };
        let icon = solid([0, 0, 255, 255], 4);
        let text = RichText::new()
            .push("H")
            .icon(InlineIcon::new(icon).height(10.0).alt("*"))
            .push("H");
        let layout = text.layout(&font, 20.0, TextDirection::Auto, None);
        let h = font.measure("H", 20.0).width;
        // The image takes 10 px of the line, as one character of the text.
        assert!((layout.size().width - (2.0 * h + 10.0)).abs() < 0.5);
        assert_eq!(text.text().chars().count(), 3);
        assert_eq!(text.plain_text(), "H*H");
        let x = layout.caret_position(text.text().len() - 1).x;
        assert!((x - (h + 10.0)).abs() < 0.5, "{x}");
        let pm = render(Size::new(80.0, 40.0), |s| {
            s.fill_layout(&layout, Point::new(0.0, 0.0), Color::BLACK);
        });
        let blue = |p: [u8; 4]| p[2] > 200 && p[0] < 80;
        let column = Rect::from_xywh(h.ceil(), 0.0, 9.0, 40.0);
        assert!(count(&pm, column, blue) >= 70, "drawn 10×10");
        // Its middle is the capitals' middle.
        let rows: Vec<u32> = (0..40)
            .filter(|&y| pm.pixel((h + 5.0) as u32, y).is_some_and(blue))
            .collect();
        let middle = (rows[0] + rows[rows.len() - 1] + 1) as f64 / 2.0;
        let caps = font.baseline(20.0) - font.cap_height_em() * 20.0 / 2.0;
        assert!((middle - caps).abs() <= 1.0, "{middle} vs {caps}");
    }

    #[test]
    fn rich_text_wraps_across_runs() {
        let Some(font) = font() else { return };
        let text = RichText::new()
            .push("the quick ")
            .bold("brown fox")
            .push(" jumps over ")
            .colored("the lazy dog", RED);
        let full = text.layout(&font, 16.0, TextDirection::Auto, None);
        assert_eq!(full.line_count(), 1);
        let half = full.size().width / 2.0;
        let wrapped = text.layout(&font, 16.0, TextDirection::Auto, Some(half));
        assert!(wrapped.line_count() > 1);
        assert!(wrapped.size().width <= half + 0.5);
        // Every byte is on a line, in order.
        let mut at = 0;
        for line in 0..wrapped.line_count() {
            let r = wrapped.line_range(line).unwrap();
            assert!(r.start >= at);
            at = r.end;
        }
        assert_eq!(at, text.text().len());
    }

    #[test]
    fn mixed_direction_runs_reorder_as_one_text() {
        let Some(font) = font() else { return };
        let text = RichText::new().push("abc ").bold("אבג").push(" def");
        let layout = text.layout(&font, 16.0, TextDirection::Auto, None);
        assert!(!layout.line_is_rtl(0));
        // The Hebrew reads right to left in the middle of the line.
        let heb = "abc ".len();
        let x = |i: usize| layout.caret_position(i).x;
        assert!(x(heb + "א".len()) > x(heb + "אב".len()));
        assert!(x(heb + "אבג".len() + 1) > x(heb));
    }

    #[test]
    fn an_outline_rings_the_glyphs() {
        let Some(font) = font() else { return };
        let area = Rect::from_xywh(0.0, 0.0, 120.0, 50.0);
        let draw = |paint: TextPaint| {
            render(area.size, |s| {
                s.fill_text("HI", Point::new(10.0, 6.0), &paint)
            })
        };
        let plain = draw(TextPaint::new(&font, 32.0, Color::WHITE));
        let ring = TextOutline::new(2.0, Color::BLACK);
        let outlined = draw(TextPaint::new(&font, 32.0, Color::WHITE).outline(ring));
        assert_eq!(count(&plain, area, dark), 0, "white on white");
        assert!(count(&outlined, area, dark) > 60, "the outline shows");
        // The glyphs stay white inside: the fill is drawn over the outline.
        let white = |p: [u8; 4]| p[0] > 240 && p[1] > 240 && p[2] > 240;
        let (left, top) = (
            10.0,
            6.0 + font.baseline(32.0) - font.cap_height_em() * 32.0,
        );
        let stem = Rect::from_xywh(left, top + 4.0, 12.0, 6.0);
        assert!(count(&outlined, stem, white) > 0);
    }

    #[test]
    fn a_shadow_falls_at_its_offset_and_a_blur_softens_it() {
        let Some(font) = font() else { return };
        let size = Size::new(120.0, 64.0);
        let at = Point::new(10.0, 4.0);
        let shadowed = |shadow: TextShadow| {
            render(size, |s| {
                let paint = TextPaint::new(&font, 32.0, Color::WHITE).shadow(shadow);
                s.fill_text("HI", at, &paint);
            })
        };
        let drop = TextShadow::new(Vec2::new(0.0, 12.0), Color::BLACK);
        let hard = shadowed(drop);
        // Just under the baseline only the shadow is drawn.
        let base = at.y + font.baseline(32.0);
        let under = Rect::from_xywh(0.0, base + 1.0, 120.0, 10.0);
        assert!(count(&hard, under, dark) > 20, "the shadow shows below");
        let grey = |p: [u8; 4]| p[0] > 40 && p[0] < 215;
        let soft = shadowed(drop.blur(3.0));
        let whole = Rect::from_xywh(0.0, 0.0, size.width, size.height);
        // Fonts differ in how much edge a glyph has (CI's Linux default
        // isn't this Mac's), so only ask that the blur adds soft pixels.
        let (soft_n, hard_n) = (count(&soft, whole, grey), count(&hard, whole, grey));
        assert!(
            soft_n > hard_n,
            "a blurred shadow has soft edges ({soft_n} grey pixels blurred, {hard_n} hard)"
        );
    }

    #[test]
    fn plain_runs_make_plain_text() {
        let (text, spans) = RichText::from("hello").push(" world").into_parts();
        assert_eq!(text, "hello world");
        assert!(spans.is_none());
        let (_, spans) = RichText::new().bold("hi").into_parts();
        assert_eq!(spans.map(|s| s.len()), Some(1));
        // Neighbors in one look join.
        let t = RichText::new().bold("a").bold("b").push("c");
        assert_eq!(t.spans.len(), 2);
    }
}
