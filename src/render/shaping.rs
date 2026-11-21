//! Shaping with the OpenType features other toolkits turn on by default, and
//! no others.
//!
//! `oxideav-scribe`'s `FaceChain::shape` forms ligatures from every ligature
//! lookup in the font, whatever feature it belongs to, so Roboto's
//! discretionary `dlig` "st" and the historical `hlig` forms showed up in
//! running text. Here the chain still picks each character's face (and does
//! its Arabic joining and Indic reordering), but each face's run goes through
//! the same pipeline as scribe's (`ccmp`, ligatures, `calt`, GPOS) with only
//! the ligature lookups of the default-on features: `liga`, `clig`, `rlig`,
//! `ccmp`, and the substitution features complex scripts require.
//!
//! Emoji sequences (skin tones, flags, keycaps, ZWJ sequences) are shaped
//! each on its own, whole, in the color font that has them, so that font's
//! ligature forms (see [`emoji_seq`]). A face with Apple's `morx` table and
//! no `GSUB` (Apple Color Emoji) is shaped through its `morx`
//! ([`super::morx`]).

use super::emoji_seq;
use oxideav_scribe::shaper::position_run_with_font;
use oxideav_scribe::shaping::{apply_calt, apply_ccmp};
use oxideav_scribe::{FaceChain, PositionedGlyph};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Features whose ligature lookups are applied: the default-on Latin ones
/// and the substitutions complex scripts need (Indic, Khmer, Myanmar,
/// Hangul jamo, Arabic's required ligatures). Never `dlig`, `hlig`, `salt`,
/// `swsh`, stylistic sets or the like, which are for an app to ask for.
const DEFAULT_FEATURES: &[[u8; 4]] = &[
    *b"ccmp", *b"locl", *b"rlig", *b"liga", *b"clig", // Latin and general
    *b"nukt", *b"akhn", *b"rphf", *b"rkrf", *b"pref", *b"blwf", *b"abvf", *b"half", *b"pstf",
    *b"vatu", *b"cjct", *b"cfar", *b"pres", *b"abvs", *b"blws", *b"psts", *b"haln", // Indic
    *b"ljmo", *b"vjmo", *b"tjmo", // Hangul
    *b"mset", // Arabic mark placement
];

/// The script tags whose default language systems are searched for those
/// features.
const SCRIPTS: &[[u8; 4]] = &[
    *b"DFLT", *b"latn", *b"cyrl", *b"grek", *b"arab", *b"hebr", *b"syrc", *b"thaa", *b"nko ",
    *b"thai", *b"lao ", *b"deva", *b"dev2", *b"beng", *b"bng2", *b"taml", *b"tml2", *b"gujr",
    *b"gjr2", *b"guru", *b"gur2", *b"knda", *b"knd2", *b"mlym", *b"mlm2", *b"orya", *b"ory2",
    *b"telu", *b"tel2", *b"sinh", *b"khmr", *b"mymr", *b"mym2", *b"tibt", *b"hang", *b"hani",
    *b"kana", *b"armn", *b"geor", *b"ethi",
];

/// GSUB LookupType 4, ligature substitution.
const LIGATURE: u16 = 4;

/// What shaping needs to know of a face, read once per face.
#[derive(Default)]
struct FaceShaping {
    /// The ligature lookups to apply, in lookup order.
    ligatures: Vec<u16>,
    /// Its `morx` table, for a font that has no `GSUB` substitutions
    /// (Apple Color Emoji and other AAT-only fonts).
    morx: Option<Vec<u8>>,
    /// Whether its glyphs draw in color (`COLR`, `sbix` or `CBDT`).
    color: bool,
    /// GPOS single adjustments that only contextual lookups use: scribe's
    /// positioning applies every single adjustment to every glyph it
    /// covers, and these again in their context, so their global pass is
    /// undone (Segoe UI Emoji's `dist` takes the advance off the first
    /// two layers of its couples that way; applied twice, they went
    /// backwards).
    nested_single_pos: Vec<u16>,
}

/// GPOS LookupType 1, single adjustment.
const SINGLE_POS: u16 = 1;

