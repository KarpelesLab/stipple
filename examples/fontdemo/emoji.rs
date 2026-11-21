//! `--emoji`: emoji sequences drawn through the system's fonts, off screen.
//!
//! Each sequence (a skin tone, a flag, a keycap, ZWJ sequences, a tag flag,
//! a heart with its selector) is measured and drawn after a Latin letter in
//! the system's default font, which loads the system's emoji family for it
//! (Apple Color Emoji's `sbix` and `morx` on macOS, Segoe UI Emoji's `COLR`
//! on Windows, Noto Color Emoji's `CBDT` on Linux). A sequence passes when
//! it is one emoji wide (one glyph, not its parts side by side) and drawn
//! in color. Windows' Segoe UI Emoji has no country flags, so they may draw as
//! their letters there.

use stipple::prelude::*;
use stipple::render::{Scene, SoftwareRenderer, TextPaint};

const SIZE: f64 = 40.0;

/// Name, text, whether the system's emoji font may lack it.
const SEQUENCES: &[(&str, &str, bool)] = &[
    ("grinning face", "\u{1F600}", false),
    ("skin tone", "\u{1F44D}\u{1F3FD}", false),
    ("flag", "\u{1F1EF}\u{1F1F5}", cfg!(windows)),
    ("keycap", "1\u{FE0F}\u{20E3}", false),
    ("technologist", "\u{1F469}\u{1F3FD}\u{200D}\u{1F4BB}", false),
    (
        "couple with heart",
        "\u{1F469}\u{200D}\u{2764}\u{FE0F}\u{200D}\u{1F468}",
        false,
    ),
    ("rainbow flag", "\u{1F3F3}\u{FE0F}\u{200D}\u{1F308}", false),
    (
        "England",
        "\u{1F3F4}\u{E0067}\u{E0062}\u{E0065}\u{E006E}\u{E0067}\u{E007F}",
        false,
    ),
    ("heart", "\u{2764}\u{FE0F}", false),
];

/// Saturated pixels in `pm`: drawn in color, not gray.
fn colored(pm: &Pixmap) -> usize {
    let size = pm.size();
    (0..size.height)
        .flat_map(|y| (0..size.width).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            pm.pixel(x, y).is_some_and(|p| {
                let (hi, lo) = (p[0].max(p[1]).max(p[2]), p[0].min(p[1]).min(p[2]));
                hi as i32 - lo as i32 > 24
            })
        })
        .count()
}

fn draw(font: &Font, text: &str, width: f64) -> Pixmap {
    let mut scene = Scene::new(Size::new(width, SIZE * 1.6));
    scene.fill_text(
        text,
        Point::new(4.0, 4.0),
        &TextPaint::new(font, SIZE, Color::BLACK),
    );
    SoftwareRenderer::new()
        .background(Color::WHITE)
        .render(scene, ScaleFactor::IDENTITY)
}

/// Run the checks, printing a line per sequence; `out` saves the strip of
/// every sequence there.
pub fn check(out: Option<&str>) -> Result<(), String> {
    let font = Font::system_default().ok_or("no system font")?;
    let one = font.measure("\u{1F600}", SIZE).width;
    let letter = font.measure("a", SIZE).width;
    let mut failed = Vec::new();
    for &(name, seq, may_split) in SEQUENCES {
        let width = font.measure(&format!("a{seq}"), SIZE).width - letter;
        let pm = draw(&font, seq, SIZE * 3.0);
        let color = colored(&pm);
        let single = (width - one).abs() < one * 0.25;
        let ok = single && color > 100;
        println!(
            "fontdemo: emoji {name}: {width:.1} wide (one emoji {one:.1}), {color} colored pixels{}",
            if ok { "" } else { " -- SPLIT OR NOT IN COLOR" }
        );
        if !ok && !may_split {
            failed.push(name);
        }
    }
    if let Some(path) = out {
        let strip: String = SEQUENCES.iter().map(|s| s.1).collect::<Vec<_>>().join(" ");
        let pm = draw(&font, &format!("a{strip}"), SIZE * 20.0);
        pm.save_png(path).map_err(|e| e.to_string())?;
    }
    if one < SIZE * 0.5 {
        return Err(format!("the plain emoji is only {one:.1} wide"));
    }
    if !failed.is_empty() {
        return Err(format!("split or not in color: {failed:?}"));
    }
    Ok(())
}
