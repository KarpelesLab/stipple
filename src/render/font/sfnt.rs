//! A minimal `sfnt` (TrueType / OpenType) header reader: just the tables that
//! font matching and decoration metrics need (`name`, `OS/2`, `post`, `head`,
//! `hhea`), read either from bytes in memory or straight from a file without
//! loading the whole font.
//!
//! Shaping and outlines stay with `oxideav-scribe`; this only answers "which
//! family, weight and slant is this face, is it monospaced, and where do its
//! underline and strikeout go".

use std::io::{Read, Seek, SeekFrom};

/// What [`read_faces`] learns about one face.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FaceInfo {
    /// Typographic family (name ID 16), else the legacy family (ID 1).
    pub family: String,
    /// Typographic subfamily (ID 17), else the legacy subfamily (ID 2).
    pub subfamily: String,
    /// `OS/2.usWeightClass`, 400 when absent.
    pub weight: u16,
    /// `OS/2.usWidthClass`: 5 is normal, lower condensed, higher expanded.
    pub width: u16,
    /// Italic or oblique (`OS/2.fsSelection` bits 0/9, `head.macStyle` bit 1,
    /// or a non-zero `post.italicAngle`).
    pub italic: bool,
    /// `post.isFixedPitch`, or a PANOSE "monospaced" proportion.
    pub monospace: bool,
    /// `true` for CFF-outline (`OTTO`) faces.
    pub cff: bool,
    pub metrics: RawMetrics,
}

/// Decoration and cell metrics in font units (y up, as the font stores them).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct RawMetrics {
    pub units_per_em: u16,
    /// Line metrics: `hhea`'s, or `OS/2`'s typographic ones when the font
    /// sets USE_TYPO_METRICS.
    pub ascender: i16,
    pub descender: i16,
    pub line_gap: i16,
    pub advance_width_max: u16,
    /// `post.underlinePosition` (negative: below the baseline).
    pub underline_position: i16,
    pub underline_thickness: i16,
    /// `OS/2.yStrikeoutPosition` / `yStrikeoutSize`.
    pub strikeout_position: i16,
    pub strikeout_size: i16,
    /// `OS/2.sxHeight` / `sCapHeight` (version 2+), 0 when absent.
    pub x_height: i16,
    pub cap_height: i16,
}

const TTCF: u32 = u32::from_be_bytes(*b"ttcf");
const OTTO: u32 = u32::from_be_bytes(*b"OTTO");

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn bei16(b: &[u8], at: usize) -> Option<i16> {
    be16(b, at).map(|v| v as i16)
}

/// Random access to a font's bytes: `read(offset, len)`.
pub(crate) trait Source {
    fn read(&mut self, offset: u64, len: usize) -> Option<Vec<u8>>;
}

impl Source for &[u8] {
    fn read(&mut self, offset: u64, len: usize) -> Option<Vec<u8>> {
        let start = usize::try_from(offset).ok()?;
        self.get(start..start.checked_add(len)?).map(<[u8]>::to_vec)
    }
}

/// A file read on demand, never in full.
pub(crate) struct FileSource(pub std::fs::File);

impl Source for FileSource {
    fn read(&mut self, offset: u64, len: usize) -> Option<Vec<u8>> {
        // Tables this reader wants are small; refuse anything absurd.
        if len > 4 << 20 {
            return None;
        }
        self.0.seek(SeekFrom::Start(offset)).ok()?;
        let mut buf = vec![0; len];
        self.0.read_exact(&mut buf).ok()?;
        Some(buf)
    }
}

/// The number of faces in a font file: the subfont count of a collection,
/// else 1. `None` when the data isn't an sfnt.
pub(crate) fn face_count(src: &mut impl Source) -> Option<u32> {
    let head = src.read(0, 12)?;
    match be32(&head, 0)? {
        TTCF => Some(be32(&head, 8)?.min(256)),
        0x0001_0000 | OTTO | 0x7472_7565 /* 'true' */ => Some(1),
        _ => None,
    }
}

