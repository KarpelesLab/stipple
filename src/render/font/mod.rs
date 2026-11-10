//! Fonts as families: the faces a [`Font`] can draw with (its regular, bold,
//! italic … cuts), the fallback fonts consulted for characters it lacks, the
//! style it draws in, and system font discovery by family name.
//!
//! A [`Font`] is a cheap handle (two reference counts): clone it into every
//! element or glyph run that draws with it. [`Font::styled`] picks another
//! weight or slant of the same family, loading that cut on first use, or
//! synthesizing it (a slant for italic, a thicker outline for bold) when the
//! family has no such face.

pub(crate) mod fontconfig;
pub(crate) mod sfnt;
pub(crate) mod system;

use crate::render::text::{Font, FontError, ShapeCache};
use oxideav_scribe::{Face, FaceChain};
use sfnt::{FaceInfo, RawMetrics};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// A font weight on the CSS / OpenType scale: 100 (thin) to 900 (black), 400
/// being regular and 700 bold. Values in between are allowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FontWeight(pub u16);

impl FontWeight {
    pub const THIN: Self = Self(100);
    pub const EXTRA_LIGHT: Self = Self(200);
    pub const LIGHT: Self = Self(300);
    pub const NORMAL: Self = Self(400);
    pub const MEDIUM: Self = Self(500);
    pub const SEMI_BOLD: Self = Self(600);
    pub const BOLD: Self = Self(700);
    pub const EXTRA_BOLD: Self = Self(800);
    pub const BLACK: Self = Self(900);
}

impl Default for FontWeight {
    fn default() -> Self {
        Self::NORMAL
    }
}

/// The style a [`Font`] draws in: a weight and whether it slants.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FontStyle {
    pub weight: FontWeight,
    /// Italic (or, when the family has no italic face, a synthesized oblique).
    pub italic: bool,
}

impl FontStyle {
    pub const NORMAL: Self = Self {
        weight: FontWeight::NORMAL,
        italic: false,
    };
    pub const BOLD: Self = Self {
        weight: FontWeight::BOLD,
        italic: false,
    };
    pub const ITALIC: Self = Self {
        weight: FontWeight::NORMAL,
        italic: true,
    };
    pub const BOLD_ITALIC: Self = Self {
        weight: FontWeight::BOLD,
        italic: true,
    };

    /// This style at `weight`.
    pub fn with_weight(mut self, weight: FontWeight) -> Self {
        self.weight = weight;
        self
    }

    /// This style, slanted or upright.
    pub fn with_italic(mut self, italic: bool) -> Self {
        self.italic = italic;
        self
    }
}

/// Identifies one drawable font (a family in one style, with its fallbacks):
/// equal ids draw identically. Handy as a cache key, e.g. for a glyph atlas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FontId(pub(crate) u64);

/// A family installed on the system, as listed by [`Font::families`].
#[derive(Clone, Debug, PartialEq)]
pub struct FontFamilyInfo {
    /// The family name, as [`Font::from_family`] takes it.
    pub name: String,
    /// Whether its faces are monospaced (every glyph the same advance).
    pub monospace: bool,
    /// The styles it has real faces for.
    pub styles: Vec<FontStyle>,
}

/// A font's vertical and decoration metrics at one size, in logical pixels.
///
/// Vertical positions are offsets from the baseline, growing **downward**
/// (as screen coordinates do): the underline's is positive, the strikeout's
/// negative. Each `*_position` is the top edge of its stroke, so the stroke
/// covers `baseline + position .. baseline + position + thickness`.
///
/// The line metrics (`ascent`, `descent`, `line_gap`) are the ones the font
/// recommends for spacing lines — its `hhea` values, or its typographic ones
/// when it asks for those — as FreeType and Core Text report them, so a
/// terminal's cells come out as tall as in other terminals. Stipple's own
/// text layout ([`Font::line_height`], [`Font::baseline`]) uses the
/// typographic box, which for some fonts is tighter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontMetrics {
    /// The size these metrics are for, in pixels per em.
    pub size: f64,
    /// Height of the font's ascenders above the baseline.
    pub ascent: f64,
    /// Depth of its descenders below the baseline (positive).
    pub descent: f64,
    /// Extra space the font asks for between lines.
    pub line_gap: f64,
    /// `ascent + descent + line_gap`: the distance between baselines.
    pub line_height: f64,
    /// Height of capital letters.
    pub cap_height: f64,
    /// Height of lowercase letters such as "x".
    pub x_height: f64,
    /// The widest advance of any glyph in the font.
    pub max_advance: f64,
    pub underline_position: f64,
    pub underline_thickness: f64,
    pub strikeout_position: f64,
    pub strikeout_thickness: f64,
    /// Whether the font is monospaced: a terminal's cell is then
    /// [`Font::advance_of`] any character wide.
    pub monospace: bool,
}

