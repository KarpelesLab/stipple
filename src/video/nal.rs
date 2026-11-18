//! H.264 and HEVC framing: the containers' configuration records (`avcC`,
//! `hvcC`) and their length-prefixed NAL units.

/// The parameter sets and NAL length size of an `avcC` record.
#[cfg_attr(
    not(any(target_os = "macos", target_os = "ios", target_os = "linux")),
    allow(dead_code)
)]
pub(super) fn parse_avcc(r: &[u8]) -> Option<(Vec<Vec<u8>>, i32)> {
    if r.len() < 7 || r[0] != 1 {
        return None;
    }
    let len_size = i32::from(r[4] & 3) + 1;
    let mut sets = Vec::new();
    let mut at = 5;
    for mask in [0x1f, 0xff] {
        let n = usize::from(*r.get(at)? & mask);
        at += 1;
        for _ in 0..n {
            let l = usize::from(u16::from_be_bytes([*r.get(at)?, *r.get(at + 1)?]));
            sets.push(r.get(at + 2..at + 2 + l)?.to_vec());
            at += 2 + l;
        }
    }
    Some((sets, len_size))
}

/// The parameter sets (VPS, SPS, PPS) and NAL length size of an `hvcC`
/// record.
#[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
pub(super) fn parse_hvcc(r: &[u8]) -> Option<(Vec<Vec<u8>>, i32)> {
    if r.len() < 23 {
        return None;
    }
    let len_size = i32::from(r[21] & 3) + 1;
    let arrays = usize::from(r[22]);
    let mut sets = Vec::new();
    let mut at = 23;
    for _ in 0..arrays {
        let kind = *r.get(at)? & 0x3f;
        let n = usize::from(u16::from_be_bytes([*r.get(at + 1)?, *r.get(at + 2)?]));
        at += 3;
        for _ in 0..n {
            let l = usize::from(u16::from_be_bytes([*r.get(at)?, *r.get(at + 1)?]));
            let nal = r.get(at + 2..at + 2 + l)?;
            // VPS, SPS, PPS (32, 33, 34); SEI and the rest aren't format.
            if (32..=34).contains(&kind) {
                sets.push(nal.to_vec());
            }
            at += 2 + l;
        }
    }
    Some((sets, len_size))
}

/// The NAL units of a packet whose units are each preceded by their
/// length in `len_size` bytes (big-endian); a truncated last one is
/// dropped.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) fn split_units(data: &[u8], len_size: usize) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + len_size <= data.len() {
        let len = data[at..at + len_size]
            .iter()
            .fold(0usize, |n, &b| (n << 8) | usize::from(b));
        at += len_size;
        let Some(unit) = data.get(at..at + len) else {
            break;
        };
        out.push(unit);
        at += len;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_records_give_their_parameter_sets() {
        // avcC: version 1, profile, compat, level, 4-byte lengths, one SPS
        // of 3 bytes, one PPS of 2.
        let avcc = [
            1, 0x4d, 0, 0x1e, 0xff, 0xe1, 0, 3, 0x67, 1, 2, 1, 0, 2, 0x68, 9,
        ];
        let (sets, len) = parse_avcc(&avcc).unwrap();
        assert_eq!(len, 4);
        assert_eq!(sets, vec![vec![0x67, 1, 2], vec![0x68, 9]]);
        assert!(parse_avcc(&avcc[..9]).is_none());

        let mut hvcc = vec![1u8; 21];
        hvcc.push(0xf3); // 4-byte lengths
        hvcc.push(2); // two arrays
        hvcc.extend_from_slice(&[0x20, 0, 1, 0, 2, 0x40, 1]); // VPS
        hvcc.extend_from_slice(&[0x27, 0, 1, 0, 1, 0x4e]); // SEI: skipped
        let (sets, len) = parse_hvcc(&hvcc).unwrap();
        assert_eq!(len, 4);
        assert_eq!(sets, vec![vec![0x40, 1]]);
    }

    #[test]
    fn length_prefixed_units_split() {
        let data = [0, 0, 0, 2, 0x65, 1, 0, 0, 0, 1, 0x41, 0, 0, 0, 9, 1];
        assert_eq!(split_units(&data, 4), vec![&[0x65, 1][..], &[0x41][..]]);
        assert_eq!(split_units(&[0, 1, 7], 2), vec![&[7][..]]);
    }
}