/// Read face `index` of the font in `src` (0 for a single-face file).
pub(crate) fn read_face(src: &mut impl Source, index: u32) -> Option<FaceInfo> {
    let head = src.read(0, 12)?;
    let offset = if be32(&head, 0)? == TTCF {
        if index >= be32(&head, 8)? {
            return None;
        }
        let at = src.read(12 + 4 * index as u64, 4)?;
        be32(&at, 0)? as u64
    } else {
        0
    };
    let dir = src.read(offset, 12)?;
    let version = be32(&dir, 0)?;
    let num_tables = be16(&dir, 4)? as usize;
    let records = src.read(offset + 12, num_tables * 16)?;
    let table = |tag: &[u8; 4]| {
        (0..num_tables).find_map(|i| {
            let r = &records[i * 16..i * 16 + 16];
            (&r[..4] == tag).then(|| (be32(r, 8).unwrap_or(0) as u64, be32(r, 12).unwrap_or(0)))
        })
    };
    let mut load = |tag: &[u8; 4]| {
        let (off, len) = table(tag)?;
        src.read(off, len as usize)
    };
    let headt = load(b"head")?;
    let hhea = load(b"hhea");
    let os2 = load(b"OS/2");
    let post = load(b"post");
    let name = load(b"name");

    let mut info = FaceInfo {
        weight: 400,
        width: 5,
        cff: version == OTTO,
        ..FaceInfo::default()
    };
    let m = &mut info.metrics;
    m.units_per_em = be16(&headt, 18).filter(|u| *u > 0).unwrap_or(1000);
    let mac_style = be16(&headt, 44).unwrap_or(0);
    if let Some(h) = &hhea {
        m.ascender = bei16(h, 4).unwrap_or(0);
        m.descender = bei16(h, 6).unwrap_or(0);
        m.line_gap = bei16(h, 8).unwrap_or(0);
        m.advance_width_max = be16(h, 10).unwrap_or(0);
    }
    let mut italic = mac_style & 2 != 0;
    if let Some(o) = &os2 {
        info.weight = be16(o, 4).filter(|w| (1..=1000).contains(w)).unwrap_or(400);
        info.width = be16(o, 6).filter(|w| (1..=9).contains(w)).unwrap_or(5);
        m.strikeout_size = bei16(o, 26).unwrap_or(0);
        m.strikeout_position = bei16(o, 28).unwrap_or(0);
        // PANOSE: Latin text family (2) with a monospaced proportion (9).
        if o.get(32) == Some(&2) && o.get(35) == Some(&9) {
            info.monospace = true;
        }
        let fs_selection = be16(o, 62).unwrap_or(0);
        italic |= fs_selection & (1 | 1 << 9) != 0;
        // USE_TYPO_METRICS: the font asks for its typographic line metrics
        // rather than `hhea`'s.
        if fs_selection & (1 << 7) != 0
            && let (Some(a), Some(d), Some(g)) = (bei16(o, 68), bei16(o, 70), bei16(o, 72))
            && a > 0
        {
            (m.ascender, m.descender, m.line_gap) = (a, d, g);
        }
        if be16(o, 0).unwrap_or(0) >= 2 {
            m.x_height = bei16(o, 86).unwrap_or(0);
            m.cap_height = bei16(o, 88).unwrap_or(0);
        }
    }
    if let Some(p) = &post {
        let angle = be32(p, 4).unwrap_or(0) as i32;
        italic |= angle.unsigned_abs() > 0x8000; // more than half a degree
        m.underline_position = bei16(p, 8).unwrap_or(0);
        m.underline_thickness = bei16(p, 10).unwrap_or(0);
        info.monospace |= be32(p, 12).unwrap_or(0) != 0;
    }
    info.italic = italic;
    if let Some(n) = &name {
        let typographic = name_string(n, 16);
        info.family = typographic
            .clone()
            .or_else(|| name_string(n, 1))
            .unwrap_or_default();
        info.subfamily = if typographic.is_some() {
            name_string(n, 17).or_else(|| name_string(n, 2))
        } else {
            name_string(n, 2)
        }
        .unwrap_or_default();
    }
    Some(info)
}

/// Read every face of the font in `src`.
pub(crate) fn read_faces(src: &mut impl Source) -> Vec<(u32, FaceInfo)> {
    let n = face_count(src).unwrap_or(0);
    (0..n)
        .filter_map(|i| read_face(src, i).map(|f| (i, f)))
        .collect()
}

