//! Right-to-left and bidirectional text: Hebrew, Arabic and mixed lines in
//! labels, a wrapped right-to-left paragraph, and editors. With `--mirror`, a
//! mirrored (right-to-left) UI instead: the same widgets an app would use, laid
//! out from the right.
//!
//! Each paragraph's direction comes from its first strong character unless set
//! with `.direction(..)`. Right-to-left lines sit against the right edge of
//! their (stretched) box; mixed lines put each run in visual order. Click into
//! the editors to see the caret and selection follow the visual order: the
//! arrow keys move on screen, and Shift+arrows can select a logical range that
//! shows as several rectangles.
//!
//! `--mirror` sets `Theme::with_layout_direction(LayoutDirection::Rtl)`: rows
//! run right to left, checkboxes and switches sit right of their labels,
//! progress bars and sliders fill from the right, the selected first tab is
//! the rightmost, grid columns count from the right, and the tree's triangles
//! point left. `--mirror-check` renders that UI off screen, checks it, prints
//! `bididemo mirror: ok` and exits (CI runs it on Windows and macOS).
//!
//! The `visual-bidi` CI job screenshots both windows and checks that the
//! right-to-left rows are inked on the right and blank on the left (and the
//! left-to-right row the other way round). The rows have fixed heights so those
//! regions don't depend on the font; the unit tests below pin them.

use stipple::prelude::*;

/// Height of each label row, so the CI crops sit at fixed places.
const ROW: f64 = 28.0;

struct Demo {
    mixed: EditBuffer,
    notes: EditBuffer,
    name: EditBuffer,
    agree: bool,
    wifi: bool,
    volume: f64,
    tab: usize,
    tree: TreeState,
}