/// What a face is drawn with that the font doesn't have.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Synthesis {
    /// Thicken the outlines (the family has no face this bold).
    pub bold: bool,
    /// Slant the outlines (the family has no italic face).
    pub oblique: bool,
}

/// One face a family can draw with: where its bytes are and what it is.
#[derive(Debug)]
pub(crate) struct FaceSource {
    origin: Origin,
    /// Subfont index within a collection (0 otherwise).
    index: u32,
    pub(crate) info: FaceInfo,
}

#[derive(Debug)]
enum Origin {
    Memory(Arc<[u8]>),
    /// Read when a variant needs it, not held in memory in between.
    File(PathBuf),
}

impl FaceSource {
    /// The faces in `bytes` (one, or each of a collection's).
    pub(crate) fn from_bytes(bytes: Vec<u8>) -> Result<Vec<Arc<FaceSource>>, FontError> {
        let faces = sfnt::read_faces(&mut &bytes[..]);
        if faces.is_empty() {
            return Err(FontError::new("not a TrueType/OpenType font"));
        }
        let shared: Arc<[u8]> = bytes.into();
        Ok(faces
            .into_iter()
            .map(|(index, info)| {
                Arc::new(FaceSource {
                    origin: Origin::Memory(shared.clone()),
                    index,
                    info,
                })
            })
            .collect())
    }

    pub(crate) fn from_file(path: PathBuf, index: u32, info: FaceInfo) -> Self {
        FaceSource {
            origin: Origin::File(path),
            index,
            info,
        }
    }

    /// Parse this face for shaping (the bytes are copied: a scribe face owns
    /// its data).
    fn parse(&self) -> Result<Face, FontError> {
        let bytes = match &self.origin {
            Origin::Memory(b) => b.to_vec(),
            Origin::File(p) => {
                std::fs::read(p).map_err(|e| FontError::new(format!("{}: {e}", p.display())))?
            }
        };
        parse_face(bytes, self.index)
    }
}

/// Parse face `index` of `bytes` with scribe.
pub(crate) fn parse_face(bytes: Vec<u8>, index: u32) -> Result<Face, FontError> {
    let result = match bytes.first_chunk::<4>() {
        Some(b"OTTO") => Face::from_otf_bytes(bytes),
        Some(b"ttcf") => Face::from_ttc_bytes(bytes, index),
        _ => Face::from_ttf_bytes(bytes),
    };
    result.map_err(|e| FontError::new(format!("{e:?}")))
}

/// The faces of one family and the fonts that stand in for characters it
/// lacks, with each style's built face chain cached.
pub(crate) struct Family {
    pub(crate) name: Option<String>,
    pub(crate) faces: Vec<Arc<FaceSource>>,
    pub(crate) fallbacks: Vec<Font>,
    variants: Mutex<HashMap<FontStyle, Arc<Variant>>>,
}

impl Family {
    pub(crate) fn new(
        name: Option<String>,
        faces: Vec<Arc<FaceSource>>,
        fallbacks: Vec<Font>,
    ) -> Arc<Self> {
        Arc::new(Self {
            name,
            faces,
            fallbacks,
            variants: Mutex::new(HashMap::new()),
        })
    }

    /// The face chain for `style`, built on first use.
    pub(crate) fn variant(&self, style: FontStyle) -> Result<Arc<Variant>, FontError> {
        if let Ok(map) = self.variants.lock()
            && let Some(v) = map.get(&style)
        {
            return Ok(v.clone());
        }
        let built = Arc::new(self.build(style)?);
        if let Ok(mut map) = self.variants.lock() {
            return Ok(map.entry(style).or_insert(built).clone());
        }
        Ok(built)
    }

