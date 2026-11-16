//! Text: font loading and shaping, bridged to the scene graph via
//! `oxideav-scribe`.
//!
//! A [`Font`] wraps a scribe `FaceChain`. [`Scene::fill_text`](crate::render::Scene)
//! shapes a string into positioned glyph outlines and emits them as
//! `oxideav-core` nodes, so text composites through the same CPU rasterizer as
//! every other primitive — no separate text pipeline.
//!
//! Apps provide font bytes via [`Font::from_bytes`]; [`Font::system_default`]
//! is a convenience that probes common OS font locations (handy for examples
//! and tests, not meant for shipping apps).

use crate::geometry::{Point, Rect, ScaleFactor, Size};
use crate::layout::LayoutDirection;
use crate::render::bidi::{
    ParagraphLevels, TextDirection, VisualChar, boundary_carets, caret_boundary, has_strong,
    visual_order,
};
use crate::render::font::{Chain, FaceSource, Family, FontStyle, FontWeight, Synthesis, Variant};
use crate::render::scene::Scene;
use crate::render::software::SoftwareRenderer;
use crate::render::{Color, ImageSource};
use core::fmt;
use core::ops::Range;
use oxideav_core::{Group, LineJoin, Node, Paint, Stroke, Transform2D};
use oxideav_scribe::{Face, FaceChain, PositionedGlyph, Shaper};
use std::sync::Arc;

/// A shapeable font: a family (its regular, bold, italic … faces) drawn in
/// one [`FontStyle`], with the fonts it falls back to for characters it
/// lacks.
///
/// Cloning is cheap (a handle): give every element or glyph run its own
/// copy. Two fonts are equal when they draw the same faces in the same style.
/// See [`Font::from_family`] for system fonts by name, [`Font::styled`] for
/// other weights and slants, and [`Font::with_fallback`] for fallbacks.
#[derive(Clone)]
pub struct Font {
    pub(crate) family: Arc<Family>,
    pub(crate) style: FontStyle,
    pub(crate) variant: Arc<Variant>,
}

impl PartialEq for Font {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.variant, &other.variant)
    }
}

impl Eq for Font {}

impl core::hash::Hash for Font {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.variant.id.hash(state);
    }
}

/// Shaped glyph runs keyed by text and size (in 1/64 px), one cache per font
/// variant (so per family, style and fallback chain). Cleared wholesale when
/// it grows past [`ShapeCache::MAX`], which a UI's working set of strings
/// stays well under.
#[derive(Default)]
pub(crate) struct ShapeCache(
    std::sync::Mutex<std::collections::HashMap<(String, u32), Vec<PositionedGlyph>>>,
);

impl ShapeCache {
    const MAX: usize = 4096;

    /// Forget every run (the chain they were shaped with changed).
    pub(crate) fn clear(&self) {
        if let Ok(mut map) = self.0.lock() {
            map.clear();
        }
    }

    pub(crate) fn shape(
        &self,
        chain: &FaceChain,
        text: &str,
        size_px: f64,
    ) -> Vec<PositionedGlyph> {
        let key = (text.to_string(), (size_px * 64.0).round() as u32);
        if let Ok(map) = self.0.lock()
            && let Some(glyphs) = map.get(&key)
        {
            return glyphs.clone();
        }
        let glyphs = chain.shape(text, size_px as f32).unwrap_or_default();
        if let Ok(mut map) = self.0.lock() {
            if map.len() >= Self::MAX {
                map.clear();
            }
            map.insert(key, glyphs.clone());
        }
        glyphs
    }
}

impl fmt::Debug for Font {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Font")
            .field("family", &self.family_name())
            .field("style", &self.style)
            .field("faces", &self.chain().len())
            .finish()
    }
}

/// Error loading a [`Font`].
#[derive(Debug)]
pub struct FontError(String);

impl FontError {
    pub(crate) fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

impl fmt::Display for FontError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "font load error: {}", self.0)
    }
}

impl std::error::Error for FontError {}

impl Font {
    /// Load a font from `sfnt` bytes: TrueType or OpenType, with `glyf`,
    /// `CFF ` or `CFF2` outlines, or a collection of them (whose faces all
    /// join the family, the first drawing by default).
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, FontError> {
        let faces = FaceSource::from_bytes(bytes)?;
        let first = faces[0].info.clone();
        let style = FontStyle {
            weight: FontWeight(first.weight),
            italic: first.italic,
        };
        let name = Some(first.family).filter(|n| !n.is_empty());
        Font::from_sources(name, faces, style)
    }

    /// Probe common operating-system font locations and load the first
    /// usable sans-serif face, with a symbol fallback and the system's
    /// fallbacks for CJK and emoji (see
    /// [`with_system_fallbacks`](Self::with_system_fallbacks): each is loaded
    /// when text first needs it). Returns `None` if none is found.
    ///
    /// This is what an [`App`](crate::App) draws with when it sets no font of
    /// its own. To choose, use [`Font::from_family`] (a family the user
    /// picked, or a generic one such as `"sans-serif"`), or bundle fonts and
    /// use [`Font::from_bytes`].
    ///
    /// On the web, where there are no system fonts, it's the font the page
    /// handed the app (`stipple.js`'s `font` option), if any.
    pub fn system_default() -> Option<Self> {
        #[cfg(target_arch = "wasm32")]
        {
            crate::platform::backend::web::page_font()
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Self::probe_system_default()
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn probe_system_default() -> Option<Self> {
        const CANDIDATES: &[&str] = &[
            // Linux (Liberation / DejaVu / Noto are near-ubiquitous).
            "/usr/share/fonts/liberation-fonts/LiberationSans-Regular.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
            "/usr/share/fonts/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/usr/share/fonts/noto/NotoSans-Regular.ttf",
            // macOS.
            "/System/Library/Fonts/SFNS.ttf",
            "/System/Library/Fonts/Helvetica.ttc",
            "/Library/Fonts/Arial.ttf",
            // Windows.
            "C:\\Windows\\Fonts\\segoeui.ttf",
            "C:\\Windows\\Fonts\\arial.ttf",
            // Android.
            "/system/fonts/Roboto-Regular.ttf",
            "/system/fonts/NotoSans-Regular.ttf",
            "/system/fonts/DroidSans.ttf",
        ];
        // Faces consulted for glyphs the main font lacks, such as the checkbox's
        // check mark, which Liberation Sans doesn't have.
        const FALLBACKS: &[&str] = &[
            "/usr/share/fonts/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/System/Library/Fonts/Apple Symbols.ttf",
            "C:\\Windows\\Fonts\\seguisym.ttf",
            "/system/fonts/NotoSansSymbols-Regular-Subsetted.ttf",
        ];
        let font = CANDIDATES
            .iter()
            .find_map(|path| Font::from_file(path).ok())?;
        let fallbacks: Vec<Font> = FALLBACKS
            .iter()
            .filter_map(|path| Font::from_file(path).ok())
            .collect();
        Some(font.refamily(&fallbacks, true))
    }

    /// Distance from the top of the text box to the baseline, in logical
    /// pixels, at `size_px`.
    pub fn ascent(&self, size_px: f64) -> f64 {
        self.chain().primary().ascent_px(size_px as f32) as f64
    }

    /// Distance from the top of a line to its baseline at `size_px`, placing
    /// capital letters at the line's vertical center.
    ///
    /// Fonts disagree about what "ascent" means and where line spacing goes, so a
    /// box's metrics alone leave text visibly high or low in a button. Measuring
    /// the capitals centers text the same way under any font.
    pub fn baseline(&self, size_px: f64) -> f64 {
        (self.line_height(size_px) + self.cap_height_em() * size_px) / 2.0
    }

    /// The measured height of a capital "H", per em.
    pub(crate) fn cap_height_em(&self) -> f64 {
        *self
            .variant
            .cap_height
            .get_or_init(|| self.measure_ink_height("H"))
    }

    /// The measured height of an "x", per em.
    pub(crate) fn x_height_em(&self) -> f64 {
        *self
            .variant
            .x_height
            .get_or_init(|| self.measure_ink_height("x"))
    }

    /// Rasterize `text` at a large size and measure its inked height, per em.
    fn measure_ink_height(&self, text: &str) -> f64 {
        const EM: f64 = 128.0;
        let side = (EM * 2.0) as u32;
        let mut scene = Scene::new(Size::new(side as f64, side as f64));
        let glyphs = Shaper::shape_to_paths(&self.chain(), text, EM as f32);
        let run = glyphs
            .into_iter()
            .map(|(_, node, transform)| {
                let glyph = Group::new()
                    .with_transform(transform)
                    .with_child(recolor(node, Color::WHITE));
                Node::Group(glyph)
            })
            .collect();
        let placed = Group::new()
            .with_transform(Transform2D::translate(8.0, EM as f32 * 1.5))
            .with_children(run);
        scene.push_node(Node::Group(placed));
        let pixmap = SoftwareRenderer::new().render(scene, ScaleFactor::IDENTITY);
        let inked = |y: u32| (0..side).any(|x| pixmap.pixel(x, y).is_some_and(|p| p[3] > 127));
        let top = (0..side).find(|y| inked(*y));
        let bottom = (0..side).rev().find(|y| inked(*y));
        match (top, bottom) {
            (Some(top), Some(bottom)) => (bottom - top + 1) as f64 / EM,
            // A font without the letter: its ascent is the closest thing it
            // reports.
            _ => self.ascent(1.0),
        }
    }

    /// Full line height (ascent + descent + line gap) at `size_px`.
    pub fn line_height(&self, size_px: f64) -> f64 {
        self.chain().primary().line_height_px(size_px as f32) as f64
    }

    /// Summed advance width of a single line (no newlines) at `size_px`.
    fn line_width(&self, line: &str, size_px: f64) -> f64 {
        self.shape_cached(line, size_px)
            .iter()
            .map(|g| g.x_advance)
            .sum::<f32>() as f64
    }

    /// Measure the rendered size of `text` at `size_px`: the widest line's
    /// advance width × the number of newline-separated lines times line height.
    /// A trailing newline counts as an extra (empty) line.
    pub fn measure(&self, text: &str, size_px: f64) -> Size {
        let mut max_w: f64 = 0.0;
        let mut lines = 0usize;
        for line in text.split('\n') {
            lines += 1;
            max_w = max_w.max(self.line_width(line, size_px));
        }
        Size::new(max_w, self.line_height(size_px) * lines as f64)
    }

    /// Greedily wrap `text` to lines no wider than `max_width` logical pixels at
    /// `size_px`, breaking at spaces. Existing `\n` are hard breaks. A single
    /// word wider than `max_width` is kept on its own (over-long) line rather
    /// than split mid-word. Returns at least one line.
    pub fn wrap(&self, text: &str, size_px: f64, max_width: f64) -> Vec<String> {
        let styling = Styling::plain(self, size_px);
        let mut out = Vec::new();
        let mut start = 0;
        for hard in text.split('\n') {
            let para = start..start + hard.len();
            for range in styling.wrap_ranges(text, para, max_width) {
                out.push(hard[range].to_string());
            }
            start += hard.len() + 1;
        }
        out
    }

    /// Lay out `text` at `size_px` for drawing, caret placement and
    /// hit-testing, running the Unicode Bidirectional Algorithm over each
    /// paragraph (`\n` separates paragraphs).
    ///
    /// `direction` sets each paragraph's base direction, or detects it from the
    /// paragraph's first strong character. With `max_width`, paragraphs
    /// word-wrap to that width as [`wrap`](Self::wrap) does. Right-to-left
    /// paragraphs are aligned to the right edge of the layout's
    /// [alignment width](TextLayout::align_width).
    pub fn layout<'a>(
        &self,
        text: &'a str,
        size_px: f64,
        direction: TextDirection,
        max_width: Option<f64>,
    ) -> TextLayout<'a> {
        self.layout_with(text, size_px, direction, max_width, LayoutDirection::Ltr)
    }

