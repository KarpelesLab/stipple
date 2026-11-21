//! Color glyphs (`COLR`/`CPAL`, `sbix`) and the default OpenType features,
//! with fonts built here from the TrueType font of `cff_tests`: glyph 1 a
//! square, glyph 2 an arch, mapped from two characters.

use super::cff_tests::ttf_with;
use crate::geometry::{Point, ScaleFactor, Size};
use crate::render::{Color, Font, Pixmap, Scene, SoftwareRenderer, TextPaint};

fn be16(v: impl Into<i32>) -> [u8; 2] {
    (v.into() as u16).to_be_bytes()
}

fn words(ws: &[i32]) -> Vec<u8> {
    ws.iter().flat_map(|&w| be16(w)).collect()
}

const RED: [u8; 4] = [255, 0, 0, 255];

/// `COLR` version 0 making glyph 1 the square in red under the arch in
/// the text's color, and its one-palette `CPAL`.
fn colr_v0() -> Vec<(&'static [u8; 4], Vec<u8>)> {
    let mut colr = words(&[0, 1]); // version, one base glyph
    colr.extend_from_slice(&14u32.to_be_bytes()); // base records
    colr.extend_from_slice(&20u32.to_be_bytes()); // layer records
    colr.extend_from_slice(&be16(2));
    colr.extend(words(&[1, 0, 2])); // glyph 1: layers 0..2
    colr.extend(words(&[1, 0, 2, 0xFFFF])); // square in red, arch in fg
    let mut cpal = words(&[0, 1, 1, 1]); // v0, 1 entry, 1 palette, 1 color
    cpal.extend_from_slice(&14u32.to_be_bytes());
    cpal.extend(words(&[0]));
    cpal.extend_from_slice(&[RED[2], RED[1], RED[0], RED[3]]); // BGRA
    vec![(b"COLR", colr), (b"CPAL", cpal)]
}

/// An `sbix` table with one strike at 10 ppem holding a 10×10 green PNG
/// for glyph 2, its bottom-left corner on the pen.
fn sbix() -> Vec<(&'static [u8; 4], Vec<u8>)> {
    let png = oxideav_png::encode_rgba8(10, 10, &[0, 200, 0, 255].repeat(100), &Default::default())
        .unwrap();
    let mut data = words(&[0, 0]); // origin offsets
    data.extend_from_slice(b"png ");
    data.extend_from_slice(&png);
    // Strike: ppem, ppi, then 4 offsets (3 glyphs + end) from its start.
    let head = 4 + 4 * 4;
    let mut strike = words(&[10, 72]);
    for off in [head, head, head, head + data.len()] {
        strike.extend_from_slice(&(off as u32).to_be_bytes());
    }
    strike.extend(data);
    let mut table = words(&[1, 1]); // version, flags
    table.extend_from_slice(&1u32.to_be_bytes());
    table.extend_from_slice(&12u32.to_be_bytes());
    table.extend(strike);
    vec![(b"sbix", table)]
}

fn render(font: &Font, text: &str, color: Color, scale: f64) -> (Pixmap, f64) {
    let mut scene = Scene::new(Size::new(120.0, 120.0));
    scene.set_scale(scale);
    scene.fill_text(
        text,
        Point::new(0.0, 0.0),
        &TextPaint::new(font, 100.0, color),
    );
    let px = SoftwareRenderer::new()
        .background(Color::WHITE)
        .render(scene, ScaleFactor::new(scale));
    (px, font.baseline(100.0))
}

fn near(p: Option<[u8; 4]>, want: [u8; 3]) -> bool {
    p.is_some_and(|p| (0..3).all(|i| (p[i] as i32 - want[i] as i32).abs() < 40))
}

/// COLR v0 layers draw in their palette colors, and the foreground layer
/// in the text's color, whatever color the text is.
#[test]
fn colr_layers_draw_in_their_colors() {
    let font = Font::from_bytes(ttf_with("Colr Test", ['X', 'Y'], colr_v0())).unwrap();
    for (color, rgb) in [
        (Color::rgb(0, 0, 255), [0, 0, 255]),
        (Color::rgb(0, 160, 0), [0, 160, 0]),
    ] {
        let (px, base) = render(&font, "X", color, 1.0);
        // The square's top left corner, above the arch: red.
        assert!(
            near(px.pixel(15, (base - 65.0) as u32), [255, 0, 0]),
            "square {:?}",
            px.pixel(15, (base - 65.0) as u32)
        );
        // Inside the arch: the text's color.
        assert!(
            near(px.pixel(40, (base - 10.0) as u32), rgb),
            "arch {:?}",
            px.pixel(40, (base - 10.0) as u32)
        );
    }
    // Glyph 2 isn't a color glyph: an outline in the text's color.
    let (px, base) = render(&font, "Y", Color::rgb(0, 0, 255), 1.0);
    assert!(near(px.pixel(40, (base - 10.0) as u32), [0, 0, 255]));
}

