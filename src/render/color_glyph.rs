//! Color glyphs that `oxideav-scribe` doesn't draw: `COLR`/`CPAL` layers
//! (version 0's flat stacks and version 1's paint graphs, as Segoe UI Emoji
//! and Twemoji ship) and `sbix` bitmaps (Apple Color Emoji). Scribe draws
//! `CBDT` bitmaps (Noto Color Emoji) itself.
//!
//! `oxideav-ttf` parses the tables; this turns them into what a scene draws:
//! a vector glyph becomes `oxideav-core` paths in their own paints, a bitmap
//! glyph a [`Pixmap`] resampled to the pixels it covers. Both are cached,
//! like outlines, so a screen of emoji isn't rebuilt every frame.

use super::{Color, Font, Pixmap};
use crate::geometry::{PhysicalSize, Rect};
use oxideav_core::{
    GradientStop, Group, LinearGradient, Node, Paint, Path, PathNode, Point as P, RadialGradient,
    Rgba, SpreadMethod, Transform2D,
};
use oxideav_ttf::{ColorLine, CompositeMode, Extend, Paint as ColrPaint, PaintRef};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

/// A glyph drawn in its own colors.
#[derive(Clone, Debug)]
pub(crate) enum ColorGlyph {
    /// Paths in their own paints, placed as an outline is: the pen at the
    /// origin, y down, in logical pixels at the size asked for.
    Vector(Node),
    /// A bitmap strike's pixels and the box they fill, relative to the pen
    /// (logical pixels, y down).
    Bitmap(Arc<Pixmap>, Rect),
}

/// Whether a face (by stable id) has color tables this module draws.
static HAS_COLOR: Mutex<Option<HashMap<u64, bool>>> = Mutex::new(None);
/// Face, glyph, size (1/64 px), physical pixels per em (1/64), foreground
/// color.
type GlyphKey = (u64, u16, u32, u32, u32);
static GLYPHS: Mutex<Option<HashMap<GlyphKey, Option<ColorGlyph>>>> = Mutex::new(None);
/// Bitmaps resampled to the pixels they're drawn into: face, glyph, width,
/// height.
type ScaledKey = (u64, u16, u32, u32);
static SCALED: Mutex<Option<HashMap<ScaledKey, Arc<Pixmap>>>> = Mutex::new(None);
const MAX_GLYPHS: usize = 4096;
const MAX_SCALED: usize = 1024;

/// How deep a COLR v1 paint graph is followed.
const MAX_DEPTH: usize = 48;

fn lock<T>(m: &Mutex<Option<T>>) -> std::sync::MutexGuard<'_, Option<T>> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn has_color(face: &oxideav_scribe::Face) -> bool {
    let id = face.stable_id();
    if let Some(&b) = lock(&HAS_COLOR).as_ref().and_then(|m| m.get(&id)) {
        return b;
    }
    let b = face
        .with_font(|f| f.has_color_layers() || f.has_sbix())
        .unwrap_or(false);
    lock(&HAS_COLOR)
        .get_or_insert_with(HashMap::new)
        .insert(id, b);
    b
}

/// Glyph `glyph` of face `face_idx` of `font`'s chain drawn in color at
/// `size` logical pixels, `scale` physical pixels per logical one, with
/// `foreground` where the font asks for the text's color. `None` for a
/// glyph with no color form here (an outline, or a `CBDT` bitmap).
pub(crate) fn color_glyph(
    font: &Font,
    face_idx: u16,
    glyph: u16,
    size: f64,
    scale: f64,
    foreground: Color,
) -> Option<ColorGlyph> {
    let chain = font.chain();
    let face = chain.face(face_idx);
    if size <= 0.0 || !has_color(face) {
        return None;
    }
    let fg = u32::from_be_bytes([foreground.r, foreground.g, foreground.b, foreground.a]);
    let key = (
        face.stable_id(),
        glyph,
        (size * 64.0).round() as u32,
        (size * scale * 64.0).round() as u32,
        fg,
    );
    if let Some(g) = lock(&GLYPHS).as_ref().and_then(|m| m.get(&key)) {
        return g.clone();
    }
    let built = face
        .with_font(|f| build(face, f, glyph, size, size * scale, foreground))
        .ok()
        .flatten();
    let mut cache = lock(&GLYPHS);
    let map = cache.get_or_insert_with(HashMap::new);
    if map.len() >= MAX_GLYPHS {
        map.clear();
    }
    map.insert(key, built.clone());
    built
}

