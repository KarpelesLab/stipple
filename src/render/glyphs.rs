//! Glyph runs: text drawn at exact positions, cluster by cluster, the way a
//! terminal draws its grid (every cell at a multiple of the cell width,
//! whatever the font's own advances say).

use crate::geometry::Point;
use crate::render::scene::Scene;
use crate::render::text::{OBLIQUE_SHEAR, bold_stroke, embolden};
use crate::render::{Color, Decoration, Font};
use oxideav_core::{Group, Node, Transform2D};
use std::collections::HashMap;
use std::sync::Mutex;

/// Text placed by the caller rather than laid out: each cluster (a character,
/// or a character with its combining marks) is drawn with its pen at a point
/// on the baseline the caller chooses, in one [`Font`] (family and style),
/// size and color, with an optional [`Decoration`] (underline in any style,
/// strikethrough, overline).
///
/// The font is per run, so a line in several fonts or styles is several runs.
/// Draw one with [`Scene::fill_glyph_run`].
///
/// ```no_run
/// use stipple::geometry::{Point, Size};
/// use stipple::render::{Color, Decoration, Font, GlyphRun, Scene, UnderlineStyle};
///
/// let font = Font::system_default().unwrap();
/// let mut scene = Scene::new(Size::new(200.0, 40.0));
/// // A terminal row: one cell every 8 px, the row's top at y = 0.
/// let mut run = GlyphRun::new(&font, 14.0, Color::WHITE);
/// run.push_cells("ls -la", Point::new(0.0, 0.0), 8.0);
/// scene.fill_glyph_run(&run);
/// // A misspelled word with an undercurl.
/// let mut word = GlyphRun::new(&font, 14.0, Color::WHITE)
///     .decoration(Decoration::underline(UnderlineStyle::Curly));
/// word.push_cells("teh", Point::new(64.0, 0.0), 8.0);
/// scene.fill_glyph_run(&word);
/// ```
#[derive(Clone, Debug)]
pub struct GlyphRun {
    font: Font,
    size: f64,
    color: Color,
    decoration: Decoration,
    /// The clusters' text, one after another.
    text: String,
    /// Each cluster's end in `text`, its pen position, and the width it
    /// takes (`None`: its own advance).
    clusters: Vec<(usize, Point, Option<f64>)>,
}

impl GlyphRun {
    /// An empty run drawn with `font` (a cheap handle, cloned) at `size`
    /// logical pixels, in `color`.
    pub fn new(font: &Font, size: f64, color: Color) -> Self {
        Self {
            font: font.clone(),
            size,
            color,
            decoration: Decoration::default(),
            text: String::new(),
            clusters: Vec::new(),
        }
    }

    /// Decorate the run: lines along each stretch of adjacent clusters, at
    /// the font's underline and strikeout positions (see
    /// [`Scene::draw_decoration`]).
    pub fn decoration(mut self, decoration: Decoration) -> Self {
        self.decoration = decoration;
        self
    }

    /// Add `cluster` with its pen (the left end of its baseline) at `pen`.
    /// The cluster is shaped on its own, so it may hold combining marks or a
    /// multi-character emoji; it takes the width of its own advance.
    pub fn push(&mut self, cluster: &str, pen: Point) -> &mut Self {
        self.push_with_width(cluster, pen, None)
    }

    /// Add `cluster` at `pen`, taking `width` logical pixels (a terminal's
    /// one or two cells) whatever its advance: what decorations span.
    pub fn push_cell(&mut self, cluster: &str, pen: Point, width: f64) -> &mut Self {
        self.push_with_width(cluster, pen, Some(width))
    }

    fn push_with_width(&mut self, cluster: &str, pen: Point, width: Option<f64>) -> &mut Self {
        if !cluster.is_empty() {
            self.text.push_str(cluster);
            self.clusters.push((self.text.len(), pen, width));
        }
        self
    }

    /// Add each character of `text` in its own cell, `advance` logical pixels
    /// apart, on the line whose top-left is `origin` (the baseline is placed
    /// from the font's metrics, as [`Scene::fill_text`] does). Characters are
    /// placed one per cell; push clusters that span cells or carry combining
    /// marks with [`push_cell`](Self::push_cell).
    pub fn push_cells(&mut self, text: &str, origin: Point, advance: f64) -> &mut Self {
        let baseline = origin.y + self.font.baseline(self.size);
        let mut buf = [0u8; 4];
        for (i, ch) in text.chars().enumerate() {
            let pen = Point::new(origin.x + i as f64 * advance, baseline);
            self.push_cell(ch.encode_utf8(&mut buf), pen, advance);
        }
        self
    }

