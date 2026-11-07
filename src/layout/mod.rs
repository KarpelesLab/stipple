//! Stipple's layout primitives.
//!
//! A self-contained flex/box model (no `taffy`): [`Constraints`] describe the
//! size range a parent offers a child, [`Axis`] picks main vs. cross, and
//! [`solve_main_axis`] distributes space along the main axis among fixed-size
//! and flexible children. `stipple::core` composes these into a full tree layout
//! pass; the algorithms here are deliberately small and independently testable.

#![forbid(unsafe_code)]

use crate::geometry::Size;

/// A layout axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

impl Axis {
    /// The component of `size` along this axis (width for horizontal).
    #[inline]
    pub fn main(self, size: Size) -> f64 {
        match self {
            Axis::Horizontal => size.width,
            Axis::Vertical => size.height,
        }
    }

    /// The component of `size` across this axis.
    #[inline]
    pub fn cross(self, size: Size) -> f64 {
        match self {
            Axis::Horizontal => size.height,
            Axis::Vertical => size.width,
        }
    }

    /// Build a [`Size`] from main/cross extents along this axis.
    #[inline]
    pub fn size(self, main: f64, cross: f64) -> Size {
        match self {
            Axis::Horizontal => Size::new(main, cross),
            Axis::Vertical => Size::new(cross, main),
        }
    }
}

/// The size range a parent offers a child: every laid-out size must satisfy
/// `min <= size <= max` component-wise.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Constraints {
    pub min: Size,
    pub max: Size,
}

impl Constraints {
    /// Loose constraints: anything from zero up to `max`.
    #[inline]
    pub fn loose(max: Size) -> Self {
        Self {
            min: Size::ZERO,
            max,
        }
    }

    /// Tight constraints: exactly `size`.
    #[inline]
    pub fn tight(size: Size) -> Self {
        Self {
            min: size,
            max: size,
        }
    }

    /// Clamp `size` into the allowed range.
    #[inline]
    pub fn constrain(&self, size: Size) -> Size {
        size.clamp(self.min, self.max)
    }

    /// Shrink the available maximum by `amount` on each side's total (e.g. for
    /// padding), keeping `min` no larger than the new `max`.
    pub fn deflate(&self, horizontal: f64, vertical: f64) -> Self {
        let max = Size::new(
            (self.max.width - horizontal).max(0.0),
            (self.max.height - vertical).max(0.0),
        );
        Self {
            min: self.min.clamp(Size::ZERO, max),
            max,
        }
    }
}

/// One participant in a main-axis flex layout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlexItem {
    /// The item's natural size along the main axis, in logical pixels.
    pub basis: f64,
    /// Share of leftover free space this item absorbs. `0.0` = fixed size.
    pub grow: f64,
    /// Share of an overflow this item gives up, weighted by its basis (as in
    /// CSS `flex-shrink`). `0.0` = never shrinks.
    pub shrink: f64,
    /// Smallest main-axis length the item may take (growing or shrinking).
    pub min: f64,
    /// Largest main-axis length the item may take. `f64::INFINITY` = no limit.
    pub max: f64,
}

impl FlexItem {
    /// A fixed-size item that never grows.
    #[inline]
    pub fn fixed(basis: f64) -> Self {
        Self {
            basis,
            grow: 0.0,
            shrink: 0.0,
            min: 0.0,
            max: f64::INFINITY,
        }
    }

    /// A flexible item with the given grow weight and zero basis.
    #[inline]
    pub fn flex(grow: f64) -> Self {
        Self {
            grow,
            ..Self::fixed(0.0)
        }
    }

    /// Set the grow weight.
    #[inline]
    pub fn with_grow(mut self, grow: f64) -> Self {
        self.grow = grow;
        self
    }

    /// Set the shrink weight.
    #[inline]
    pub fn with_shrink(mut self, shrink: f64) -> Self {
        self.shrink = shrink;
        self
    }

