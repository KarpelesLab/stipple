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
pub(crate) mod warm;

#[cfg(test)]
pub(crate) mod cff_tests;
#[cfg(test)]
mod color_tests;

use crate::render::text::{Font, FontError, ShapeCache};
use oxideav_scribe::{Face, FaceChain, PositionedGlyph};
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
#[non_exhaustive]
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
    pub fn weight(mut self, weight: FontWeight) -> Self {
        self.weight = weight;
        self
    }

    /// This style, slanted or upright.
    pub fn italic(mut self, italic: bool) -> Self {
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
#[non_exhaustive]
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
#[non_exhaustive]
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
    /// The underline's top edge, below the baseline.
    pub underline_position: f64,
    /// The underline's thickness.
    pub underline_thickness: f64,
    /// The strikeout's top edge (negative: above the baseline).
    pub strikeout_position: f64,
    /// The strikeout's thickness.
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
///
/// Every face goes through scribe's TrueType face, CFF-outline (`OTTO`) ones
/// included: `oxideav-ttf` reads `CFF ` and `CFF2` charstrings (subroutines,
/// CID-keyed fonts' FDSelect) into the same outlines `glyf` gives, so shaping,
/// fallback and measuring treat both alike. (Scribe's own CFF face kind draws
/// outlines but can't shape or join a face chain.)
pub(crate) fn parse_face(bytes: Vec<u8>, index: u32) -> Result<Face, FontError> {
    let result = match bytes.first_chunk::<4>() {
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
    /// Whether the system's fallbacks (symbols, CJK, emoji) join the chain,
    /// each loaded when text first needs it.
    pub(crate) system_fallbacks: bool,
    variants: Mutex<HashMap<FontStyle, Arc<Variant>>>,
}

impl Family {
    pub(crate) fn new(
        name: Option<String>,
        faces: Vec<Arc<FaceSource>>,
        fallbacks: Vec<Font>,
        system_fallbacks: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            name,
            faces,
            fallbacks,
            system_fallbacks,
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
        let (primary, _) = self
            .pick(style)
            .ok_or_else(|| FontError::new("font family has no faces"))?;
        Ok(Variant {
            id: next_id(),
            chain: Mutex::new(Arc::new(self.build_chain(style, Vec::new())?)),
            metrics: primary.info.metrics,
            monospace: primary.info.monospace,
            cap_height: OnceLock::new(),
            x_height: OnceLock::new(),
            shapes: ShapeCache::default(),
        })
    }

    /// The shaping chain for `style`: its face, the fallbacks' (in order,
    /// each in the requested style, then their own fallbacks), then the
    /// system families `system` names. A face already in the chain isn't
    /// added twice. A chain built with more `system` families starts with
    /// the same faces as one built with fewer, so glyphs shaped with the
    /// shorter one keep their face indices.
    pub(crate) fn build_chain(
        &self,
        style: FontStyle,
        system: Vec<&'static str>,
    ) -> Result<Chain, FontError> {
        let (primary, synth) = self
            .pick(style)
            .ok_or_else(|| FontError::new("font family has no faces"))?;
        let mut chain = FaceChain::new(primary.parse()?);
        let mut synths = vec![synth];
        let mut seen: Vec<*const FaceSource> = vec![Arc::as_ptr(primary)];
        let extra: Vec<Arc<Family>> = system
            .iter()
            .filter_map(|g| system_family(g).ok())
            .collect();
        let mut queue: Vec<&Family> = self.fallbacks.iter().map(|f| &*f.family).collect();
        let mut extra_queued = false;
        let mut i = 0;
        loop {
            if i == queue.len() && !extra_queued {
                queue.extend(extra.iter().map(|f| &**f));
                extra_queued = true;
            }
            if i >= queue.len() || i >= 64 {
                break;
            }
            let family = queue[i];
            i += 1;
            if let Some((source, synth)) = family.pick(style)
                && !seen.contains(&Arc::as_ptr(source))
                && let Ok(face) = source.parse()
            {
                seen.push(Arc::as_ptr(source));
                chain = chain.push_fallback(face);
                synths.push(synth);
            }
            queue.extend(family.fallbacks.iter().map(|f| &*f.family));
        }
        Ok(Chain {
            faces: chain,
            synth: synths,
            system,
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

/// A style's shaping chain: the faces glyphs are drawn from, in order.
pub(crate) struct Chain {
    pub(crate) faces: FaceChain,
    /// What each face (by index) is drawn with.
    pub(crate) synth: Vec<Synthesis>,
    /// The system fallback families (generic names) added to the chain so
    /// far, found or not.
    pub(crate) system: Vec<&'static str>,
}

impl core::ops::Deref for Chain {
    type Target = FaceChain;
    fn deref(&self) -> &FaceChain {
        &self.faces
    }
}

/// One family in one style: the shaping chain (primary face plus fallbacks)
/// and its caches.
pub(crate) struct Variant {
    pub(crate) id: u64,
    /// Replaced by a longer one when a system fallback is loaded (see
    /// `Font::load_system_fallbacks`); never shortened.
    chain: Mutex<Arc<Chain>>,
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

impl Variant {
    /// The current shaping chain.
    pub(crate) fn chain(&self) -> Arc<Chain> {
        self.chain.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Replace the chain by `longer`, unless another thread already
    /// extended it past `from`.
    pub(crate) fn extend_chain(&self, from: &Arc<Chain>, longer: Chain) {
        let mut chain = self.chain.lock().unwrap_or_else(|e| e.into_inner());
        if Arc::ptr_eq(&chain, from) {
            *chain = Arc::new(longer);
            self.shapes.clear();
        }
    }
}

/// The installed family `name` resolves to (see [`Font::from_family`]),
/// without building any style of it. One family per installed family, shared
/// by every call: a font asked for again (by a view rebuilt each frame) reuses
/// the loaded faces and compares equal, so an unchanged view still skips its
/// repaint.
pub(crate) fn system_family(name: &str) -> Result<Arc<Family>, FontError> {
    static LOADED: OnceLock<Mutex<HashMap<String, Arc<Family>>>> = OnceLock::new();
    let (family, faces) = system::index()
        .resolve(name)
        .ok_or_else(|| FontError::new(format!("font family {name:?} not found")))?;
    let key = system::normalize(&family);
    let loaded = LOADED.get_or_init(Default::default);
    let mut map = loaded.lock().unwrap_or_else(|e| e.into_inner());
    let shared = map.entry(key).or_insert_with(|| {
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
        Family::new(Some(family), sources, Vec::new(), false)
    });
    Ok(shared.clone())
}

/// Which system fallback (a generic family) `c` would come from when the
/// font lacks it, past those `tried`: `None` for characters a fallback
/// isn't loaded for (ASCII, spaces, controls, joiners and selectors).
pub(crate) fn system_fallback_for(c: char, tried: &[&str]) -> Option<&'static str> {
    let u = c as u32;
    if u < 0x80 || c.is_whitespace() || c.is_control() {
        return None;
    }
    let candidates: &[&'static str] = match u {
        // Zero-width joiners and marks, bidi controls, variation selectors.
        0x200B..=0x200F | 0x2028..=0x202E | 0x2060..=0x206F | 0xFE00..=0xFE0F => return None,
        0xE0000..=0xE007F | 0xE0100..=0xE01EF => return None,
        // Hangul jamo and syllables: a CJK family covers them where it's
        // pan-CJK (Noto Sans CJK), a Korean one otherwise.
        0x1100..=0x11FF | 0x3130..=0x318F | 0xA960..=0xA97F | 0xAC00..=0xD7FF => &["cjk", "korean"],
        // Han, kana, bopomofo, CJK symbols and punctuation, full-width forms.
        0x2E80..=0x2FFF
        | 0x3000..=0x9FFF
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFFEF
        | 0x20000..=0x3FFFF => &["cjk"],
        // Pictographs and emoji.
        0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0x2B00..=0x2BFF => &["emoji", "symbol"],
        _ => &["symbol"],
    };
    candidates.iter().copied().find(|g| !tried.contains(g))
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
        if faces.is_empty() {
            return Err(FontError::new("font family has no faces"));
        }
        let family = Family::new(name, faces, Vec::new(), false);
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

    /// Load the font file at `path`: TrueType or OpenType (`glyf`, `CFF ` or
    /// `CFF2` outlines), or a collection of them (`.ttc`, `.otc`), whose
    /// faces all join the family so [`styled`](Self::styled) can pick among
    /// them, the first drawing by default.
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
        Font::in_family(system_family(name)?, style)
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
            self.family.system_fallbacks,
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
        self.refamily(fallbacks, self.family.system_fallbacks)
    }

    /// This font's faces with `fallbacks` added to its own, and the system's
    /// fallbacks on or off.
    pub(crate) fn refamily(&self, fallbacks: &[Font], system_fallbacks: bool) -> Font {
        let mut all = self.family.fallbacks.clone();
        all.extend(fallbacks.iter().cloned());
        let family = Family::new(
            self.family.name.clone(),
            self.family.faces.clone(),
            all,
            system_fallbacks,
        );
        Font::in_family(family, self.style).unwrap_or_else(|_| self.clone())
    }

    /// This font with the system's fallbacks for symbols, CJK (Japanese,
    /// Chinese, Korean) and emoji consulted, after its own, for characters
    /// it has no glyph for: whichever of [`from_family`](Self::from_family)'s
    /// generic families this system has. Each is loaded the first time text
    /// needs it, so a font that never draws Japanese never loads a CJK font
    /// (they are large: 5 to 20 MB each).
    ///
    /// [`Font::system_default`] has them already.
    pub fn with_system_fallbacks(&self) -> Font {
        if self.family.system_fallbacks {
            return self.clone();
        }
        self.refamily(&[], true)
    }

    /// Glyphs shaped through the variant's cache. With the system's
    /// fallbacks on, a glyph missing from every face first loads the system
    /// family that has it (see [`system_fallback_for`]) and shapes again.
    pub(crate) fn shape_cached(&self, text: &str, size_px: f64) -> Vec<PositionedGlyph> {
        let mut glyphs = self.variant.shapes.shape(&self.chain(), text, size_px);
        // A few rounds: Hangul tries the CJK family, then a Korean one.
        for _ in 0..3 {
            if !self.family.system_fallbacks
                || !(glyphs.iter().any(|g| g.glyph_id == 0) || self.wants_color_emoji(text))
                || !self.load_system_fallbacks(text)
            {
                break;
            }
            glyphs = self.variant.shapes.shape(&self.chain(), text, size_px);
        }
        glyphs
    }

    /// Whether `text` has an emoji sequence (a keycap, a flag, an emoji
    /// with its selector …) and the system's emoji family hasn't been
    /// tried: its characters may all be in the text font (digits, a
    /// monochrome heart), but the sequence asks for the color emoji.
    fn wants_color_emoji(&self, text: &str) -> bool {
        !self.chain().system.contains(&"emoji")
            && !crate::render::emoji_seq::sequences(text).is_empty()
    }

    /// Add to the chain the system fallbacks `text`'s missing characters
    /// need and that haven't been tried, and the emoji family for its
    /// emoji sequences. Whether the chain grew.
    fn load_system_fallbacks(&self, text: &str) -> bool {
        let chain = self.chain();
        let mut want: Vec<&'static str> = Vec::new();
        let mut checked: Vec<char> = Vec::new();
        for c in text.chars() {
            let Some(generic) = system_fallback_for(c, &chain.system) else {
                continue;
            };
            if want.contains(&generic) || checked.contains(&c) {
                continue;
            }
            checked.push(c);
            let covered = (0..chain.len()).any(|i| {
                chain
                    .face(i as u16)
                    .with_font(|f| f.glyph_index(c))
                    .ok()
                    .flatten()
                    .is_some_and(|g| g != 0)
            });
            if !covered {
                want.push(generic);
            }
        }
        if !want.contains(&"emoji") && self.wants_color_emoji(text) {
            want.push("emoji");
        }
        // While this thread takes text input, a fallback that isn't ready
        // loads in the background (see `warm`): the text goes without it
        // until the App draws again.
        if want.is_empty() || warm::must_wait(&want) {
            return false;
        }
        let mut system = chain.system.clone();
        system.extend(want);
        match self.family.build_chain(self.style, system) {
            Ok(longer) => {
                self.variant.extend_chain(&chain, longer);
                true
            }
            Err(_) => false,
        }
    }
    /// Whether `c` draws as a real glyph, from this font or a fallback (the
    /// system's, loaded if `c` needs one), rather than as the missing-glyph
    /// box. On the App's thread once a text field has had the input method,
    /// a system fallback loads in the background, and this is `false` until
    /// it's ready.
    pub fn has_glyph(&self, c: char) -> bool {
        let mut buf = [0u8; 4];
        let glyphs = self.shape_cached(c.encode_utf8(&mut buf), 16.0);
        !glyphs.is_empty() && glyphs.iter().all(|g| g.glyph_id != 0)
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
        self.styled(self.style.weight(weight))
    }

    /// This font bold.
    pub fn bold(&self) -> Font {
        self.with_weight(FontWeight::BOLD)
    }

    /// This font italic.
    pub fn italic(&self) -> Font {
        self.styled(self.style.italic(true))
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
        let s = self.chain().synth[0];
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
        let upem = self.chain().primary().units_per_em().max(1) as f64;
        let scale = size_px / upem;
        let (ascent, descent, line_gap) = if m.ascender > 0 {
            (
                m.ascender as f64 * scale,
                -(m.descender as f64) * scale,
                m.line_gap.max(0) as f64 * scale,
            )
        } else {
            let ascent = self.ascent(size_px);
            let descent = -(self.chain().primary().descent_px(size_px as f32) as f64);
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
