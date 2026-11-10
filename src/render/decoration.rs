//! Text decorations: underlines in several styles, strikethrough and
//! overline, positioned and sized from the font's [`FontMetrics`].

use crate::geometry::{Point, Rect};
use crate::render::font::FontMetrics;
use crate::render::{Color, Scene};
use oxideav_core::{LineCap, Node, Path, PathNode, Point as OxPoint, Stroke};

/// How an underline is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum UnderlineStyle {
    /// One straight line.
    #[default]
    Single,
    /// Two straight lines, one stroke-width apart.
    Double,
    /// A wave (as spell checkers mark words).
    Curly,
    /// A row of dots.
    Dotted,
    /// A row of dashes.
    Dashed,
}

/// The lines drawn along a run of text: an underline (in one of several
/// styles), a strikethrough and an overline, in the text's color or their own.
///
/// These map onto a terminal's SGR attributes: 4 and 4:1–4:5 (underline
/// styles), 9 (strikethrough), 53 (overline) and 58 (decoration color).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Decoration {
    pub underline: Option<UnderlineStyle>,
    pub strikethrough: bool,
    pub overline: bool,
    /// The lines' color; `None` draws them in the text's color.
    pub color: Option<Color>,
}

impl Decoration {
    /// No lines.
    pub const NONE: Self = Self {
        underline: None,
        strikethrough: false,
        overline: false,
        color: None,
    };

    /// An underline in `style`.
    pub fn underline(style: UnderlineStyle) -> Self {
        Self {
            underline: Some(style),
            ..Self::NONE
        }
    }

    /// A strikethrough.
    pub fn strikethrough() -> Self {
        Self {
            strikethrough: true,
            ..Self::NONE
        }
    }

    /// An overline.
    pub fn overline() -> Self {
        Self {
            overline: true,
            ..Self::NONE
        }
    }

    /// These lines, also underlined in `style`.
    pub fn with_underline(mut self, style: UnderlineStyle) -> Self {
        self.underline = Some(style);
        self
    }

    /// These lines, also struck through.
    pub fn with_strikethrough(mut self) -> Self {
        self.strikethrough = true;
        self
    }

    /// These lines, also overlined.
    pub fn with_overline(mut self) -> Self {
        self.overline = true;
        self
    }

    /// These lines in `color` instead of the text's.
    pub fn with_color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    /// Whether this draws nothing.
    pub fn is_none(&self) -> bool {
        self.underline.is_none() && !self.strikethrough && !self.overline
    }
}

/// A stroke's thickness, at least one pixel so thin fonts' lines still show.
fn thickness(t: f64) -> f64 {
    t.round().max(1.0)
}

impl Scene {
    /// Draw `decoration`'s lines along a run of text `width` wide whose
    /// baseline starts at `baseline` (its left end), with positions and
    /// thickness from `metrics` (see [`Font::metrics`](crate::render::Font::metrics)).
    /// The lines take the decoration's color, else `text_color`.
    ///
    /// Straight lines are pixel-snapped rectangles; a curly underline is a
    /// stroked wave.
    pub fn draw_decoration(
        &mut self,
        metrics: &FontMetrics,
        baseline: Point,
        width: f64,
        decoration: &Decoration,
        text_color: Color,
    ) {
        if decoration.is_none() || width <= 0.0 {
            return;
        }
        let color = decoration.color.unwrap_or(text_color);
        let x = baseline.x;
        let line = |scene: &mut Scene, top: f64, t: f64| {
            scene.fill_rect(Rect::from_xywh(x, top.round(), width, t), color);
        };
        if let Some(style) = decoration.underline {
            let t = thickness(metrics.underline_thickness);
            let top = (baseline.y + metrics.underline_position).round();
            match style {
                UnderlineStyle::Single => line(self, top, t),
                UnderlineStyle::Double => {
                    line(self, top, t);
                    line(self, top + 2.0 * t, t);
                }
                UnderlineStyle::Dotted | UnderlineStyle::Dashed => {
                    let (on, off) = if style == UnderlineStyle::Dotted {
                        (t, t)
                    } else {
                        (3.0 * t, 2.0 * t)
                    };
                    let mut at = 0.0;
                    while at < width {
                        let w = on.min(width - at);
                        self.fill_rect(Rect::from_xywh(x + at, top, w, t), color);
                        at += on + off;
                    }
                }
                UnderlineStyle::Curly => self.curly(x, top, width, t, color),
            }
        }
        if decoration.strikethrough {
            let t = thickness(metrics.strikeout_thickness);
            line(self, baseline.y + metrics.strikeout_position, t);
        }
        if decoration.overline {
            let t = thickness(metrics.underline_thickness);
            line(self, baseline.y - metrics.ascent, t);
        }
    }