    /// The family's best face for `style`, and what must be synthesized.
    fn pick(&self, style: FontStyle) -> Option<(&Arc<FaceSource>, Synthesis)> {
        // Only the faces nearest normal width: a family that also ships
        // condensed or expanded cuts (DejaVu Sans Condensed shares DejaVu
        // Sans's typographic family) shouldn't draw bold text condensed.
        let off = |f: &FaceSource| (f.info.width as i32 - 5).abs();
        let nearest = self.faces.iter().map(|f| off(f)).min()?;
        let faces: Vec<&Arc<FaceSource>> =
            self.faces.iter().filter(|f| off(f) == nearest).collect();
        let cands: Vec<(u16, bool)> = faces
            .iter()
            .map(|f| (f.info.weight, f.info.italic))
            .collect();
        let (i, synth) = best_match(&cands, style)?;
        Some((faces[i], synth))
    }

    fn build(&self, style: FontStyle) -> Result<Variant, FontError> {
        let (primary, synth) = self
            .pick(style)
            .ok_or_else(|| FontError::new("font family has no faces"))?;
        let mut chain = FaceChain::new(primary.parse()?);
        let mut synths = vec![synth];
        // Fallbacks in order, each in the requested style, then their own
        // fallbacks; a face already in the chain isn't added twice.
        let mut seen: Vec<*const FaceSource> = vec![Arc::as_ptr(primary)];
        let mut queue: Vec<&Font> = self.fallbacks.iter().collect();
        let mut i = 0;
        while i < queue.len() && i < 64 {
            let family = &queue[i].family;
            i += 1;
            if let Some((source, synth)) = family.pick(style)
                && !seen.contains(&Arc::as_ptr(source))
                && !source.info.cff
                && let Ok(face) = source.parse()
            {
                seen.push(Arc::as_ptr(source));
                chain = chain.push_fallback(face);
                synths.push(synth);
            }
            queue.extend(family.fallbacks.iter());
        }
        Ok(Variant {
            id: next_id(),
            chain,
            synth: synths,
            metrics: primary.info.metrics,
            monospace: primary.info.monospace,
            cap_height: OnceLock::new(),
            x_height: OnceLock::new(),
            shapes: ShapeCache::default(),
        })
    }

    /// The real styles this family has faces for.
    pub(crate) fn styles(&self) -> Vec<FontStyle> {
        let mut out: Vec<FontStyle> = self
            .faces
            .iter()
            .map(|f| FontStyle {
                weight: FontWeight(f.info.weight),
                italic: f.info.italic,
            })
            .collect();
        out.sort_by_key(|s| (s.italic, s.weight));
        out.dedup();
        out
    }
}

/// One family in one style: the shaping chain (primary face plus fallbacks)
/// and its caches.
pub(crate) struct Variant {
    pub(crate) id: u64,
    pub(crate) chain: FaceChain,
    /// What each chain face (by index) is drawn with.
    pub(crate) synth: Vec<Synthesis>,
    /// The primary face's metrics, in font units.
    pub(crate) metrics: RawMetrics,
    pub(crate) monospace: bool,
    /// Capital-letter and "x" height per em, measured once on first use.
    pub(crate) cap_height: OnceLock<f64>,
    pub(crate) x_height: OnceLock<f64>,
    /// Shaped runs by (text, size): views rebuild every frame and measure the
    /// same strings several times per layout, and shaping is the costly part.
    pub(crate) shapes: ShapeCache,
}

fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Pick the face for `want` from `(weight, italic)` candidates, the way CSS
/// font matching does: the requested slant first (else the other one,
/// slanted by synthesis), then the nearest weight — for a weight between 400
/// and 500, heavier faces up to 500 first, then lighter, then heavier; below
/// 400, lighter first; above 500, heavier first. A bold request met by a face
/// of 500 or lighter is emboldened.
pub(crate) fn best_match(cands: &[(u16, bool)], want: FontStyle) -> Option<(usize, Synthesis)> {
    if cands.is_empty() {
        return None;
    }
    let slant_ok = cands.iter().any(|c| c.1 == want.italic);
    let pool: Vec<usize> = (0..cands.len())
        .filter(|&i| !slant_ok || cands[i].1 == want.italic)
        .collect();
    let w = want.weight.0;
    // Lower rank is better.
    let rank = |weight: u16| -> (u8, u16) {
        let (lighter, heavier) = (w.saturating_sub(weight), weight.saturating_sub(w));
        if weight == w {
            (0, 0)
        } else if (400..=500).contains(&w) {
            if weight > w && weight <= 500 {
                (1, heavier)
            } else if weight < w {
                (2, lighter)
            } else {
                (3, heavier)
            }
        } else if w < 400 {
            if weight < w {
                (1, lighter)
            } else {
                (2, heavier)
            }
        } else if weight > w {
            (1, heavier)
        } else {
            (2, lighter)
        }
    };
    let best = pool.into_iter().min_by_key(|&i| rank(cands[i].0))?;
    let synth = Synthesis {
        bold: w >= 600 && cands[best].0 <= 500,
        oblique: want.italic && !cands[best].1,
    };
    Some((best, synth))
}

