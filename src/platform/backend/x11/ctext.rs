//! Decoding COMPOUND_TEXT, the ISO 2022 based encoding XIM servers send text
//! in (IBus offers no other).
//!
//! A server built on libX11 converts its UTF-8 to compound text through the
//! locale's charsets, in order, so a UTF-8 locale gives ASCII and Latin-1 as
//! they are, other Latin, Cyrillic, Greek… scripts in an ISO 8859 part,
//! Japanese, Korean and Chinese in JIS X 0208, KS C 5601 or GB 2312, and the
//! rest as a UTF-8 segment (`ESC % G`). The single-byte and 94×94 tables come
//! from `ctext.bin`: fourteen ISO 8859 right halves (96 code points each),
//! then GB 2312, JIS X 0208 and KS C 5601 (94 × 94 each), as little-endian
//! UTF-16 units, 0 where a position is unassigned. It was generated from
//! Python's codecs.

const TABLES: &[u8] = include_bytes!("ctext.bin");

/// The ISO 8859 parts in `ctext.bin`, by the final byte of their `ESC -`
/// designation.
const SETS96: [(u8, &str); 14] = [
    (b'B', "iso8859-2"),
    (b'C', "iso8859-3"),
    (b'D', "iso8859-4"),
    (b'L', "iso8859-5"),
    (b'G', "iso8859-6"),
    (b'F', "iso8859-7"),
    (b'H', "iso8859-8"),
    (b'M', "iso8859-9"),
    (b'V', "iso8859-10"),
    (b'T', "iso8859-11"),
    (b'Y', "iso8859-13"),
    (b'_', "iso8859-14"),
    (b'b', "iso8859-15"),
    (b'f', "iso8859-16"),
];

/// The 94×94 sets in `ctext.bin`, by the final byte of `ESC $ (` / `ESC $ )`.
const SETS94N: [u8; 3] = *b"ABC";

/// What a half (GL or GR) of the code table currently holds.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Set {
    Ascii,
    Latin1,
    /// JIS X 0201: Roman in GL (`ESC ( J`), katakana in GR (`ESC ) I`).
    JisRoman,
    JisKana,
    /// An ISO 8859 part, as an index into [`SETS96`].
    Iso8859(usize),
    /// A 94×94 set, as an index into [`SETS94N`].
    Double(usize),
    /// A set we have no table for: its bytes become U+FFFD.
    Unknown {
        double: bool,
    },
}

fn table_u16(index: usize) -> u16 {
    let at = index * 2;
    TABLES
        .get(at..at + 2)
        .map_or(0, |b| u16::from_le_bytes([b[0], b[1]]))
}

/// One character of `set` from the 7-bit `byte` (0x20..=0x7f).
fn single(set: Set, byte: u8) -> char {
    let b = byte & 0x7f;
    match set {
        Set::Ascii => b as char,
        Set::Latin1 => char::from(b | 0x80),
        Set::JisRoman => match b {
            0x5c => '¥',
            0x7e => '‾',
            _ => b as char,
        },
        Set::JisKana => match b {
            0x21..=0x5f => char::from_u32(0xff61 + (b as u32 - 0x21)).unwrap_or('\u{fffd}'),
            _ => '\u{fffd}',
        },
        Set::Iso8859(i) => {
            let v = table_u16(i * 96 + (b as usize).saturating_sub(0x20));
            char::from_u32(v as u32)
                .filter(|_| v != 0)
                .unwrap_or('\u{fffd}')
        }
        Set::Double(_) | Set::Unknown { .. } => '\u{fffd}',
    }
}

/// One character of the 94×94 set `i` from its two 7-bit bytes.
fn double(i: usize, b1: u8, b2: u8) -> char {
    let (r, c) = ((b1 & 0x7f) as usize, (b2 & 0x7f) as usize);
    if !(0x21..0x7f).contains(&r) || !(0x21..0x7f).contains(&c) {
        return '\u{fffd}';
    }
    let v = table_u16(SETS96.len() * 96 + i * 94 * 94 + (r - 0x21) * 94 + (c - 0x21));
    char::from_u32(v as u32)
        .filter(|_| v != 0)
        .unwrap_or('\u{fffd}')
}

/// Decode compound text to a string. Unknown or malformed parts become
/// U+FFFD; it never fails.
pub(super) fn decode(bytes: &[u8]) -> String {
    let mut out = String::new();
    let (mut gl, mut gr) = (Set::Ascii, Set::Latin1);
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            0x1b => {
                i += 1;
                i = escape(bytes, i, &mut gl, &mut gr, &mut out);
            }
            // CSI: direction changes (`CSI 1 ]`, `CSI 2 ]`, `CSI ]`), which
            // don't change the characters. Skip to the final byte.
            0x9b => {
                i += 1;
                while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                    i += 1;
                }
                i += 1;
            }
            b'\n' | b'\t' => {
                out.push(b as char);
                i += 1;
            }
            0x20..=0x7f | 0xa0..=0xff => {
                let set = if b < 0x80 { gl } else { gr };
                // Space and DEL are the same in every GL set.
                if b == 0x20 || b == 0x7f {
                    out.push(b as char);
                    i += 1;
                } else if let Set::Double(t) = set {
                    out.push(match bytes.get(i + 1) {
                        Some(&b2) => double(t, b, b2),
                        None => '\u{fffd}',
                    });
                    i += 2;
                } else if let Set::Unknown { double: true } = set {
                    out.push('\u{fffd}');
                    i += 2;
                } else {
                    out.push(single(set, b));
                    i += 1;
                }
            }
            _ => i += 1, // other control characters
        }
    }
    out
}