    /// A wave `t` thick filling `x .. x + width`, its crests at `top` and its
    /// troughs about `2t` lower: whole half-waves, stretched to fit.
    fn curly(&mut self, x: f64, top: f64, width: f64, t: f64, color: Color) {
        let amp = t.max(1.0);
        let mid = top + t / 2.0 + amp / 2.0;
        let half = (2.0 * amp).max(2.0);
        let n = (width / half).round().max(1.0) as usize;
        let half = width / n as f64;
        let p = |x: f64, y: f64| OxPoint::new(x as f32, y as f32);
        let mut path = Path::new();
        path.move_to(p(x, mid));
        for i in 0..n {
            let x0 = x + i as f64 * half;
            // A cubic whose controls sit 4/3 out reaches `amp` at its middle.
            let dy = if i % 2 == 0 { -amp } else { amp } * 4.0 / 3.0 / 2.0;
            path.cubic_to(
                p(x0 + half / 3.0, mid + dy),
                p(x0 + 2.0 * half / 3.0, mid + dy),
                p(x0 + half, mid),
            );
        }
        let stroke = Stroke::solid(t as f32, color.into()).with_cap(LineCap::Butt);
        self.push_node(Node::Path(PathNode::new(path).with_stroke(stroke)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{ScaleFactor, Size};
    use crate::render::{Pixmap, SoftwareRenderer};

    /// Metrics of a 20 px font with round numbers.
    fn metrics() -> FontMetrics {
        FontMetrics {
            size: 20.0,
            ascent: 16.0,
            descent: 4.0,
            line_gap: 0.0,
            line_height: 20.0,
            cap_height: 14.0,
            x_height: 10.0,
            max_advance: 12.0,
            underline_position: 2.0,
            underline_thickness: 1.0,
            strikeout_position: -6.0,
            strikeout_thickness: 2.0,
            monospace: true,
        }
    }

    /// Draw `deco` along x 10..90 with the baseline at y 30, black on white.
    fn draw(deco: Decoration) -> Pixmap {
        let mut scene = Scene::new(Size::new(100.0, 50.0));
        scene.fill_rect(Rect::from_xywh(0.0, 0.0, 100.0, 50.0), Color::WHITE);
        scene.draw_decoration(
            &metrics(),
            Point::new(10.0, 30.0),
            80.0,
            &deco,
            Color::BLACK,
        );
        SoftwareRenderer::new().render(scene, ScaleFactor::IDENTITY)
    }

    fn dark(px: &Pixmap, x: u32, y: u32) -> bool {
        px.pixel(x, y).is_some_and(|p| p[0] < 128)
    }

    /// The rows with any dark pixel.
    fn inked_rows(px: &Pixmap) -> Vec<u32> {
        (0..50)
            .filter(|&y| (0..100).any(|x| dark(px, x, y)))
            .collect()
    }

    #[test]
    fn nothing_draws_nothing() {
        assert!(inked_rows(&draw(Decoration::NONE)).is_empty());
    }

    #[test]
    fn single_underline_sits_at_the_underline_position() {
        let px = draw(Decoration::underline(UnderlineStyle::Single));
        assert_eq!(inked_rows(&px), [32]);
        // Exactly the run's extent.
        assert!(dark(&px, 10, 32) && dark(&px, 89, 32));
        assert!(!dark(&px, 9, 32) && !dark(&px, 90, 32));
    }

    #[test]
    fn double_underline_has_two_lines_with_a_gap() {
        let px = draw(Decoration::underline(UnderlineStyle::Double));
        assert_eq!(inked_rows(&px), [32, 34]);
    }

    #[test]
    fn strikethrough_and_overline_use_their_metrics() {
        let px = draw(Decoration::strikethrough());
        assert_eq!(
            inked_rows(&px),
            [24, 25],
            "2 px thick, 6 above the baseline"
        );
        let px = draw(Decoration::overline());
        assert_eq!(inked_rows(&px), [14], "at the ascent");
    }

    #[test]
    fn dotted_and_dashed_lines_are_broken() {
        let dotted = draw(Decoration::underline(UnderlineStyle::Dotted));
        let row: Vec<bool> = (10..90).map(|x| dark(&dotted, x, 32)).collect();
        assert!(row.iter().step_by(2).all(|d| *d), "every other pixel inked");
        assert!(row.iter().skip(1).step_by(2).all(|d| !*d));
        let dashed = draw(Decoration::underline(UnderlineStyle::Dashed));
        let row: Vec<bool> = (10..90).map(|x| dark(&dashed, x, 32)).collect();
        assert_eq!(&row[..6], [true, true, true, false, false, true]);
        let inked = row.iter().filter(|d| **d).count();
        assert!((40..=56).contains(&inked), "{inked} of 80 pixels in dashes");
    }

    #[test]
    fn curly_underline_waves_within_the_run() {
        let px = draw(Decoration::underline(UnderlineStyle::Curly));
        let rows = inked_rows(&px);
        assert!(rows.len() >= 2, "a wave spans rows: {rows:?}");
        assert!(rows.iter().all(|&y| (31..=36).contains(&y)), "{rows:?}");
        // Ink in each column of the run, none outside it.
        let cols = |x: u32| (0..50).any(|y| px.pixel(x, y).is_some_and(|p| p[0] < 200));
        assert!((12..88).all(cols));
        assert!(!cols(5) && !cols(95));
        // Crest and trough columns differ: the top row isn't inked everywhere.
        let top = rows[0];
        assert!((10..90).any(|x| !dark(&px, x, top)));
    }

    #[test]
    fn decoration_color_overrides_the_text_color() {
        let red = Color::rgb(255, 0, 0);
        let mut scene = Scene::new(Size::new(100.0, 50.0));
        let deco = Decoration::underline(UnderlineStyle::Single)
            .with_strikethrough()
            .with_color(red);
        scene.draw_decoration(
            &metrics(),
            Point::new(10.0, 30.0),
            80.0,
            &deco,
            Color::BLACK,
        );
        let px = SoftwareRenderer::new().render(scene, ScaleFactor::IDENTITY);
        assert_eq!(px.pixel(50, 32), Some([255, 0, 0, 255]));
        assert_eq!(px.pixel(50, 24), Some([255, 0, 0, 255]));
    }
}
