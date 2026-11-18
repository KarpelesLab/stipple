//! Glyph runs: text drawn at exact positions, cluster by cluster, the way a
//! terminal draws its grid (every cell at a multiple of the cell width,
//! whatever the font's own advances say).

use crate::geometry::{Point, Rect};
use crate::render::color_glyph::{ColorGlyph, color_glyph};
use crate::render::font::Synthesis;
use crate::render::glyph_mask::FastHash;
use crate::render::lower::{Bounds, subtree_bounds};
use crate::render::scene::Scene;
use crate::render::text::{OBLIQUE_SHEAR, bold_stroke, embolden, glyph_cache_key};
use crate::render::{Color, Decoration, Font};
use oxideav_core::{Group, Node, Transform2D};
use oxideav_scribe::Face;
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
///     .decoration(Decoration::NONE.underline(UnderlineStyle::Curly));
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

/// Glyph outlines by face, glyph and size, with their bounds: building an
/// outline node is the costly part of drawing a glyph, and a grid draws the
/// same few glyphs over and over. Cleared wholesale past `MAX_OUTLINES`
/// entries.
type OutlineKey = (u64, u16, u32);
type Outlines = HashMap<OutlineKey, Option<(Node, Bounds)>, FastHash>;
static OUTLINES: Mutex<Option<Outlines>> = Mutex::new(None);
const MAX_OUTLINES: usize = 8192;

pub(crate) fn outline(font: &Font, face_idx: u16, glyph: u16, size: f64) -> Option<Node> {
    let chain = font.chain();
    let face = chain.face(face_idx);
    with_outline(face, face.stable_id(), glyph, size, |n, _| n.clone())
}

/// `f` of the outline of `glyph` of `face` (whose `stable_id` is `id`: it
/// hashes the face's header, worth doing once per run, not per glyph) at
/// `size`, and its bounds; `None` if it has none (a space).
fn with_outline<R>(
    face: &Face,
    id: u64,
    glyph: u16,
    size: f64,
    f: impl FnOnce(&Node, Bounds) -> R,
) -> Option<R> {
    let key = (id, glyph, (size * 64.0).round() as u32);
    let mut cache = OUTLINES.lock().unwrap_or_else(|e| e.into_inner());
    let map = cache.get_or_insert_with(Outlines::default);
    if let Some(entry) = map.get(&key) {
        return entry.as_ref().map(|(n, b)| f(n, *b));
    }
    let entry = face.glyph_node(glyph, size as f32).and_then(|n| {
        let b = subtree_bounds(std::slice::from_ref(&n))?;
        Some((n, b))
    });
    if map.len() >= MAX_OUTLINES {
        map.clear();
    }
    let out = entry.as_ref().map(|(n, b)| f(n, *b));
    map.insert(key, entry);
    out
}

/// `base` and `v` mixed into one key (SplitMix64's finalizer).
fn mix(base: u64, v: u64) -> u64 {
    let mut z = base ^ v.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A face of the chain a [`GlyphDraw`] uses.
#[derive(Debug)]
struct RunFace {
    /// Its index in the font's chain.
    idx: u16,
    /// Its `stable_id`.
    id: u64,
    synth: Synthesis,
    /// What its glyphs' keys have in common.
    base: u64,
}

/// One glyph of a [`GlyphDraw`]: which, and where its origin is.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RunGlyph {
    /// Into the draw's faces.
    face: u16,
    glyph: u16,
    /// One face, glyph, size and synthesis: what its raster is kept by.
    pub key: u64,
    x: f32,
    y: f32,
}

/// A [`GlyphRun`] as a scene records it: its glyphs, shaped and placed, drawn
/// when the scene is lowered. The software compositor blends each from a
/// coverage mask it keeps (`render::glyph_mask`); anything else gets
/// [`node`](Self::node), cache-keyed outlines (which the GPU renderers draw
/// from their glyph atlas).
#[derive(Debug)]
pub(crate) struct GlyphDraw {
    font: Font,
    size: f64,
    color: Color,
    faces: Vec<RunFace>,
    glyphs: Vec<RunGlyph>,
}

impl GlyphDraw {
    pub(crate) fn glyphs(&self) -> &[RunGlyph] {
        &self.glyphs
    }

    pub(crate) fn color(&self) -> Color {
        self.color
    }

