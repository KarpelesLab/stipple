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
}

impl FlexItem {
    /// A fixed-size item that never grows.
    #[inline]
    pub fn fixed(basis: f64) -> Self {
        Self { basis, grow: 0.0 }
    }

    /// A flexible item with the given grow weight and zero basis.
    #[inline]
    pub fn flex(grow: f64) -> Self {
        Self { basis: 0.0, grow }
    }
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
/// separated by `gap`. Free space (after bases and gaps) is distributed to
/// items in proportion to their `grow` weight; if no item grows, leftover
/// space is left unused (items pack at the start).
pub fn solve_main_axis(available: f64, gap: f64, items: &[FlexItem]) -> Vec<Span> {
    if items.is_empty() {
        return Vec::new();
    }
    let total_gap = gap * (items.len() as f64 - 1.0);
    let total_basis: f64 = items.iter().map(|i| i.basis).sum();
    let total_grow: f64 = items.iter().map(|i| i.grow).sum();
    let free = (available - total_basis - total_gap).max(0.0);

    let mut spans = Vec::with_capacity(items.len());
    let mut cursor = 0.0;
    for item in items {
        let extra = if total_grow > 0.0 {
            free * (item.grow / total_grow)
        } else {
            0.0
        };
        let length = item.basis + extra;
        spans.push(Span {
            offset: cursor,
            length,
        });
        cursor += length + gap;
    }
    spans
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
}
