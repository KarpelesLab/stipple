//! Fonts and text decorations: a system monospace family found by name and
//! drawn regular, bold, italic and bold italic (the family's own faces, or
//! synthesized ones where it has none), with a fallback for symbols; the
//! metrics a terminal sizes its cells with; and every decoration — the five
//! underline styles, strikethrough, overline, and lines in their own color.
//!
//! `--list` prints the installed families (what a font picker would offer).
//! `--family NAME` draws NAME instead of `monospace`. `--check` renders the
//! window off screen, checks that the bold row carries more ink than the
//! regular one and that each decorated row has its lines, prints
//! `fontdemo: ok` and exits.
//!
//! Rows are `ROW` high, row `k` spanning y = 16 + `ROW`·k, so the CI checks
//! (the `visual-fonts` job) look at fixed places.

use stipple::prelude::*;

/// Height of each row.
const ROW: f64 = 30.0;
/// Text size of the sample rows.
const SIZE: f64 = 16.0;
/// Sample text: Latin, digits, and a check mark most monospace fonts lack.
const SAMPLE: &str = "The quick brown fox 0123 \u{2713}";

struct Fonts {
    mono: Font,
}

/// The decorated rows: a caption and its decoration.
fn decorations() -> Vec<(&'static str, Decoration)> {
    let red = Color::rgb(220, 30, 30);
    let blue = Color::rgb(30, 90, 220);
    vec![
        (
            "single underline",
            Decoration::underline(UnderlineStyle::Single),
        ),
        (
            "double underline",
            Decoration::underline(UnderlineStyle::Double),
        ),
        (
            "curly underline, red",
            Decoration::underline(UnderlineStyle::Curly).with_color(red),
        ),
        (
            "dotted underline",
            Decoration::underline(UnderlineStyle::Dotted),
        ),
        (
            "dashed underline",
            Decoration::underline(UnderlineStyle::Dashed),
        ),
        ("strikethrough", Decoration::strikethrough()),
        ("overline", Decoration::overline()),
        (
            "all three, blue",
            Decoration::underline(UnderlineStyle::Single)
                .with_strikethrough()
                .with_overline()
                .with_color(blue),
        ),
    ]
}

/// The style rows: a caption and its style.
const STYLES: [(&str, FontStyle); 4] = [
    ("Regular    ", FontStyle::NORMAL),
    ("Bold       ", FontStyle::BOLD),
    ("Italic     ", FontStyle::ITALIC),
    ("Bold italic", FontStyle::BOLD_ITALIC),
];

fn view(state: &Fonts, cx: &mut Cx<Fonts>) -> Element {
    let theme = *cx.theme();
    let ink = theme.palette.text;
    let line = |el: Element| row(vec![el]).height(ROW).align(Align::Start, Align::Center);
    let mono = &state.mono;
    let m = mono.metrics(SIZE);
    let name = mono.family_name().unwrap_or("?").to_string();
    let mut rows = vec![line(heading(&theme, format!("{name}, {SIZE} px")))];
    for (caption, style) in STYLES {
        let (bold, oblique) = mono.styled(style).is_synthesized();
        let note = match (bold, oblique) {
            (false, false) => "",
            (true, false) => "  (synthetic bold)",
            (false, true) => "  (synthetic oblique)",
            (true, true) => "  (synthetic)",
        };
        rows.push(line(
            Element::text(format!("{caption}  {SAMPLE}{note}"), SIZE, ink)
                .font(mono.clone())
                .weight(style.weight)
                .italic_if(style.italic),
        ));
    }
    for (caption, deco) in decorations() {
        rows.push(line(
            Element::text(format!("{caption:<22}{SAMPLE}"), SIZE, ink)
                .font(mono.clone())
                .decorate(deco),
        ));
    }
    rows.push(line(label(
        &theme,
        format!(
            "cell {:.1} x {:.1} px, ascent {:.1}, descent {:.1}, underline +{:.1} ({:.1} thick), \
             strikeout {:.1}",
            mono.advance_of('M', SIZE),
            m.line_height,
            m.ascent,
            m.descent,
            m.underline_position,
            m.underline_thickness,
            m.strikeout_position,
        ),
    )));
    column(rows)
        .padding(Insets::new(16.0, 24.0, 16.0, 24.0))
        .align(Align::Start, Align::Stretch)
        .grow(1.0)
}

/// `italic()` when `on`.
trait ItalicIf {
    fn italic_if(self, on: bool) -> Self;
}

impl ItalicIf for Element {
    fn italic_if(self, on: bool) -> Self {
        if on { self.italic() } else { self }
    }
}