/// The single adjustment lookups of `font` no feature lists itself.
fn nested_single_pos(font: &oxideav_ttf::Font<'_>) -> Vec<u16> {
    let single: Vec<u16> = font
        .gpos_lookup_list()
        .into_iter()
        .filter(|&(_, kind, _)| kind == SINGLE_POS)
        .map(|(i, _, _)| i)
        .collect();
    if single.is_empty() {
        return single;
    }
    let mut listed = std::collections::HashSet::new();
    for &script in SCRIPTS {
        for feature in font.gpos_features_for_script(script, None) {
            listed.extend(feature.lookup_indices);
        }
    }
    single.into_iter().filter(|i| !listed.contains(i)).collect()
}

/// Positions for `glyphs`, as scribe gives them less the global pass of
/// the nested-only single adjustments (see
/// [`FaceShaping::nested_single_pos`]).
fn position(
    font: &oxideav_ttf::Font<'_>,
    glyphs: &[u16],
    components: &[u16],
    scale: f32,
    face_idx: u16,
    info: &FaceShaping,
) -> Vec<PositionedGlyph> {
    let mut out = position_run_with_font(font, glyphs, components, scale, face_idx);
    for g in out.iter_mut() {
        for &lookup in &info.nested_single_pos {
            if let Some(v) = font.gpos_apply_lookup_type_1(lookup, g.glyph_id) {
                g.x_offset -= f32::from(v.x_placement) * scale;
                g.y_offset += f32::from(v.y_placement) * scale;
                g.x_advance -= f32::from(v.x_advance) * scale;
            }
        }
    }
    out
}

/// Per face, by its stable id.
static FACES: Mutex<Option<HashMap<u64, Arc<FaceShaping>>>> = Mutex::new(None);

/// The ligature lookups of `font` that belong to a default-on feature.
fn default_ligatures(font: &oxideav_ttf::Font<'_>) -> Vec<u16> {
    let ligatures: Vec<u16> = font
        .gsub_lookup_list()
        .into_iter()
        .filter(|&(_, kind, _)| kind == LIGATURE)
        .map(|(i, _, _)| i)
        .collect();
    if ligatures.is_empty() {
        return ligatures;
    }
    let mut on = std::collections::HashSet::new();
    for &script in SCRIPTS {
        for feature in font.gsub_features_for_script(script, None) {
            if DEFAULT_FEATURES.contains(&feature.tag) {
                on.extend(feature.lookup_indices);
            }
        }
    }
    ligatures.into_iter().filter(|i| on.contains(i)).collect()
}

fn face_shaping(face: &oxideav_scribe::Face) -> Arc<FaceShaping> {
    let id = face.stable_id();
    let mut cache = FACES.lock().unwrap_or_else(|e| e.into_inner());
    let map = cache.get_or_insert_with(HashMap::new);
    if let Some(l) = map.get(&id) {
        return l.clone();
    }
    let info = face
        .with_font(|font| {
            let gsub = !font.gsub_lookup_list().is_empty();
            let header = super::morx::face_header(face.raw_bytes(), face.subfont_index());
            FaceShaping {
                ligatures: default_ligatures(font),
                nested_single_pos: nested_single_pos(font),
                morx: if gsub {
                    None
                } else {
                    super::morx::find_table(face.raw_bytes(), header, b"morx").map(<[u8]>::to_vec)
                },
                color: font.has_color_layers() || font.has_sbix() || font.has_color_bitmaps(),
            }
        })
        .unwrap_or_default();
    let info = Arc::new(info);
    map.insert(id, info.clone());
    info
}

/// `text` shaped through `chain` at `size_px`, with the default features
/// only.
pub(crate) fn shape(chain: &FaceChain, text: &str, size_px: f32) -> Vec<PositionedGlyph> {
    if text.is_empty() || size_px <= 0.0 {
        return Vec::new();
    }
    let sequences = emoji_seq::sequences(text);
    if sequences.is_empty() {
        return shape_text(chain, text, size_px);
    }
    // Each emoji sequence on its own, in the color face that has it; the
    // text between them as usual.
    let mut out = Vec::new();
    let mut at = 0;
    for seq in sequences {
        let piece = &text[seq.clone()];
        let Some(mut glyphs) = shape_sequence(chain, piece, size_px) else {
            continue; // no color face has it: shaped with the text around it
        };
        if at < seq.start {
            out.append(&mut shape_text(chain, &text[at..seq.start], size_px));
        }
        out.append(&mut glyphs);
        at = seq.end;
    }
    if at < text.len() {
        out.append(&mut shape_text(chain, &text[at..], size_px));
    }
    out
}