    /// Bound the item's length to `min..=max`.
    #[inline]
    pub fn with_bounds(mut self, min: f64, max: f64) -> Self {
        self.min = min;
        self.max = max;
        self
    }

    /// `length` clamped into this item's `min..=max` (see [`clamp_len`]).
    #[inline]
    pub fn clamp(&self, length: f64) -> f64 {
        clamp_len(length, self.min, self.max)
    }
}

/// Clamp `length` to `min..=max`, letting `min` win when they conflict (the CSS
/// rule: a min-width larger than the max-width still applies).
#[inline]
pub fn clamp_len(length: f64, min: f64, max: f64) -> f64 {
    length.min(max).max(min)
}

/// The resolved position of one item along the main axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    /// Offset from the start of the content area.
    pub offset: f64,
    /// Length along the main axis.
    pub length: f64,
}

/// Lay items out along a single axis within `available` main-axis space,
/// separated by `gap`.
///
/// Each item starts at its `basis` clamped to its `min..=max`. Free space (after
/// those sizes and the gaps) is distributed to items in proportion to their
/// `grow` weight; an item that reaches its `max` stops there and the rest is
/// shared among the others. If the items overflow instead, items with a
/// `shrink` weight give up space in proportion to `shrink × basis`, stopping at
/// their `min`. With no growing item, leftover space is left unused (items pack
/// at the start); with no shrinking item, an overflow overflows.
pub fn solve_main_axis(available: f64, gap: f64, items: &[FlexItem]) -> Vec<Span> {
    if items.is_empty() {
        return Vec::new();
    }
    let total_gap = gap * (items.len() as f64 - 1.0);
    let base: Vec<f64> = items.iter().map(|i| i.clamp(i.basis)).collect();
    let free = available - base.iter().sum::<f64>() - total_gap;
    let mut lengths = base.clone();

    if free > 0.0 {
        // Grow: share `free` by weight; freeze items that hit their max and
        // re-share what's left among the rest until nobody overshoots.
        let mut frozen: Vec<bool> = items.iter().map(|i| i.grow <= 0.0).collect();
        loop {
            let taken: f64 = (0..items.len())
                .filter(|&k| frozen[k])
                .map(|k| lengths[k] - base[k])
                .sum();
            let remaining = (free - taken).max(0.0);
            let weight: f64 = (0..items.len())
                .filter(|&k| !frozen[k])
                .map(|k| items[k].grow)
                .sum();
            if weight <= 0.0 {
                break;
            }
            let mut violated = false;
            for k in 0..items.len() {
                if frozen[k] {
                    continue;
                }
                let target = base[k] + remaining * items[k].grow / weight;
                if target > items[k].max {
                    lengths[k] = items[k].max.max(base[k]);
                    frozen[k] = true;
                    violated = true;
                } else {
                    lengths[k] = target;
                }
            }
            if !violated {
                break;
            }
        }
    } else if free < 0.0 {
        // Shrink: give up the overflow weighted by shrink × basis, freezing
        // items that reach their min (never below zero).
        let deficit = -free;
        let mut frozen: Vec<bool> = items
            .iter()
            .zip(&base)
            .map(|(i, b)| i.shrink <= 0.0 || *b <= 0.0)
            .collect();
        loop {
            let given: f64 = (0..items.len())
                .filter(|&k| frozen[k])
                .map(|k| base[k] - lengths[k])
                .sum();
            let remaining = (deficit - given).max(0.0);
            let weight: f64 = (0..items.len())
                .filter(|&k| !frozen[k])
                .map(|k| items[k].shrink * base[k])
                .sum();
            if weight <= 0.0 {
                break;
            }
            let mut violated = false;
            for k in 0..items.len() {
                if frozen[k] {
                    continue;
                }
                let target = base[k] - remaining * items[k].shrink * base[k] / weight;
                let floor = items[k].min.max(0.0);
                if target < floor {
                    lengths[k] = floor.min(base[k]);
                    frozen[k] = true;
                    violated = true;
                } else {
                    lengths[k] = target;
                }
            }
            if !violated {
                break;
            }
        }
    }

    pack(&lengths, gap)
}