/// The family to draw, with a symbol fallback.
fn load(family: &str) -> Option<Font> {
    let mono = Font::from_family(family, FontStyle::NORMAL).ok()?;
    let symbols = [
        "DejaVu Sans",
        "Segoe UI Symbol",
        "Apple Symbols",
        "Noto Sans Symbols2",
    ]
    .iter()
    .filter_map(|f| Font::from_family(f, FontStyle::NORMAL).ok())
    .collect::<Vec<_>>();
    Some(mono.with_fallbacks(&symbols))
}

fn demo_app(mono: Font) -> App<Fonts> {
    let mut app = App::new(Fonts { mono }, view)
        .title("Stipple — fonts and decorations")
        .theme(Theme::light())
        .logical_size(Size::new(720.0, 520.0));
    if let Some(ui) = Font::system_default() {
        app = app.font(ui);
    }
    app
}

/// Dark pixels in row `k`'s band, between x `x0` and `x1`.
fn ink_in(pm: &Pixmap, k: u32, x0: u32, x1: u32) -> usize {
    let top = 16 + ROW as u32 * k;
    (top..top + ROW as u32)
        .flat_map(|y| (x0..x1).map(move |x| (x, y)))
        .filter(|&(x, y)| pm.pixel(x, y).is_some_and(|p| p[0] < 128))
        .count()
}

/// Rows of row `k`'s band where most pixels between x `x0` and `x1` are
/// colored (not the background): a decoration line across the text.
fn lines_in(pm: &Pixmap, k: u32, x0: u32, x1: u32) -> usize {
    let top = 16 + ROW as u32 * k;
    let bg = pm.pixel(2, 2).unwrap();
    (top..top + ROW as u32)
        .filter(|&y| {
            let marked = (x0..x1).filter(|&x| pm.pixel(x, y) != Some(bg)).count();
            marked * 10 >= (x1 - x0) as usize * 6
        })
        .count()
}

/// The checks `--check` and the unit test make.
fn check(pm: &Pixmap, cell: f64) -> Result<(), String> {
    let (regular, bold) = (ink_in(pm, 1, 24, 700), ink_in(pm, 2, 24, 700));
    println!("fontdemo: regular ink {regular}, bold ink {bold}");
    if bold * 10 < regular * 11 {
        return Err("the bold row isn't heavier than the regular one".into());
    }
    // Decorated rows 5..=12 span the sample text, which starts after the
    // 22-column caption; lines must cross it where plain text alone doesn't.
    let x0 = 24 + (22.0 * cell).ceil() as u32;
    let plain = lines_in(pm, 1, x0, x0 + 150);
    for k in 5..=12 {
        let n = lines_in(pm, k, x0, x0 + 150);
        println!("fontdemo: row {k}: {n} line rows (plain text: {plain})");
        // Dotted and dashed lines cover about half the run.
        let broken = k == 8 || k == 9;
        if !broken && n <= plain {
            return Err(format!("row {k} has no decoration line"));
        }
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--list") {
        for f in Font::families() {
            let styles: Vec<String> = f
                .styles
                .iter()
                .map(|s| format!("{}{}", s.weight.0, if s.italic { "i" } else { "" }))
                .collect();
            let mono = if f.monospace { " [mono]" } else { "" };
            println!("{}{mono}: {}", f.name, styles.join(" "));
        }
        return;
    }
    let family = args
        .iter()
        .position(|a| a == "--family")
        .and_then(|i| args.get(i + 1))
        .map_or("monospace", String::as_str);
    let Some(mono) = load(family) else {
        eprintln!("fontdemo: no font family {family:?} on this system");
        std::process::exit(1);
    };
    let m = mono.metrics(SIZE);
    println!(
        "fontdemo: {:?} styles {:?}; at {SIZE}px cell {:.2} line {:.2} ascent {:.2} \
         descent {:.2} underline {:.2}/{:.2} strikeout {:.2}/{:.2} mono {}",
        mono.family_name(),
        mono.available_styles(),
        mono.advance_of('M', SIZE),
        m.line_height,
        m.ascent,
        m.descent,
        m.underline_position,
        m.underline_thickness,
        m.strikeout_position,
        m.strikeout_thickness,
        m.monospace,
    );
    for (caption, style) in STYLES {
        let (bold, oblique) = mono.styled(style).is_synthesized();
        if bold || oblique {
            println!("fontdemo: {} is synthetic", caption.trim());
        }
    }
    let cell = mono.advance_of('M', SIZE);
    let mut app = demo_app(mono);
    if args.iter().any(|a| a == "--check") {
        match check(&app.render_once(), cell) {
            Ok(()) => println!("fontdemo: ok"),
            Err(e) => {
                println!("fontdemo: FAILED: {e}");
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

    #[test]
    fn bold_is_heavier_and_decorations_draw() {
        let Some(mono) = load("monospace") else {
            return;
        };
        let cell = mono.advance_of('M', SIZE);
        check(&demo_app(mono).render_once(), cell).unwrap();
    }
}