/// `text` (no emoji sequence in it, or none a color face has) shaped
/// through `chain`.
fn shape_text(chain: &FaceChain, text: &str, size_px: f32) -> Vec<PositionedGlyph> {
    // Each character's face and glyph, one entry per glyph.
    let mapped: Vec<(u16, u16)> = match simple_mapping(chain, text) {
        Some(m) => m,
        // The chain's own mapping, after its Arabic joining, Indic
        // reordering and keeping marks with their base. It positions the
        // glyphs too, which is thrown away: so only for text that needs it.
        None => match chain.shape_with_features(text, size_px, &[]) {
            Ok(glyphs) => glyphs.iter().map(|g| (g.face_idx, g.glyph_id)).collect(),
            Err(_) => return Vec::new(),
        },
    };
    let mut out = Vec::with_capacity(mapped.len());
    let mut start = 0;
    while start < mapped.len() {
        let face_idx = mapped[start].0;
        let mut end = start + 1;
        while end < mapped.len() && mapped[end].0 == face_idx {
            end += 1;
        }
        let gids: Vec<u16> = mapped[start..end].iter().map(|g| g.1).collect();
        let face = chain.face(face_idx);
        let info = face_shaping(face);
        match face.with_font(|font| shape_run(font, &gids, size_px, face_idx, &info)) {
            Ok(mut run) => out.append(&mut run),
            Err(_) => return chain.shape(text, size_px).unwrap_or_default(),
        }
        start = end;
    }
    out
}

/// Whether the chain maps `c` on its own, from the first face that has it:
/// Latin before the combining marks, and the punctuation, currency,
/// letterlike and arrow blocks without the joiners and direction marks.
/// No combining mark, Arabic or Indic letter is among them, which are what
/// the chain's joining, reordering and mark handling change.
fn maps_alone(c: char) -> bool {
    matches!(c,
        '\0'..='\u{02FF}'
        | '\u{2000}'..='\u{200B}'
        | '\u{2010}'..='\u{2027}'
        | '\u{202F}'..='\u{205F}'
        | '\u{20A0}'..='\u{20CF}'
        | '\u{2100}'..='\u{21FF}')
}

/// The chain's mapping of `text` when every character [maps
/// alone](maps_alone): each from the first face that has it (the
/// primary's `.notdef` if none does). That is what the chain's own mapping
/// gives such text, without the positioning pass that comes with it.
fn simple_mapping(chain: &FaceChain, text: &str) -> Option<Vec<(u16, u16)>> {
    if !text.chars().all(maps_alone) {
        return None;
    }
    let mut out = Vec::with_capacity(text.len());
    for ch in text.chars() {
        let mut found = (0, 0);
        for idx in 0..chain.len() as u16 {
            match chain.face(idx).with_font(|font| font.glyph_index(ch)) {
                Ok(Some(gid)) if gid != 0 => {
                    found = (idx, gid);
                    break;
                }
                Ok(_) => {}
                Err(_) => return None,
            }
        }
        out.push(found);
    }
    Some(out)
}

/// One face's run: `ccmp`, the face's default ligatures (lookup by lookup),
/// `calt`, then GPOS positioning, as
/// `oxideav_scribe::shaper::shape_run_with_font` does with every ligature
/// lookup. A face with a `morx` table and no `GSUB` goes through its `morx`
/// instead.
fn shape_run(
    font: &oxideav_ttf::Font<'_>,
    gids: &[u16],
    size_px: f32,
    face_idx: u16,
    info: &FaceShaping,
) -> Vec<PositionedGlyph> {
    let scale = size_px / font.units_per_em().max(1) as f32;
    if let Some(morx) = &info.morx {
        let (shaped, components) = super::morx::apply(morx, font.glyph_count(), gids);
        return position(font, &shaped, &components, scale, face_idx, info);
    }
    let lookups = &info.ligatures;
    let mut shaped = apply_ccmp(font, gids);
    let mut components: Vec<u16> = vec![1; shaped.len()];
    // Each lookup over the whole run in turn, as OpenType applies them: a
    // later lookup sees an earlier one's ligatures (Segoe UI Emoji forms
    // the emoji heart of U+2764 U+FE0F before the couple it is part of).
    for &lookup in lookups.iter() {
        let mut out = Vec::with_capacity(shaped.len());
        let mut counts = Vec::with_capacity(shaped.len());
        let mut i = 0;
        while i < shaped.len() {
            if let Some((glyph, count)) = font.gsub_apply_lookup_type_4(lookup, &shaped[i..])
                && count >= 2
                && i + count <= shaped.len()
            {
                out.push(glyph);
                counts.push(
                    components[i..i + count]
                        .iter()
                        .fold(0u16, |a, &c| a.saturating_add(c)),
                );
                i += count;
            } else {
                out.push(shaped[i]);
                counts.push(components[i]);
                i += 1;
            }
        }
        shaped = out;
        components = counts;
    }
    let before = shaped.len();
    let shaped = apply_calt(font, &shaped);
    if shaped.len() != before {
        components = vec![1; shaped.len()];
    }
    position(font, &shaped, &components, scale, face_idx, info)
}