fn build(
    face: &oxideav_scribe::Face,
    font: &oxideav_ttf::Font<'_>,
    glyph: u16,
    size: f64,
    ppem: f64,
    fg: Color,
) -> Option<ColorGlyph> {
    let upem = font.units_per_em().max(1) as f32;
    let s = size as f32 / upem;
    // Font units (y up) to pixels at `size` (y down).
    let to_px = Transform2D {
        a: s,
        b: 0.0,
        c: 0.0,
        d: -s,
        e: 0.0,
        f: 0.0,
    };
    let painter = Painter {
        face,
        font,
        fg: fg.to_oxideav(),
        reach: 4.0 * upem,
    };
    if let Some(root) = font.color_paint_root(glyph) {
        let mut seen = HashSet::new();
        let children = painter.paint(root, 0, &mut seen);
        if !children.is_empty() {
            let group = Group::new().with_transform(to_px).with_children(children);
            return Some(ColorGlyph::Vector(Node::Group(group)));
        }
    }
    let layers = font.color_layers(glyph);
    if !layers.is_empty() {
        let children: Vec<Node> = layers
            .iter()
            .filter_map(|l| {
                let path = face.glyph_path(l.layer_glyph_id)?;
                let color = painter.color(l.palette_index, 1.0);
                Some(Node::Path(
                    PathNode::new(path).with_fill(Paint::Solid(color)),
                ))
            })
            .collect();
        if !children.is_empty() {
            let group = Group::new().with_transform(to_px).with_children(children);
            return Some(ColorGlyph::Vector(Node::Group(group)));
        }
    }
    if font.has_sbix() {
        return sbix(font, glyph, size, ppem);
    }
    None
}

/// The `sbix` strike for `ppem` physical pixels per em: the smallest at
/// least that big, else the biggest.
fn sbix(font: &oxideav_ttf::Font<'_>, glyph: u16, size: f64, ppem: f64) -> Option<ColorGlyph> {
    let strikes = font.sbix_strikes();
    let strike = strikes
        .iter()
        .copied()
        .find(|&p| p as f64 >= ppem)
        .or(strikes.last().copied())?;
    let g = font.sbix_glyph_resolved(glyph, strike)?;
    if !matches!(&g.graphic_type, b"png " | b"jpg ") {
        return None;
    }
    let pm = Pixmap::decode(g.bytes).ok()?;
    let (w, h) = (pm.size().width as f64, pm.size().height as f64);
    if w == 0.0 || h == 0.0 {
        return None;
    }
    // The strike's pixels to logical ones; the offsets are the bitmap's
    // bottom-left corner from the pen, in strike pixels, y up.
    let k = size / strike as f64;
    let rect = Rect::from_xywh(
        g.origin_x as f64 * k,
        -(g.origin_y as f64 + h) * k,
        w * k,
        h * k,
    );
    Some(ColorGlyph::Bitmap(Arc::new(pm), rect))
}

/// `pixmap` (bitmap glyph `glyph` of face `face`) resampled to `size`:
/// averaged down (an emoji strike is mostly bigger than the text), or
/// interpolated up.
pub(crate) fn scaled_bitmap(
    face: u64,
    glyph: u16,
    pixmap: &Arc<Pixmap>,
    size: PhysicalSize,
) -> Arc<Pixmap> {
    if size == pixmap.size() || size.width == 0 || size.height == 0 {
        return pixmap.clone();
    }
    let key = (face, glyph, size.width, size.height);
    if let Some(p) = lock(&SCALED).as_ref().and_then(|m| m.get(&key)) {
        return p.clone();
    }
    let out = Arc::new(resample(pixmap, size));
    let mut cache = lock(&SCALED);
    let map = cache.get_or_insert_with(HashMap::new);
    if map.len() >= MAX_SCALED {
        map.clear();
    }
    map.insert(key, out.clone());
    out
}