    /// Where `g`'s outline is drawn: at its origin, leaning when its face
    /// is a synthesized oblique (about the baseline point; y grows downward,
    /// so up is negative y).
    pub(crate) fn place(&self, g: &RunGlyph) -> Transform2D {
        let oblique = self.faces[g.face as usize].synth.oblique;
        Transform2D {
            a: 1.0,
            b: 0.0,
            c: if oblique { -OBLIQUE_SHEAR } else { 0.0 },
            d: 1.0,
            e: g.x,
            f: g.y,
        }
    }

    /// `g`'s outline at its origin, filled with `color` (and stroked, for a
    /// synthesized bold).
    pub(crate) fn outline(&self, g: &RunGlyph, color: Color) -> Option<Node> {
        let face = &self.faces[g.face as usize];
        let chain = self.font.chain();
        let node = with_outline(chain.face(face.idx), face.id, g.glyph, self.size, |n, _| {
            n.clone()
        })?;
        let stroke = if face.synth.bold {
            bold_stroke(self.size)
        } else {
            0.0
        };
        Some(embolden(node, color, stroke))
    }

    /// Glyph `g` as a node: its outline in the run's color, placed; in a
    /// group keyed for the GPU renderers' glyph atlas when `keyed` (not for
    /// the CPU rasterizer: it would render a keyed group it hasn't seen into
    /// a buffer the size of what it draws on).
    pub(crate) fn glyph_node(&self, g: &RunGlyph, keyed: bool) -> Option<Node> {
        let outline = self.outline(g, self.color)?;
        let inner = if keyed {
            Node::Group(Group {
                cache_key: Some(g.key),
                children: vec![outline],
                ..Group::default()
            })
        } else {
            outline
        };
        Some(Node::Group(
            Group::new().with_transform(self.place(g)).with_child(inner),
        ))
    }