    /// [`layout`](Self::layout) with [`TextDirection::Auto`], for text inside
    /// a UI laid out in `ui` direction, as the widgets lay theirs out. Left to
    /// right it is the same as `layout`. Right to left, a paragraph with no
    /// strong character (an empty field, a number) runs right to left instead
    /// of left to right, and every line lines up against the right edge of
    /// the alignment width, whatever its own direction. (With an explicit
    /// `Ltr` or `Rtl` direction the UI's makes no difference: use `layout`.)
    pub fn layout_in<'a>(
        &self,
        text: &'a str,
        size_px: f64,
        max_width: Option<f64>,
        ui: LayoutDirection,
    ) -> TextLayout<'a> {
        self.layout_with(text, size_px, TextDirection::Auto, max_width, ui)
    }

    /// [`layout`](Self::layout) into the region `flow` describes, around its
    /// objects: floats, shapes placed anywhere, images whose transparent
    /// pixels text may run over. See
    /// [`TextFlow`](crate::render::TextFlow).
    pub fn layout_flow<'a>(
        &self,
        text: &'a str,
        size_px: f64,
        flow: &crate::render::TextFlow,
    ) -> TextLayout<'a> {
        Styling::plain(self, size_px).layout_flow(
            text,
            flow.direction,
            Some(flow.width),
            LayoutDirection::Ltr,
            Some(flow),
        )
    }

    /// [`layout`](Self::layout) in direction `direction` inside a UI laid out
    /// in `ui` direction: [`layout_in`](Self::layout_in) when `direction` is
    /// `Auto`, `layout` otherwise.
    pub(crate) fn layout_with<'a>(
        &self,
        text: &'a str,
        size_px: f64,
        direction: TextDirection,
        max_width: Option<f64>,
        ui: LayoutDirection,
    ) -> TextLayout<'a> {
        Styling::plain(self, size_px).layout(text, direction, max_width, ui)
    }
}

/// One look a stretch of a [`TextLayout`] is drawn in: a font at a size, a
/// color and decoration of its own (else the ones the layout is drawn with),
/// or an inline image in place of its one character.
#[derive(Clone, Debug)]
pub(crate) struct SpanFace {
    pub font: Font,
    pub size: f64,
    pub color: Option<Color>,
    pub decoration: Option<crate::render::Decoration>,
    pub icon: Option<(ImageSource, Size)>,
    /// A floating image anchored at this span's one character (which then
    /// takes no room in the line): text flows around it.
    pub float: Option<Arc<crate::render::flow::FloatImage>>,
}

impl SpanFace {
    /// Text in `font` at `size`, in the layout's color and decoration.
    pub(crate) fn plain(font: &Font, size: f64) -> Self {
        Self {
            font: font.clone(),
            size,
            color: None,
            decoration: None,
            icon: None,
            float: None,
        }
    }

    /// The height of the middle of the capitals above the baseline: where an
    /// inline image is centered.
    pub(super) fn cap_middle(&self) -> f64 {
        self.font.cap_height_em() * self.size / 2.0
    }

    /// How far it reaches above the baseline.
    pub(super) fn ascent(&self) -> f64 {
        match &self.icon {
            Some((_, s)) => self.cap_middle() + s.height / 2.0,
            None => self.font.baseline(self.size),
        }
    }

    /// How far it reaches below the baseline.
    pub(super) fn descent(&self) -> f64 {
        match &self.icon {
            Some((_, s)) => (s.height / 2.0 - self.cap_middle()).max(0.0),
            None => self.font.line_height(self.size) - self.font.baseline(self.size),
        }
    }

    /// The advance of `piece` (no newlines) in this look.
    fn width(&self, piece: &str) -> f64 {
        match &self.icon {
            Some((_, s)) => s.width * piece.chars().count() as f64,
            None => self.font.line_width(piece, self.size),
        }
    }
}

/// The looks of a layout's text: [`SpanFace`]s, and from which byte on each
/// is used. The first span starts at 0.
#[derive(Clone, Debug)]
pub(crate) struct Styling {
    pub(super) faces: Vec<SpanFace>,
    /// `(start byte, face index)`, by start.
    pub(super) spans: Vec<(usize, usize)>,
}

impl Styling {
    /// All of the text in `font` at `size`.
    pub(crate) fn plain(font: &Font, size: f64) -> Self {
        Self {
            faces: vec![SpanFace::plain(font, size)],
            spans: vec![(0, 0)],
        }
    }

    /// `faces`, used from each span's start byte on; `base` where no span
    /// starts at 0 (and for empty text).
    pub(crate) fn new(base: SpanFace, faces: Vec<SpanFace>, spans: Vec<(usize, usize)>) -> Self {
        let mut all = vec![base];
        all.extend(faces);
        let mut out: Vec<(usize, usize)> = spans.into_iter().map(|(s, f)| (s, f + 1)).collect();
        if out.first().is_none_or(|s| s.0 > 0) {
            out.insert(0, (0, 0));
        }
        Self {
            faces: all,
            spans: out,
        }
    }

    /// The span holding byte `b`.
    pub(super) fn span_at(&self, b: usize) -> usize {
        self.spans.partition_point(|s| s.0 <= b).saturating_sub(1)
    }

    /// Where span `k` ends.
    pub(super) fn span_end(&self, k: usize) -> usize {
        self.spans.get(k + 1).map_or(usize::MAX, |s| s.0)
    }

    /// The look of byte `b`.
    pub(super) fn face_at(&self, b: usize) -> &SpanFace {
        &self.faces[self.spans[self.span_at(b)].1]
    }

    /// The advance of `text[r]` (no newlines), each span in its look.
    pub(super) fn width(&self, text: &str, r: Range<usize>) -> f64 {
        let mut w = 0.0;
        let mut s = r.start;
        while s < r.end {
            let k = self.span_at(s);
            let e = self.span_end(k).min(r.end);
            w += self.faces[self.spans[k].1].width(&text[s..e]);
            s = e;
        }
        w
    }

    /// The byte ranges (relative to the paragraph) of the lines the
    /// paragraph `text[para]` (no `\n`) wraps into. The space a line breaks
    /// at belongs to neither line. Returns at least one (possibly empty)
    /// range.
    fn wrap_ranges(&self, text: &str, para: Range<usize>, max_width: f64) -> Vec<Range<usize>> {
        // Measure each word once and accumulate widths (a space between
        // words), rather than re-measuring the growing line.
        let p = &text[para.clone()];
        let w = |r: Range<usize>| self.width(text, para.start + r.start..para.start + r.end);
        let mut out = Vec::new();
        let mut line: Option<Range<usize>> = None;
        let mut width = 0.0;
        let mut at = 0usize;
        for word in p.split(' ') {
            let (ws, we) = (at, at + word.len());
            at = we + 1;
            let ww = w(ws..we);
            let space = if ws > 0 { w(ws - 1..ws) } else { 0.0 };
            match &mut line {
                // The first word, or a line that so far is empty (a run of
                // spaces keeps joining it).
                Some(l) if l.start == l.end => {
                    l.end = we;
                    width += space + ww;
                }
                None => {
                    line = Some(ws..we);
                    width = ww;
                }
                Some(l) if width + space + ww <= max_width => {
                    l.end = we;
                    width += space + ww;
                }
                Some(l) => {
                    out.push(l.clone());
                    line = Some(ws..we);
                    width = ww;
                }
            }
        }
        out.push(line.unwrap_or(0..0));
        out
    }

