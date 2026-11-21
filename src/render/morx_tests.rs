//! `morx` subtables built by hand into the two-glyph test font of
//! `cff_tests` (glyph 1 for 'X', glyph 2 for 'Y'), and the AAT lookup
//! formats.

use super::*;
use crate::render::Font;
use crate::render::font::cff_tests::ttf_with;

fn words(ws: &[u32]) -> Vec<u8> {
    ws.iter().flat_map(|&w| (w as u16).to_be_bytes()).collect()
}

fn longs(ls: &[u32]) -> Vec<u8> {
    ls.iter().flat_map(|l| l.to_be_bytes()).collect()
}

/// A `morx` version 2 table: one chain, its default flags 1, holding
/// `subtables` (kind, body), each enabled by flag 1, horizontal.
fn morx(subtables: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut subs = Vec::new();
    for (kind, body) in subtables {
        subs.extend(longs(&[12 + body.len() as u32, *kind, 1]));
        subs.extend(body);
    }
    let mut t = words(&[2, 0]);
    t.extend(longs(&[1]));
    t.extend(longs(&[
        1,
        16 + subs.len() as u32,
        0,
        subtables.len() as u32,
    ]));
    t.extend(subs);
    t
}

/// Class lookup (format 2): 'X' class 4, 'Y' class 5.
fn classes() -> Vec<u8> {
    let mut t = words(&[2, 6, 2, 12, 1, 0]);
    t.extend(words(&[1, 1, 4, 2, 2, 5]));
    t
}

/// Three states over six classes: X goes to state 2 (entry 1), Y in
/// state 2 takes entry 2, anything else entry 0 back to state 0.
fn states() -> Vec<u8> {
    let mut s = Vec::new();
    for row in [[0, 0, 0, 0, 1, 0], [0, 0, 0, 0, 1, 0], [0, 0, 0, 0, 1, 2]] {
        s.extend(words(&row));
    }
    s
}

/// An extended state table header and its class table and states, with
/// `extra` offsets (to parts after the entries) following it; the
/// entries come right after the states. Returns the body so far and
/// where the entries start.
fn state_table(entries: &[u8], extra: usize) -> (Vec<u8>, usize) {
    let head = 16 + extra * 4;
    let class_at = head;
    let states_at = class_at + classes().len();
    let entries_at = states_at + states().len();
    let mut b = longs(&[6, class_at as u32, states_at as u32, entries_at as u32]);
    b.resize(head, 0);
    b.extend(classes());
    b.extend(states());
    b.extend(entries);
    (b, 16)
}

fn shaped(table: Vec<u8>, text: &str) -> Vec<u16> {
    // Faces are cached by their size and leading bytes: a name of its own
    // length for each font.
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = format!("Morx {}", "x".repeat(n));
    let font = Font::from_bytes(ttf_with(&name, ['X', 'Y'], vec![(b"morx", table)])).unwrap();
    font.shaped(text, 16.0).iter().map(|g| g.glyph_id).collect()
}

/// A ligature subtable: X then Y joins into glyph 2.
fn ligature() -> Vec<u8> {
    let entries = [
        words(&[0, 0, 0]),
        words(&[2, 0x8000, 0]),
        words(&[0, 0x8000 | 0x2000, 0]),
    ]
    .concat();
    let (mut b, extra_at) = state_table(&entries, 3);
    let actions_at = b.len();
    b.extend(longs(&[0, 0x8000_0000])); // Y: add, then X: add and finish
    let components_at = b.len();
    b.extend(words(&[0, 0, 1])); // by glyph: Y counts 1
    let ligatures_at = b.len();
    b.extend(words(&[0, 2])); // index 1: glyph 2
    b[extra_at..extra_at + 12].copy_from_slice(&longs(&[
        actions_at as u32,
        components_at as u32,
        ligatures_at as u32,
    ]));
    morx(&[(2, b)])
}

#[test]
fn ligatures_join_their_components() {
    let t = ligature;
    assert_eq!(shaped(t(), "XY"), [2]);
    assert_eq!(shaped(t(), "YX"), [2, 1]);
    assert_eq!(shaped(t(), "XX"), [1, 1]);
    assert_eq!(shaped(t(), "XYX"), [2, 1]);
    assert_eq!(shaped(t(), "XXY"), [1, 2], "the later X joins");
    // Without the font's table, nothing joins.
    let plain = Font::from_bytes(ttf_with("No Morx", ['X', 'Y'], vec![])).unwrap();
    let ids: Vec<u16> = plain
        .shaped("XY", 16.0)
        .iter()
        .map(|g| g.glyph_id)
        .collect();
    assert_eq!(ids, [1, 2]);
}

#[test]
fn a_ligature_reports_its_components() {
    let (glyphs, counts) = apply(&ligature(), 3, &[1, 1, 2, 2]);
    assert_eq!(glyphs, [1, 2, 2]);
    assert_eq!(counts, [1, 2, 1]);
}