/// Spans for `lengths` laid end to end from zero with `gap` between them.
fn pack(lengths: &[f64], gap: f64) -> Vec<Span> {
    let mut spans = Vec::with_capacity(lengths.len());
    let mut cursor = 0.0;
    for &length in lengths {
        spans.push(Span {
            offset: cursor,
            length,
        });
        cursor += length + gap;
    }
    spans
}

/// Break items of the given main-axis `lengths` into lines no longer than
/// `available`, with `gap` between neighbours on a line (flex-wrap). Greedy: an
/// item moves to a new line when it would overflow the current one; an item
/// longer than `available` gets a line of its own. Returns each line's index
/// range. An infinite `available` keeps everything on one line.
pub fn break_lines(available: f64, gap: f64, lengths: &[f64]) -> Vec<std::ops::Range<usize>> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut used = 0.0;
    for (k, &len) in lengths.iter().enumerate() {
        if k == start {
            used = len;
        } else if used + gap + len > available + 1e-9 {
            lines.push(start..k);
            start = k;
            used = len;
        } else {
            used += gap + len;
        }
    }
    if start < lengths.len() {
        lines.push(start..lengths.len());
    }
    lines
}

/// How a grid track (a column or a row) is sized.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Track {
    /// A fixed length in logical pixels.
    Fixed(f64),
    /// A share of the space left after fixed and auto tracks (CSS `fr`). When
    /// the space is indefinite, `fr` tracks size to their content, keeping
    /// their weights proportional.
    Fr(f64),
    /// As large as the largest item in the track.
    Auto,
}

impl Track {
    /// A fixed track `length` logical pixels long.
    #[inline]
    pub fn px(length: f64) -> Self {
        Track::Fixed(length)
    }

    /// A fractional track with weight `weight`.
    #[inline]
    pub fn fr(weight: f64) -> Self {
        Track::Fr(weight)
    }
}

/// An item's footprint on one grid axis, for [`solve_tracks`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackItem {
    /// First track the item occupies.
    pub start: usize,
    /// Number of tracks it spans (at least 1).
    pub span: usize,
    /// The item's natural length along this axis.
    pub size: f64,
}