    /// Lay `text` out in these looks; see [`Font::layout_with`].
    pub(crate) fn layout(
        self,
        text: &str,
        direction: TextDirection,
        max_width: Option<f64>,
        ui: LayoutDirection,
    ) -> TextLayout<'_> {
        // Floating images take text around them: the flow engine lays it out.
        if self.faces.iter().any(|f| f.float.is_some()) {
            return self.layout_flow(text, direction, max_width, ui, None);
        }
        let start_right = ui.is_rtl() && direction == TextDirection::Auto;
        let mut lines = Vec::new();
        let mut start = 0usize;
        let mut top = 0.0;
        for para in text.split('\n') {
            let para_dir = if start_right && !has_strong(para) {
                TextDirection::Rtl
            } else {
                direction
            };
            let levels = ParagraphLevels::new(para, para_dir);
            // Byte offset of each char, plus the end.
            let offsets: Vec<usize> = para
                .char_indices()
                .map(|(i, _)| i)
                .chain(std::iter::once(para.len()))
                .collect();
            let char_at = |b: usize| offsets.partition_point(|&o| o < b);
            let ranges = match max_width {
                Some(w) if w.is_finite() => self.wrap_ranges(text, start..start + para.len(), w),
                _ => std::iter::once(0..para.len()).collect(),
            };
            for range in ranges {
                let chars = char_at(range.start)..char_at(range.end);
                let line_levels = levels.line_levels(chars.clone());
                let mut runs = self.shape_line(para, start, &offsets, chars, &line_levels);
                for run in &mut runs {
                    run.range = start + run.range.start..start + run.range.end;
                }
                let (ascent, descent) = if runs.is_empty() {
                    let f = self.face_at(start + range.start);
                    (f.ascent(), f.descent())
                } else {
                    runs.iter().fold((0.0_f64, 0.0_f64), |(a, d), r| {
                        let f = &self.faces[r.face];
                        (a.max(f.ascent()), d.max(f.descent()))
                    })
                };
                lines.push(LayoutLine {
                    range: start + range.start..start + range.end,
                    rtl: levels.base == 1,
                    width: runs.iter().map(|r| r.width).sum(),
                    runs,
                    top,
                    ascent,
                    height: ascent + descent,
                    left: 0.0,
                    slot: None,
                });
                top += ascent + descent;
            }
            start += para.len() + 1;
        }
        let width = lines.iter().map(|l| l.width).fold(0.0, f64::max);
        TextLayout {
            text,
            styling: self,
            lines,
            width,
            align_width: width,
            start_right,
            flow: None,
        }
    }

    /// Shape one display line: cut it into runs of one embedding level and
    /// one look, shape each in logical order (mirroring brackets in
    /// right-to-left runs, rule L4), and place the runs left to right in
    /// visual order (rule L2). `chars` is the line's char range within
    /// `para`, which starts at byte `para_start` of the text; `levels` are
    /// the line's levels.
    pub(super) fn shape_line(
        &self,
        para: &str,
        para_start: usize,
        offsets: &[usize],
        chars: Range<usize>,
        levels: &[u8],
    ) -> Vec<GlyphRun> {
        let n = levels.len();
        if n == 0 {
            return Vec::new();
        }
        let order = visual_order(levels);
        let mut visual_pos = vec![0usize; n];
        for (v, &l) in order.iter().enumerate() {
            visual_pos[l] = v;
        }
        let mut runs = Vec::new();
        let mut s = 0usize;
        while s < n {
            let k = self.span_at(para_start + offsets[chars.start + s]);
            let span_end = self.span_end(k);
            let mut e = s + 1;
            while e < n
                && levels[e] == levels[s]
                && para_start + offsets[chars.start + e] < span_end
            {
                e += 1;
            }
            let bytes = offsets[chars.start + s]..offsets[chars.start + e];
            let rtl = levels[s] & 1 == 1;
            let face_idx = self.spans[k].1;
            let face = &self.faces[face_idx];
            let (glyphs, shaped, width) = if let Some((_, size)) = &face.icon {
                let piece = para[bytes.clone()].to_string();
                (Vec::new(), piece, size.width * (e - s) as f64)
            } else {
                let shaped: String = if rtl {
                    para[bytes.clone()]
                        .chars()
                        .map(|c| oxideav_scribe::bidi::mirrored_glyph(c).unwrap_or(c))
                        .collect()
                } else {
                    para[bytes.clone()].to_string()
                };
                let mut glyphs = face.font.shape_cached(&shaped, face.size);
                if rtl {
                    glyphs.reverse();
                }
                let width = glyphs.iter().map(|g| g.x_advance as f64).sum();
                (glyphs, shaped, width)
            };
            let rank = (s..e).map(|i| visual_pos[i]).min().unwrap_or(s);
            runs.push((
                rank,
                GlyphRun {
                    range: bytes,
                    rtl,
                    level: levels[s],
                    x: 0.0,
                    width,
                    glyphs,
                    shaped,
                    face: face_idx,
                },
            ));
            s = e;
        }
        runs.sort_by_key(|(rank, _)| *rank);
        let mut x = 0.0;
        runs.into_iter()
            .map(|(_, mut run)| {
                run.x = x;
                x += run.width;
                run
            })
            .collect()
    }
}

impl Font {
    /// The shaping chain: the primary face of this style, then fallbacks.
    /// It can grow (a system fallback loaded for text that needs it), never
    /// shrink: a glyph's face index stays valid in later chains.
    pub(crate) fn chain(&self) -> Arc<Chain> {
        self.variant.chain()
    }

    /// `text` shaped as one left-to-right run at `size_px`, through the
    /// shaping cache (for [glyph runs](crate::render::GlyphRun)).
    pub(crate) fn shaped(&self, text: &str, size_px: f64) -> Vec<PositionedGlyph> {
        self.shape_cached(text, size_px)
    }

    /// What is synthesized for face `face_idx` of the shaping chain.
    pub(crate) fn synthesis(&self, face_idx: u16) -> crate::render::font::Synthesis {
        self.chain()
            .synth
            .get(face_idx as usize)
            .copied()
            .unwrap_or_default()
    }
}

/// Recolor an outline glyph to `color`, recursing into groups (the shaper
/// wraps each glyph's path in a cache-keyed `Group`). Non-outline leaves (e.g.
/// color-bitmap emoji `Node::Image`) are left untouched.
pub(crate) fn recolor(node: Node, color: Color) -> Node {
    embolden(node, color, 0.0)
}

/// [`recolor`], and with `stroke` > 0 also outline each glyph path that wide
/// in the same color: synthetic bold.
pub(crate) fn embolden(node: Node, color: Color, stroke: f32) -> Node {
    match node {
        Node::Path(mut path) => {
            let paint = Paint::Solid(color.to_oxideav());
            if stroke > 0.0 {
                path.stroke = Some(Stroke::new(stroke, paint.clone()).with_join(LineJoin::Round));
            }
            path.fill = Some(paint);
            Node::Path(path)
        }
        Node::Group(mut group) => {
            group.children = group
                .children
                .into_iter()
                .map(|c| embolden(c, color, stroke))
                .collect();
            Node::Group(group)
        }
        other => other,
    }
}

/// The horizontal shear of a synthesized oblique: about 12 degrees, as
/// renderers conventionally slant a face that has no italic.
pub(crate) const OBLIQUE_SHEAR: f32 = 0.2126;

/// How wide synthetic bold strokes a glyph's outline at `size_px`: each side
/// grows by half of it.
pub(crate) fn bold_stroke(size_px: f64) -> f32 {
    (size_px / 30.0).max(0.5) as f32
}

/// A key for the rasterizer's glyph bitmap cache: one face, glyph, size and
/// synthesis.
fn glyph_cache_key(face: &Face, glyph_id: u16, size_px: f64, synth: Synthesis) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    b"stipple.glyph.v2".hash(&mut h);
    face.stable_id().hash(&mut h);
    glyph_id.hash(&mut h);
    ((size_px * 256.0).round() as u32).hash(&mut h);
    synth.hash(&mut h);
    h.finish()
}

/// Text laid out for drawing and editing: paragraphs cut into display lines,
/// each line's bidirectional runs shaped and placed in visual order.
///
/// Made by [`Font::layout`] or [`RichText::layout`](crate::render::RichText::layout).
/// Positions are in logical pixels relative to the top-left of the text box.
/// Byte indices are into the text the layout was made from; caret positions
/// are logical (the order the text is stored in), and the layout maps them to
/// and from visual x positions.
#[derive(Debug)]
pub struct TextLayout<'a> {
    pub(super) text: &'a str,
    pub(super) styling: Styling,
    pub(super) lines: Vec<LayoutLine>,
    pub(super) width: f64,
    pub(super) align_width: f64,
    /// Line every line up against the right edge (text in a right-to-left UI).
    pub(super) start_right: bool,
    /// Where the objects the text flowed around went, when it was laid out
    /// by the flow engine ([`crate::render::flow`]).
    pub(super) flow: Option<Box<crate::render::flow::FlowGeometry>>,
}

/// One display line of a [`TextLayout`]: in flowed text, one segment of a
/// line band, beside or between the shapes it flows around.
#[derive(Clone, Debug)]
pub(super) struct LayoutLine {
    /// Byte range in the layout's text, without the `\n` or the space the
    /// line wrapped at.
    pub(super) range: Range<usize>,
    /// Whether the line's paragraph runs right to left.
    pub(super) rtl: bool,
    pub(super) width: f64,
    /// The line's runs, left to right.
    pub(super) runs: Vec<GlyphRun>,
    /// The line's top, below the previous lines.
    pub(super) top: f64,
    /// From the top to the baseline: the most any of its runs reaches above
    /// it.
    pub(super) ascent: f64,
    pub(super) height: f64,
    /// The left edge of the room the line was placed in (flowed text).
    pub(super) left: f64,
    /// The width of that room, when the flow engine placed the line in one:
    /// the line sits at its start (its right edge, right to left) instead of
    /// against the layout's alignment width.
    pub(super) slot: Option<f64>,
}

/// A run of one embedding level and one look, shaped.
#[derive(Clone, Debug)]
pub(super) struct GlyphRun {
    /// Byte range in the layout's text (logical order).
    pub(super) range: Range<usize>,
    pub(super) rtl: bool,
    /// Resolved embedding level.
    pub(super) level: u8,
    /// Left edge within the line.
    pub(super) x: f64,
    pub(super) width: f64,
    /// Shaped glyphs in visual (left-to-right) order (none for an image).
    pub(super) glyphs: Vec<PositionedGlyph>,
    /// The text that was shaped: the run's text, with brackets mirrored in a
    /// right-to-left run.
    pub(super) shaped: String,
    /// The index of its [`SpanFace`] in the layout's styling.
    pub(super) face: usize,
}

/// One character's box on a line.
#[derive(Clone, Copy, Debug)]
struct CharBox {
    /// Byte range of the character in the layout's text.
    start: usize,
    end: usize,
    left: f64,
    right: f64,
    /// Resolved embedding level (odd: right to left).
    level: u8,
}