/// A contextual subtable: X sets the mark; a Y after it turns the marked
/// X into glyph 2 (lookup 0) and itself into glyph 1 (lookup 1).
fn contextual() -> Vec<u8> {
    let entries = [
        words(&[0, 0, 0xFFFF, 0xFFFF]),
        words(&[2, 0x8000, 0xFFFF, 0xFFFF]),
        words(&[0, 0, 0, 1]),
    ]
    .concat();
    let (mut b, extra_at) = state_table(&entries, 1);
    let subs_at = b.len();
    // Two lookups: format 6 (X → 2), format 8 (Y → 1).
    let mut l0 = words(&[6, 4, 1, 4, 0, 0]);
    l0.extend(words(&[1, 2]));
    let l1 = words(&[8, 2, 1, 1]);
    b.extend(longs(&[8, 8 + l0.len() as u32]));
    b.extend(l0);
    b.extend(l1);
    b[extra_at..extra_at + 4].copy_from_slice(&longs(&[subs_at as u32]));
    morx(&[(1, b)])
}

#[test]
fn contextual_substitutes_the_mark_and_the_current_glyph() {
    let t = contextual;
    assert_eq!(shaped(t(), "XY"), [2, 1]);
    assert_eq!(shaped(t(), "YY"), [2, 2]);
    assert_eq!(shaped(t(), "XX"), [1, 1]);
    assert_eq!(shaped(t(), "XYX"), [2, 1, 1]);
}

/// A rearrangement subtable: X marks the first glyph, a Y after it the
/// last, with verb 1 (Ax ⇒ xA).
#[test]
fn rearrangement_moves_the_marked_glyphs() {
    let entries = [words(&[0, 0]), words(&[2, 0x8000]), words(&[0, 0x2000 | 1])].concat();
    let (b, _) = state_table(&entries, 0);
    let t = || morx(&[(0, b.clone())]);
    assert_eq!(shaped(t(), "XY"), [2, 1]);
    assert_eq!(shaped(t(), "XXY"), [1, 2, 1]);
    assert_eq!(shaped(t(), "YX"), [2, 1]);
}

/// A noncontextual subtable swaps X and Y; chained after the ligature it
/// sees the ligature's output.
#[test]
fn noncontextual_and_chained_subtables() {
    let swap = words(&[0, 0, 2, 1]);
    assert_eq!(shaped(morx(&[(4, swap.clone())]), "XYY"), [2, 1, 1]);
    // Ligature first (X Y → 2), then the swap (2 → 1).
    let lig = ligature();
    let lig_body = lig[8 + 16 + 12..].to_vec();
    let both = morx(&[(2, lig_body), (4, swap)]);
    assert_eq!(shaped(both, "XYX"), [1, 2]);
}

/// A subtable whose flags the chain doesn't enable, or vertical only, is
/// skipped.
#[test]
fn disabled_and_vertical_subtables_are_skipped() {
    let swap = words(&[0, 0, 2, 1]);
    let mut t = morx(&[(4, swap.clone())]);
    t[8 + 16 + 8..8 + 16 + 12].copy_from_slice(&longs(&[2])); // flags 2
    assert_eq!(shaped(t, "XY"), [1, 2]);
    let t = morx(&[(VERTICAL | 4, swap)]);
    assert_eq!(shaped(t, "XY"), [1, 2]);
}

#[test]
fn lookup_formats() {
    // 0: one value per glyph.
    let f0 = words(&[0, 10, 11, 12]);
    assert_eq!(lookup(&f0, 1, 3), Some(11));
    assert_eq!(lookup(&f0, 3, 3), None);
    // 2: segments, with the 0xFFFF terminator.
    let mut f2 = words(&[2, 6, 3, 12, 1, 6]);
    f2.extend(words(&[5, 3, 7, 9, 8, 9, 0xFFFF, 0xFFFF, 0]));
    assert_eq!(lookup(&f2, 4, 100), Some(7));
    assert_eq!(lookup(&f2, 8, 100), Some(9));
    assert_eq!(lookup(&f2, 6, 100), None);
    assert_eq!(lookup(&f2, 0xFFFF, 100), None);
    // 4: segments pointing to value arrays (offsets from the table).
    let mut f4 = words(&[4, 6, 1, 6, 0, 0]);
    f4.extend(words(&[12, 10, 18]));
    f4.extend(words(&[100, 101, 102]));
    assert_eq!(lookup(&f4, 11, 100), Some(101));
    assert_eq!(lookup(&f4, 13, 100), None);
    // 6: single glyphs.
    let mut f6 = words(&[6, 4, 2, 8, 1, 0]);
    f6.extend(words(&[3, 30, 7, 70]));
    assert_eq!(lookup(&f6, 7, 100), Some(70));
    assert_eq!(lookup(&f6, 5, 100), None);
    // 8: a trimmed array.
    let f8 = words(&[8, 20, 2, 200, 201]);
    assert_eq!(lookup(&f8, 21, 100), Some(201));
    assert_eq!(lookup(&f8, 22, 100), None);
    // 10: a trimmed array of one-byte values.
    let mut f10 = words(&[10, 1, 5, 2]);
    f10.extend([42, 43]);
    assert_eq!(lookup(&f10, 6, 100), Some(43));
}

#[test]
fn garbage_is_ignored() {
    for t in [vec![], vec![0, 2], vec![0, 2, 0, 0, 0, 0, 0, 9, 1, 2, 3]] {
        assert_eq!(apply(&t, 3, &[1, 2]).0, [1, 2]);
    }
}