    /// The run's font.
    pub fn font(&self) -> &Font {
        &self.font
    }

    /// The run's size in logical pixels.
    pub fn size(&self) -> f64 {
        self.size
    }

    /// The run's color.
    pub fn color(&self) -> Color {
        self.color
    }

    /// How many clusters the run holds.
    pub fn len(&self) -> usize {
        self.clusters.len()
    }

    /// Whether the run holds no clusters.
    pub fn is_empty(&self) -> bool {
        self.clusters.is_empty()
    }

    /// Each cluster and its pen position.
    pub fn clusters(&self) -> impl Iterator<Item = (&str, Point)> {
        let mut start = 0;
        self.clusters.iter().map(move |&(end, pen, _)| {
            let s = &self.text[start..end];
            start = end;
            (s, pen)
        })
    }
}

/// Glyph outlines by face, glyph and size: building an outline node is the
/// costly part of drawing a glyph, and a grid draws the same few glyphs over
/// and over. Cleared wholesale past `MAX_OUTLINES` entries.
///
/// (Glyphs are drawn as plain outlines, not through the rasterizer's bitmap
/// cache: its entries are keyed by position, so a grid rarely hits them, and
/// rendering each glyph offscreen for the cache costs more than filling it.)
type OutlineKey = (u64, u16, u32);
static OUTLINES: Mutex<Option<HashMap<OutlineKey, Option<Node>>>> = Mutex::new(None);
const MAX_OUTLINES: usize = 8192;

fn outline(font: &Font, face_idx: u16, glyph: u16, size: f64) -> Option<Node> {
    let face = font.chain().face(face_idx);
    let key = (face.stable_id(), glyph, (size * 64.0).round() as u32);
    let mut cache = OUTLINES.lock().unwrap_or_else(|e| e.into_inner());
    let map = cache.get_or_insert_with(HashMap::new);
    if let Some(node) = map.get(&key) {
        return node.clone();
    }
    let node = face.glyph_node(glyph, size as f32);
    if map.len() >= MAX_OUTLINES {
        map.clear();
    }
    map.insert(key, node.clone());
    node
}