/// Handle the escape sequence starting at `bytes[i]` (just after ESC). Returns
/// where decoding goes on.
fn escape(bytes: &[u8], i: usize, gl: &mut Set, gr: &mut Set, out: &mut String) -> usize {
    let at = |k: usize| bytes.get(k).copied().unwrap_or(0);
    match (at(i), at(i + 1), at(i + 2)) {
        (b'(', b'B', _) => *gl = Set::Ascii,
        (b'(', b'J', _) => *gl = Set::JisRoman,
        (b'(', b'I', _) => *gl = Set::JisKana,
        (b')', b'I', _) => *gr = Set::JisKana,
        (b'(' | b')', _, _) => {
            let set = Set::Unknown { double: false };
            if at(i) == b'(' {
                *gl = set;
            } else {
                *gr = set;
            }
        }
        (b'-', b'A', _) => *gr = Set::Latin1,
        (b'-', f, _) => {
            *gr = SETS96
                .iter()
                .position(|&(fin, _)| fin == f)
                .map_or(Set::Unknown { double: false }, Set::Iso8859);
        }
        (b'$', side @ (b'(' | b')'), f) => {
            let set = SETS94N
                .iter()
                .position(|&fin| fin == f)
                .map_or(Set::Unknown { double: true }, Set::Double);
            if side == b'(' {
                *gl = set;
            } else {
                *gr = set;
            }
            return i + 3;
        }
        // The short form `ESC $ A` (GB 2312 to GL), from old encoders.
        (b'$', f @ (b'A' | b'B'), _) => {
            *gl = Set::Double(SETS94N.iter().position(|&fin| fin == f).unwrap_or(0));
        }
        // A UTF-8 segment, to `ESC % @`.
        (b'%', b'G', _) => {
            let start = i + 2;
            let end = bytes[start..]
                .windows(3)
                .position(|w| w == b"\x1b%@")
                .map_or(bytes.len(), |p| start + p);
            out.push_str(&String::from_utf8_lossy(&bytes[start..end]));
            return (end + 3).min(bytes.len());
        }
        // An extended segment: `ESC % / F M L name STX data`, where (M, L)
        // is the length of name, STX and data.
        (b'%', b'/', _) => {
            let (m, l) = (at(i + 3), at(i + 4));
            let len = ((m & 0x7f) as usize) * 128 + (l & 0x7f) as usize;
            let start = (i + 5).min(bytes.len());
            let end = (start + len).min(bytes.len());
            let seg = &bytes[start..end];
            let (name, data) = match seg.iter().position(|&c| c == 0x02) {
                Some(p) => (&seg[..p], &seg[p + 1..]),
                None => (seg, &[][..]),
            };
            let name = String::from_utf8_lossy(name).to_ascii_lowercase();
            if name == "iso10646-1" || name == "utf-8" {
                out.push_str(&String::from_utf8_lossy(data));
            } else if let Some(t) = SETS96.iter().position(|&(_, n)| n == name) {
                for &c in data {
                    out.push(if c < 0x80 {
                        c as char
                    } else {
                        single(Set::Iso8859(t), c)
                    });
                }
            } else {
                out.extend(data.iter().map(|_| '\u{fffd}'));
            }
            return end;
        }
        _ => {}
    }
    // Most sequences are ESC plus two bytes.
    (i + 2).min(bytes.len())
}

#[cfg(test)]
mod tests {
    use super::decode;

    #[test]
    fn ascii_and_latin1_need_no_escapes() {
        assert_eq!(decode(b"abc \xe9\xa9"), "abc é©");
    }

    #[test]
    fn utf8_segments() {
        assert_eq!(decode("\x1b%G中文\x1b%@!".as_bytes()), "中文!");
        // Unterminated, as IBus sends when libX11's conversion fails.
        assert_eq!(decode("\x1b%Gあ".as_bytes()), "あ");
    }

    #[test]
    fn double_byte_sets_in_gl_and_gr() {
        // 中 is 0x4366 in JIS X 0208 and 0x5650 in GB 2312; 한 is 0x4751 in
        // KS C 5601.
        assert_eq!(decode(b"\x1b$(BCf\x1b(B!"), "中!");
        assert_eq!(decode(b"\x1b$)A\xd6\xd0"), "中");
        assert_eq!(decode(b"\x1b$(CGQ"), "한");
        // Hiragana あ (JIS X 0208 0x2422).
        assert_eq!(decode(b"\x1b$(B$\""), "あ");
    }

    #[test]
    fn iso8859_parts_and_kana() {
        // ISO 8859-5 0xd0 is а (U+0430); 8859-7 0xe1 is α.
        assert_eq!(decode(b"\x1b-L\xd0\x1b-F\xe1"), "аα");
        // Back to Latin-1.
        assert_eq!(decode(b"\x1b-L\xd0\x1b-A\xe9"), "аé");
        // Half-width katakana ｱ in JIS X 0201's right half.
        assert_eq!(decode(b"\x1b)I\xb1"), "ｱ");
    }

    #[test]
    fn extended_segments() {
        // ESC % / 1, length 12 = "iso8859-15" (10) + STX + 0xa4 (€).
        assert_eq!(decode(b"\x1b%/1\x80\x8ciso8859-15\x02\xa4"), "€");
        // Unknown names give replacement characters, then decoding goes on.
        assert_eq!(
            decode(b"\x1b%/2\x80\x89big5-0\x02\xa4@x"),
            "\u{fffd}\u{fffd}x"
        );
    }

    #[test]
    fn malformed_input_does_not_panic() {
        for bad in [&b"\x1b"[..], b"\x1b$", b"\x1b$(B\x30", b"\x9b1", b"\x1b%/"] {
            let _ = decode(bad);
        }
    }
}