/// Size the `tracks` of one grid axis and return each track's [`Span`].
///
/// Fixed tracks take their length; auto tracks take the largest item that sits
/// in them alone; items spanning several tracks then grow the auto tracks they
/// cross (shared equally) if those are too small. With `available` space,
/// `fr` tracks share what's left after fixed tracks, auto tracks and gaps
/// (an `fr` track can be squeezed to zero, like CSS `minmax(0, 1fr)`); with
/// `None` (indefinite space) they size to their content in proportion to their
/// weights.
pub fn solve_tracks(
    tracks: &[Track],
    available: Option<f64>,
    gap: f64,
    items: &[TrackItem],
) -> Vec<Span> {
    let n = tracks.len();
    if n == 0 {
        return Vec::new();
    }
    let mut sizes: Vec<f64> = tracks
        .iter()
        .map(|t| match t {
            Track::Fixed(v) => v.max(0.0),
            _ => 0.0,
        })
        .collect();
    // Content each fr track would need, for the indefinite case.
    let mut fr_content = vec![0.0_f64; n];

    for it in items.iter().filter(|it| it.span <= 1) {
        let k = it.start.min(n - 1);
        match tracks[k] {
            Track::Auto => sizes[k] = sizes[k].max(it.size),
            Track::Fr(_) => fr_content[k] = fr_content[k].max(it.size),
            Track::Fixed(_) => {}
        }
    }
    let mut spanning: Vec<&TrackItem> = items.iter().filter(|it| it.span > 1).collect();
    spanning.sort_by_key(|it| it.span);
    for it in spanning {
        let start = it.start.min(n - 1);
        let range = start..(start + it.span).min(n);
        let current: f64 = range.clone().map(|k| sizes[k] + fr_content[k]).sum::<f64>()
            + gap * (range.len() as f64 - 1.0);
        let deficit = it.size - current;
        if deficit <= 0.0 {
            continue;
        }
        let autos: Vec<usize> = range
            .clone()
            .filter(|&k| tracks[k] == Track::Auto)
            .collect();
        if !autos.is_empty() {
            for &k in &autos {
                sizes[k] += deficit / autos.len() as f64;
            }
            continue;
        }
        let frs: Vec<usize> = range
            .filter(|&k| matches!(tracks[k], Track::Fr(_)))
            .collect();
        for &k in &frs {
            fr_content[k] += deficit / frs.len() as f64;
        }
    }

    let weight = |t: &Track| match t {
        Track::Fr(w) => w.max(0.0),
        _ => 0.0,
    };
    let total_weight: f64 = tracks.iter().map(weight).sum();
    if total_weight > 0.0 {
        let unit = match available {
            Some(avail) => {
                let used: f64 = sizes.iter().sum::<f64>() + gap * (n as f64 - 1.0);
                (avail - used).max(0.0) / total_weight
            }
            // The smallest "1fr" that fits every fr track's content.
            None => tracks
                .iter()
                .zip(&fr_content)
                .filter(|(t, _)| weight(t) > 0.0)
                .map(|(t, c)| c / weight(t))
                .fold(0.0_f64, f64::max),
        };
        for (size, t) in sizes.iter_mut().zip(tracks) {
            if matches!(t, Track::Fr(_)) {
                *size = unit * weight(t);
            }
        }
    }

    pack(&sizes, gap)
}

/// The extent of tracks `start..start + span` in `spans`, gaps included, as
/// `(offset, length)`. Spans running past the last track are clipped to it.
pub fn span_extent(spans: &[Span], start: usize, span: usize) -> (f64, f64) {
    if spans.is_empty() {
        return (0.0, 0.0);
    }
    let first = start.min(spans.len() - 1);
    let last = (first + span.max(1) - 1).min(spans.len() - 1);
    let offset = spans[first].offset;
    (offset, spans[last].offset + spans[last].length - offset)
}

