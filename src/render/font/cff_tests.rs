//! CFF-outline (`OTTO`) faces end to end, with fonts built here: a tiny CFF
//! font and its TrueType twin (the same two glyphs, the curve a cubic in one
//! and the equivalent quadratic in the other), a CID-keyed CFF font (as CJK
//! fonts are) and a collection holding one. Each must load, shape, measure
//! and draw like the TrueType font.

use super::FontStyle;
use super::sfnt::tests::{build_sfnt, name_table};
use crate::geometry::{Point, Rect, ScaleFactor, Size};
use crate::render::{Color, Font, Pixmap, Scene, SoftwareRenderer, TextPaint};

const UPEM: u16 = 1000;
/// The glyphs' advances: `.notdef`, then the two drawn glyphs.
const ADVANCES: [u16; 3] = [500, 800, 800];

fn be16(v: impl Into<i32>) -> [u8; 2] {
    (v.into() as u16).to_be_bytes()
}

/// The tables both flavors share; `maxp` and the outline tables differ.
fn common_tables(family: &str, chars: [char; 2]) -> Vec<(&'static [u8; 4], Vec<u8>)> {
    let mut head = vec![0u8; 54];
    head[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    head[12..16].copy_from_slice(&0x5F0F_3CF5u32.to_be_bytes());
    head[18..20].copy_from_slice(&UPEM.to_be_bytes());
    head[40..42].copy_from_slice(&be16(700));
    head[42..44].copy_from_slice(&be16(700));
    let mut hhea = vec![0u8; 36];
    hhea[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    hhea[4..6].copy_from_slice(&be16(800));
    hhea[6..8].copy_from_slice(&be16(-200));
    hhea[10..12].copy_from_slice(&be16(800));
    hhea[34..36].copy_from_slice(&be16(3));
    let hmtx: Vec<u8> = ADVANCES
        .iter()
        .flat_map(|&a| [be16(a), be16(0)].concat())
        .collect();
    let mut os2 = vec![0u8; 96];
    os2[0..2].copy_from_slice(&be16(4));
    os2[4..6].copy_from_slice(&be16(400));
    os2[6..8].copy_from_slice(&be16(5));
    os2[62..64].copy_from_slice(&be16(0x40));
    // Typographic and Windows ascent and descent.
    os2[68..70].copy_from_slice(&be16(800));
    os2[70..72].copy_from_slice(&be16(-200));
    os2[74..76].copy_from_slice(&be16(800));
    os2[76..78].copy_from_slice(&be16(200));
    let mut post = vec![0u8; 32];
    post[0..4].copy_from_slice(&0x0003_0000u32.to_be_bytes());
    vec![
        (b"OS/2", os2),
        (b"cmap", cmap(&[(chars[0], 1), (chars[1], 2)])),
        (b"head", head),
        (b"hhea", hhea),
        (b"hmtx", hmtx),
        (b"name", name_table(&[(1, family), (2, "Regular")])),
        (b"post", post),
    ]
}

/// A format-4 `cmap` mapping each character to its glyph (BMP only).
fn cmap(map: &[(char, u16)]) -> Vec<u8> {
    let mut segs: Vec<(u16, u16)> = map.iter().map(|&(c, g)| (c as u16, g)).collect();
    segs.sort();
    let n = segs.len() + 1; // and the 0xFFFF terminator
    let mut sub = Vec::new();
    let log = n.ilog2() as u16;
    let range = 2u16 << log;
    for v in [
        4u16,
        0,
        0,
        (n * 2) as u16,
        range,
        log,
        (n * 2) as u16 - range,
    ] {
        sub.extend_from_slice(&v.to_be_bytes());
    }
    let codes: Vec<u16> = segs.iter().map(|s| s.0).chain([0xFFFF]).collect();
    for c in &codes {
        sub.extend_from_slice(&c.to_be_bytes()); // endCode
    }
    sub.extend_from_slice(&[0, 0]);
    for c in &codes {
        sub.extend_from_slice(&c.to_be_bytes()); // startCode
    }
    for (code, gid) in &segs {
        sub.extend_from_slice(&gid.wrapping_sub(*code).to_be_bytes());
    }
    sub.extend_from_slice(&1u16.to_be_bytes());
    sub.extend(std::iter::repeat_n(0u8, n * 2)); // idRangeOffset
    let len = sub.len() as u16;
    sub[2..4].copy_from_slice(&len.to_be_bytes());
    let mut out = Vec::new();
    for v in [0u16, 1, 3, 1] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    out.extend_from_slice(&12u32.to_be_bytes());
    out.extend_from_slice(&sub);
    out
}

/// The TrueType twin: glyph 1 a square, glyph 2 an arch (one quadratic).
fn ttf(family: &str, chars: [char; 2]) -> Vec<u8> {
    // (x, y, on-curve) per point, absolute; one contour per glyph.
    let square = [
        (100, 0, true),
        (700, 0, true),
        (700, 700, true),
        (100, 700, true),
    ];
    let arch = [(100, 0, true), (400, 600, false), (700, 0, true)];
    let glyph = |pts: &[(i16, i16, bool)]| {
        let mut g = Vec::new();
        for v in [1i16, 100, 0, 700, 700, pts.len() as i16 - 1, 0] {
            g.extend_from_slice(&v.to_be_bytes());
        }
        g.extend(pts.iter().map(|p| p.2 as u8));
        let (mut x, mut y) = (0, 0);
        for p in pts {
            g.extend_from_slice(&(p.0 - x).to_be_bytes());
            x = p.0;
        }
        for p in pts {
            g.extend_from_slice(&(p.1 - y).to_be_bytes());
            y = p.1;
        }
        if g.len() % 2 == 1 {
            g.push(0);
        }
        g
    };
    let (a, b) = (glyph(&square), glyph(&arch));
    let mut loca = Vec::new();
    for off in [0, 0, a.len(), a.len() + b.len()] {
        loca.extend_from_slice(&be16((off / 2) as i32));
    }
    let mut maxp = vec![0u8; 32];
    maxp[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    maxp[4..6].copy_from_slice(&be16(3));
    let mut tables = common_tables(family, chars);
    tables.push((b"glyf", [a, b].concat()));
    tables.push((b"loca", loca));
    tables.push((b"maxp", maxp));
    tables.sort_by_key(|t| *t.0);
    build_sfnt(0x0001_0000, &tables)
}

/// A CFF INDEX of `items` (4-byte offsets).
fn index(items: &[Vec<u8>]) -> Vec<u8> {
    if items.is_empty() {
        return vec![0, 0];
    }
    let mut out = be16(items.len() as i32).to_vec();
    out.push(4);
    let mut off = 1u32;
    out.extend_from_slice(&off.to_be_bytes());
    for it in items {
        off += it.len() as u32;
        out.extend_from_slice(&off.to_be_bytes());
    }
    for it in items {
        out.extend_from_slice(it);
    }
    out
}

/// A DICT entry: integer operands (5-byte form, so sizes don't depend on
/// values) then the operator (`>= 1200` for the two-byte `12 x` ones).
fn dict(operands: &[i32], op: u16) -> Vec<u8> {
    let mut out = Vec::new();
    for v in operands {
        out.push(29);
        out.extend_from_slice(&v.to_be_bytes());
    }
    if op >= 1200 {
        out.extend_from_slice(&[12, (op - 1200) as u8]);
    } else {
        out.push(op as u8);
    }
    out
}

/// A Type 2 charstring: numbers (as `28 int16`) and operators.
enum Cs {
    N(i16),
    Op(u8),
    /// `hintmask` and its one mask byte.
    Mask(u8),
}

fn charstring(ops: &[Cs]) -> Vec<u8> {
    let mut out = Vec::new();
    for o in ops {
        match o {
            Cs::N(v) => {
                out.push(28);
                out.extend_from_slice(&v.to_be_bytes());
            }
            Cs::Op(op) => out.push(*op),
            Cs::Mask(m) => out.extend_from_slice(&[19, *m]),
        }
    }
    out
}

// Charstring operators.
const HSTEM: u8 = 1;
const VSTEM: u8 = 3;
const RLINETO: u8 = 5;
const RRCURVETO: u8 = 8;
const CALLSUBR: u8 = 10;
const RETURN: u8 = 11;
const ENDCHAR: u8 = 14;
const RMOVETO: u8 = 21;
const CALLGSUBR: u8 = 29;
/// Subroutine numbers are biased by 107 in INDEXes of fewer than 1240.
const BIAS: i16 = 107;

/// The CFF table: the same square and arch as [`ttf`], the square drawn
/// through a global and a local subroutine after stem hints and a hint
/// mask; with `cid`, a CID-keyed font whose two drawn glyphs belong to two
/// Font DICTs (FDSelect), each with its own local subroutines.
fn cff_table(cid: bool) -> Vec<u8> {
    use Cs::*;
    let notdef = charstring(&[Op(ENDCHAR)]);
    let square = charstring(&[
        N(0),
        N(50),
        N(650),
        N(50),
        Op(HSTEM),
        N(100),
        N(50),
        N(450),
        N(50),
        Op(VSTEM),
        Mask(0xF0),
        N(100),
        N(0),
        Op(RMOVETO),
        N(-BIAS),
        Op(CALLGSUBR),
        N(-BIAS),
        Op(CALLSUBR),
        Op(ENDCHAR),
    ]);
    // In the CID font the arch's Font DICT has its own subroutine 0.
    let arch = if cid {
        charstring(&[
            N(100),
            N(0),
            Op(RMOVETO),
            N(-BIAS),
            Op(CALLSUBR),
            Op(ENDCHAR),
        ])
    } else {
        charstring(&[
            N(100),
            N(0),
            Op(RMOVETO),
            N(200),
            N(400),
            N(200),
            N(0),
            N(200),
            N(-400),
            Op(RRCURVETO),
            Op(ENDCHAR),
        ])
    };
    let gsubrs = index(&[charstring(&[
        N(600),
        N(0),
        Op(RLINETO),
        N(0),
        N(700),
        Op(RLINETO),
        Op(RETURN),
    ])]);
    let square_rest = charstring(&[N(-600), N(0), Op(RLINETO), Op(RETURN)]);
    let arch_curve = charstring(&[
        N(200),
        N(400),
        N(200),
        N(0),
        N(200),
        N(-400),
        Op(RRCURVETO),
        Op(RETURN),
    ]);
    let charstrings = index(&[notdef, square, arch]);
    // A Private DICT whose local Subrs INDEX follows it directly.
    let private = |subrs: &[u8]| {
        let mut d = dict(&[0], 20); // defaultWidthX
        d.extend(dict(&[0], 21)); // nominalWidthX
        let len = d.len() as i32 + 6;
        d.extend(dict(&[len], 19)); // Subrs, relative to the DICT
        let dict_len = d.len();
        (d, dict_len, subrs.to_vec())
    };

    // Lay out: header, Name, Top DICT, String, Global Subr INDEXes, then
    // CharStrings, the Private DICT(s) with their Subrs, FDArray, FDSelect.
    let header = vec![1u8, 0, 4, 4];
    let name = index(&[b"T".to_vec()]);
    let strings = index(&[]);
    let top_dict = |cs: i32, private: (i32, i32), fd_array: i32, fd_select: i32| {
        let mut d = Vec::new();
        if cid {
            d.extend(dict(&[0, 0, 0], 1230)); // ROS
            d.extend(dict(&[fd_array], 1236));
            d.extend(dict(&[fd_select], 1237));
        } else {
            d.extend(dict(&[private.0, private.1], 18));
        }
        d.extend(dict(&[cs], 17));
        d
    };
    let top_len = index(&[top_dict(0, (0, 0), 0, 0)]).len();
    let cs_off = header.len() + name.len() + top_len + strings.len() + gsubrs.len();
    let mut tail = charstrings.clone();
    let mut privs = Vec::new();
    let subr_sets: Vec<Vec<u8>> = if cid {
        vec![index(&[square_rest]), index(&[arch_curve])]
    } else {
        vec![index(&[square_rest])]
    };
    for subrs in &subr_sets {
        let (d, dict_len, subrs) = private(subrs);
        let at = cs_off + tail.len();
        privs.push((dict_len as i32, at as i32));
        tail.extend(d);
        tail.extend(subrs);
    }
    let mut fd_array_off = 0;
    let mut fd_select_off = 0;
    if cid {
        fd_array_off = (cs_off + tail.len()) as i32;
        let fds: Vec<Vec<u8>> = privs.iter().map(|p| dict(&[p.0, p.1], 18)).collect();
        tail.extend(index(&fds));
        fd_select_off = (cs_off + tail.len()) as i32;
        // Format 3: glyphs 0..2 in Font DICT 0, glyph 2 in Font DICT 1.
        tail.extend_from_slice(&[3, 0, 2, 0, 0, 0, 0, 2, 1, 0, 3]);
    }
    let top = index(&[top_dict(
        cs_off as i32,
        privs[0],
        fd_array_off,
        fd_select_off,
    )]);
    assert_eq!(top.len(), top_len);
    [header, name, top, strings, gsubrs, tail].concat()
}

/// The CFF font: [`common_tables`], a version 0.5 `maxp` and `CFF `.
fn otf(family: &str, chars: [char; 2], cid: bool) -> Vec<u8> {
    let mut maxp = 0x0000_5000u32.to_be_bytes().to_vec();
    maxp.extend_from_slice(&be16(3));
    let mut tables = common_tables(family, chars);
    tables.push((b"CFF ", cff_table(cid)));
    tables.push((b"maxp", maxp));
    tables.sort_by_key(|t| *t.0);
    build_sfnt(u32::from_be_bytes(*b"OTTO"), &tables)
}

/// A collection of `faces` (each table offset rebased to the file).
fn ttc(faces: &[Vec<u8>]) -> Vec<u8> {
    let header = 12 + 4 * faces.len();
    let mut out = b"ttcf".to_vec();
    out.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    out.extend_from_slice(&(faces.len() as u32).to_be_bytes());
    let mut at = header;
    for f in faces {
        out.extend_from_slice(&(at as u32).to_be_bytes());
        at += f.len();
    }
    let mut base = header;
    for f in faces {
        let mut f = f.clone();
        let n = u16::from_be_bytes([f[4], f[5]]) as usize;
        for i in 0..n {
            let p = 12 + i * 16 + 8;
            let off = u32::from_be_bytes(f[p..p + 4].try_into().unwrap()) + base as u32;
            f[p..p + 4].copy_from_slice(&off.to_be_bytes());
        }
        base += f.len();
        out.extend_from_slice(&f);
    }
    out
}

fn draw(font: &Font, text: &str) -> Pixmap {
    let mut scene = Scene::new(Size::new(80.0, 40.0));
    scene.fill_rect(Rect::from_xywh(0.0, 0.0, 80.0, 40.0), Color::WHITE);
    scene.fill_text(
        text,
        Point::new(2.0, 2.0),
        &TextPaint::new(font, 32.0, Color::BLACK),
    );
    SoftwareRenderer::new().render(scene, ScaleFactor::IDENTITY)
}

/// `(pixels that differ by more than `tolerance` in red, inked pixels)`.
fn compare(a: &Pixmap, b: &Pixmap, tolerance: u8) -> (usize, usize) {
    assert_eq!(a.size(), b.size());
    let (mut differ, mut inked) = (0, 0);
    for y in 0..a.size().height {
        for x in 0..a.size().width {
            let (pa, pb) = (a.pixel(x, y).unwrap(), b.pixel(x, y).unwrap());
            if pa[0].abs_diff(pb[0]) > tolerance {
                differ += 1;
            }
            if pa[0] < 128 {
                inked += 1;
            }
        }
    }
    (differ, inked)
}

#[test]
fn a_cff_font_draws_like_its_truetype_twin() {
    let tt = Font::from_bytes(ttf("Twin TT", ['A', 'B'])).unwrap();
    let cff = Font::from_bytes(otf("Twin CFF", ['A', 'B'], false)).unwrap();
    assert_eq!(cff.family_name(), Some("Twin CFF"));
    // Shaped and measured alike: the same glyphs and advances.
    let ids = |f: &Font| -> Vec<u16> { f.shaped("AB", 32.0).iter().map(|g| g.glyph_id).collect() };
    assert_eq!(ids(&cff), [1, 2]);
    assert_eq!(cff.measure("AB", 32.0), tt.measure("AB", 32.0));
    assert_eq!(cff.metrics(32.0), tt.metrics(32.0));
    // The square, all straight lines (drawn through subroutines, after hints
    // and a hint mask), comes out pixel for pixel.
    let (a_tt, a_cff) = (draw(&tt, "A"), draw(&cff, "A"));
    let (differ, inked) = compare(&a_tt, &a_cff, 0);
    assert!(inked > 300, "the square drew {inked} pixels");
    assert_eq!(differ, 0, "the CFF square differs from the TrueType one");
    // The arch's cubic is flattened to lines, the quadratic isn't: edges
    // differ by a fraction of a pixel at most.
    let (b_tt, b_cff) = (draw(&tt, "B"), draw(&cff, "B"));
    let (differ, inked) = compare(&b_tt, &b_cff, 96);
    assert!(inked > 100, "the arch drew {inked} pixels");
    assert!(differ <= 4, "{differ} arch pixels differ noticeably");
}

/// CJK fonts are CID-keyed: glyphs reach their subroutines through FDSelect
/// and per-Font-DICT Private DICTs. Android and macOS ship them in
/// collections.
#[test]
fn cid_keyed_cff_in_a_collection_draws() {
    let twin = Font::from_bytes(ttf("Twin TT", ['日', '本'])).unwrap();
    let cid = otf("Twin CID", ['日', '本'], true);
    let alone = Font::from_bytes(cid.clone()).unwrap();
    let (a, b) = (draw(&twin, "日本"), draw(&alone, "日本"));
    let (differ, inked) = compare(&a, &b, 96);
    assert!(inked > 450, "drew {inked} pixels");
    assert!(differ <= 4, "{differ} pixels differ noticeably");
    // The second face of a collection whose first is a bold TrueType face.
    let collection = ttc(&[with_weight(ttf("Mixed", ['x', 'y']), 700), cid]);
    let font = Font::from_bytes(collection).unwrap();
    assert_eq!(font.style(), FontStyle::BOLD);
    let regular = font.styled(FontStyle::NORMAL);
    assert!(
        !regular.is_synthesized().0,
        "the CFF face is the regular one"
    );
    let (differ, _) = compare(&b, &draw(&regular, "日本"), 0);
    assert_eq!(differ, 0);
}

/// `font` with `OS/2.usWeightClass` set to `weight`.
fn with_weight(mut font: Vec<u8>, weight: u16) -> Vec<u8> {
    let n = u16::from_be_bytes([font[4], font[5]]) as usize;
    let rec = (0..n)
        .map(|i| 12 + i * 16)
        .find(|&r| &font[r..r + 4] == b"OS/2")
        .unwrap();
    let off = u32::from_be_bytes(font[rec + 8..rec + 12].try_into().unwrap()) as usize;
    font[off + 4..off + 6].copy_from_slice(&weight.to_be_bytes());
    font
}

/// Font discovery no longer skips CFF faces: one installed (here, in a
/// scanned directory) is a family like any other, and serves as a fallback.
#[test]
fn installed_cff_faces_are_listed_and_fall_back() {
    let dir = std::env::temp_dir().join(format!("stipple-cffscan-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Cid.otf"), otf("Scanned CID", ['日', '本'], true)).unwrap();
    std::fs::write(
        dir.join("Pair.ttc"),
        ttc(&[otf("Scanned Pair", ['A', 'B'], false)]),
    )
    .unwrap();
    let faces = super::system::scan(std::slice::from_ref(&dir));
    std::fs::remove_dir_all(&dir).ok();
    let index = super::system::Index::new(faces, Default::default());
    let (name, faces) = index
        .resolve("scanned cid")
        .expect("the CFF face is listed");
    assert_eq!(name, "Scanned CID");
    assert!(faces[0].info.cff);
    assert!(index.resolve("Scanned Pair").is_some());
    let names: Vec<String> = index.families().into_iter().map(|f| f.name).collect();
    assert_eq!(names, ["Scanned CID", "Scanned Pair"]);

    let latin = Font::from_bytes(ttf("Latin", ['A', 'B'])).unwrap();
    let cjk = Font::from_bytes(otf("Fallback CID", ['日', '本'], true)).unwrap();
    let chained = latin.with_fallback(&cjk);
    let glyphs = chained.shaped("A日", 32.0);
    assert_eq!(
        glyphs
            .iter()
            .map(|g| (g.face_idx, g.glyph_id))
            .collect::<Vec<_>>(),
        [(0, 1), (1, 1)]
    );
    assert!(chained.has_glyph('日') && chained.has_glyph('A'));
    assert!(!latin.has_glyph('日'));
}
