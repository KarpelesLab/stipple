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

/// Per face (by its stable id): the ligature lookups to apply, in lookup
/// order.
static LIGATURES: Mutex<Option<HashMap<u64, Arc<Vec<u16>>>>> = Mutex::new(None);

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

fn ligatures_of(face: &oxideav_scribe::Face) -> Arc<Vec<u16>> {
    let id = face.stable_id();
    let mut cache = LIGATURES.lock().unwrap_or_else(|e| e.into_inner());
    let map = cache.get_or_insert_with(HashMap::new);
    if let Some(l) = map.get(&id) {
        return l.clone();
    }
    let l = Arc::new(face.with_font(default_ligatures).unwrap_or_default());
    map.insert(id, l.clone());
    l
}

/// `text` shaped through `chain` at `size_px`, with the default features
/// only.
pub(crate) fn shape(chain: &FaceChain, text: &str, size_px: f32) -> Vec<PositionedGlyph> {
    if text.is_empty() || size_px <= 0.0 {
        return Vec::new();
    }
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
        let lookups = ligatures_of(face);
        match face.with_font(|font| shape_run(font, &gids, size_px, face_idx, &lookups)) {
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

/// One face's run: `ccmp`, the ligatures of `lookups` (at each glyph the
/// first lookup that matches wins), `calt`, then GPOS positioning, as
/// `oxideav_scribe::shaper::shape_run_with_font` does with every ligature
/// lookup.
fn shape_run(
    font: &oxideav_ttf::Font<'_>,
    gids: &[u16],
    size_px: f32,
    face_idx: u16,
    lookups: &[u16],
) -> Vec<PositionedGlyph> {
    let scale = size_px / font.units_per_em().max(1) as f32;
    let raw = apply_ccmp(font, gids);
    let mut shaped = Vec::with_capacity(raw.len());
    let mut components = Vec::with_capacity(raw.len());
    let mut i = 0;
    'glyphs: while i < raw.len() {
        for &lookup in lookups {
            if let Some((glyph, count)) = font.gsub_apply_lookup_type_4(lookup, &raw[i..])
                && count >= 2
            {
                shaped.push(glyph);
                components.push(count.min(u16::MAX as usize) as u16);
                i += count;
                continue 'glyphs;
            }
        }
        shaped.push(raw[i]);
        components.push(1);
        i += 1;
    }
    let before = shaped.len();
    let shaped = apply_calt(font, &shaped);
    if shaped.len() != before {
        components = vec![1; shaped.len()];
    }
    position_run_with_font(font, &shaped, &components, scale, face_idx)
}