impl TextLayout<'_> {
    /// Set the width right-to-left lines are aligned against (normally the
    /// width of the box the text is drawn in). Defaults to the widest line.
    pub fn align_width(mut self, width: f64) -> Self {
        self.align_width = width;
        self
    }

    /// The size of the laid-out text: the widest line by the lines' heights
    /// (each line as tall as the tallest text or image in it).
    pub fn size(&self) -> Size {
        if let Some(flow) = &self.flow {
            return flow.size;
        }
        let height = self.lines.last().map_or(0.0, |l| l.top + l.height);
        Size::new(self.width, height)
    }

    /// The number of display lines.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// The byte range of display line `line`, or `None` past the last line.
    pub fn line_range(&self, line: usize) -> Option<Range<usize>> {
        self.lines.get(line).map(|l| l.range.clone())
    }

    /// Whether display line `line` belongs to a right-to-left paragraph.
    pub fn line_is_rtl(&self, line: usize) -> bool {
        self.lines.get(line).is_some_and(|l| l.rtl)
    }

    /// The left edge of a line within the box: right-to-left lines sit against
    /// the right edge of the alignment width.
    pub(super) fn line_x(&self, line: &LayoutLine) -> f64 {
        if let Some(slot) = line.slot {
            return if line.rtl || self.start_right {
                line.left + slot - line.width
            } else {
                line.left
            };
        }
        if line.rtl || self.start_right {
            self.align_width - line.width
        } else {
            0.0
        }
    }

    /// The boxes of a line's characters, in visual (left-to-right) order,
    /// positioned within the text box.
    fn char_boxes(&self, line: &LayoutLine) -> Vec<CharBox> {
        let lx = self.line_x(line);
        let mut out = Vec::new();
        for run in &line.runs {
            let text = &self.text[run.range.clone()];
            let advances = self.run_advances(run, text);
            let mut pen = 0.0;
            let first = out.len();
            for ((i, c), adv) in text.char_indices().zip(advances) {
                let (left, right) = if run.rtl {
                    let right = run.x + run.width - pen;
                    (right - adv, right)
                } else {
                    (run.x + pen, run.x + pen + adv)
                };
                pen += adv;
                out.push(CharBox {
                    start: run.range.start + i,
                    end: run.range.start + i + c.len_utf8(),
                    left: lx + left,
                    right: lx + right,
                    level: run.level,
                });
            }
            // A right-to-left run's characters read right to left.
            if run.rtl {
                out[first..].reverse();
            }
        }
        out
    }

    /// A line's character boxes in visual order and the caret position (a
    /// byte index) at each boundary between them; see
    /// [`boundary_carets`](crate::render::bidi::boundary_carets).
    fn caret_stops(&self, line: &LayoutLine) -> (Vec<CharBox>, Vec<VisualChar>, Vec<usize>) {
        let boxes = self.char_boxes(line);
        let vis: Vec<VisualChar> = boxes
            .iter()
            .map(|c| VisualChar {
                start: c.start,
                end: c.end,
                level: c.level,
            })
            .collect();
        let carets = boundary_carets(&vis, line.range.start);
        (boxes, vis, carets)
    }

    /// The advance of each character of a run, in logical order.
    ///
    /// When the shaper made one glyph per character, glyph `i` is character
    /// `i`. Otherwise (a ligature, or an Indic cluster) the shaper doesn't say
    /// which glyphs came from which characters, so each character's advance is
    /// measured as the growth of the shaped prefix ending with it.
    fn run_advances(&self, run: &GlyphRun, text: &str) -> Vec<f64> {
        let n = text.chars().count();
        let face = &self.styling.faces[run.face];
        if face.icon.is_some() {
            return vec![run.width / n.max(1) as f64; n];
        }
        if run.glyphs.len() == n {
            let mut adv: Vec<f64> = run.glyphs.iter().map(|g| g.x_advance as f64).collect();
            if run.rtl {
                adv.reverse();
            }
            return adv;
        }
        let mut out = Vec::with_capacity(n);
        let mut prev = 0.0;
        let ends = run
            .shaped
            .char_indices()
            .skip(1)
            .map(|(i, _)| i)
            .chain(std::iter::once(run.shaped.len()));
        for end in ends {
            let w = face
                .font
                .line_width(&run.shaped[..end], face.size)
                .max(prev);
            out.push(w - prev);
            prev = w;
        }
        out
    }

    /// Of the lines sharing line `li`'s band (flowed text puts several
    /// segments side by side, around a shape), the one under `x` or nearest
    /// it.
    fn segment_near(&self, li: usize, x: f64) -> usize {
        let top = self.lines[li].top;
        let same = |l: &LayoutLine| l.slot.is_some() && (l.top - top).abs() < 1e-6;
        if !same(&self.lines[li]) {
            return li;
        }
        let mut first = li;
        while first > 0 && same(&self.lines[first - 1]) {
            first -= 1;
        }
        let dist = |l: &LayoutLine| {
            let (a, b) = (l.left, l.left + l.slot.unwrap_or(l.width));
            if x < a {
                a - x
            } else if x > b {
                x - b
            } else {
                0.0
            }
        };
        (first..=li)
            .min_by(|&a, &b| dist(&self.lines[a]).total_cmp(&dist(&self.lines[b])))
            .unwrap_or(li)
    }

    /// The display line holding caret position `index`.
    fn line_of(&self, index: usize) -> usize {
        self.lines
            .iter()
            .position(|l| index <= l.range.end)
            .unwrap_or(self.lines.len().saturating_sub(1))
    }

    /// The height of display line `line` (0 past the last).
    pub fn line_height(&self, line: usize) -> f64 {
        self.lines.get(line).map_or(0.0, |l| l.height)
    }

    /// Where the caret for byte `index` is drawn: the top of its line and its x
    /// position, relative to the text box.
    ///
    /// Where runs of different directions meet, the boundary between them
    /// belongs to the more deeply embedded run: in "abc אבג" the caret after
    /// the space is drawn at the right end of the Hebrew (before א), and the
    /// caret at the end of the text between the space and ג. Each logical
    /// position then has one place on screen, matching what
    /// [`index_at`](Self::index_at) returns for a click there.
    pub fn caret_position(&self, index: usize) -> Point {
        let li = self.line_of(index);
        let Some(line) = self.lines.get(li) else {
            return Point::new(0.0, 0.0);
        };
        let (boxes, vis, carets) = self.caret_stops(line);
        let v = caret_boundary(&vis, &carets, index);
        let x = match (boxes.get(v), boxes.last()) {
            (Some(b), _) => b.left,
            (None, Some(b)) => b.right,
            (None, None) => self.line_x(line),
        };
        Point::new(x, line.top)
    }

    /// The caret position (a byte index) nearest `point`, relative to the text
    /// box: `y` picks the line, `x` the nearer edge of the character under it.
    /// Clicking the left half of a right-to-left character puts the caret after
    /// it in the text.
    pub fn index_at(&self, point: Point) -> usize {
        if self.lines.is_empty() {
            return 0;
        }
        let li = self
            .lines
            .iter()
            .rposition(|l| l.top <= point.y)
            .unwrap_or(0);
        let li = self.segment_near(li, point.x);
        let line = &self.lines[li];
        let (boxes, _, carets) = self.caret_stops(line);
        let x = point.x;
        let n = boxes.len();
        let v = match (boxes.first(), boxes.last()) {
            (Some(first), _) if x < first.left => 0,
            (_, Some(last)) if x >= last.right => n,
            (Some(_), Some(_)) => {
                // The box under the pointer, or in a gap between boxes that
                // don't quite touch, the nearest one.
                let k = boxes
                    .iter()
                    .position(|c| x >= c.left && x < c.right)
                    .unwrap_or_else(|| {
                        let dist = |c: &CharBox| (c.left - x).abs().min((c.right - x).abs());
                        (0..n)
                            .min_by(|&a, &b| dist(&boxes[a]).total_cmp(&dist(&boxes[b])))
                            .unwrap_or(0)
                    });
                let c = &boxes[k];
                if x < (c.left + c.right) / 2.0 {
                    k
                } else {
                    k + 1
                }
            }
            _ => 0,
        };
        carets[v]
    }

    /// The rectangles that highlight the selected byte range `[start, end)`,
    /// relative to the text box: one or more per line, since in mixed-direction
    /// text a logical range can be visually discontiguous. A selection that
    /// runs past the end of a line adds a narrow mark for the line break at
    /// the line's visual end.
    pub fn selection_rects(&self, start: usize, end: usize) -> Vec<Rect> {
        let mut out = Vec::new();
        if end <= start {
            return out;
        }
        for line in &self.lines {
            if line.range.end < start || line.range.start >= end {
                continue;
            }
            let mut spans: Vec<(f64, f64)> = self
                .char_boxes(line)
                .into_iter()
                .filter(|c| c.start >= start && c.end <= end)
                .map(|c| (c.left, c.right))
                .collect();
            spans.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut merged: Vec<(f64, f64)> = Vec::new();
            for (l, r) in spans {
                match merged.last_mut() {
                    Some(last) if l <= last.1 + 0.5 => last.1 = last.1.max(r),
                    _ => merged.push((l, r)),
                }
            }
            // Hint a selected line break with a narrow mark at the line's end.
            let breaks = self.text.as_bytes().get(line.range.end) == Some(&b'\n');
            if breaks && end > line.range.end {
                let lx = self.line_x(line);
                merged.push(if line.rtl {
                    (lx - 6.0, lx)
                } else {
                    (lx + line.width, lx + line.width + 6.0)
                });
            }
            for (l, r) in merged {
                out.push(Rect::from_xywh(l, line.top, r - l, line.height));
            }
        }
        out
    }
}

/// How [`Scene::fill_text`] and [`Scene::draw_decoration`] draw text: in
/// which font, at what size, in what color, with which
/// [`Decoration`](crate::render::Decoration) (none unless set), and with an
/// outline or a shadow (none unless set).
#[derive(Clone, Debug)]
pub struct TextPaint {
    pub(crate) font: Font,
    pub(crate) size: f64,
    pub(crate) color: Color,
    pub(crate) decoration: crate::render::Decoration,
    pub(crate) outline: Option<TextOutline>,
    pub(crate) shadow: Option<TextShadow>,
}