/// An sbix bitmap fills its box (strike pixels scaled to the text size),
/// at 1x and 2x.
#[test]
fn sbix_bitmaps_fill_their_box() {
    let font = Font::from_bytes(ttf_with("Sbix Test", ['X', 'Y'], sbix())).unwrap();
    for scale in [1.0, 2.0] {
        let (px, base) = render(&font, "Y", Color::BLACK, scale);
        let at = |x: f64, y: f64| px.pixel((x * scale) as u32, (y * scale) as u32);
        assert!(
            near(at(50.0, base - 50.0), [0, 200, 0]),
            "{:?}",
            at(50.0, base - 50.0)
        );
        assert!(near(at(5.0, base - 50.0), [0, 200, 0]));
        // Nothing above the bitmap's top (100 px over the baseline).
        if base > 101.0 {
            assert!(near(at(50.0, base - 101.0), [255, 255, 255]));
        }
    }
    // Glyph 1 has no bitmap: the square outline.
    let (px, base) = render(&font, "X", Color::rgb(0, 0, 255), 1.0);
    assert!(near(px.pixel(15, (base - 65.0) as u32), [0, 0, 255]));
}

/// A `GSUB` with a `dlig` ligature (glyphs 1 2 → 2) and a `liga` one
/// (2 1 → 1), under `DFLT`.
fn gsub() -> Vec<(&'static [u8; 4], Vec<u8>)> {
    gsub_with(b"dlig", [1, 2, 2], [2, 1, 1])
}

/// A `GSUB` with lookup 0 (`first` feature) and lookup 1 (`liga`), each a
/// ligature `[first glyph, second glyph, ligature]`, under `DFLT`.
fn gsub_with(first: &[u8; 4], l0: [i32; 3], l1: [i32; 3]) -> Vec<(&'static [u8; 4], Vec<u8>)> {
    // One LigatureSubst format 1 lookup: `first second` → `out`.
    let lookup = |first: i32, second: i32, out: i32| {
        // Lookup header (8), subtable at 8: format, coverage at 8,
        // 1 set at 14; coverage (6 bytes) at 8; set at 14: 1 ligature at 4;
        // ligature: glyph, 2 components, the second.
        let mut l = words(&[4, 0, 1, 8]);
        l.extend(words(&[1, 8, 1, 14]));
        l.extend(words(&[1, 1, first]));
        l.extend(words(&[1, 4]));
        l.extend(words(&[out, 2, second]));
        l
    };
    let (dlig, liga) = (lookup(l0[0], l0[1], l0[2]), lookup(l1[0], l1[1], l1[2]));
    // ScriptList: DFLT → default LangSys with features 0 and 1.
    let mut scripts = words(&[1]);
    scripts.extend_from_slice(b"DFLT");
    scripts.extend(words(&[8, 4, 0, 0, 0xFFFF, 2, 0, 1]));
    // FeatureList: dlig → lookup 0, liga → lookup 1.
    let mut features = words(&[2]);
    features.extend_from_slice(first);
    features.extend(words(&[14]));
    features.extend_from_slice(b"liga");
    features.extend(words(&[20]));
    features.extend(words(&[0, 1, 0, 0, 1, 1]));
    // LookupList.
    let mut lookups = words(&[2, 6, 6 + dlig.len() as i32]);
    lookups.extend(dlig);
    lookups.extend(liga);
    let s = 10;
    let f = s + scripts.len();
    let l = f + features.len();
    let mut table = words(&[1, 0, s as i32, f as i32, l as i32]);
    table.extend(scripts);
    table.extend(features);
    table.extend(lookups);
    vec![(b"GSUB", table)]
}