fn demo_state() -> Demo {
    let mut tree = TreeState::new();
    tree.set_expanded(0, true);
    Demo {
        mixed: EditBuffer::from_text("abc אבג 123 def (שלום)"),
        notes: EditBuffer::from_text(
            "שלום, זו שורה בעברית.\nAn English line.\nمرحبا، هذا سطر بالعربية.",
        ),
        name: EditBuffer::new(),
        agree: true,
        wifi: true,
        volume: 0.25,
        tab: 0,
        tree,
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

/// A small folder tree: 0 and 1 have children, 2 is a leaf.
struct Folders;

impl TreeModel for Folders {
    fn children(&self, parent: Option<u64>) -> Vec<u64> {
        match parent {
            None => vec![0, 1, 2],
            Some(0) => vec![10, 11],
            Some(1) => vec![20],
            Some(_) => Vec::new(),
        }
    }
}

fn folder_name(id: u64) -> &'static str {
    match id {
        0 => "Documents (מסמכים)",
        1 => "Pictures",
        2 => "Music",
        10 => "report.txt",
        11 => "דוח.txt",
        _ => "photo.png",
    }
}

/// The mirrored UI. Rows 0 to 5 are `ROW` high, so row `k` spans
/// y = 24 + 36k .. +28 (what the checks look at).
fn mirrored_view(state: &Demo, cx: &mut Cx<Demo>) -> Element {
    let theme = *cx.theme();
    let line = |children: Vec<Element>| {
        row(children)
            .gap(8.0)
            .align(Align::Start, Align::Center)
            .height(ROW)
    };
    let checkbox = checkbox(cx, &theme, state.agree, |s: &mut Demo| s.agree = !s.agree);
    let switch = switch(cx, &theme, state.wifi, |s: &mut Demo| s.wifi = !s.wifi);
    let slider = slider(cx, &theme, state.volume, |s: &mut Demo, v| s.volume = v);
    let tabs = tabs(
        cx,
        &theme,
        &["One", "Two", "Three"],
        state.tab,
        |s: &mut Demo, i| {
            s.tab = i;
        },
    );
    let name = text_editor(cx, &theme, &state.name, |s: &mut Demo| &mut s.name).width(240.0);
    let tree = tree(
        cx,
        &theme,
        &state.tree,
        |s: &mut Demo| &mut s.tree,
        &Folders,
        TreeSpec::default().height(96.0),
        |_cx, row| label(&theme, folder_name(row.id)),
    );
    column(vec![
        // 0: Latin text in a mirrored UI starts on the right.
        line(vec![heading(&theme, "Mirrored UI")]),
        // 1, 2: the control comes first, so it sits rightmost.
        line(vec![checkbox, label(&theme, "Checkbox label")]),
        line(vec![switch, label(&theme, "Switch label")]),
        // 3, 4: filled from the right.
        line(vec![progress_bar(&theme, 0.3).width(592.0)]),
        line(vec![slider.width(592.0)]),
        // 5: the first (selected) tab is the rightmost.
        tabs.height(ROW),
        // Grid columns count from the right: the label is right of the field.
        grid(
            vec![Track::Auto, Track::fr(1.0)],
            vec![label(&theme, "Name"), row(vec![name])],
        )
        .gap(8.0)
        .align(Align::Start, Align::Center),
        line(vec![
            label(&theme, "שלום! Hebrew and English."),
            spacer(),
            button_labeled(&theme, "OK"),
        ]),
        tree,
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

fn mirrored_app() -> App<Demo> {
    App::new(demo_state(), mirrored_view)
        .title("Stipple - mirrored UI")
        .theme(Theme::light().with_layout_direction(LayoutDirection::Rtl))
        .logical_size(Size::new(640.0, 480.0))
}

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

/// Check a rendering of the mirrored UI: rows 0 to 2 are inked only on the
/// right (left region x 24..224, right 416..616, row k at y = 24 + 36k), and
/// the progress bar (row 3) and the tab strip (row 5) show the primary color
/// at their right ends, not their left. The same checks as the CI shell steps.
fn check_mirrored(pm: &Pixmap) -> Result<(), String> {
    let y = |k: u32| 24 + 36 * k;
    for k in 0..=2 {
        let (left, right) = (
            colors(pm, 24, y(k), 200, 28),
            colors(pm, 416, y(k), 200, 28),
        );
        println!("bididemo mirror: row {k} left={left} right={right}");
        if left != 1 || right <= 2 {
            return Err(format!("row {k} is not inked on the right only"));
        }
    }
    let px = |x: u32, y: u32| pm.pixel(x, y).unwrap();
    // Row 3: the bar is 8 high, centered in the row; row 5: a tab's fill.
    let (bar_right, bar_left) = (px(612, y(3) + 14), px(28, y(3) + 14));
    let (tab_right, tab_left) = (px(612, y(5) + 14), px(28, y(5) + 14));
    println!(
        "bididemo mirror: bar right={bar_right:?} left={bar_left:?}, \
         tab right={tab_right:?} left={tab_left:?}"
    );
    if bar_right != tab_right || bar_left == bar_right || tab_left == tab_right {
        return Err("the bar and the selected tab are not filled from the right".into());
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let check = args.iter().any(|a| a == "--mirror-check");
    let mirror = check || args.iter().any(|a| a == "--mirror");
    let mut app = if mirror { mirrored_app() } else { demo_app() };
    if let Some(font) = Font::system_default() {
        app = app.font(font);
    }
    if check {
        match check_mirrored(&app.render_once()) {
            Ok(()) => println!("bididemo mirror: ok"),
            Err(e) => {
                println!("bididemo mirror: FAILED: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    app.run();
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// What `--mirror-check` and the CI job check of the mirrored UI.
    #[test]
    fn the_mirrored_ui_is_laid_out_from_the_right() {
        let Some(font) = Font::system_default() else {
            return;
        };
        let pm = mirrored_app().font(font).render_once();
        check_mirrored(&pm).unwrap();
    }

    /// Without the theme's direction, the same view is laid out from the left
    /// and fails the checks: they do test the mirroring.
    #[test]
    fn the_checks_fail_left_to_right() {
        let Some(font) = Font::system_default() else {
            return;
        };
        let pm = mirrored_app()
            .theme(Theme::light())
            .font(font)
            .render_once();
        assert!(check_mirrored(&pm).is_err());
    }
}
