//! The badge Stipple draws itself, where the system shows a picture rather
//! than a label: the overlay icon on a Windows taskbar button. A red disc
//! with the count in white (`99+` past 99), or the label itself when it is
//! one or two characters; a longer label shows the disc alone, a dot.

use crate::geometry::{Rect, Size};
use crate::render::{Color, Font, Pixmap, Scene, SoftwareRenderer};

/// What a badge label shows on a small icon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BadgeLook {
    /// Text in the disc: a count, or a label short enough.
    Text(String),
    /// The disc alone.
    Dot,
}

/// How `label` shows on an icon a few pixels wide.
pub(crate) fn look(label: &str) -> BadgeLook {
    let label = label.trim();
    if !label.is_empty() && label.bytes().all(|b| b.is_ascii_digit()) {
        let digits = label.trim_start_matches('0');
        return match digits.len() {
            0 => BadgeLook::Text("0".into()),
            1 | 2 => BadgeLook::Text(digits.into()),
            _ => BadgeLook::Text("99+".into()),
        };
    }
    match label.chars().count() {
        1 | 2 => BadgeLook::Text(label.into()),
        _ => BadgeLook::Dot,
    }
}

/// The disc's color: a notification red, as Windows' own badges use.
pub(crate) const BADGE_RED: Color = Color::rgb(0xD1, 0x34, 0x38);

/// Draw `label`'s badge `px` pixels square, its text in `font` (the disc
/// alone without one).
pub(crate) fn render(label: &str, px: u32, font: Option<&Font>) -> Pixmap {
    let px = px.max(8);
    let side = px as f64;
    let mut scene = Scene::new(Size::new(side, side));
    let disc = Rect::from_xywh(0.0, 0.0, side, side);
    scene.fill_round_rect(disc, side / 2.0, BADGE_RED);
    if let (BadgeLook::Text(text), Some(font)) = (look(label), font) {
        // As large as fits across the disc, inside a small margin.
        let room = side * if text.chars().count() > 2 { 0.95 } else { 0.8 };
        let mut size = side * 0.72;
        let mut measured = font.measure(&text, size);
        if measured.width > room && measured.width > 0.0 {
            size *= room / measured.width;
            measured = font.measure(&text, size);
        }
        let el = crate::core::Element::text(text, size, Color::WHITE).bold();
        let at = Rect::from_xywh(
            (side - measured.width) / 2.0,
            (side - measured.height) / 2.0,
            measured.width + 1.0,
            measured.height,
        );
        let tree = crate::core::layout(&el, at, Some(font));
        crate::core::paint(&tree, &mut scene, Some(font));
    }
    SoftwareRenderer::new().render_at(scene, crate::geometry::PhysicalSize::new(px, px))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_short_labels_show_and_longer_ones_are_a_dot() {
        let text = |s: &str| BadgeLook::Text(s.into());
        assert_eq!(look("3"), text("3"));
        assert_eq!(look("042"), text("42"));
        assert_eq!(look("0"), text("0"));
        assert_eq!(look("100"), text("99+"));
        assert_eq!(look("!"), text("!"));
        assert_eq!(look("é!"), text("é!"));
        assert_eq!(look("New"), BadgeLook::Dot);
        assert_eq!(look(""), BadgeLook::Dot);
    }

    #[test]
    fn the_badge_is_a_red_disc_with_white_text() {
        let font = Font::system_default();
        let p = render("7", 32, font.as_ref());
        assert_eq!(p.size(), crate::geometry::PhysicalSize::new(32, 32));
        // The corners are outside the disc.
        assert_eq!(p.pixel(0, 0).unwrap()[3], 0);
        assert_eq!(p.pixel(31, 31).unwrap()[3], 0);
        // Its rim is red.
        let rim = p.pixel(3, 16).unwrap();
        assert!(rim[0] > 180 && rim[1] < 90 && rim[3] > 200, "{rim:?}");
        // With a font, some of the middle is white.
        if font.is_some() {
            let white = (8..24)
                .flat_map(|y| (8..24).map(move |x| (x, y)))
                .filter(|&(x, y)| {
                    let c = p.pixel(x, y).unwrap();
                    c[0] > 220 && c[1] > 220 && c[2] > 220
                })
                .count();
            assert!(white > 4, "{white} white pixels");
        }
        // Without one, a plain disc.
        let dot = render("New", 16, None);
        let mid = dot.pixel(8, 8).unwrap();
        assert!(mid[0] > 180 && mid[1] < 90, "{mid:?}");
    }
}
