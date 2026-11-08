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
use crate::render::Color;
use crate::render::bidi::{
    ParagraphLevels, TextDirection, VisualChar, boundary_carets, caret_boundary, visual_order,
};
use crate::render::scene::Scene;
use crate::render::software::SoftwareRenderer;
use core::fmt;
use core::ops::Range;
use oxideav_core::{Group, Node, Paint, Transform2D};
use oxideav_scribe::{Face, FaceChain, PositionedGlyph, Shaper};
use std::sync::OnceLock;

/// A loaded, shapeable font face.
pub struct Font {
    chain: FaceChain,
    /// Capital-letter height per em, measured once on first use.
    cap_height: OnceLock<f64>,
}

impl fmt::Debug for Font {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Font")
            .field("units_per_em", &self.chain.primary().units_per_em())
            .finish()
    }
}

/// Error loading a [`Font`].
#[derive(Debug)]
pub struct FontError(String);

impl fmt::Display for FontError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "font load error: {}", self.0)
    }
}

impl std::error::Error for FontError {}

impl Font {
    /// Load a font from `sfnt` bytes (TrueType, OpenType/CFF, or TrueType
    /// Collection — the first face of a collection is used).
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, FontError> {
        let face = parse_face(bytes)?;
        Ok(Self {
            chain: FaceChain::new(face),
            cap_height: OnceLock::new(),
        })
    }