impl TextPaint {
    /// Text in `font` (a cheap handle, cloned) at `size` logical pixels, in
    /// `color`, undecorated.
    pub fn new(font: &Font, size: f64, color: Color) -> Self {
        Self {
            font: font.clone(),
            size,
            color,
            decoration: crate::render::Decoration::NONE,
            outline: None,
            shadow: None,
        }
    }

    /// Draw the text with `decoration`'s lines.
    pub fn decoration(mut self, decoration: crate::render::Decoration) -> Self {
        self.decoration = decoration;
        self
    }

    /// Draw an outline around the glyphs, behind them.
    pub fn outline(mut self, outline: TextOutline) -> Self {
        self.outline = Some(outline);
        self
    }

    /// Draw a shadow under the glyphs (and their outline).
    pub fn shadow(mut self, shadow: TextShadow) -> Self {
        self.shadow = Some(shadow);
        self
    }

    /// The font.
    pub fn font(&self) -> &Font {
        &self.font
    }

    /// The size, in logical pixels.
    pub fn size(&self) -> f64 {
        self.size
    }

    /// The color.
    pub fn color(&self) -> Color {
        self.color
    }
}

/// A stroke around text's glyphs, drawn behind them so the letters keep
/// their shape: what makes a name readable over artwork. `width` is how far
/// it reaches beyond the glyphs' edges, in logical pixels.
///
/// ```
/// use stipple::render::{Color, TextOutline};
/// let outline = TextOutline::new(2.0, Color::BLACK);
/// assert_eq!(outline.width, 2.0);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct TextOutline {
    /// How far the outline reaches beyond the glyphs, in logical pixels.
    pub width: f64,
    /// Its color.
    pub color: Color,
}

impl TextOutline {
    /// An outline `width` logical pixels wide in `color`.
    pub fn new(width: f64, color: Color) -> Self {
        Self { width, color }
    }
}

/// A shadow under text: the glyphs (and their outline) again, `offset` away,
/// in `color`, softened by `blur`.
///
/// A blur is approximated by three stroked layers of the glyphs, each wider
/// and fainter than the last, rather than a true Gaussian: cheap, cached as
/// glyphs are, and close enough at the few pixels text shadows use.
///
/// ```
/// use stipple::geometry::Vec2;
/// use stipple::render::{Color, TextShadow};
/// let drop = TextShadow::new(Vec2::new(1.0, 2.0), Color::rgba(0, 0, 0, 160)).blur(2.0);
/// assert_eq!(drop.blur, 2.0);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct TextShadow {
    /// How far the shadow is moved from the text (x right, y down).
    pub offset: crate::geometry::Vec2,
    /// How far its edges soften, in logical pixels (0: a hard shadow).
    pub blur: f64,
    /// Its color (its alpha is the shadow's opacity).
    pub color: Color,
}

impl TextShadow {
    /// A hard shadow `offset` away in `color`.
    pub fn new(offset: crate::geometry::Vec2, color: Color) -> Self {
        Self {
            offset,
            blur: 0.0,
            color,
        }
    }

    /// Soften the shadow's edges by `blur` logical pixels.
    pub fn blur(mut self, blur: f64) -> Self {
        self.blur = blur.max(0.0);
        self
    }

    /// How far beyond the text's box the shadow can reach, given an outline
    /// `outline` wide: its offset, plus its blur and the outline around it.
    pub(crate) fn reach(&self, outline: f64) -> crate::geometry::Insets {
        let spread = self.blur + outline + 1.0;
        let (dx, dy) = (self.offset.dx, self.offset.dy);
        crate::geometry::Insets {
            top: (spread - dy).max(0.0),
            right: (spread + dx).max(0.0),
            bottom: (spread + dy).max(0.0),
            left: (spread - dx).max(0.0),
        }
    }
}

/// A pass drawing every glyph of a layout in one solid color: an outline or
/// a shadow layer.
#[derive(Clone, Copy)]
struct Solid {
    /// The extra stroke width around each glyph (both sides together).
    stroke: f32,
    /// Mixed into the glyphs' cache keys, so the pass's bitmaps don't stand
    /// in for the fill's.
    salt: u64,
    /// The pass's opacity (its color is drawn opaque, the pass composited
    /// at this).
    opacity: f32,
}

impl Solid {
    fn new(stroke: f64, color: Color, opacity: f32) -> Self {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        b"stipple.text.solid".hash(&mut h);
        (stroke as f32).to_bits().hash(&mut h);
        (color.r, color.g, color.b).hash(&mut h);
        Self {
            stroke: stroke as f32,
            salt: h.finish(),
            opacity,
        }
    }
}

/// `key` made distinct for a pass salted `salt`.
fn salted(key: u64, salt: u64) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    salt.hash(&mut h);
    h.finish()
}

impl Scene {
    /// Draw `decoration`'s lines under, through or over every line of
    /// `layout`, placed as [`fill_layout`](Self::fill_layout) draws them at
    /// `origin`, using the layout's font metrics. `text_color` colors lines
    /// the decoration gives no color of its own. A stretch of rich text with a
    /// decoration (or color) of its own draws that instead.
    pub fn decorate_layout(
        &mut self,
        layout: &TextLayout<'_>,
        origin: Point,
        decoration: &crate::render::Decoration,
        text_color: Color,
    ) {
        if decoration.is_none()
            && layout
                .styling
                .faces
                .iter()
                .all(|f| f.decoration.is_none_or(|d| d.is_none()))
        {
            return;
        }
        for line in &layout.lines {
            if line.runs.is_empty() {
                continue;
            }
            let x = origin.x + layout.line_x(line);
            let y = origin.y + line.top + line.ascent;
            // Neighboring runs in one look draw one stretch of lines.
            let mut i = 0;
            while i < line.runs.len() {
                let fi = line.runs[i].face;
                let mut j = i + 1;
                while j < line.runs.len() && line.runs[j].face == fi {
                    j += 1;
                }
                let face = &layout.styling.faces[fi];
                let deco = face.decoration.unwrap_or(*decoration);
                if face.icon.is_none() && !deco.is_none() && face.size > 0.0 {
                    let x0 = line.runs[i].x;
                    let last = &line.runs[j - 1];
                    let metrics = face.font.metrics(face.size);
                    self.decoration_lines(
                        &metrics,
                        Point::new(x + x0, y),
                        last.x + last.width - x0,
                        &deco,
                        face.color.unwrap_or(text_color),
                    );
                }
                i = j;
            }
        }
    }

    /// Shape and paint `text` at `origin` (the top-left of the text box,
    /// logical pixels) in `paint`'s font, size and color, with its
    /// decoration, outline and shadow if it has them. Newlines (`\n`) start a
    /// new line, each dropped by one `line_height` from the last.
    ///
    /// Each line goes through the Unicode Bidirectional Algorithm, with its
    /// direction taken from its first strong character; right-to-left lines
    /// are aligned to the right edge of the widest line. Use
    /// [`Font::layout`] and [`Scene::fill_layout`] to choose the direction or
    /// the alignment width.
    ///
    /// ```no_run
    /// # use stipple::geometry::{Point, Size};
    /// # use stipple::render::{Color, Font, Scene, TextPaint};
    /// # let font = Font::system_default().unwrap();
    /// let mut scene = Scene::new(Size::new(200.0, 40.0));
    /// scene.fill_text("Hello", Point::new(4.0, 4.0), &TextPaint::new(&font, 16.0, Color::BLACK));
    /// ```
    pub fn fill_text(&mut self, text: &str, origin: Point, paint: &TextPaint) {
        if text.is_empty() || paint.size <= 0.0 {
            return;
        }
        let layout = paint
            .font
            .layout(text, paint.size, TextDirection::Auto, None);
        self.paint_layout(&layout, origin, paint);
    }

    /// Draw `layout` at `origin` as `paint` says (its color, decoration,
    /// outline and shadow; the layout has its own font and size).
    pub(crate) fn paint_layout(
        &mut self,
        layout: &TextLayout<'_>,
        origin: Point,
        paint: &TextPaint,
    ) {
        let outline = paint.outline.map_or(0.0, |o| o.width.max(0.0));
        if let Some(shadow) = &paint.shadow {
            self.shadow_pass(layout, origin, shadow, outline);
        }
        if let Some(o) = &paint.outline {
            self.outline_layout(layout, origin, o);
        }
        self.fill_layout(layout, origin, paint.color);
        self.decorate_layout(layout, origin, &paint.decoration, paint.color);
    }

    /// Draw `outline` around the glyphs of `layout` at `origin`, as
    /// [`fill_layout`](Self::fill_layout) would place them: call it first,
    /// so the fill covers the outline's inner half.
    pub fn outline_layout(
        &mut self,
        layout: &TextLayout<'_>,
        origin: Point,
        outline: &TextOutline,
    ) {
        if outline.width <= 0.0 || outline.color.a == 0 {
            return;
        }
        let opaque = outline.color.with_alpha(255);
        let pass = Solid::new(
            outline.width * 2.0,
            opaque,
            f32::from(outline.color.a) / 255.0,
        );
        self.glyph_pass(layout, origin, opaque, Some(pass));
    }

    /// Draw `shadow` under the glyphs of `layout` at `origin`, as
    /// [`fill_layout`](Self::fill_layout) would place them: call it first.
    pub fn shadow_layout(&mut self, layout: &TextLayout<'_>, origin: Point, shadow: &TextShadow) {
        self.shadow_pass(layout, origin, shadow, 0.0);
    }

    /// [`shadow_layout`](Self::shadow_layout) for text with an outline
    /// `outline` wide, which the shadow takes the shape of.
    fn shadow_pass(
        &mut self,
        layout: &TextLayout<'_>,
        origin: Point,
        shadow: &TextShadow,
        outline: f64,
    ) {
        if shadow.color.a == 0 {
            return;
        }
        let at = Point::new(origin.x + shadow.offset.dx, origin.y + shadow.offset.dy);
        let opaque = shadow.color.with_alpha(255);
        let alpha = f64::from(shadow.color.a) / 255.0;
        if shadow.blur <= 0.0 {
            let pass = Solid::new(outline * 2.0, opaque, alpha as f32);
            self.glyph_pass(layout, at, opaque, Some(pass));
            return;
        }
        // Layers, each wider and fainter, stacking up to the shadow's
        // opacity where they all cover.
        const LAYERS: u32 = 3;
        let each = 1.0 - (1.0 - alpha).powf(1.0 / f64::from(LAYERS));
        for i in (1..=LAYERS).rev() {
            let spread = shadow.blur * f64::from(i) / f64::from(LAYERS);
            let pass = Solid::new((outline + spread) * 2.0, opaque, each as f32);
            self.glyph_pass(layout, at, opaque, Some(pass));
        }
    }