/// Emoji sequence `seq` shaped in the first color face of `chain` that has
/// its first character, so the face's ligature (from `GSUB` or `morx`) can
/// draw it as one glyph. `None` when no color face has it.
///
/// Selectors and joiners the face doesn't map are left out. The sequence
/// is tried with the face's own selector glyphs and without them (fonts
/// list their sequences either way; HarfBuzz skips them when matching),
/// and the form with fewer glyphs wins. What joiners and selectors are
/// left (a ZWJ sequence the font lacks draws as its parts) are dropped.
fn shape_sequence(chain: &FaceChain, seq: &str, size_px: f32) -> Option<Vec<PositionedGlyph>> {
    let first = seq.chars().next()?;
    let face_idx = (0..chain.len() as u16).find(|&i| {
        let face = chain.face(i);
        face_shaping(face).color
            && face
                .with_font(|f| f.glyph_index(first).is_some_and(|g| g != 0))
                .unwrap_or(false)
    })?;
    let face = chain.face(face_idx);
    let info = face_shaping(face);
    face.with_font(|font| {
        let chars: Vec<char> = seq.chars().collect();
        // Each character with its glyph, and its glyph for the selector
        // after it (`cmap` format 14) where the font has one.
        let mut mapped: Vec<(char, u16, u16)> = Vec::with_capacity(chars.len());
        for (k, &c) in chars.iter().enumerate() {
            let gid = font.glyph_index(c).unwrap_or(0);
            let selected = match chars.get(k + 1).copied() {
                Some(vs @ (emoji_seq::VS15 | emoji_seq::VS16)) => font
                    .lookup_variation(c, vs)
                    .filter(|&g| g != 0)
                    .unwrap_or(gid),
                _ => gid,
            };
            if gid == 0 && emoji_seq::is_invisible(c) {
                continue;
            }
            mapped.push((c, gid, selected));
        }
        let invisible: Vec<u16> = mapped
            .iter()
            .filter(|(c, ..)| emoji_seq::is_invisible(*c))
            .map(|&(_, g, _)| g)
            .collect();
        let selector = |c: char| c == emoji_seq::VS15 || c == emoji_seq::VS16;
        // How many glyphs are left other than joiners and selectors (a
        // sequence drawn as one has one, or one and an empty glyph Apple
        // Color Emoji leaves after some ligatures; a keycap's combining
        // mark has no advance, yet is a glyph), then how wide the run is
        // (Segoe UI Emoji draws a couple as three layers over each other,
        // as many glyphs as its parts but one emoji wide).
        let rank = |run: &[PositionedGlyph]| {
            let left = run
                .iter()
                .filter(|g| !invisible.contains(&g.glyph_id))
                .count();
            let width: f32 = run.iter().map(|g| g.x_advance).sum();
            (left, (width * 64.0).round() as i64)
        };
        // The forms tried: the selectors' own glyphs for their characters
        // or not, and the selectors kept or left out.
        let has_selector = mapped.iter().any(|&(c, ..)| selector(c));
        let has_variant = mapped.iter().any(|&(_, g, v)| g != v);
        let mut best: Option<Vec<PositionedGlyph>> = None;
        for use_variant in [true, false] {
            if !use_variant && !has_variant {
                continue;
            }
            for keep_selectors in [true, false] {
                if !keep_selectors && !has_selector {
                    continue;
                }
                let gids: Vec<u16> = mapped
                    .iter()
                    .filter(|&&(c, ..)| keep_selectors || !selector(c))
                    .map(|&(_, g, v)| if use_variant { v } else { g })
                    .collect();
                let run = shape_run(font, &gids, size_px, face_idx, &info);
                if best.as_ref().is_none_or(|b| rank(&run) < rank(b)) {
                    best = Some(run);
                }
            }
        }
        let mut best = best.unwrap_or_default();
        best.retain(|g| !invisible.contains(&g.glyph_id));
        best
    })
    .ok()
}