    /// Probe common operating-system font directories and load the first
    /// usable sans-serif face. Returns `None` if none is found.
    ///
    /// Intended for examples and tests; shipping apps should bundle or
    /// explicitly locate their fonts and use [`Font::from_bytes`].
    pub fn system_default() -> Option<Self> {
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
        ];
        // Faces consulted for glyphs the main font lacks, such as the checkbox's
        // check mark, which Liberation Sans doesn't have.
        const FALLBACKS: &[&str] = &[
            "/usr/share/fonts/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/System/Library/Fonts/Apple Symbols.ttf",
            "C:\\Windows\\Fonts\\seguisym.ttf",
        ];
        let mut font = CANDIDATES.iter().find_map(|path| {
            let bytes = std::fs::read(path).ok()?;
            Font::from_bytes(bytes).ok()
        })?;
        for path in FALLBACKS {
            if let Ok(bytes) = std::fs::read(path)
                && let Ok(face) = parse_face(bytes)
            {
                font.chain = font.chain.push_fallback(face);
            }
        }
        Some(font)
    }

    /// Distance from the top of the text box to the baseline, in logical
    /// pixels, at `size_px`.
    pub fn ascent(&self, size_px: f64) -> f64 {
        self.chain.primary().ascent_px(size_px as f32) as f64
    }

    /// Distance from the top of a line to its baseline at `size_px`, placing
    /// capital letters at the line's vertical center.
    ///
    /// Fonts disagree about what "ascent" means and where line spacing goes, so a
    /// box's metrics alone leave text visibly high or low in a button. Measuring
    /// the capitals centers text the same way under any font.
    pub fn baseline(&self, size_px: f64) -> f64 {
        let cap = *self.cap_height.get_or_init(|| self.measure_cap_height());
        (self.line_height(size_px) + cap * size_px) / 2.0
    }

    /// Rasterize an "H" at a large size and measure its height, per em.
    fn measure_cap_height(&self) -> f64 {
        const EM: f64 = 128.0;
        let side = (EM * 2.0) as u32;
        let mut scene = Scene::new(Size::new(side as f64, side as f64));
        let glyphs = Shaper::shape_to_paths(self.chain(), "H", EM as f32);
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
            // A font without an "H": its ascent is the closest thing it reports.
            _ => self.ascent(1.0),
        }
    }

    /// Full line height (ascent + descent + line gap) at `size_px`.
    pub fn line_height(&self, size_px: f64) -> f64 {
        self.chain.primary().line_height_px(size_px as f32) as f64
    }

    /// Summed advance width of a single line (no newlines) at `size_px`.
    fn line_width(&self, line: &str, size_px: f64) -> f64 {
        match self.chain.shape(line, size_px as f32) {
            Ok(glyphs) => glyphs.iter().map(|g| g.x_advance).sum::<f32>() as f64,
            Err(_) => 0.0,
        }
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
        let mut out = Vec::new();
        for hard in text.split('\n') {
            for range in self.wrap_ranges(hard, size_px, max_width) {
                out.push(hard[range].to_string());
            }
        }
        out
    }

    /// The byte ranges of the lines `wrap` cuts one paragraph (no `\n`) into.
    /// The space a line breaks at belongs to neither line. Returns at least one
    /// (possibly empty) range.
    fn wrap_ranges(&self, para: &str, size_px: f64, max_width: f64) -> Vec<Range<usize>> {
        // Shape each word once and accumulate widths (a space between words),
        // rather than re-shaping the growing line — O(words) shapes per call.
        let space = self.line_width(" ", size_px);
        let mut out = Vec::new();
        let mut line: Option<Range<usize>> = None;
        let mut width = 0.0;
        let mut at = 0usize;
        for word in para.split(' ') {
            let (ws, we) = (at, at + word.len());
            at = we + 1;
            let ww = self.line_width(word, size_px);
            match &mut line {
                // The first word, or a line that so far is empty (a run of
                // spaces keeps joining it, as `wrap` always did).
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

    /// Lay out `text` at `size_px` for drawing, caret placement and
    /// hit-testing, running the Unicode Bidirectional Algorithm over each
    /// paragraph (`\n` separates paragraphs).
    ///
    /// `direction` sets each paragraph's base direction, or detects it from the
    /// paragraph's first strong character. With `max_width`, paragraphs
    /// word-wrap to that width as [`wrap`](Self::wrap) does. Right-to-left
    /// paragraphs are aligned to the right edge of the layout's
    /// [alignment width](TextLayout::with_align_width).
    pub fn layout<'a>(
        &'a self,
        text: &'a str,
        size_px: f64,
        direction: TextDirection,
        max_width: Option<f64>,
    ) -> TextLayout<'a> {
        let mut lines = Vec::new();
        let mut start = 0usize;
        for para in text.split('\n') {
            let levels = ParagraphLevels::new(para, direction);
            // Byte offset of each char, plus the end.
            let offsets: Vec<usize> = para
                .char_indices()
                .map(|(i, _)| i)
                .chain(std::iter::once(para.len()))
                .collect();
            let char_at = |b: usize| offsets.partition_point(|&o| o < b);
            let ranges = match max_width {
                Some(w) if w.is_finite() => self.wrap_ranges(para, size_px, w),
                _ => std::iter::once(0..para.len()).collect(),
            };
            for range in ranges {
                let chars = char_at(range.start)..char_at(range.end);
                let line_levels = levels.line_levels(chars.clone());
                let mut runs = self.shape_line(para, &offsets, chars, &line_levels, size_px);
                for run in &mut runs {
                    run.range = start + run.range.start..start + run.range.end;
                }
                lines.push(LayoutLine {
                    range: start + range.start..start + range.end,
                    rtl: levels.base == 1,
                    width: runs.iter().map(|r| r.width).sum(),
                    runs,
                });
            }
            start += para.len() + 1;
        }
        let width = lines.iter().map(|l| l.width).fold(0.0, f64::max);
        TextLayout {
            font: self,
            text,
            size: size_px,
            line_height: self.line_height(size_px),
            lines,
            width,
            align_width: width,
        }
    }

    /// Shape one display line: cut it into runs of one embedding level, shape
    /// each in logical order (mirroring brackets in right-to-left runs, rule
    /// L4), and place the runs left to right in visual order (rule L2).
    /// `chars` is the line's char range within `para`, `levels` its levels.
    fn shape_line(
        &self,
        para: &str,
        offsets: &[usize],
        chars: Range<usize>,
        levels: &[u8],
        size_px: f64,
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
            let mut e = s + 1;
            while e < n && levels[e] == levels[s] {
                e += 1;
            }
            let bytes = offsets[chars.start + s]..offsets[chars.start + e];
            let rtl = levels[s] & 1 == 1;
            let shaped: String = if rtl {
                para[bytes.clone()]
                    .chars()
                    .map(|c| oxideav_scribe::bidi::mirrored_glyph(c).unwrap_or(c))
                    .collect()
            } else {
                para[bytes.clone()].to_string()
            };
            let mut glyphs = self
                .chain
                .shape(&shaped, size_px as f32)
                .unwrap_or_default();
            if rtl {
                glyphs.reverse();
            }
            let rank = (s..e).map(|i| visual_pos[i]).min().unwrap_or(s);
            runs.push((
                rank,
                GlyphRun {
                    range: bytes,
                    rtl,
                    level: levels[s],
                    x: 0.0,
                    width: glyphs.iter().map(|g| g.x_advance as f64).sum(),
                    glyphs,
                    shaped,
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

    pub(crate) fn chain(&self) -> &FaceChain {
        &self.chain
    }
}

fn parse_face(bytes: Vec<u8>) -> Result<Face, FontError> {
    let result = match bytes.first_chunk::<4>() {
        Some(b"OTTO") => Face::from_otf_bytes(bytes),
        Some(b"ttcf") => Face::from_ttc_bytes(bytes, 0),
        _ => Face::from_ttf_bytes(bytes),
    };
    result.map_err(|e| FontError(format!("{e:?}")))
}

/// Recolor an outline glyph to `color`, recursing into groups (the shaper
/// wraps each glyph's path in a cache-keyed `Group`). Non-outline leaves (e.g.
/// color-bitmap emoji `Node::Image`) are left untouched.
fn recolor(node: Node, color: Color) -> Node {
    match node {
        Node::Path(mut path) => {
            path.fill = Some(Paint::Solid(color.to_oxideav()));
            Node::Path(path)
        }
        Node::Group(mut group) => {
            group.children = group
                .children
                .into_iter()
                .map(|c| recolor(c, color))
                .collect();
            Node::Group(group)
        }
        other => other,
    }
}

/// A key for the rasterizer's glyph bitmap cache: one face, glyph and size.
fn glyph_cache_key(face: &Face, glyph_id: u16, size_px: f64) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    b"stipple.glyph.v1".hash(&mut h);
    face.stable_id().hash(&mut h);
    glyph_id.hash(&mut h);
    ((size_px * 256.0).round() as u32).hash(&mut h);
    h.finish()
}

/// Text laid out for drawing and editing: paragraphs cut into display lines,
/// each line's bidirectional runs shaped and placed in visual order.
///
/// Made by [`Font::layout`]. Positions are in logical pixels relative to the
/// top-left of the text box. Byte indices are into the text the layout was made
/// from; caret positions are logical (the order the text is stored in), and the
/// layout maps them to and from visual x positions.
#[derive(Debug)]
pub struct TextLayout<'a> {
    font: &'a Font,
    text: &'a str,
    size: f64,
    line_height: f64,
    lines: Vec<LayoutLine>,
    width: f64,
    align_width: f64,
}

/// One display line of a [`TextLayout`].
#[derive(Debug)]
struct LayoutLine {
    /// Byte range in the layout's text, without the `\n` or the space the
    /// line wrapped at.
    range: Range<usize>,
    /// Whether the line's paragraph runs right to left.
    rtl: bool,
    width: f64,
    /// The line's runs, left to right.
    runs: Vec<GlyphRun>,
}

/// A run of one embedding level, shaped.
#[derive(Debug)]
struct GlyphRun {
    /// Byte range in the layout's text (logical order).
    range: Range<usize>,
    rtl: bool,
    /// Resolved embedding level.
    level: u8,
    /// Left edge within the line.
    x: f64,
    width: f64,
    /// Shaped glyphs in visual (left-to-right) order.
    glyphs: Vec<PositionedGlyph>,
    /// The text that was shaped: the run's text, with brackets mirrored in a
    /// right-to-left run.
    shaped: String,
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
    pub fn with_align_width(mut self, width: f64) -> Self {
        self.align_width = width;
        self
    }

    /// The size of the laid-out text: the widest line by the number of lines
    /// times the line height.
    pub fn size(&self) -> Size {
        Size::new(self.width, self.line_height * self.lines.len() as f64)
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
    fn line_x(&self, line: &LayoutLine) -> f64 {
        if line.rtl {
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
            let w = self
                .font
                .line_width(&run.shaped[..end], self.size)
                .max(prev);
            out.push(w - prev);
            prev = w;
        }
        out
    }

    /// The display line holding caret position `index`.
    fn line_of(&self, index: usize) -> usize {
        self.lines
            .iter()
            .position(|l| index <= l.range.end)
            .unwrap_or(self.lines.len().saturating_sub(1))
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
        let y = li as f64 * self.line_height;
        let (boxes, vis, carets) = self.caret_stops(line);
        let v = caret_boundary(&vis, &carets, index);
        let x = match (boxes.get(v), boxes.last()) {
            (Some(b), _) => b.left,
            (None, Some(b)) => b.right,
            (None, None) => self.line_x(line),
        };
        Point::new(x, y)
    }

    /// The caret position (a byte index) nearest `point`, relative to the text
    /// box: `y` picks the line, `x` the nearer edge of the character under it.
    /// Clicking the left half of a right-to-left character puts the caret after
    /// it in the text.
    pub fn index_at(&self, point: Point) -> usize {
        if self.lines.is_empty() {
            return 0;
        }
        let li = ((point.y / self.line_height.max(1.0)).floor().max(0.0) as usize)
            .min(self.lines.len() - 1);
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
        for (li, line) in self.lines.iter().enumerate() {
            if line.range.end < start || line.range.start >= end {
                continue;
            }
            let y = li as f64 * self.line_height;
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
                out.push(Rect::from_xywh(l, y, r - l, self.line_height));
            }
        }
        out
    }
}

impl Scene {
    /// Shape and paint `text` with `font` at `origin` (the top-left of the text
    /// box, logical pixels), `size_px`, and `color`. Newlines (`\n`) start a new
    /// line, each dropped by one `line_height` from the last.
    ///
    /// Each line goes through the Unicode Bidirectional Algorithm, with its
    /// direction taken from its first strong character; right-to-left lines
    /// are aligned to the right edge of the widest line. Use
    /// [`Font::layout`] and [`Scene::fill_layout`] to choose the direction or
    /// the alignment width.
    pub fn fill_text(
        &mut self,
        font: &Font,
        text: &str,
        origin: Point,
        size_px: f64,
        color: Color,
    ) {
        if text.is_empty() || size_px <= 0.0 {
            return;
        }
        let layout = font.layout(text, size_px, TextDirection::Auto, None);
        self.fill_layout(&layout, origin, color);
    }

    /// Paint a [`TextLayout`] with its top-left at `origin`, in `color`.
    ///
    /// Glyphs are emitted as scene-graph nodes under a group translated to each
    /// line's baseline, so they rasterize and composite like any other
    /// primitive.
    pub fn fill_layout(&mut self, layout: &TextLayout<'_>, origin: Point, color: Color) {
        let size = layout.size;
        if size <= 0.0 {
            return;
        }
        let font = layout.font;
        let first_baseline = font.baseline(size);
        for (i, line) in layout.lines.iter().enumerate() {
            if line.runs.is_empty() {
                continue;
            }
            let lx = layout.line_x(line);
            let top = origin.y + i as f64 * layout.line_height;
            // Record a structured command for the GPU backend (the CPU
            // rasterizer uses the glyph nodes pushed below).
            self.record_text(
                &layout.text[line.range.clone()],
                Point::new(origin.x + lx, top),
                size,
                color,
            );
            let mut glyphs = Vec::new();
            for run in &line.runs {
                let mut pen = run.x as f32;
                for g in &run.glyphs {
                    let x = pen + g.x_offset;
                    pen += g.x_advance;
                    let face = font.chain.face(g.face_idx);
                    let Some(node) = face.glyph_node(g.glyph_id, size as f32) else {
                        continue;
                    };
                    // Wrapped in a cache-keyed group so the rasterizer reuses
                    // the glyph's bitmap wherever it's drawn.
                    let cached = Group {
                        cache_key: Some(glyph_cache_key(face, g.glyph_id, size)),
                        children: vec![recolor(node, color)],
                        ..Group::default()
                    };
                    let glyph = Group::new()
                        .with_transform(Transform2D::translate(x, g.y_offset))
                        .with_child(Node::Group(cached));
                    glyphs.push(Node::Group(glyph));
                }
            }
            if glyphs.is_empty() {
                continue;
            }
            let baseline = (top + first_baseline) as f32;
            let placed = Group::new()
                .with_transform(Transform2D::translate((origin.x + lx) as f32, baseline))
                .with_children(glyphs);
            self.push_node(Node::Group(placed));
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
            scene.fill_text(&font, "HEH", Point::new(0.0, 10.0), size, white);
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

    #[test]
    fn text_paints_visible_pixels() {
        let Some(font) = Font::system_default() else {
            eprintln!("skipping: no system font found");
            return;
        };
        let mut scene = Scene::new(Size::new(200.0, 50.0));
        // White background so black text stands out.
        scene.fill_rect(Rect::from_xywh(0.0, 0.0, 200.0, 50.0), Color::WHITE);
        scene.fill_text(&font, "Hello", Point::new(8.0, 8.0), 28.0, Color::BLACK);

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
            &font,
            "RED",
            Point::new(8.0, 8.0),
            32.0,
            Color::rgb(255, 0, 0),
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
            .with_align_width(300.0);
        assert!(layout.line_is_rtl(0));
        let w = layout.size().width;
        assert!((layout.caret_position(0).x - 300.0).abs() < 0.01);
        assert!((layout.caret_position(text.len()).x - (300.0 - w)).abs() < 0.01);
        // Forcing left-to-right puts the same text at the left edge, still
        // reading right to left within its run.
        let ltr = font
            .layout(text, 16.0, TextDirection::Ltr, None)
            .with_align_width(300.0);
        assert!(!ltr.line_is_rtl(0));
        assert!((ltr.caret_position(0).x - w).abs() < 0.01);
        assert!(ltr.caret_position(text.len()).x.abs() < 0.01);
        // An empty right-to-left field puts its caret at the right edge.
        let empty = font
            .layout("", 16.0, TextDirection::Rtl, None)
            .with_align_width(120.0);
        assert_eq!(empty.caret_position(0).x, 120.0);
    }

    #[test]
    fn rtl_paragraphs_wrap_into_right_aligned_lines() {
        let Some(font) = hebrew_font() else { return };
        let text = "אחת שתיים שלוש ארבע חמש שש שבע שמונה";
        let full = font.measure(text, 16.0).width;
        let layout = font
            .layout(text, 16.0, TextDirection::Auto, Some(full / 2.0))
            .with_align_width(full / 2.0);
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
}