impl Scene {
    /// Draw a [`GlyphRun`]: every cluster at its own pen position, in the
    /// run's font (synthesizing bold or oblique where the family lacks the
    /// face), then its decoration along each stretch of adjacent clusters.
    pub fn fill_glyph_run(&mut self, run: &GlyphRun) {
        if run.is_empty() || run.size <= 0.0 {
            return;
        }
        let font = &run.font;
        let top = font.baseline(run.size);
        let mut glyphs = Vec::with_capacity(run.len());
        // Stretches of adjacent clusters on one baseline: (pen, width).
        let mut spans: Vec<(Point, f64)> = Vec::new();
        let mut start = 0;
        for &(end, pen, width) in &run.clusters {
            let cluster = &run.text[start..end];
            start = end;
            // A structured command per cluster, for backends that draw from
            // them (the CPU rasterizer uses the nodes below).
            if !cluster.trim().is_empty() {
                self.record_text(cluster, Point::new(pen.x, pen.y - top), run.size, run.color);
            }
            let mut x = pen.x as f32;
            for g in font.shaped(cluster, run.size) {
                let at = x + g.x_offset;
                x += g.x_advance;
                let Some(node) = outline(font, g.face_idx, g.glyph_id, run.size) else {
                    continue;
                };
                let synth = font.synthesis(g.face_idx);
                let stroke = if synth.bold {
                    bold_stroke(run.size)
                } else {
                    0.0
                };
                let shear = if synth.oblique { -OBLIQUE_SHEAR } else { 0.0 };
                let place = Transform2D {
                    a: 1.0,
                    b: 0.0,
                    c: shear,
                    d: 1.0,
                    e: at,
                    f: pen.y as f32 + g.y_offset,
                };
                glyphs.push(Node::Group(
                    Group::new()
                        .with_transform(place)
                        .with_child(embolden(node, run.color, stroke)),
                ));
            }
            if !run.decoration.is_none() {
                let w = width.unwrap_or((x - pen.x as f32) as f64);
                match spans.last_mut() {
                    Some((p, sw)) if p.y == pen.y && (p.x + *sw - pen.x).abs() < 0.5 => {
                        *sw = pen.x + w - p.x;
                    }
                    _ => spans.push((pen, w)),
                }
            }
        }
        if !glyphs.is_empty() {
            self.push_node(Node::Group(Group::new().with_children(glyphs)));
        }
        if !spans.is_empty() {
            let metrics = font.metrics(run.size);
            for (pen, width) in spans {
                self.draw_decoration(&metrics, pen, width, &run.decoration, run.color);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{Rect, ScaleFactor, Size};
    use crate::render::{SoftwareRenderer, UnderlineStyle};

    #[test]
    fn clusters_keep_their_text_and_place() {
        let Some(font) = Font::system_default() else {
            return;
        };
        let mut run = GlyphRun::new(&font, 12.0, Color::WHITE);
        run.push_cells("a b", Point::new(10.0, 0.0), 8.0);
        run.push("e\u{301}", Point::new(50.0, 9.0));
        let got: Vec<_> = run.clusters().map(|(s, p)| (s.to_string(), p.x)).collect();
        assert_eq!(
            got,
            [
                ("a".to_string(), 10.0),
                (" ".to_string(), 18.0),
                ("b".to_string(), 26.0),
                ("e\u{301}".to_string(), 50.0)
            ]
        );
    }

    fn red_in(out: &crate::render::Pixmap, r: Rect) -> bool {
        (r.min_y() as u32..r.max_y() as u32).any(|y| {
            (r.min_x() as u32..r.max_x() as u32)
                .any(|x| out.pixel(x, y).is_some_and(|p| p[0] > 128 && p[1] < 64))
        })
    }

    /// Each cell's glyph is inked inside its own cell, in the run's color.
    #[test]
    fn a_run_draws_each_glyph_in_its_cell() {
        let Some(font) = Font::system_default() else {
            return;
        };
        let mut scene = Scene::new(Size::new(80.0, 20.0));
        let mut run = GlyphRun::new(&font, 16.0, Color::rgb(255, 0, 0));
        run.push_cells("I   I", Point::new(0.0, 0.0), 16.0);
        scene.fill_glyph_run(&run);
        let out = SoftwareRenderer::new()
            .with_background(Color::BLACK)
            .render(scene, ScaleFactor::IDENTITY);
        assert!(
            red_in(&out, Rect::from_xywh(0.0, 0.0, 16.0, 20.0)),
            "first cell"
        );
        assert!(
            !red_in(&out, Rect::from_xywh(16.0, 0.0, 48.0, 20.0)),
            "blank cells"
        );
        assert!(
            red_in(&out, Rect::from_xywh(64.0, 0.0, 16.0, 20.0)),
            "fifth cell"
        );
    }

    /// A decorated run underlines its cells, spaces included, from the
    /// first cell's left edge to the last one's right edge.
    #[test]
    fn a_decorated_run_underlines_across_its_cells() {
        let Some(font) = Font::system_default() else {
            return;
        };
        let mut scene = Scene::new(Size::new(80.0, 24.0));
        let mut run = GlyphRun::new(&font, 16.0, Color::rgb(255, 0, 0))
            .decoration(Decoration::underline(UnderlineStyle::Single));
        run.push_cells("I  I", Point::new(8.0, 0.0), 16.0);
        scene.fill_glyph_run(&run);
        let out = SoftwareRenderer::new()
            .with_background(Color::BLACK)
            .render(scene, ScaleFactor::IDENTITY);
        let y = font.baseline(16.0) + 0.5;
        let h = 24.0 - y;
        assert!(
            red_in(&out, Rect::from_xywh(30.0, y, 20.0, h)),
            "under the blanks"
        );
        assert!(
            !red_in(&out, Rect::from_xywh(0.0, y, 7.0, h)),
            "before the run"
        );
        assert!(
            !red_in(&out, Rect::from_xywh(73.0, y, 7.0, h)),
            "after the run"
        );
    }
}