impl Font {
    /// A font from one family's faces.
    pub(crate) fn from_sources(
        name: Option<String>,
        faces: Vec<Arc<FaceSource>>,
        style: FontStyle,
    ) -> Result<Font, FontError> {
        // CFF-outline faces can't be shaped by the face chain yet.
        let faces: Vec<_> = faces.into_iter().filter(|f| !f.info.cff).collect();
        if faces.is_empty() {
            return Err(FontError::new("no usable TrueType-outline face"));
        }
        let family = Family::new(name, faces, Vec::new());
        Font::in_family(family, style)
    }

    fn in_family(family: Arc<Family>, style: FontStyle) -> Result<Font, FontError> {
        let variant = family.variant(style)?;
        Ok(Font {
            family,
            style,
            variant,
        })
    }

    /// Load the first face of the font file at `path` (TrueType, or a
    /// TrueType collection; every face of a collection joins the family, so
    /// [`styled`](Self::styled) can pick among them).
    pub fn from_file(path: impl AsRef<Path>) -> Result<Font, FontError> {
        let path = path.as_ref();
        let bytes =
            std::fs::read(path).map_err(|e| FontError::new(format!("{}: {e}", path.display())))?;
        Font::from_bytes(bytes)
    }

    /// The installed font family `name` in `style`: a family name such as
    /// `"JetBrains Mono"` (matched ignoring case and spaces), or a generic
    /// family — `"sans-serif"`, `"serif"`, `"monospace"`, `"system-ui"`,
    /// `"emoji"` or `"cjk"` — resolved to what this system has.
    ///
    /// The family's faces are all available to [`styled`](Self::styled): a
    /// bold request uses its bold face, and is synthesized only when it has
    /// none. Families come from each OS's font directories (on Linux and
    /// BSDs, those fontconfig's configuration lists, with its aliases); the
    /// first call scans them, which can take a moment on a system with
    /// thousands of fonts. On the web there are no system fonts: load what the
    /// app ships with [`Font::from_bytes`].
    pub fn from_family(name: &str, style: FontStyle) -> Result<Font, FontError> {
        // One family per installed family, shared by every call: a font asked
        // for again (by a view rebuilt each frame) reuses the loaded faces and
        // compares equal, so an unchanged view still skips its repaint.
        static LOADED: OnceLock<Mutex<HashMap<String, Arc<Family>>>> = OnceLock::new();
        let index = system::index();
        let (family, faces) = index
            .resolve(name)
            .ok_or_else(|| FontError::new(format!("font family {name:?} not found")))?;
        let key = system::normalize(&family);
        let loaded = LOADED.get_or_init(Default::default);
        if let Some(shared) = loaded.lock().ok().and_then(|m| m.get(&key).cloned()) {
            return Font::in_family(shared, style);
        }
        let sources = faces
            .iter()
            .map(|f| {
                Arc::new(FaceSource::from_file(
                    f.path.clone(),
                    f.index,
                    f.info.clone(),
                ))
            })
            .collect();
        let font = Font::from_sources(Some(family), sources, style)?;
        if let Ok(mut m) = loaded.lock() {
            let shared = m.entry(key).or_insert_with(|| font.family.clone()).clone();
            return Font::in_family(shared, style);
        }
        Ok(font)
    }

    /// The font families installed on this system, sorted by name: what a
    /// font picker lists. Empty on the web.
    pub fn families() -> Vec<FontFamilyInfo> {
        system::index().families()
    }