/// Ligatures of the default features form; discretionary ones don't.
#[test]
fn only_default_ligatures_form() {
    let font = Font::from_bytes(ttf_with("Liga Test", ['X', 'Y'], gsub())).unwrap();
    let ids = |s: &str| -> Vec<u16> { font.shaped(s, 16.0).iter().map(|g| g.glyph_id).collect() };
    // Scribe's own shaping takes every ligature, `dlig`'s too.
    let all = font.chain().shape("XY", 16.0).unwrap();
    assert_eq!(all.len(), 1, "the font's dlig is read");
    assert_eq!(ids("YX"), [1], "liga applies");
    assert_eq!(ids("XY"), [1, 2], "dlig doesn't");
    assert_eq!(
        ids("XYX"),
        [1, 1],
        "liga after a glyph dlig would have taken"
    );
}

/// Ligature lookups apply one after the other over the whole run: the
/// second sees the first's ligatures (X Y → Y, then Y Y → X).
#[test]
fn ligature_lookups_apply_in_turn() {
    let font = Font::from_bytes(ttf_with(
        "Lookup Order",
        ['X', 'Y'],
        gsub_with(b"liga", [1, 2, 2], [2, 2, 1]),
    ))
    .unwrap();
    let ids = |s: &str| -> Vec<u16> { font.shaped(s, 16.0).iter().map(|g| g.glyph_id).collect() };
    assert_eq!(ids("XYY"), [1]);
    assert_eq!(ids("YY"), [1]);
    assert_eq!(ids("XY"), [2]);
}

/// Apple Color Emoji (`sbix`) draws emoji in color, where the system has
/// it.
#[test]
fn apple_color_emoji_draws_in_color() {
    let path = "/System/Library/Fonts/Apple Color Emoji.ttc";
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let Ok(font) = Font::from_bytes(bytes) else {
        return;
    };
    let mut scene = Scene::new(Size::new(60.0, 60.0));
    scene.fill_text(
        "\u{1F600}",
        Point::new(5.0, 5.0),
        &TextPaint::new(&font, 40.0, Color::BLACK),
    );
    let px = SoftwareRenderer::new()
        .background(Color::WHITE)
        .render(scene, ScaleFactor::IDENTITY);
    // The face is yellow: plenty of pixels with much more red than blue.
    let yellow = (0..60)
        .flat_map(|y| (0..60).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            px.pixel(x, y)
                .is_some_and(|p| p[0] > 180 && p[1] > 120 && p[2] < 100)
        })
        .count();
    assert!(yellow > 200, "{yellow} yellow pixels");
}

/// Real emoji sequences: skin tone, flag, keycap, ZWJ ones (one with a
/// skin tone, one with a selector inside), the rainbow
/// flag (selector and joiner), England's tag flag, a heart with its
/// selector. `true` for the ones every system font joins (Segoe UI Emoji
/// has no country flags, by design).
pub(crate) const SEQUENCES: &[(&str, &str, bool)] = &[
    ("skin tone", "\u{1F44D}\u{1F3FD}", true),
    ("flag", "\u{1F1EF}\u{1F1F5}", false),
    ("keycap", "1\u{FE0F}\u{20E3}", true),
    ("technologist", "\u{1F469}\u{1F3FD}\u{200D}\u{1F4BB}", true),
    (
        "couple with heart",
        "\u{1F469}\u{200D}\u{2764}\u{FE0F}\u{200D}\u{1F468}",
        true,
    ),
    ("rainbow flag", "\u{1F3F3}\u{FE0F}\u{200D}\u{1F308}", true),
    (
        "England",
        "\u{1F3F4}\u{E0067}\u{E0062}\u{E0065}\u{E006E}\u{E0067}\u{E007F}",
        true,
    ),
    ("heart", "\u{2764}\u{FE0F}", true),
];

