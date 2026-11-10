//! Pure window-geometry helpers for the Win32 backend: snapping a resize to
//! whole steps (`WM_SIZING`), and the UTF-16 conversions used for titles.

/// `WM_SIZING`'s `wParam`: which edge or corner is being dragged.
pub(super) const WMSZ_LEFT: u32 = 1;
pub(super) const WMSZ_RIGHT: u32 = 2;
pub(super) const WMSZ_TOP: u32 = 3;
pub(super) const WMSZ_TOPLEFT: u32 = 4;
pub(super) const WMSZ_TOPRIGHT: u32 = 5;
pub(super) const WMSZ_BOTTOM: u32 = 6;
pub(super) const WMSZ_BOTTOMLEFT: u32 = 7;
pub(super) const WMSZ_BOTTOMRIGHT: u32 = 8;

/// The largest `base + k * step` (k ≥ 0) that is at most `size`, or `size`
/// itself when it is below `base` or the step is empty.
pub(super) fn snap(size: i32, base: i32, step: i32) -> i32 {
    if step <= 0 || size <= base {
        return size;
    }
    base + (size - base) / step * step
}

/// Snap a window rectangle (`[left, top, right, bottom]`, physical pixels)
/// being dragged by `edge`, so the client area is a whole number of `step`s
/// above `base` on each axis being dragged. `frame` is the window size minus
/// the client size. The edge opposite the dragged one stays put.
pub(super) fn snap_sizing(
    edge: u32,
    rect: [i32; 4],
    frame: (i32, i32),
    base: (i32, i32),
    step: (i32, i32),
) -> [i32; 4] {
    let [mut left, mut top, mut right, mut bottom] = rect;
    let horizontal = matches!(
        edge,
        WMSZ_LEFT | WMSZ_RIGHT | WMSZ_TOPLEFT | WMSZ_TOPRIGHT | WMSZ_BOTTOMLEFT | WMSZ_BOTTOMRIGHT
    );
    let vertical = matches!(
        edge,
        WMSZ_TOP | WMSZ_BOTTOM | WMSZ_TOPLEFT | WMSZ_TOPRIGHT | WMSZ_BOTTOMLEFT | WMSZ_BOTTOMRIGHT
    );
    if horizontal {
        let client = snap(right - left - frame.0, base.0, step.0);
        let width = client + frame.0;
        if matches!(edge, WMSZ_LEFT | WMSZ_TOPLEFT | WMSZ_BOTTOMLEFT) {
            left = right - width;
        } else {
            right = left + width;
        }
    }
    if vertical {
        let client = snap(bottom - top - frame.1, base.1, step.1);
        let height = client + frame.1;
        if matches!(edge, WMSZ_TOP | WMSZ_TOPLEFT | WMSZ_TOPRIGHT) {
            top = bottom - height;
        } else {
            bottom = top + height;
        }
    }
    [left, top, right, bottom]
}

/// `s` as NUL-terminated UTF-16.
pub(super) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// UTF-16 text read back from the system (up to its first NUL), lossily.
pub(super) fn from_wide(units: &[u16]) -> String {
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_rounds_down_to_whole_steps_above_the_base() {
        assert_eq!(snap(107, 20, 10), 100);
        assert_eq!(snap(100, 20, 10), 100);
        assert_eq!(snap(15, 20, 10), 15, "below the base is left alone");
        assert_eq!(snap(107, 0, 0), 107, "no step");
    }

    #[test]
    fn dragging_the_bottom_right_moves_only_those_edges() {
        // Frame 16x39, client 803x605 -> snapped to 800x600 in 8x12 cells.
        let r = snap_sizing(
            WMSZ_BOTTOMRIGHT,
            [100, 50, 100 + 803 + 16, 50 + 605 + 39],
            (16, 39),
            (0, 0),
            (8, 12),
        );
        assert_eq!(r, [100, 50, 100 + 800 + 16, 50 + 600 + 39]);
    }

    #[test]
    fn dragging_the_top_left_keeps_the_bottom_right() {
        let r = snap_sizing(
            WMSZ_TOPLEFT,
            [95, 45, 1000, 800],
            (10, 30),
            (0, 0),
            (10, 10),
        );
        // Client 895x725 -> 890x720.
        assert_eq!(r, [1000 - 900, 800 - 750, 1000, 800]);
    }

    #[test]
    fn a_side_edge_snaps_only_its_axis() {
        let r = snap_sizing(WMSZ_LEFT, [0, 0, 105, 77], (0, 0), (0, 0), (10, 10));
        assert_eq!(r, [5, 0, 105, 77]);
        let r = snap_sizing(WMSZ_BOTTOM, [0, 0, 105, 77], (0, 0), (0, 0), (10, 10));
        assert_eq!(r, [0, 0, 105, 70]);
    }

    #[test]
    fn utf16_round_trips_titles() {
        let w = wide("Ünïcødé — 終端 🦀");
        assert_eq!(w.last(), Some(&0));
        assert_eq!(from_wide(&w), "Ünïcødé — 終端 🦀");
        assert_eq!(from_wide(&[0x41, 0, 0x42]), "A");
    }
}
