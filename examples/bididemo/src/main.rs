//! Right-to-left and bidirectional text: Hebrew, Arabic and mixed lines in
//! labels, a wrapped right-to-left paragraph, and editors.
//!
//! Each paragraph's direction comes from its first strong character unless set
//! with `.direction(..)`. Right-to-left lines sit against the right edge of
//! their (stretched) box; mixed lines put each run in visual order. Click into
//! the editors to see the caret and selection follow the visual order: the
//! arrow keys move on screen, and Shift+arrows can select a logical range that
//! shows as several rectangles.
//!
//! The `visual-bidi` CI job screenshots the window and checks that the Hebrew
//! and Arabic rows are inked on the right and blank on the left, and the
//! left-to-right row the other way round. The rows have fixed heights so those
//! regions don't depend on the font; the unit test below pins them.

use stipple::prelude::*;

/// Height of each label row, so the CI crops sit at fixed places.
const ROW: f64 = 28.0;

struct Demo {
    mixed: EditBuffer,
    notes: EditBuffer,
}

fn demo_state() -> Demo {
    Demo {
        mixed: EditBuffer::from_text("abc אבג 123 def (שלום)"),
        notes: EditBuffer::from_text(
            "שלום, זו שורה בעברית.\nAn English line.\nمرحبا، هذا سطر بالعربية.",
        ),
    }
}

fn view(state: &Demo, cx: &mut Cx<Demo>) -> Element {
    let theme = *cx.theme();
    let row = |el: Element| el.height(ROW);
    column(vec![
        row(heading(&theme, "Bidirectional text")),
        // Left to right, with a Hebrew word and a number inside.
        row(label(&theme, "Mixed: abc אבג 123 def")),
        // Right to left, detected from the first letter.
        row(label(&theme, "שלום עולם! זו שורה בעברית.")),
        // Arabic: the letters join, and the line is right to left.
        row(label(&theme, "مرحبا بالعالم! هذا سطر بالعربية.")),
        // Forced right to left although it starts with a number and Latin.
        row(label(&theme, "2024 – English בתוך עברית (עם סוגריים)").direction(TextDirection::Rtl)),
        // A wrapped right-to-left paragraph: every line is right-aligned.
        paragraph(
            &theme,
            "זוהי פסקה ארוכה בעברית שנשברת לכמה שורות. המילים נשברות ברווחים, \
             וכל שורה מיושרת לימין, כמו שצריך. גם מספרים כמו 3.14 ו-42 נשארים בסדר הנכון.",
        ),
        text_editor(cx, &theme, &state.mixed, |s: &mut Demo| &mut s.mixed).width(592.0),
        text_area(cx, &theme, &state.notes, |s: &mut Demo| &mut s.notes)
            .width(592.0)
            .height(96.0),
    ])
    .gap(8.0)
    .padding(Insets::uniform(24.0))
    .align(Align::Start, Align::Stretch)
    .grow(1.0)
}

fn demo_app() -> App<Demo> {
    App::new(demo_state(), view)
        .title("Stipple — bidirectional text")
        .theme(Theme::light())
        .logical_size(Size::new(640.0, 480.0))
}

fn main() {
    let mut app = demo_app();
    if let Some(font) = Font::system_default() {
        app = app.font(font);
    }
    app.run();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Distinct colors in a `w`×`h` region at `(x, y)` of `pm`.
    fn colors(pm: &Pixmap, x: u32, y: u32, w: u32, h: u32) -> usize {
        let mut seen = std::collections::HashSet::new();
        for yy in y..y + h {
            for xx in x..x + w {
                seen.insert(pm.pixel(xx, yy).unwrap());
            }
        }
        seen.len()
    }

    /// The regions the `visual-bidi` CI job checks: row `k` spans
    /// y = 24 + 36k .. +28; the left region is x 24..224, the right 416..616.
    #[test]
    fn rtl_rows_are_inked_on_the_right() {
        let Some(font) = Font::system_default() else {
            return;
        };
        // Direction comes from the characters, not the font, so this holds
        // even where the font draws missing-glyph boxes.
        let pm = demo_app().font(font).render_once();
        let y = |k: u32| 24 + 36 * k;
        // Row 1: left to right.
        assert!(colors(&pm, 24, y(1), 200, 28) > 2);
        assert_eq!(colors(&pm, 416, y(1), 200, 28), 1);
        // Rows 2 to 4: right to left.
        for k in 2..=4 {
            assert_eq!(colors(&pm, 24, y(k), 200, 28), 1, "row {k} left");
            assert!(colors(&pm, 416, y(k), 200, 28) > 2, "row {k} right");
        }
    }
}