/// Checks an installed emoji font joins [`SEQUENCES`], alone and as the
/// fallback of a text font, and draws them in color. Returns the names of
/// the sequences that didn't join, or `None` when the font isn't there.
fn joins_sequences(paths: &[&str]) -> Option<Vec<&'static str>> {
    let bytes = paths.iter().find_map(|p| std::fs::read(p).ok())?;
    let emoji = Font::from_bytes(bytes).ok()?;
    let text = super::cff_tests::ttf_with("Emoji Host", ['a', 'b'], vec![]);
    let host = Font::from_bytes(text)
        .unwrap()
        .with_fallbacks(std::slice::from_ref(&emoji));
    let mut split = Vec::new();
    for &(name, seq, _) in SEQUENCES {
        let alone = emoji.shaped(seq, 40.0);
        let hosted = host.shaped(&format!("a{seq}b"), 40.0);
        eprintln!(
            "{name}: alone {:?}, hosted {:?}",
            alone.iter().map(|g| g.glyph_id).collect::<Vec<_>>(),
            hosted
                .iter()
                .map(|g| (g.face_idx, g.glyph_id))
                .collect::<Vec<_>>()
        );
        // One glyph that takes room (Apple Color Emoji leaves an empty one
        // after some of its ligatures).
        let drawn = |glyphs: &[oxideav_scribe::PositionedGlyph]| {
            glyphs.iter().filter(|g| g.x_advance > 0.0).count()
        };
        let alone: Vec<_> = alone.into_iter().filter(|g| g.x_advance > 0.0).collect();
        if drawn(&alone) != 1 {
            split.push(name);
            continue;
        }
        // The same glyph between the host's letters, from the emoji face.
        let hosted: Vec<_> = hosted.into_iter().filter(|g| g.x_advance > 0.0).collect();
        assert_eq!(hosted.len(), 3, "{name} between letters");
        assert_eq!(hosted[1].glyph_id, alone[0].glyph_id, "{name} hosted");
        assert_eq!(hosted[1].face_idx, 1, "{name} from the emoji face");
        // One emoji wide.
        let one = emoji.measure("\u{1F600}", 40.0).width;
        let w = host.measure(seq, 40.0).width;
        assert!(
            (w - one).abs() < one * 0.25,
            "{name}: {w} wide, one emoji {one}"
        );
        // Drawn in color: plenty of saturated pixels.
        let mut scene = Scene::new(Size::new(80.0, 80.0));
        scene.fill_text(
            seq,
            Point::new(5.0, 5.0),
            &TextPaint::new(&host, 48.0, Color::BLACK),
        );
        let px = SoftwareRenderer::new()
            .background(Color::WHITE)
            .render(scene, ScaleFactor::IDENTITY);
        let colored = (0..80u32)
            .flat_map(|y| (0..80u32).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                px.pixel(x, y).is_some_and(|p| {
                    let (hi, lo) = (p[0].max(p[1]).max(p[2]), p[0].min(p[1]).min(p[2]));
                    hi as i32 - lo as i32 > 24
                })
            })
            .count();
        assert!(colored > 100, "{name}: {colored} colored pixels");
    }
    Some(split)
}

fn assert_joined(font: &str, split: Option<Vec<&'static str>>) {
    let Some(split) = split else {
        eprintln!("{font} isn't installed: skipped");
        return;
    };
    for &(name, _, always) in SEQUENCES {
        assert!(!always || !split.contains(&name), "{font} split the {name}");
    }
    eprintln!("{font}: split {split:?}");
}

/// Noto Color Emoji (`CBDT`, its sequences in `GSUB`), where installed
/// (Linux): every sequence joins.
#[test]
fn noto_color_emoji_joins_sequences() {
    let split = joins_sequences(&[
        "/usr/share/fonts/noto-emoji/NotoColorEmoji.ttf",
        "/usr/share/fonts/truetype/noto/NotoColorEmoji.ttf",
        "/usr/share/fonts/google-noto-emoji/NotoColorEmoji.ttf",
        "/system/fonts/NotoColorEmoji.ttf",
    ]);
    if let Some(split) = &split {
        assert!(split.is_empty(), "Noto Color Emoji split {split:?}");
    }
    assert_joined("Noto Color Emoji", split);
}

/// Segoe UI Emoji (`COLR`, `GSUB`), on Windows.
#[test]
fn segoe_ui_emoji_joins_sequences() {
    let split = joins_sequences(&["C:\\Windows\\Fonts\\seguiemj.ttf"]);
    assert_joined("Segoe UI Emoji", split);
}

/// Apple Color Emoji (`sbix`, its sequences in `morx`), on macOS: every
/// sequence joins.
#[test]
fn apple_color_emoji_joins_sequences() {
    let split = joins_sequences(&["/System/Library/Fonts/Apple Color Emoji.ttc"]);
    if let Some(split) = &split {
        assert!(split.is_empty(), "Apple Color Emoji split {split:?}");
    }
    assert_joined("Apple Color Emoji", split);
}