/// Resample straight-alpha `src` to `size`: each output pixel the
/// alpha-weighted average of the source area it covers (at least one
/// source pixel wide, so enlarging takes the nearest one, blended at the
/// seams).
pub(crate) fn resample(src: &Pixmap, size: PhysicalSize) -> Pixmap {
    let (sw, sh) = (src.size().width as usize, src.size().height as usize);
    let (dw, dh) = (size.width as usize, size.height as usize);
    let bytes = src.as_bytes();
    let stride = src.stride();
    let fx = sw as f64 / dw as f64;
    let fy = sh as f64 / dh as f64;
    // Source spans (index, weight) covering each output column / row.
    let spans = |n: usize, f: f64, len: usize| -> Vec<Vec<(usize, f64)>> {
        (0..n)
            .map(|i| {
                let (mut a, mut b) = (i as f64 * f, (i + 1) as f64 * f);
                if b - a < 1.0 {
                    let mid = (a + b) / 2.0;
                    a = (mid - 0.5).max(0.0);
                    b = (a + 1.0).min(len as f64);
                }
                let mut out = Vec::new();
                let mut x = a.floor() as usize;
                while (x as f64) < b && x < len {
                    let w = (b.min(x as f64 + 1.0) - a.max(x as f64)).max(0.0);
                    if w > 0.0 {
                        out.push((x, w));
                    }
                    x += 1;
                }
                out
            })
            .collect()
    };
    let cols = spans(dw, fx, sw);
    let rows = spans(dh, fy, sh);
    let mut data = vec![0u8; dw * dh * 4];
    for (y, row) in rows.iter().enumerate() {
        for (x, col) in cols.iter().enumerate() {
            let mut acc = [0.0f64; 4];
            let mut total = 0.0;
            for &(sy, wy) in row {
                for &(sx, wx) in col {
                    let w = wx * wy;
                    let p = &bytes[sy * stride + sx * 4..sy * stride + sx * 4 + 4];
                    let a = p[3] as f64 * w;
                    acc[0] += p[0] as f64 * a;
                    acc[1] += p[1] as f64 * a;
                    acc[2] += p[2] as f64 * a;
                    acc[3] += a;
                    total += w;
                }
            }
            let o = (y * dw + x) * 4;
            if acc[3] > 0.0 {
                for c in 0..3 {
                    data[o + c] = (acc[c] / acc[3]).round().clamp(0.0, 255.0) as u8;
                }
                data[o + 3] = (acc[3] / total).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    Pixmap::from_rgba8(size, data)
}

/// Turns a COLR v1 paint graph into nodes, in font units (y up).
struct Painter<'f, 'a> {
    face: &'f oxideav_scribe::Face,
    font: &'f oxideav_ttf::Font<'a>,
    fg: Rgba,
    /// How far a fill with no shape of its own (a gradient under a
    /// transform) reaches from the origin.
    reach: f32,
}

impl Painter<'_, '_> {
    /// A palette entry (or the foreground, `0xFFFF`) with `alpha` applied.
    fn color(&self, entry: u16, alpha: f32) -> Rgba {
        let alpha = alpha.clamp(0.0, 1.0);
        match self.font.colr_effective_color(0, entry, alpha) {
            Some([r, g, b, a]) => Rgba::new(r, g, b, a),
            None => Rgba::new(
                self.fg.r,
                self.fg.g,
                self.fg.b,
                (self.fg.a as f32 * alpha).round() as u8,
            ),
        }
    }

    fn stops(&self, line: &ColorLine) -> Vec<GradientStop> {
        line.stops
            .iter()
            .map(|s| GradientStop::new(s.stop_offset, self.color(s.palette_index, s.alpha)))
            .collect()
    }

    fn spread(line: &ColorLine) -> SpreadMethod {
        match line.extend {
            Extend::Pad => SpreadMethod::Pad,
            Extend::Repeat => SpreadMethod::Repeat,
            Extend::Reflect => SpreadMethod::Reflect,
        }
    }

    /// The fill a paint is, if it is one (a solid or a gradient).
    fn fill(&self, paint: &ColrPaint) -> Option<Paint> {
        Some(match paint {
            ColrPaint::Solid {
                palette_index,
                alpha,
            } => Paint::Solid(self.color(*palette_index, *alpha)),
            ColrPaint::LinearGradient {
                color_line,
                x0,
                y0,
                x1,
                y1,
                x2,
                y2,
            } => {
                // The gradient runs from p0 to p1 projected onto the normal
                // of p0→p2 (its color lines are parallel to p0→p2).
                let (rx, ry) = (x2 - x0, y2 - y0);
                let (dx, dy) = (x1 - x0, y1 - y0);
                let (nx, ny) = (ry, -rx);
                let nn = nx * nx + ny * ny;
                let end = if nn > f32::EPSILON {
                    let t = (dx * nx + dy * ny) / nn;
                    P::new(x0 + nx * t, y0 + ny * t)
                } else {
                    P::new(*x1, *y1)
                };
                let mut g = LinearGradient::new(P::new(*x0, *y0), end);
                g.stops = self.stops(color_line);
                g.spread = Self::spread(color_line);
                Paint::LinearGradient(g)
            }
            ColrPaint::RadialGradient {
                color_line,
                x0,
                y0,
                radius0,
                x1,
                y1,
                radius1,
            } => {
                // Between two circles: drawn from the first's center out
                // to the second, the stops moved out by the first's radius.
                let r1 = radius1.max(f32::EPSILON);
                let inner = (radius0 / r1).clamp(0.0, 1.0);
                let mut g = RadialGradient::new(P::new(*x1, *y1), r1);
                g.focal = Some(P::new(*x0, *y0));
                g.stops = self
                    .stops(color_line)
                    .into_iter()
                    .map(|s| GradientStop::new(inner + s.offset * (1.0 - inner), s.color))
                    .collect();
                g.spread = Self::spread(color_line);
                Paint::RadialGradient(g)
            }
            // No sweep gradients in the scene graph: the color halfway
            // round.
            ColrPaint::SweepGradient { color_line, .. } => {
                let stops = &color_line.stops;
                let mid = stops.get(stops.len() / 2)?;
                Paint::Solid(self.color(mid.palette_index, mid.alpha))
            }
            _ => return None,
        })
    }

    /// A rectangle wide enough to stand for the whole plane.
    fn plane(&self) -> Path {
        let r = self.reach;
        let mut p = Path::new();
        p.move_to(P::new(-r, -r))
            .line_to(P::new(r, -r))
            .line_to(P::new(r, r))
            .line_to(P::new(-r, r))
            .close();
        p
    }

    fn transformed(
        &self,
        t: Transform2D,
        child: PaintRef,
        depth: usize,
        seen: &mut HashSet<PaintRef>,
    ) -> Vec<Node> {
        let children = self.paint(child, depth + 1, seen);
        if children.is_empty() {
            return children;
        }
        vec![Node::Group(
            Group::new().with_transform(t).with_children(children),
        )]
    }

    /// The nodes `paint` draws.
    fn paint(&self, at: PaintRef, depth: usize, seen: &mut HashSet<PaintRef>) -> Vec<Node> {
        if depth > MAX_DEPTH || !seen.insert(at) {
            return Vec::new();
        }
        let out = self.paint_inner(at, depth, seen);
        seen.remove(&at);
        out
    }

    fn paint_inner(&self, at: PaintRef, depth: usize, seen: &mut HashSet<PaintRef>) -> Vec<Node> {
        let Some(paint) = self.font.color_paint(at) else {
            return Vec::new();
        };
        match paint {
            ColrPaint::ColrLayers { layers } => layers
                .into_iter()
                .flat_map(|l| self.paint(l, depth + 1, seen))
                .collect(),
            ColrPaint::Glyph { paint, glyph_id } => {
                let Some(outline) = self.face.glyph_path(glyph_id) else {
                    return Vec::new();
                };
                // A plain fill fills the outline; anything else is drawn
                // clipped to it.
                if let Some(fill) = self.font.color_paint(paint).and_then(|p| self.fill(&p)) {
                    return vec![Node::Path(PathNode::new(outline).with_fill(fill))];
                }
                let children = self.paint(paint, depth + 1, seen);
                if children.is_empty() {
                    return children;
                }
                let mut g = Group::new().with_children(children);
                g.clip = Some(outline);
                vec![Node::Group(g)]
            }
            ColrPaint::ColrGlyph { glyph_id } => match self.font.color_paint_root(glyph_id) {
                Some(root) => self.paint(root, depth + 1, seen),
                None => Vec::new(),
            },
            ColrPaint::Transform {
                paint,
                transform: m,
            } => self.transformed(
                Transform2D {
                    a: m.xx,
                    b: m.yx,
                    c: m.xy,
                    d: m.yy,
                    e: m.dx,
                    f: m.dy,
                },
                paint,
                depth,
                seen,
            ),
            ColrPaint::Translate { paint, dx, dy } => {
                self.transformed(Transform2D::translate(dx, dy), paint, depth, seen)
            }
            ColrPaint::Scale {
                paint,
                scale_x,
                scale_y,
                center_x,
                center_y,
            } => self.transformed(
                about(
                    Transform2D {
                        a: scale_x,
                        b: 0.0,
                        c: 0.0,
                        d: scale_y,
                        e: 0.0,
                        f: 0.0,
                    },
                    center_x,
                    center_y,
                ),
                paint,
                depth,
                seen,
            ),
            ColrPaint::Rotate {
                paint,
                angle_degrees,
                center_x,
                center_y,
            } => {
                let (sin, cos) = angle_degrees.to_radians().sin_cos();
                self.transformed(
                    about(
                        Transform2D {
                            a: cos,
                            b: sin,
                            c: -sin,
                            d: cos,
                            e: 0.0,
                            f: 0.0,
                        },
                        center_x,
                        center_y,
                    ),
                    paint,
                    depth,
                    seen,
                )
            }
            ColrPaint::Skew {
                paint,
                x_skew_degrees,
                y_skew_degrees,
                center_x,
                center_y,
            } => self.transformed(
                about(
                    Transform2D {
                        a: 1.0,
                        b: y_skew_degrees.to_radians().tan(),
                        c: -x_skew_degrees.to_radians().tan(),
                        d: 1.0,
                        e: 0.0,
                        f: 0.0,
                    },
                    center_x,
                    center_y,
                ),
                paint,
                depth,
                seen,
            ),
            // Compositing other than source-over isn't in the scene graph:
            // the modes that keep one side keep it, the rest draw the
            // source over the backdrop.
            ColrPaint::Composite {
                source,
                mode,
                backdrop,
            } => match mode {
                CompositeMode::Clear => Vec::new(),
                CompositeMode::Src | CompositeMode::SrcIn | CompositeMode::SrcOut => {
                    self.paint(source, depth + 1, seen)
                }
                CompositeMode::Dest | CompositeMode::DestIn | CompositeMode::DestOut => {
                    self.paint(backdrop, depth + 1, seen)
                }
                CompositeMode::DestOver => {
                    let mut v = self.paint(source, depth + 1, seen);
                    v.extend(self.paint(backdrop, depth + 1, seen));
                    v
                }
                _ => {
                    let mut v = self.paint(backdrop, depth + 1, seen);
                    v.extend(self.paint(source, depth + 1, seen));
                    v
                }
            },
            // A fill with no outline around it (under a transform inside a
            // glyph): the whole plane, clipped by the glyph above.
            other => match self.fill(&other) {
                Some(fill) => vec![Node::Path(PathNode::new(self.plane()).with_fill(fill))],
                None => Vec::new(),
            },
        }
    }
}

/// `t` applied about `(cx, cy)` instead of the origin.
fn about(t: Transform2D, cx: f32, cy: f32) -> Transform2D {
    Transform2D::translate(cx, cy)
        .compose(&t)
        .compose(&Transform2D::translate(-cx, -cy))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shrinking averages the area each pixel covers, weighting colors by
    /// alpha (a transparent pixel's color doesn't bleed in); growing keeps
    /// the colors.
    #[test]
    fn resampling_averages_by_area() {
        // 4×1: red, transparent green, blue, blue.
        let src = Pixmap::from_rgba8(
            PhysicalSize::new(4, 1),
            vec![255, 0, 0, 255, 0, 255, 0, 0, 0, 0, 255, 255, 0, 0, 255, 255],
        );
        let half = resample(&src, PhysicalSize::new(2, 1));
        assert_eq!(half.pixel(0, 0), Some([255, 0, 0, 128]));
        assert_eq!(half.pixel(1, 0), Some([0, 0, 255, 255]));
        let big = resample(&src, PhysicalSize::new(8, 2));
        assert_eq!(big.pixel(0, 1), Some([255, 0, 0, 255]));
        assert_eq!(big.pixel(7, 0), Some([0, 0, 255, 255]));
    }
}