    /// Paint a [`TextLayout`] with its top-left at `origin`, in `color` (rich
    /// text's stretches with a color of their own in that, and its images).
    ///
    /// Glyphs are emitted as scene-graph nodes under a group translated to each
    /// line's baseline, so they rasterize and composite like any other
    /// primitive.
    pub fn fill_layout(&mut self, layout: &TextLayout<'_>, origin: Point, color: Color) {
        self.glyph_pass(layout, origin, color, None);
    }

    /// Emit the glyphs (and, for the fill, the images) of `layout` at
    /// `origin`: the fill in each look's color (else `color`), or with
    /// `solid` every glyph in `color`, stroked as it says.
    fn glyph_pass(
        &mut self,
        layout: &TextLayout<'_>,
        origin: Point,
        color: Color,
        solid: Option<Solid>,
    ) {
        if solid.is_none()
            && let Some(flow) = &layout.flow
        {
            for (rect, source) in &flow.images {
                self.draw_image_source(
                    rect.translate(crate::geometry::Vec2::new(origin.x, origin.y)),
                    source,
                    crate::render::ImageFit::Fill,
                );
            }
        }
        for line in &layout.lines {
            if line.runs.is_empty() {
                continue;
            }
            let lx = layout.line_x(line);
            let top = origin.y + line.top;
            if solid.is_none() {
                // Record a structured command for the GPU backend (the CPU
                // rasterizer uses the glyph nodes pushed below).
                self.record_text(
                    &layout.text[line.range.clone()],
                    Point::new(origin.x + lx, top),
                    layout.styling.faces[0].size,
                    color,
                );
            }
            let baseline = top + line.ascent;
            let mut glyphs = Vec::new();
            let mut images = Vec::new();
            for run in &line.runs {
                let face = &layout.styling.faces[run.face];
                if let Some((source, size)) = &face.icon {
                    if solid.is_none() && size.width > 0.0 {
                        let y = baseline - face.cap_middle() - size.height / 2.0;
                        let n = layout.text[run.range.clone()].chars().count();
                        for k in 0..n {
                            let x = origin.x + lx + run.x + k as f64 * size.width;
                            images.push((Rect::from_xywh(x, y, size.width, size.height), source));
                        }
                    }
                    continue;
                }
                let (font, size) = (&face.font, face.size);
                if size <= 0.0 {
                    continue;
                }
                let fill = match solid {
                    Some(_) => color,
                    None => face.color.unwrap_or(color),
                };
                let chain = font.chain();
                let mut pen = run.x as f32;
                for g in &run.glyphs {
                    let x = pen + g.x_offset;
                    pen += g.x_advance;
                    let gface = chain.face(g.face_idx);
                    // Cached: building a glyph's node re-parses its font.
                    let Some(node) = super::glyphs::outline(font, g.face_idx, g.glyph_id, size)
                    else {
                        continue;
                    };
                    let synth = chain
                        .synth
                        .get(g.face_idx as usize)
                        .copied()
                        .unwrap_or_default();
                    let mut stroke = if synth.bold { bold_stroke(size) } else { 0.0 };
                    let mut key = glyph_cache_key(gface, g.glyph_id, size, synth);
                    if let Some(s) = solid {
                        stroke += s.stroke;
                        key = salted(key, s.salt);
                    }
                    // Wrapped in a cache-keyed group so the rasterizer reuses
                    // the glyph's bitmap wherever it's drawn.
                    let cached = Group {
                        cache_key: Some(key),
                        children: vec![embolden(node, fill, stroke)],
                        ..Group::default()
                    };
                    // A synthesized oblique leans the glyph about its baseline
                    // point (y grows downward, so up is negative y).
                    let shear = if synth.oblique { -OBLIQUE_SHEAR } else { 0.0 };
                    let place = Transform2D {
                        a: 1.0,
                        b: 0.0,
                        c: shear,
                        d: 1.0,
                        e: x,
                        f: g.y_offset,
                    };
                    let glyph = Group::new()
                        .with_transform(place)
                        .with_child(Node::Group(cached));
                    glyphs.push(Node::Group(glyph));
                }
            }
            if !glyphs.is_empty() {
                let mut placed = Group::new()
                    .with_transform(Transform2D::translate(
                        (origin.x + lx) as f32,
                        baseline as f32,
                    ))
                    .with_children(glyphs);
                if let Some(s) = solid {
                    placed.opacity = s.opacity;
                }
                self.push_node(Node::Group(placed));
            }
            for (rect, source) in images {
                self.draw_image_source(rect, source, crate::render::ImageFit::Fill);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::ScaleFactor;
    use crate::render::SoftwareRenderer;

    /// Capitals sit at the center of the line, under whichever font this host has.
    #[test]
    fn capitals_are_centered_in_the_line() {
        let Some(font) = Font::system_default() else {
            return;
        };
        for size in [14.0, 28.0] {
            let mut scene = Scene::new(Size::new(200.0, 60.0));
            let white = Color::rgb(255, 255, 255);
            scene.fill_text(
                "HEH",
                Point::new(0.0, 10.0),
                &TextPaint::new(&font, size, white),
            );
            let px = crate::render::software::SoftwareRenderer::new()
                .render(scene, crate::geometry::ScaleFactor::IDENTITY);
            let inked = |y: u32| (0..200).any(|x| px.pixel(x, y).is_some_and(|p| p[3] > 127));
            let top = (0..60).find(|y| inked(*y)).unwrap() as f64;
            let bottom = (0..60).rev().find(|y| inked(*y)).unwrap() as f64 + 1.0;
            let center = (top + bottom) / 2.0 - 10.0;
            let want = font.line_height(size) / 2.0;
            assert!(
                (center - want).abs() <= 1.0,
                "{center} vs {want} at {size}px"
            );
        }
    }

    #[test]
    fn measure_is_monotonic_in_length() {
        let Some(font) = Font::system_default() else {
            eprintln!("skipping: no system font found");
            return;
        };
        let short = font.measure("i", 16.0);
        let long = font.measure("internationalization", 16.0);
        assert!(long.width > short.width);
        assert!(short.height > 0.0);
    }

    #[test]
    fn measure_counts_newline_separated_lines() {
        let Some(font) = Font::system_default() else {
            eprintln!("skipping: no system font found");
            return;
        };
        let one = font.measure("Hello", 16.0);
        let two = font.measure("Hello\nWorld!", 16.0);
        // Two lines are about twice as tall as one (within rounding).
        assert!((two.height - 2.0 * one.height).abs() < 1.0);
        // Width is the widest line ("World!" > "Hello").
        assert!(two.width >= one.width);
        // A trailing newline adds an (empty) third line of height.
        let trailing = font.measure("Hello\nWorld!\n", 16.0);
        assert!((trailing.height - 3.0 * one.height).abs() < 1.0);
    }

    #[test]
    fn wrap_breaks_at_spaces_within_width() {
        let Some(font) = Font::system_default() else {
            eprintln!("skipping: no system font found");
            return;
        };
        let text = "the quick brown fox jumps over the lazy dog";
        let full = font.measure(text, 16.0).width;
        // Wrapping to half the natural width yields more than one line, and no
        // wrapped line exceeds that width (each individual word fits).
        let lines = font.wrap(text, 16.0, full / 2.0);
        assert!(lines.len() > 1);
        for line in &lines {
            assert!(font.measure(line, 16.0).width <= full / 2.0 + 0.5);
        }
        // Hard newlines are preserved as breaks.
        assert_eq!(font.wrap("a\nb", 16.0, 10_000.0), vec!["a", "b"]);
    }

    /// Dark pixels in `pm` (glyph ink on white).
    fn dark_pixels(pm: &crate::render::Pixmap) -> usize {
        let mut n = 0;
        for y in 0..pm.size().height {
            for x in 0..pm.size().width {
                if let Some([r, _, _, _]) = pm.pixel(x, y)
                    && r < 128
                {
                    n += 1;
                }
            }
        }
        n
    }

    /// The system's CJK family (a CFF-outline collection on macOS, Android
    /// and most Linux distributions) shapes and draws Japanese, alone and as
    /// a fallback.
    #[test]
    fn system_cjk_font_draws_japanese() {
        let Ok(cjk) = Font::from_family("cjk", FontStyle::NORMAL) else {
            eprintln!("skipping: no system CJK font");
            return;
        };
        eprintln!("cjk family: {:?}", cjk.family_name());
        // Without a CJK font installed the lookup can land on a general
        // face (DejaVu Sans on a bare Linux): nothing to test then.
        if !cjk.has_glyph('日') {
            eprintln!("skipping: the system's CJK family has no CJK glyphs");
            return;
        }
        let text = "日本語";
        let shaped = cjk.shaped(text, 32.0);
        assert_eq!(shaped.len(), 3, "{shaped:?}");
        assert!(shaped.iter().all(|g| g.glyph_id != 0), "{shaped:?}");
        assert!(cjk.measure(text, 32.0).width > 60.0);
        let draw = |font: &Font| {
            let mut scene = Scene::new(Size::new(160.0, 50.0));
            scene.fill_rect(Rect::from_xywh(0.0, 0.0, 160.0, 50.0), Color::WHITE);
            scene.fill_text(
                text,
                Point::new(4.0, 4.0),
                &TextPaint::new(font, 32.0, Color::BLACK),
            );
            dark_pixels(&SoftwareRenderer::new().render(scene, ScaleFactor::IDENTITY))
        };
        let alone = draw(&cjk);
        assert!(alone > 150, "expected CJK glyph ink, got {alone}");
        // As a fallback of a Latin font, the same characters come from it.
        if let Some(latin) = Font::system_default() {
            let chained = draw(&latin.with_fallback(&cjk));
            assert!(chained > 150, "expected fallback ink, got {chained}");
            // The default font loads the system's CJK family by itself, the
            // first time text needs it.
            let before = latin.chain().len();
            let lazily = draw(&latin);
            assert!(lazily > 150, "expected system fallback ink, got {lazily}");
            assert!(latin.chain().len() > before);
            assert!(latin.shaped(text, 32.0).iter().all(|g| g.glyph_id != 0));
        }
    }

    /// Only text a system fallback could help loads one, and each family is
    /// tried once.
    #[test]
    fn system_fallbacks_are_chosen_by_character() {
        use crate::render::font::system_fallback_for as pick;
        assert_eq!(pick('a', &[]), None);
        assert_eq!(pick('\u{200D}', &[]), None);
        assert_eq!(pick('\u{3000}', &[]), None, "ideographic space");
        assert_eq!(pick('日', &[]), Some("cjk"));
        assert_eq!(pick('カ', &[]), Some("cjk"));
        assert_eq!(pick('日', &["cjk"]), None);
        assert_eq!(pick('한', &[]), Some("cjk"));
        assert_eq!(pick('한', &["cjk"]), Some("korean"));
        assert_eq!(pick('😀', &[]), Some("emoji"));
        assert_eq!(pick('✓', &["emoji"]), Some("symbol"));
        assert_eq!(pick('→', &[]), Some("symbol"));
        assert_eq!(pick('→', &["symbol"]), None);
    }

    #[test]
    fn text_paints_visible_pixels() {
        let Some(font) = Font::system_default() else {
            eprintln!("skipping: no system font found");
            return;
        };
        let mut scene = Scene::new(Size::new(200.0, 50.0));
        // White background so black text stands out.
        scene.fill_rect(Rect::from_xywh(0.0, 0.0, 200.0, 50.0), Color::WHITE);
        scene.fill_text(
            "Hello",
            Point::new(8.0, 8.0),
            &TextPaint::new(&font, 28.0, Color::BLACK),
        );

        let pm = SoftwareRenderer::new().render(scene, ScaleFactor::IDENTITY);
        // Some pixel must be darkened by a glyph (not pure white).
        let mut darkened = 0;
        for y in 0..pm.size().height {
            for x in 0..pm.size().width {
                if let Some([r, _, _, _]) = pm.pixel(x, y)
                    && r < 128
                {
                    darkened += 1;
                }
            }
        }
        assert!(
            darkened > 20,
            "expected glyph coverage, got {darkened} dark pixels"
        );
    }

    #[test]
    fn text_uses_requested_color() {
        let Some(font) = Font::system_default() else {
            eprintln!("skipping: no system font found");
            return;
        };
        let mut scene = Scene::new(Size::new(160.0, 50.0));
        scene.fill_rect(Rect::from_xywh(0.0, 0.0, 160.0, 50.0), Color::WHITE);
        // Pure red text: glyph interiors must be red, not the default black.
        scene.fill_text(
            "RED",
            Point::new(8.0, 8.0),
            &TextPaint::new(&font, 32.0, Color::rgb(255, 0, 0)),
        );

        let pm = SoftwareRenderer::new().render(scene, ScaleFactor::IDENTITY);
        let mut reddish = 0;
        for y in 0..pm.size().height {
            for x in 0..pm.size().width {
                if let Some([r, g, b, _]) = pm.pixel(x, y)
                    && r > 180
                    && g < 80
                    && b < 80
                {
                    reddish += 1;
                }
            }
        }
        assert!(
            reddish > 20,
            "expected red glyph coverage, got {reddish} red pixels"
        );
    }

    /// Byte index of char `c` in `s`.
    fn byte(s: &str, c: usize) -> usize {
        s.char_indices().nth(c).map(|(i, _)| i).unwrap_or(s.len())
    }

    /// The system font, if it (or a fallback) has Hebrew letters.
    fn hebrew_font() -> Option<Font> {
        let font = Font::system_default()?;
        let glyphs = font.chain().shape("א", 16.0).ok()?;
        (glyphs.first()?.glyph_id != 0).then_some(font)
    }

    const MIXED: &str = "abc אבג 123 def";

    #[test]
    fn mixed_line_is_laid_out_in_visual_order() {
        let Some(font) = hebrew_font() else { return };
        let layout = font.layout(MIXED, 16.0, TextDirection::Auto, None);
        let x = |c: usize| layout.caret_position(byte(MIXED, c)).x;
        // Visually: "abc 123 גבא def". The left-to-right parts advance left to
        // right; the Hebrew runs right to left.
        assert_eq!(x(0), 0.0);
        assert!(x(0) < x(1) && x(1) < x(2) && x(2) < x(3));
        assert!(
            x(8) < x(9) && x(9) < x(10),
            "the number reads left to right"
        );
        assert!(x(6) < x(5) && x(5) < x(4), "the Hebrew reads right to left");
        // The number sits right after "abc ", left of the Hebrew.
        assert!(x(3) < x(8) && x(10) < x(6));
        // Before א is its right edge, where the trailing space starts. After
        // "123" (logically just before that space) the caret stays with the
        // more deeply embedded number, right after its "3".
        assert!(x(5) < x(4) && x(4) < x(12));
        assert!(x(10) < x(11) && x(11) < x(6));
        assert!(x(12) < x(13) && x(14) < x(15));
        assert!(!layout.line_is_rtl(0));
        let runs = &layout.lines[0].runs;
        assert_eq!(runs.len(), 4, "{runs:?}");
        assert!(!runs[0].rtl && !runs[1].rtl && runs[2].rtl && !runs[3].rtl);
    }

    #[test]
    fn clicks_map_to_logical_positions() {
        let Some(font) = hebrew_font() else { return };
        let layout = font.layout(MIXED, 16.0, TextDirection::Auto, None);
        let x = |c: usize| layout.caret_position(byte(MIXED, c)).x;
        let at = |px: f64| layout.index_at(Point::new(px, 5.0));
        // א spans x(5)..x(4): its left half puts the caret after it, its right
        // half before it.
        assert_eq!(at(x(5) + 1.0), byte(MIXED, 5));
        assert_eq!(at(x(4) - 1.0), byte(MIXED, 4));
        // Left of everything is the start; right of everything the end.
        assert_eq!(at(-10.0), 0);
        assert_eq!(at(10_000.0), MIXED.len());
        // Every position whose place is unambiguous round-trips.
        for c in [1, 2, 9, 13] {
            assert_eq!(at(x(c)), byte(MIXED, c), "char {c}");
        }
    }

    #[test]
    fn selection_across_a_direction_change_is_split() {
        let Some(font) = hebrew_font() else { return };
        let layout = font.layout(MIXED, 16.0, TextDirection::Auto, None);
        // "c אב": "c " sits left of the number, "אב" right of it.
        let rects = layout.selection_rects(byte(MIXED, 2), byte(MIXED, 6));
        assert_eq!(rects.len(), 2, "{rects:?}");
        // A left-to-right-only range is one rectangle.
        assert_eq!(layout.selection_rects(0, 3).len(), 1);
        assert!(layout.selection_rects(3, 3).is_empty());
    }

    #[test]
    fn rtl_paragraphs_align_right() {
        let Some(font) = hebrew_font() else { return };
        let text = "שלום עולם";
        let layout = font
            .layout(text, 16.0, TextDirection::Auto, None)
            .align_width(300.0);
        assert!(layout.line_is_rtl(0));
        let w = layout.size().width;
        assert!((layout.caret_position(0).x - 300.0).abs() < 0.01);
        assert!((layout.caret_position(text.len()).x - (300.0 - w)).abs() < 0.01);
        // Forcing left-to-right puts the same text at the left edge, still
        // reading right to left within its run.
        let ltr = font
            .layout(text, 16.0, TextDirection::Ltr, None)
            .align_width(300.0);
        assert!(!ltr.line_is_rtl(0));
        assert!((ltr.caret_position(0).x - w).abs() < 0.01);
        assert!(ltr.caret_position(text.len()).x.abs() < 0.01);
        // An empty right-to-left field puts its caret at the right edge.
        let empty = font
            .layout("", 16.0, TextDirection::Rtl, None)
            .align_width(120.0);
        assert_eq!(empty.caret_position(0).x, 120.0);
    }

    #[test]
    fn text_in_a_mirrored_ui_starts_on_the_right() {
        let Some(font) = hebrew_font() else { return };
        let rtl_ui = LayoutDirection::Rtl;
        // No strong character: right to left in a mirrored UI, left to right
        // otherwise. The caret of an empty field sits at the right edge.
        for neutral in ["", " ", "123", "(42)"] {
            let mirrored = font
                .layout_in(neutral, 16.0, None, rtl_ui)
                .align_width(120.0);
            assert!(mirrored.line_is_rtl(0), "{neutral:?}");
            let plain = font.layout(neutral, 16.0, TextDirection::Auto, None);
            assert!(!plain.line_is_rtl(0), "{neutral:?}");
        }
        let empty = font.layout_in("", 16.0, None, rtl_ui).align_width(120.0);
        assert_eq!(empty.caret_position(0).x, 120.0);
        // Latin text keeps its left-to-right reading order but lines up right;
        // each line of a multi-line text decides its own direction.
        let text = "abc\n12";
        let layout = font.layout_in(text, 16.0, None, rtl_ui).align_width(300.0);
        assert!(!layout.line_is_rtl(0) && layout.line_is_rtl(1));
        let w = font.measure("abc", 16.0).width;
        assert!((layout.caret_position(0).x - (300.0 - w)).abs() < 0.01);
        assert!((layout.caret_position(3).x - 300.0).abs() < 0.01);
        // An explicit direction is laid out as `layout` would.
        let forced = font
            .layout_with("abc", 16.0, TextDirection::Ltr, None, rtl_ui)
            .align_width(300.0);
        assert!(forced.caret_position(0).x.abs() < 0.01);
        // Left to right, `layout_in` is `layout`.
        let a = font.layout_in("abc אבג", 16.0, None, LayoutDirection::Ltr);
        let b = font.layout("abc אבג", 16.0, TextDirection::Auto, None);
        assert_eq!(a.caret_position(5), b.caret_position(5));
    }

    #[test]
    fn rtl_paragraphs_wrap_into_right_aligned_lines() {
        let Some(font) = hebrew_font() else { return };
        let text = "אחת שתיים שלוש ארבע חמש שש שבע שמונה";
        let full = font.measure(text, 16.0).width;
        let layout = font
            .layout(text, 16.0, TextDirection::Auto, Some(full / 2.0))
            .align_width(full / 2.0);
        assert!(layout.line_count() > 1);
        for line in 0..layout.line_count() {
            assert!(layout.line_is_rtl(line));
            let start = layout.line_range(line).unwrap().start;
            // Each line starts at the right edge.
            assert!((layout.caret_position(start).x - full / 2.0).abs() < 0.01);
        }
        // The lines are the same as `wrap` makes.
        let wrapped = font.wrap(text, 16.0, full / 2.0);
        assert_eq!(wrapped.len(), layout.line_count());
    }

    #[test]
    fn arabic_letters_join() {
        let Some(font) = Font::system_default() else {
            return;
        };
        let word = "سلام";
        let Ok(joined) = font.chain().shape(word, 16.0) else {
            return;
        };
        if joined.iter().any(|g| g.glyph_id == 0) {
            return; // no Arabic font on this host
        }
        // Shaped as a word, the letters take their joining forms, unlike the
        // same letters shaped one by one (isolated).
        let isolated: Vec<u16> = word
            .chars()
            .map(|c| font.chain().shape(&c.to_string(), 16.0).unwrap()[0].glyph_id)
            .collect();
        let ids: Vec<u16> = joined.iter().map(|g| g.glyph_id).collect();
        assert_ne!(ids, isolated);
        // And the layout draws them right to left, first letter rightmost.
        let layout = font.layout(word, 16.0, TextDirection::Auto, None);
        assert!(layout.line_is_rtl(0));
        assert!(layout.caret_position(0).x > layout.caret_position(word.len()).x);
    }

    #[test]
    fn brackets_mirror_in_rtl_runs() {
        let Some(font) = hebrew_font() else { return };
        // In "א(ב)" the brackets resolve right to left, so the shaped run holds
        // the mirrored characters.
        let layout = font.layout("א(ב)", 16.0, TextDirection::Auto, None);
        let run = &layout.lines[0].runs[0];
        assert!(run.rtl);
        assert_eq!(run.shaped, "א)ב(");
    }

    /// Render `text` white on black with `font` and return the pixmap.
    fn ink(font: &Font, text: &str, size: f64) -> crate::render::Pixmap {
        let mut scene = Scene::new(Size::new(200.0, 60.0));
        scene.fill_text(
            text,
            Point::new(10.0, 5.0),
            &TextPaint::new(font, size, Color::WHITE),
        );
        SoftwareRenderer::new()
            .background(Color::BLACK)
            .render(scene, ScaleFactor::IDENTITY)
    }

    fn inked(px: &crate::render::Pixmap) -> usize {
        let s = px.size();
        (0..s.height)
            .flat_map(|y| (0..s.width).map(move |x| (x, y)))
            .filter(|&(x, y)| px.pixel(x, y).is_some_and(|p| p[0] > 127))
            .count()
    }

    /// The first font of `families` this host has.
    fn any_family(families: &[&str]) -> Option<Font> {
        families
            .iter()
            .find_map(|f| Font::from_family(f, FontStyle::NORMAL).ok())
    }

    #[test]
    fn styled_variants_are_shared_and_distinct() {
        let Some(font) = Font::system_default() else {
            return;
        };
        let bold = font.bold();
        assert_eq!(bold, font.bold(), "built once, then shared");
        assert_ne!(bold, font);
        assert_ne!(bold.id(), font.id());
        assert_eq!(bold.style().weight, FontWeight::BOLD);
        assert_eq!(bold.styled(font.style()), font);
        // Each variant shapes into its own cache: the same text measured in
        // both styles keeps both results.
        let (a, b) = (
            font.measure("Wide text", 20.0),
            bold.measure("Wide text", 20.0),
        );
        assert_eq!(a, font.measure("Wide text", 20.0));
        assert_eq!(b, bold.measure("Wide text", 20.0));
    }

    #[test]
    fn a_single_face_synthesizes_bold_and_oblique() {
        let Some(path) = [
            "/usr/share/fonts/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/usr/share/fonts/liberation-fonts/LiberationSans-Regular.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        ]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists()) else {
            return;
        };
        let regular = Font::from_file(path).unwrap();
        assert_eq!(regular.style(), FontStyle::NORMAL);
        assert_eq!(regular.is_synthesized(), (false, false));
        let bold = regular.bold();
        let oblique = regular.italic();
        assert_eq!(bold.is_synthesized(), (true, false));
        assert_eq!(oblique.is_synthesized(), (false, true));
        // Emboldening adds ink; metrics (and so layout) stay the same.
        let (r, b) = (ink(&regular, "HHH", 32.0), ink(&bold, "HHH", 32.0));
        assert!(
            inked(&b) > inked(&r) * 11 / 10,
            "{} vs {}",
            inked(&b),
            inked(&r)
        );
        assert_eq!(regular.measure("HHH", 32.0), bold.measure("HHH", 32.0));
        // The oblique leans right: an "l"'s top sits right of its foot.
        let o = ink(&oblique, "l", 40.0);
        let left_at = |y: u32| (0..200).find(|&x| o.pixel(x, y).is_some_and(|p| p[0] > 127));
        let rows: Vec<u32> = (0..60).filter(|&y| left_at(y).is_some()).collect();
        let (top, foot) = (rows[0], *rows.last().unwrap());
        assert!(left_at(top).unwrap() >= left_at(foot).unwrap() + 4);
    }

    #[test]
    fn a_family_uses_its_real_bold_face() {
        let Some(font) = any_family(&["DejaVu Sans", "Liberation Sans", "Noto Sans", "Arial"])
        else {
            return;
        };
        if !font.available_styles().contains(&FontStyle::BOLD) {
            return;
        }
        assert_eq!(font.bold().is_synthesized(), (false, false));
        // A real bold face has its own (wider) advances.
        assert!(font.bold().measure("mmmm", 20.0).width > font.measure("mmmm", 20.0).width);
    }

    #[test]
    fn monospace_metrics_suit_a_terminal() {
        let Some(mono) = any_family(&["DejaVu Sans Mono", "Liberation Mono", "monospace"]) else {
            return;
        };
        let m = mono.metrics(16.0);
        assert!(m.monospace && mono.is_monospace());
        let cell = mono.advance_of('M', 16.0);
        for c in ['i', 'W', '0', ' ', '_'] {
            assert!((mono.advance_of(c, 16.0) - cell).abs() < 0.01, "{c:?}");
        }
        assert!(m.max_advance >= cell);
        assert!(m.ascent > 0.0 && m.descent > 0.0);
        assert!((m.ascent + m.descent + m.line_gap - m.line_height).abs() < 0.01);
        // Underline centred below the baseline (its top edge may touch or
        // cross it for fonts with a thick, high underline), inside the
        // descent; strikeout above it, below the x-height.
        let underline_center = m.underline_position + m.underline_thickness / 2.0;
        assert!(
            underline_center > 0.0 && m.underline_position < m.descent,
            "underline {} (thickness {}), descent {}, family {:?}",
            m.underline_position,
            m.underline_thickness,
            m.descent,
            mono.family_name()
        );
        assert!(m.underline_thickness > 0.0 && m.strikeout_thickness > 0.0);
        assert!(m.strikeout_position < 0.0 && -m.strikeout_position < m.x_height);
        assert!(m.x_height < m.cap_height && m.cap_height < m.ascent);
        // Metrics scale with the size.
        let m2 = mono.metrics(32.0);
        assert!((m2.underline_position - 2.0 * m.underline_position).abs() < 0.01);
    }

    /// The faces of `families` (that this host has) that draw `c` while
    /// `primary` doesn't.
    fn fallbacks_for(primary: &Font, c: char, families: &[&str]) -> Vec<Font> {
        let has = |f: &Font| {
            f.chain()
                .shape(&c.to_string(), 16.0)
                .is_ok_and(|g| g.first().is_some_and(|g| g.glyph_id != 0))
        };
        if has(primary) {
            return Vec::new();
        }
        families
            .iter()
            .filter_map(|f| Font::from_family(f, FontStyle::NORMAL).ok())
            .filter(|f| has(f))
            .collect()
    }

    #[test]
    fn fallbacks_are_consulted_in_order() {
        let Some(primary) = any_family(&["Liberation Sans", "Arial"]) else {
            return;
        };
        let check = '\u{2713}';
        let found = fallbacks_for(
            &primary,
            check,
            &[
                "DejaVu Sans",
                "DejaVu Sans Mono",
                "DejaVu Serif",
                "FreeSerif",
                "Symbola",
            ],
        );
        let [a, b, ..] = found.as_slice() else {
            return;
        };
        let face_of = |font: &Font| {
            let g = font.chain().shape(&check.to_string(), 16.0).unwrap()[0];
            assert_ne!(g.glyph_id, 0);
            (g.face_idx, font.chain().face(g.face_idx).stable_id())
        };
        let primary_id = |f: &Font| f.chain().primary().stable_id();
        let ab = primary.with_fallback(a).with_fallback(b);
        assert_eq!(face_of(&ab), (1, primary_id(a)));
        let ba = primary.with_fallbacks(&[b.clone(), a.clone()]);
        assert_eq!(face_of(&ba), (2 - 1, primary_id(b)));
        assert_eq!(ab.fallbacks().len(), 2);
        // Latin text stays with the primary face.
        let g = ab.chain().shape("A", 16.0).unwrap()[0];
        assert_eq!(g.face_idx, 0);
        // A bold request takes each fallback's bold face, where it has one.
        let bold = ab.bold();
        let idx = bold.chain().shape(&check.to_string(), 16.0).unwrap()[0].face_idx;
        let a_has_bold = a
            .available_styles()
            .iter()
            .any(|s| s.weight.0 >= 600 && !s.italic);
        assert_eq!(bold.chain().synth[idx as usize].bold, !a_has_bold);
        if a_has_bold {
            assert_ne!(
                bold.chain().face(idx).stable_id(),
                ab.chain().face(1).stable_id(),
                "the fallback's bold face"
            );
        }
    }
}