    /// Add another face (a bold or italic cut, say) to this font's family,
    /// from `sfnt` bytes: [`styled`](Self::styled) then uses it instead of
    /// synthesizing that style. This is how an app that bundles its fonts
    /// builds a family.
    pub fn with_face(&self, bytes: Vec<u8>) -> Result<Font, FontError> {
        let mut faces = self.family.faces.clone();
        faces.extend(FaceSource::from_bytes(bytes)?);
        let family = Family::new(
            self.family.name.clone(),
            faces,
            self.family.fallbacks.clone(),
        );
        Font::in_family(family, self.style)
    }

    /// This font with `fallback` consulted, after its existing fallbacks, for
    /// characters it has no glyph for (CJK, emoji, symbols). The fallback is
    /// drawn in whatever style this font is drawn in, from its own faces for
    /// that style.
    ///
    /// This makes a new font, loading its faces again: build it once (at
    /// startup, or in the app's state), not in a view function.
    pub fn with_fallback(&self, fallback: &Font) -> Font {
        self.with_fallbacks(std::slice::from_ref(fallback))
    }

    /// [`with_fallback`](Self::with_fallback) for several fonts, in order.
    pub fn with_fallbacks(&self, fallbacks: &[Font]) -> Font {
        let mut all = self.family.fallbacks.clone();
        all.extend(fallbacks.iter().cloned());
        let family = Family::new(self.family.name.clone(), self.family.faces.clone(), all);
        Font::in_family(family, self.style).unwrap_or_else(|_| self.clone())
    }

    /// This font with the system's fallbacks for symbols, CJK and emoji
    /// appended (whichever this system has; see
    /// [`from_family`](Self::from_family)'s generic names). CJK and emoji
    /// fonts are large, so this costs memory for each style drawn.
    pub fn with_system_fallbacks(&self) -> Font {
        let found: Vec<Font> = ["symbol", "cjk", "emoji"]
            .iter()
            .filter_map(|g| Font::from_family(g, self.style).ok())
            .collect();
        self.with_fallbacks(&found)
    }

    /// The fonts consulted, in order, for characters this one lacks.
    pub fn fallbacks(&self) -> &[Font] {
        &self.family.fallbacks
    }

    /// This font's family in `style`: its face for that weight and slant,
    /// loaded on first use (later calls share it), or synthesized when the
    /// family has none.
    pub fn styled(&self, style: FontStyle) -> Font {
        if style == self.style {
            return self.clone();
        }
        match self.family.variant(style) {
            Ok(variant) => Font {
                family: self.family.clone(),
                style,
                variant,
            },
            Err(_) => self.clone(),
        }
    }

    /// This font at `weight`, keeping its slant.
    pub fn with_weight(&self, weight: FontWeight) -> Font {
        self.styled(self.style.with_weight(weight))
    }

    /// This font bold.
    pub fn bold(&self) -> Font {
        self.with_weight(FontWeight::BOLD)
    }

    /// This font italic.
    pub fn italic(&self) -> Font {
        self.styled(self.style.with_italic(true))
    }

    /// The style this font draws in.
    pub fn style(&self) -> FontStyle {
        self.style
    }

    /// The family name: the one asked for by [`from_family`](Self::from_family),
    /// else the primary face's own.
    pub fn family_name(&self) -> Option<&str> {
        self.family.name.as_deref().or_else(|| {
            self.family
                .faces
                .first()
                .map(|f| f.info.family.as_str())
                .filter(|s| !s.is_empty())
        })
    }

    /// The real styles this font's family has faces for.
    pub fn available_styles(&self) -> Vec<FontStyle> {
        self.family.styles()
    }

    /// Whether drawing this font in its style synthesizes bold or a slant
    /// (its family has no such face): `(bold, oblique)`.
    pub fn is_synthesized(&self) -> (bool, bool) {
        let s = self.variant.synth[0];
        (s.bold, s.oblique)
    }

    /// Whether the primary face is monospaced.
    pub fn is_monospace(&self) -> bool {
        self.variant.monospace
    }

    /// Identifies this font in this style; see [`FontId`].
    pub fn id(&self) -> FontId {
        FontId(self.variant.id)
    }

    /// The advance width of `c` at `size_px`: for a monospaced font, a
    /// terminal cell's width. Uses a fallback's glyph when this font has
    /// none for `c`.
    pub fn advance_of(&self, c: char, size_px: f64) -> f64 {
        let mut buf = [0u8; 4];
        self.measure(c.encode_utf8(&mut buf), size_px).width
    }