    /// All of it as outlines, for the CPU rasterizer.
    pub(crate) fn node(&self) -> Node {
        Node::Group(
            Group::new().with_children(
                self.glyphs
                    .iter()
                    .filter_map(|g| self.glyph_node(g, false))
                    .collect(),
            ),
        )
    }
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
        let chain = font.chain();
        let mut faces: Vec<RunFace> = Vec::new();
        let mut glyphs = Vec::with_capacity(run.len());
        // Color glyphs (emoji): layered ones as nodes, bitmaps after.
        let mut colored = Vec::new();
        let mut bitmaps = Vec::new();
        // The box around the glyphs' outlines.
        let mut ink = [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ];
        // Stretches of adjacent clusters on one baseline: (pen, width).
        let mut spans: Vec<(Point, f64)> = Vec::new();
        let mut start = 0;
        for &(end, pen, width) in &run.clusters {
            let cluster = &run.text[start..end];
            start = end;
            // A structured command per cluster, for backends that draw from
            // them (the CPU rasterizer uses the glyphs below).
            if !cluster.trim().is_empty() {
                self.record_text(cluster, Point::new(pen.x, pen.y - top), run.size, run.color);
            }
            let mut x = pen.x as f32;
            for g in font.shaped(cluster, run.size) {
                let at = x + g.x_offset;
                x += g.x_advance;
                if let Some(art) = color_glyph(
                    font,
                    g.face_idx,
                    g.glyph_id,
                    run.size,
                    self.scale(),
                    run.color,
                ) {
                    match art {
                        ColorGlyph::Vector(node) => colored.push(Node::Group(
                            Group::new()
                                .with_transform(Transform2D::translate(
                                    at,
                                    pen.y as f32 + g.y_offset,
                                ))
                                .with_child(node),
                        )),
                        ColorGlyph::Bitmap(pixmap, rect) => {
                            let face = chain.face(g.face_idx).stable_id();
                            let rect = rect.translate(crate::geometry::Vec2::new(
                                at as f64,
                                pen.y + g.y_offset as f64,
                            ));
                            bitmaps.push((rect, face, g.glyph_id, pixmap));
                        }
                    }
                    continue;
                }
                let face = chain.face(g.face_idx);
                let fi = match faces.iter().position(|f| f.idx == g.face_idx) {
                    Some(i) => i,
                    None => {
                        let synth = font.synthesis(g.face_idx);
                        faces.push(RunFace {
                            idx: g.face_idx,
                            id: face.stable_id(),
                            synth,
                            base: glyph_cache_key(face, 0, run.size, synth),
                        });
                        faces.len() - 1
                    }
                };
                let f = &faces[fi];
                let Some(b) = with_outline(face, f.id, g.glyph_id, run.size, |_, b| b) else {
                    continue;
                };
                let glyph = RunGlyph {
                    face: fi as u16,
                    glyph: g.glyph_id,
                    key: mix(f.base, g.glyph_id as u64),
                    x: at,
                    y: pen.y as f32 + g.y_offset,
                };
                // Bold strokes and the oblique's lean reach past the
                // outline: a size's worth either side is plenty.
                let pad = if f.synth.bold || f.synth.oblique {
                    run.size as f32 * 0.5
                } else {
                    0.0
                };
                ink = [
                    ink[0].min(glyph.x + b[0] - pad),
                    ink[1].min(glyph.y + b[1] - pad),
                    ink[2].max(glyph.x + b[2] + pad),
                    ink[3].max(glyph.y + b[3] + pad),
                ];
                glyphs.push(glyph);
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
            // One pixel around for antialiasing.
            let bounds = Rect::from_xywh(
                ink[0] as f64 - 1.0,
                ink[1] as f64 - 1.0,
                (ink[2] - ink[0]) as f64 + 2.0,
                (ink[3] - ink[1]) as f64 + 2.0,
            );
            self.draw_glyphs(
                bounds,
                GlyphDraw {
                    font: font.clone(),
                    size: run.size,
                    color: run.color,
                    faces,
                    glyphs,
                },
            );
        }
        if !colored.is_empty() {
            self.push_node(Node::Group(Group::new().with_children(colored)));
        }
        for (rect, face, glyph, pixmap) in bitmaps {
            self.draw_color_bitmap(rect, face, glyph, &pixmap);
        }
        if !spans.is_empty() {
            let metrics = font.metrics(run.size);
            for (pen, width) in spans {
                self.decoration_lines(&metrics, pen, width, &run.decoration, run.color);
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
            .background(Color::BLACK)
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
            .decoration(Decoration::NONE.underline(UnderlineStyle::Single));
        run.push_cells("I  I", Point::new(8.0, 0.0), 16.0);
        scene.fill_glyph_run(&run);
        let out = SoftwareRenderer::new()
            .background(Color::BLACK)
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

    /// Drawn from coverage masks, runs are what their outlines filled by
    /// the rasterizer are (up to the masks' sixteenth-of-a-pixel
    /// positions), at any scale, synthesized bold and oblique included.
    #[test]
    fn masks_match_the_outlines() {
        let Some(font) = Font::system_default() else {
            return;
        };
        let mut scene = Scene::new(Size::new(160.0, 60.0));
        for (i, f) in [font.clone(), font.bold(), font.italic()]
            .iter()
            .enumerate()
        {
            let c = [
                Color::WHITE,
                Color::rgb(90, 200, 120),
                Color::rgb(80, 140, 255),
            ][i];
            let mut run = GlyphRun::new(f, 12.5, c);
            run.push_cells(
                "Wg@|~ 0Oj#",
                Point::new(3.3 + i as f64 * 0.21, i as f64 * 17.7),
                7.3,
            );
            scene.fill_glyph_run(&run);
        }
        // The same glyphs as plain outlines.
        let mut outlines = Scene::new(scene.logical_size());
        for node in scene.lower_root(None, &Transform2D::identity()) {
            outlines.push_node(node);
        }
        let r = SoftwareRenderer::new().background(Color::rgb(20, 22, 28));
        for scale in [1.0, 1.5, 2.0] {
            let scale = ScaleFactor::new(scale);
            let a = r.render(scene.clone(), scale);
            let b = r.render(outlines.clone(), scale);
            let (mut sum, mut worst) = (0u64, 0u8);
            for (p, q) in a.as_bytes().iter().zip(b.as_bytes()) {
                sum += p.abs_diff(*q) as u64;
                worst = worst.max(p.abs_diff(*q));
            }
            let mean = sum as f64 / a.as_bytes().len() as f64;
            assert!(
                mean < 0.3 && worst <= 16,
                "at {scale:?}: mean {mean}, worst {worst}"
            );
        }
    }
}