/// Name `id` from a `name` table, preferring US English Windows (Unicode)
/// records, then any Unicode record, then Mac Roman.
fn name_string(table: &[u8], id: u16) -> Option<String> {
    let count = be16(table, 2)? as usize;
    let storage = be16(table, 4)? as usize;
    let mut best: Option<(u8, String)> = None;
    for i in 0..count {
        let r = 6 + i * 12;
        let (platform, encoding, language, name_id) = (
            be16(table, r)?,
            be16(table, r + 2)?,
            be16(table, r + 4)?,
            be16(table, r + 6)?,
        );
        if name_id != id {
            continue;
        }
        let len = be16(table, r + 8)? as usize;
        let off = storage + be16(table, r + 10)? as usize;
        let Some(raw) = table.get(off..off + len) else {
            continue;
        };
        let (rank, text) = match (platform, encoding) {
            (3, 1 | 10) | (0, _) => {
                let units: Vec<u16> = raw
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_be_bytes(*c))
                    .collect();
                let rank = if platform == 3 && language == 0x409 {
                    0
                } else {
                    1
                };
                (rank, String::from_utf16_lossy(&units))
            }
            // Mac Roman: ASCII is all a family name realistically needs.
            (1, 0) => (2, raw.iter().map(|&b| b as char).collect()),
            _ => continue,
        };
        if best.as_ref().is_none_or(|(r, _)| rank < *r) {
            best = Some((rank, text));
        }
    }
    best.map(|(_, s)| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Build a minimal single-face sfnt from `(tag, bytes)` tables.
    pub(crate) fn build_sfnt(version: u32, tables: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&version.to_be_bytes());
        out.extend_from_slice(&(tables.len() as u16).to_be_bytes());
        out.extend_from_slice(&[0; 6]);
        let mut data_off = 12 + tables.len() * 16;
        let mut data = Vec::new();
        for (tag, bytes) in tables {
            out.extend_from_slice(*tag);
            out.extend_from_slice(&0u32.to_be_bytes());
            out.extend_from_slice(&(data_off as u32).to_be_bytes());
            out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
            data.extend_from_slice(bytes);
            while data.len() % 4 != 0 {
                data.push(0);
            }
            data_off = 12 + tables.len() * 16 + data.len();
        }
        out.extend_from_slice(&data);
        out
    }

    /// A `name` table holding `(id, text)` Windows US-English records.
    pub(crate) fn name_table(names: &[(u16, &str)]) -> Vec<u8> {
        let mut records = Vec::new();
        let mut strings = Vec::new();
        for (id, text) in names {
            let utf16: Vec<u8> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
            for v in [
                3u16,
                1,
                0x409,
                *id,
                utf16.len() as u16,
                strings.len() as u16,
            ] {
                records.extend_from_slice(&v.to_be_bytes());
            }
            strings.extend_from_slice(&utf16);
        }
        let mut out = Vec::new();
        for v in [0u16, names.len() as u16, 6 + records.len() as u16] {
            out.extend_from_slice(&v.to_be_bytes());
        }
        out.extend_from_slice(&records);
        out.extend_from_slice(&strings);
        out
    }

    /// A synthetic face description: family, subfamily, weight, italic, mono.
    pub(crate) fn fake_face(
        family: &str,
        sub: &str,
        weight: u16,
        italic: bool,
        mono: bool,
    ) -> Vec<u8> {
        let mut head = vec![0u8; 54];
        head[18..20].copy_from_slice(&2048u16.to_be_bytes());
        let mut os2 = vec![0u8; 96];
        os2[0..2].copy_from_slice(&4u16.to_be_bytes());
        os2[4..6].copy_from_slice(&weight.to_be_bytes());
        os2[26..28].copy_from_slice(&102i16.to_be_bytes());
        os2[28..30].copy_from_slice(&530i16.to_be_bytes());
        os2[62..64].copy_from_slice(&(if italic { 1u16 } else { 0x40 }).to_be_bytes());
        os2[86..88].copy_from_slice(&1062i16.to_be_bytes());
        os2[88..90].copy_from_slice(&1466i16.to_be_bytes());
        let mut post = vec![0u8; 32];
        post[8..10].copy_from_slice(&(-217i16).to_be_bytes());
        post[10..12].copy_from_slice(&150i16.to_be_bytes());
        post[12..16].copy_from_slice(&(mono as u32).to_be_bytes());
        let mut hhea = vec![0u8; 36];
        hhea[4..6].copy_from_slice(&1854i16.to_be_bytes());
        hhea[6..8].copy_from_slice(&(-434i16).to_be_bytes());
        hhea[8..10].copy_from_slice(&67i16.to_be_bytes());
        hhea[10..12].copy_from_slice(&2400u16.to_be_bytes());
        build_sfnt(
            0x0001_0000,
            &[
                (b"OS/2", os2),
                (b"head", head),
                (b"hhea", hhea),
                (b"name", name_table(&[(1, family), (2, sub)])),
                (b"post", post),
            ],
        )
    }

    #[test]
    fn reads_names_styles_and_metrics() {
        let bytes = fake_face("Test Mono", "Bold Italic", 700, true, true);
        let mut src: &[u8] = &bytes;
        assert_eq!(face_count(&mut src), Some(1));
        let info = read_face(&mut src, 0).unwrap();
        assert_eq!(info.family, "Test Mono");
        assert_eq!(info.subfamily, "Bold Italic");
        assert_eq!(info.weight, 700);
        assert!(info.italic && info.monospace && !info.cff);
        let m = info.metrics;
        assert_eq!(m.units_per_em, 2048);
        assert_eq!((m.ascender, m.descender, m.line_gap), (1854, -434, 67));
        assert_eq!((m.underline_position, m.underline_thickness), (-217, 150));
        assert_eq!((m.strikeout_position, m.strikeout_size), (530, 102));
        assert_eq!((m.x_height, m.cap_height), (1062, 1466));
        assert_eq!(m.advance_width_max, 2400);
    }

    #[test]
    fn typographic_family_wins_over_legacy() {
        let name = name_table(&[(1, "Foo Light"), (2, "Regular"), (16, "Foo"), (17, "Light")]);
        let mut head = vec![0u8; 54];
        head[18..20].copy_from_slice(&1000u16.to_be_bytes());
        let bytes = build_sfnt(OTTO, &[(b"head", head), (b"name", name)]);
        let info = read_face(&mut &bytes[..], 0).unwrap();
        assert_eq!(
            (info.family.as_str(), info.subfamily.as_str()),
            ("Foo", "Light")
        );
        assert!(info.cff);
        assert_eq!(info.weight, 400);
    }

    #[test]
    fn collections_list_every_face() {
        let a = fake_face("A", "Regular", 400, false, false);
        let b = fake_face("B", "Bold", 700, false, false);
        // ttcf header: tag, version, count, two offsets; faces follow. Each
        // face's table offsets are relative to the file, so shift them.
        let header_len = 12 + 8;
        let mut out = Vec::new();
        out.extend_from_slice(b"ttcf");
        out.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        out.extend_from_slice(&2u32.to_be_bytes());
        let shift = |face: &[u8], base: usize| {
            let mut f = face.to_vec();
            let n = u16::from_be_bytes([f[4], f[5]]) as usize;
            for i in 0..n {
                let at = 12 + i * 16 + 8;
                let off = u32::from_be_bytes(f[at..at + 4].try_into().unwrap()) + base as u32;
                f[at..at + 4].copy_from_slice(&off.to_be_bytes());
            }
            f
        };
        let a_off = header_len;
        let b_off = a_off + a.len();
        out.extend_from_slice(&(a_off as u32).to_be_bytes());
        out.extend_from_slice(&(b_off as u32).to_be_bytes());
        out.extend_from_slice(&shift(&a, a_off));
        out.extend_from_slice(&shift(&b, b_off));
        let faces = read_faces(&mut &out[..]);
        assert_eq!(faces.len(), 2);
        assert_eq!(faces[0].1.family, "A");
        assert_eq!((faces[1].1.family.as_str(), faces[1].1.weight), ("B", 700));
    }

    #[test]
    fn garbage_is_not_a_font() {
        assert_eq!(face_count(&mut &b"hello world!"[..]), None);
        assert!(read_faces(&mut &b"\x00\x01"[..]).is_empty());
    }
}