/// Place grid items in order, row by row, into `columns` columns (CSS
/// `grid-auto-flow: row`). Each entry of `spans` is an item's
/// `(column_span, row_span)`; column spans wider than the grid are clipped to
/// it. An item goes to the first free position at or after the previous item
/// that fits it (earlier holes are not back-filled). Returns each item's
/// `(row, column)` and the number of rows used.
pub fn place_cells(columns: usize, spans: &[(usize, usize)]) -> (Vec<(usize, usize)>, usize) {
    let columns = columns.max(1);
    let mut occupied: Vec<Vec<bool>> = Vec::new();
    let mut placed = Vec::with_capacity(spans.len());
    let (mut row, mut col) = (0usize, 0usize);
    let mut rows = 0;
    for &(cs, rs) in spans {
        let cs = cs.clamp(1, columns);
        let rs = rs.max(1);
        loop {
            if col + cs > columns {
                row += 1;
                col = 0;
                continue;
            }
            let free = (row..row + rs).all(|r| {
                occupied
                    .get(r)
                    .is_none_or(|line| line[col..col + cs].iter().all(|o| !o))
            });
            if free {
                break;
            }
            col += 1;
        }
        for r in row..row + rs {
            if occupied.len() <= r {
                occupied.resize(r + 1, vec![false; columns]);
            }
            for cell in &mut occupied[r][col..col + cs] {
                *cell = true;
            }
        }
        placed.push((row, col));
        rows = rows.max(row + rs);
        col += cs;
    }
    (placed, rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_items_pack_with_gap() {
        let spans = solve_main_axis(200.0, 10.0, &[FlexItem::fixed(40.0), FlexItem::fixed(60.0)]);
        assert_eq!(
            spans[0],
            Span {
                offset: 0.0,
                length: 40.0
            }
        );
        assert_eq!(
            spans[1],
            Span {
                offset: 50.0,
                length: 60.0
            }
        );
    }

    #[test]
    fn grow_distributes_free_space() {
        // 200 available, 20 gap (×2 = 40), one fixed 40 + two flex(1):
        // free = 200 - 40 - 40 = 120, split 60/60.
        let spans = solve_main_axis(
            200.0,
            20.0,
            &[
                FlexItem::fixed(40.0),
                FlexItem::flex(1.0),
                FlexItem::flex(1.0),
            ],
        );
        assert_eq!(spans[0].length, 40.0);
        assert_eq!(spans[1].length, 60.0);
        assert_eq!(spans[2].length, 60.0);
        // offsets account for the 20px gaps
        assert_eq!(spans[1].offset, 60.0);
        assert_eq!(spans[2].offset, 140.0);
    }

    #[test]
    fn overflow_clamps_free_to_zero() {
        // Basis (40) already exceeds available (30): no free space, grow gets 0.
        let spans = solve_main_axis(30.0, 0.0, &[FlexItem::fixed(40.0), FlexItem::flex(1.0)]);
        assert_eq!(spans[1].length, 0.0);
    }

    #[test]
    fn axis_main_cross_roundtrip() {
        let s = Axis::Vertical.size(10.0, 4.0);
        assert_eq!(s, Size::new(4.0, 10.0));
        assert_eq!(Axis::Vertical.main(s), 10.0);
        assert_eq!(Axis::Vertical.cross(s), 4.0);
    }

    fn lengths(spans: &[Span]) -> Vec<f64> {
        spans.iter().map(|s| s.length).collect()
    }

    fn offsets(spans: &[Span]) -> Vec<f64> {
        spans.iter().map(|s| s.offset).collect()
    }

    #[test]
    fn grow_stops_at_max_and_the_rest_goes_to_others() {
        // 300 free between two flex(1) items; the first caps at 100, so the
        // second takes the remaining 200.
        let spans = solve_main_axis(
            300.0,
            0.0,
            &[
                FlexItem::flex(1.0).with_bounds(0.0, 100.0),
                FlexItem::flex(1.0),
            ],
        );
        assert_eq!(lengths(&spans), vec![100.0, 200.0]);
        assert_eq!(offsets(&spans), vec![0.0, 100.0]);
    }

    #[test]
    fn grow_with_every_item_capped_leaves_space_unused() {
        let spans = solve_main_axis(
            500.0,
            10.0,
            &[
                FlexItem::flex(1.0).with_bounds(0.0, 50.0),
                FlexItem::flex(3.0).with_bounds(0.0, 80.0),
            ],
        );
        assert_eq!(lengths(&spans), vec![50.0, 80.0]);
        assert_eq!(offsets(&spans), vec![0.0, 60.0]);
    }

    #[test]
    fn grow_cascading_caps_redistribute_repeatedly() {
        // 300 among weights 1/1/1 → 100 each; item 0 caps at 40 → 130/130;
        // item 1 caps at 120 → item 2 gets 300 - 40 - 120 = 140.
        let spans = solve_main_axis(
            300.0,
            0.0,
            &[
                FlexItem::flex(1.0).with_bounds(0.0, 40.0),
                FlexItem::flex(1.0).with_bounds(0.0, 120.0),
                FlexItem::flex(1.0),
            ],
        );
        assert_eq!(lengths(&spans), vec![40.0, 120.0, 140.0]);
    }

    #[test]
    fn min_and_max_clamp_the_basis() {
        // A basis below min starts at min; one above max starts at max.
        let spans = solve_main_axis(
            1000.0,
            0.0,
            &[
                FlexItem::fixed(10.0).with_bounds(60.0, f64::INFINITY),
                FlexItem::fixed(500.0).with_bounds(0.0, 200.0),
            ],
        );
        assert_eq!(lengths(&spans), vec![60.0, 200.0]);
        // min wins over a conflicting max.
        assert_eq!(clamp_len(5.0, 50.0, 20.0), 50.0);
    }

    #[test]
    fn grow_item_with_min_keeps_it_when_space_runs_out() {
        // No free space, but the flex item's min still reserves 80.
        let spans = solve_main_axis(
            100.0,
            0.0,
            &[
                FlexItem::fixed(100.0),
                FlexItem::flex(1.0).with_bounds(80.0, f64::INFINITY),
            ],
        );
        assert_eq!(lengths(&spans), vec![100.0, 80.0]);
    }

    #[test]
    fn shrink_absorbs_overflow_weighted_by_basis() {
        // 100 + 300 into 200: overflow 200, shared 1:3 by basis → 50 / 150.
        let spans = solve_main_axis(
            200.0,
            0.0,
            &[
                FlexItem::fixed(100.0).with_shrink(1.0),
                FlexItem::fixed(300.0).with_shrink(1.0),
            ],
        );
        assert_eq!(lengths(&spans), vec![50.0, 150.0]);
    }

    #[test]
    fn shrink_stops_at_min() {
        // Overflow 200; item 0 would shrink to 50 but its min is 90, so it stops
        // there and item 1 gives up the rest: 400 - 200 - (100 - 90) → 110.
        let spans = solve_main_axis(
            200.0,
            0.0,
            &[
                FlexItem::fixed(100.0)
                    .with_shrink(1.0)
                    .with_bounds(90.0, f64::INFINITY),
                FlexItem::fixed(300.0).with_shrink(1.0),
            ],
        );
        assert_eq!(lengths(&spans), vec![90.0, 110.0]);
    }

    #[test]
    fn non_shrinking_items_overflow_and_shrink_never_goes_negative() {
        let spans = solve_main_axis(
            50.0,
            0.0,
            &[
                FlexItem::fixed(100.0),
                FlexItem::fixed(20.0).with_shrink(1.0),
            ],
        );
        assert_eq!(lengths(&spans), vec![100.0, 0.0]);
    }

    #[test]
    fn shrink_accounts_for_gaps() {
        // 2×100 + 20 gap into 120: overflow 100, 50 each.
        let spans = solve_main_axis(
            120.0,
            20.0,
            &[
                FlexItem::fixed(100.0).with_shrink(1.0),
                FlexItem::fixed(100.0).with_shrink(1.0),
            ],
        );
        assert_eq!(lengths(&spans), vec![50.0, 50.0]);
        assert_eq!(offsets(&spans), vec![0.0, 70.0]);
    }

    #[test]
    fn break_lines_flows_items_with_gap() {
        // 100 wide, gap 10: [40, 40] fits (90), the third 40 would need 140.
        assert_eq!(
            break_lines(100.0, 10.0, &[40.0, 40.0, 40.0, 40.0, 40.0]),
            vec![0..2, 2..4, 4..5]
        );
        // An exact fit stays on the line.
        assert_eq!(break_lines(90.0, 10.0, &[40.0, 40.0]), vec![0..2]);
    }

    #[test]
    fn break_lines_gives_oversized_items_their_own_line() {
        assert_eq!(
            break_lines(100.0, 0.0, &[30.0, 150.0, 30.0, 30.0]),
            vec![0..1, 1..2, 2..4]
        );
        assert_eq!(break_lines(f64::INFINITY, 5.0, &[1e6, 1e6]), vec![0..2]);
        assert!(break_lines(100.0, 0.0, &[]).is_empty());
    }

    fn item(start: usize, span: usize, size: f64) -> TrackItem {
        TrackItem { start, span, size }
    }

    #[test]
    fn tracks_fixed_auto_fr_mix() {
        // 400 wide, gap 10: fixed 100, auto = widest item (60), fr shares the
        // rest 1:3 → 400 - 100 - 60 - 3×10 = 210 → 52.5 / 157.5.
        let tracks = [
            Track::px(100.0),
            Track::Auto,
            Track::fr(1.0),
            Track::fr(3.0),
        ];
        let spans = solve_tracks(
            &tracks,
            Some(400.0),
            10.0,
            &[item(1, 1, 40.0), item(1, 1, 60.0), item(2, 1, 500.0)],
        );
        assert_eq!(lengths(&spans), vec![100.0, 60.0, 52.5, 157.5]);
        assert_eq!(offsets(&spans), vec![0.0, 110.0, 180.0, 242.5]);
    }

    #[test]
    fn tracks_fr_squeezes_to_zero_when_space_is_short() {
        let spans = solve_tracks(&[Track::px(100.0), Track::fr(1.0)], Some(80.0), 0.0, &[]);
        assert_eq!(lengths(&spans), vec![100.0, 0.0]);
    }

    #[test]
    fn tracks_fr_indefinite_sizes_to_content_proportionally() {
        // 1fr holding 30 and 2fr holding 40: the unit must fit both → max(30/1,
        // 40/2) = 30, so 30 / 60.
        let spans = solve_tracks(
            &[Track::fr(1.0), Track::fr(2.0), Track::Auto],
            None,
            5.0,
            &[item(0, 1, 30.0), item(1, 1, 40.0), item(2, 1, 12.0)],
        );
        assert_eq!(lengths(&spans), vec![30.0, 60.0, 12.0]);
        assert_eq!(offsets(&spans), vec![0.0, 35.0, 100.0]);
    }

    #[test]
    fn tracks_spanning_item_grows_auto_tracks() {
        // Two auto columns of 20 and 30 with gap 10 = 60; an item spanning both
        // needs 100 → 40 more, shared equally → 40 / 50.
        let spans = solve_tracks(
            &[Track::Auto, Track::Auto],
            None,
            10.0,
            &[item(0, 1, 20.0), item(1, 1, 30.0), item(0, 2, 100.0)],
        );
        assert_eq!(lengths(&spans), vec![40.0, 50.0]);
        // A spanning item that already fits changes nothing.
        let spans = solve_tracks(
            &[Track::Auto, Track::px(50.0)],
            None,
            0.0,
            &[item(0, 1, 20.0), item(0, 2, 60.0)],
        );
        assert_eq!(lengths(&spans), vec![20.0, 50.0]);
    }

    #[test]
    fn span_extent_includes_gaps() {
        let spans = solve_tracks(&[Track::px(10.0); 3], None, 5.0, &[]);
        assert_eq!(span_extent(&spans, 0, 3), (0.0, 40.0));
        assert_eq!(span_extent(&spans, 1, 1), (15.0, 10.0));
        // Clipped to the last track.
        assert_eq!(span_extent(&spans, 2, 4), (30.0, 10.0));
    }

    #[test]
    fn place_cells_flows_row_major() {
        let (cells, rows) = place_cells(3, &[(1, 1); 7]);
        assert_eq!(
            cells,
            vec![(0, 0), (0, 1), (0, 2), (1, 0), (1, 1), (1, 2), (2, 0)]
        );
        assert_eq!(rows, 3);
    }

    #[test]
    fn place_cells_honors_spans() {
        // A 2-wide item that doesn't fit at column 2 wraps; a 2-tall item blocks
        // the cell below it.
        let (cells, rows) = place_cells(3, &[(1, 2), (1, 1), (2, 1), (1, 1), (5, 1)]);
        assert_eq!(cells, vec![(0, 0), (0, 1), (1, 1), (2, 0), (3, 0)]);
        assert_eq!(rows, 4);
    }
}