    /// The font's metrics at `size_px`; see [`FontMetrics`].
    pub fn metrics(&self, size_px: f64) -> FontMetrics {
        let m = &self.variant.metrics;
        let upem = self.variant.chain.primary().units_per_em().max(1) as f64;
        let scale = size_px / upem;
        let (ascent, descent, line_gap) = if m.ascender > 0 {
            (
                m.ascender as f64 * scale,
                -(m.descender as f64) * scale,
                m.line_gap.max(0) as f64 * scale,
            )
        } else {
            let ascent = self.ascent(size_px);
            let descent = -(self.variant.chain.primary().descent_px(size_px as f32) as f64);
            (
                ascent,
                descent,
                (self.line_height(size_px) - ascent - descent).max(0.0),
            )
        };
        let line_height = ascent + descent + line_gap;
        let cap_height = if m.cap_height > 0 {
            m.cap_height as f64 * scale
        } else {
            self.cap_height_em() * size_px
        };
        let x_height = if m.x_height > 0 {
            m.x_height as f64 * scale
        } else {
            self.x_height_em() * size_px
        };
        // Fonts that leave these out get the usual typographic defaults.
        let underline_thickness = if m.underline_thickness > 0 {
            m.underline_thickness as f64 * scale
        } else {
            size_px / 14.0
        };
        // `post.underlinePosition` is taken as the stroke's center (as
        // FreeType does).
        let underline_center = if m.underline_position != 0 {
            -(m.underline_position as f64) * scale
        } else {
            size_px / 10.0
        };
        let strikeout_thickness = if m.strikeout_size > 0 {
            m.strikeout_size as f64 * scale
        } else {
            underline_thickness
        };
        let strikeout_position = if m.strikeout_position > 0 {
            -(m.strikeout_position as f64) * scale
        } else {
            -(x_height / 2.0) - strikeout_thickness / 2.0
        };
        FontMetrics {
            size: size_px,
            ascent,
            descent,
            line_gap,
            line_height,
            cap_height,
            x_height,
            max_advance: if m.advance_width_max > 0 {
                m.advance_width_max as f64 * scale
            } else {
                self.advance_of('M', size_px)
            },
            underline_position: underline_center - underline_thickness / 2.0,
            underline_thickness,
            strikeout_position,
            strikeout_thickness,
            monospace: self.variant.monospace,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style(weight: u16, italic: bool) -> FontStyle {
        FontStyle {
            weight: FontWeight(weight),
            italic,
        }
    }

    #[test]
    fn exact_matches_win() {
        let faces = [(400, false), (700, false), (400, true), (700, true)];
        for (i, &(w, it)) in faces.iter().enumerate() {
            assert_eq!(
                best_match(&faces, style(w, it)),
                Some((i, Synthesis::default()))
            );
        }
    }

    #[test]
    fn missing_styles_are_synthesized() {
        let regular = [(400, false)];
        let (i, s) = best_match(&regular, FontStyle::BOLD_ITALIC).unwrap();
        assert_eq!(i, 0);
        assert!(s.bold && s.oblique);
        // A semibold face satisfies bold without synthesis.
        let (i, s) = best_match(&[(400, false), (600, false)], FontStyle::BOLD).unwrap();
        assert_eq!((i, s), (1, Synthesis::default()));
        // An italic request takes the italic face even at the wrong weight,
        // emboldening it.
        let (i, s) = best_match(&[(700, false), (400, true)], FontStyle::BOLD_ITALIC).unwrap();
        assert_eq!(i, 1);
        assert!(s.bold && !s.oblique);
    }

    #[test]
    fn weights_follow_css_matching() {
        let faces = [(300, false), (500, false), (600, false), (900, false)];
        let pick = |w| best_match(&faces, style(w, false)).unwrap().0;
        // 400: heavier up to 500 first.
        assert_eq!(pick(400), 1);
        // 450 with no face in 450..=500 but 500: still 500.
        assert_eq!(pick(450), 1);
        // Below 400: lighter first.
        assert_eq!(pick(350), 0);
        assert_eq!(pick(200), 0, "nothing lighter: the nearest heavier");
        // Above 500: heavier first.
        assert_eq!(pick(700), 3);
        assert_eq!(pick(950), 3, "nothing heavier: the nearest lighter");
        assert!(best_match(&[], FontStyle::NORMAL).is_none());
    }
}
