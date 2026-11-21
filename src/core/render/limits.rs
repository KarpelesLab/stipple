//! How small and how large an element tree can be laid out without its
//! content overflowing: the window's size limits by default (see
//! `docs/layout.md`, *Window size limits*).
//!
//! This reuses [`measure`] and the stack solver rather than running a
//! layout of its own: the smallest width adds up the tree's (a row's
//! children end to end, a column's widest, wrapped text's longest word, a
//! scroll container's floor, a child that may shrink at its own minimum),
//! and the smallest height is measured at a given width, with no room down
//! (so scroll containers shrink to their floors).

use super::{
    clamp_len, decided_height, decided_width, flex_item, max_size, measure, min_size, scroll_floor,
    settle_row_aspect,
};
use crate::core::element::{Align, Element, ElementKind};
use crate::geometry::Size;
use crate::layout::{Axis, Track, solve_main_axis};
use crate::render::Font;

/// The smallest and largest size `el` fits in, laid out `width` wide (the
/// smallest height depends on the width when text wraps): `min` is the
/// smallest size at which nothing overflows, `max` the largest the content
/// has a use for, infinite on an axis where something can take any length.
pub(crate) fn content_limits(el: &Element, width: f64, font: Option<&Font>) -> (Size, Size) {
    let max = Size::new(
        max_extent(el, Axis::Horizontal),
        max_extent(el, Axis::Vertical),
    );
    let min_w = min_width(el, font);
    let width = clamp_len(width, min_w, max.width);
    let min = Size::new(min_w, min_height(el, width, font));
    (
        min,
        Size::new(max.width.max(min.width), max.height.max(min.height)),
    )
}

/// The size `el` would like to have with no limit on its room: its
/// unwrapped width, and the height at that width. A scroll container counts
/// at its content's full length.
pub(crate) fn natural_size(el: &Element, font: Option<&Font>) -> Size {
    let width = measure(el, Size::new(f64::INFINITY, f64::INFINITY), font).width;
    let height = measure(el, Size::new(width, f64::INFINITY), font).height;
    Size::new(width, height)
}

/// All the room there is: what natural sizes are measured in.
const UNBOUNDED: Size = Size::new(f64::INFINITY, f64::INFINITY);

/// What child `c` of a stack along `axis` takes along it at the least,
/// measured in `room`: a child that may shrink, its floor; any other, its
/// natural length (the stack gives it no less).
fn main_min(c: &Element, axis: Axis, room: Size, font: Option<&Font>) -> f64 {
    let item = flex_item(c, axis, measure(c, room, font));
    let base = item.clamp(item.basis);
    if item.shrink > 0.0 {
        item.min.max(0.0).min(base)
    } else {
        base
    }
}

/// Whether `el` scrolls along `axis`, so that its content's length there
/// doesn't count.
fn scrolls_along(el: &Element, axis: Axis) -> bool {
    let stack_axis = match el.kind {
        ElementKind::Stack { axis, .. } => axis,
        _ => Axis::Vertical,
    };
    match axis {
        Axis::Horizontal => {
            el.scroll_x.is_some() || (el.scroll.is_some() && stack_axis == Axis::Horizontal)
        }
        Axis::Vertical => el.scroll.is_some() && stack_axis == Axis::Vertical,
    }
}

/// The narrowest `el` can be without its content overflowing.
///
/// Worked out from the tree rather than measured in no width at all, which
/// would lay every paragraph out a word (or a glyph) per line, and text
/// flowing around a box that can't fit beside it nowhere.
fn min_width(el: &Element, font: Option<&Font>) -> f64 {
    let (lo, hi) = (min_size(el).width, max_size(el).width);
    // A fixed width (or one following a fixed height) is what it is: the
    // content measures in it, as laid out.
    if decided_width(el).is_some() {
        return measure(el, Size::new(0.0, f64::INFINITY), font).width;
    }
    let pad = el.layout.padding.horizontal();
    if el.scroll.is_some() || el.scroll_x.is_some() {
        // A scroll container counts at its own minimum, not its content's
        // (which it clips): a strip that scrolls sideways at its floor, a
        // list at its `min_width`.
        let floor = match el.kind {
            ElementKind::Stack {
                axis: Axis::Horizontal,
                ..
            } if el.scroll.is_some() => {
                scroll_floor(el, Axis::Horizontal, measure(el, UNBOUNDED, font).width)
            }
            _ => lo,
        };
        return clamp_len(floor, lo, hi);
    }
    // The aspect ratio's height follows whatever width it gets.
    if el.layout.aspect.is_some() {
        return lo;
    }
    let content = match &el.kind {
        ElementKind::Stack {
            axis: Axis::Horizontal,
            gap,
            children,
            ..
        } => {
            // A child that shrinks gives up all but its minimum; any other
            // keeps what it needs.
            let mains = children.iter().map(|c| {
                if c.layout.shrink > 0.0 {
                    min_size(c).width
                } else {
                    min_width(c, font)
                }
            });
            if el.wrap {
                // Each child may get a line of its own.
                mains.fold(0.0, f64::max)
            } else {
                let n = children.len() as f64;
                mains.sum::<f64>() + gap * (n - 1.0).max(0.0)
            }
        }
        ElementKind::Stack {
            axis: Axis::Vertical,
            children,
            ..
        } => children
            .iter()
            .map(|c| min_width(c, font))
            .fold(0.0, f64::max),
        // Layers, and text flowing around boxes: the widest of them.
        ElementKind::Layers { children, .. } => children
            .iter()
            .map(|c| min_width(c, font))
            .fold(0.0, f64::max),
        ElementKind::Grid {
            columns,
            column_gap,
            children,
            ..
        } => grid_min_width(columns, *column_gap, children, font),
        ElementKind::Text {
            text, size, style, ..
        } => {
            if style.ellipsis().is_some() {
                // Cut to whatever room there is.
                0.0
            } else if el.wrap || style.flow.is_some() {
                // Broken between words (text that flows around images
                // always is, the images sized to its width): as wide as its
                // longest.
                match style.resolve(font) {
                    Some(f) => text
                        .split_whitespace()
                        .map(|word| f.measure(word, *size).width)
                        .fold(0.0, f64::max),
                    None => 0.0,
                }
            } else {
                return measure(el, UNBOUNDED, font).width;
            }
        }
        // An image, a box, a viewport or a canvas: its own size.
        _ => return measure(el, UNBOUNDED, font).width,
    };
    clamp_len(content + pad, lo, hi)
}

/// The narrowest a grid with `columns` can be: its pixel columns, its
/// auto columns as wide as the narrowest their children get (children
/// spanning several columns aside), its `fr` columns nothing, and the gaps.
fn grid_min_width(columns: &[Track], gap: f64, children: &[Element], font: Option<&Font>) -> f64 {
    let auto = [Track::Auto];
    let columns = if columns.is_empty() {
        &auto[..]
    } else {
        columns
    };
    let ncols = columns.len();
    let spans: Vec<(usize, usize)> = children
        .iter()
        .map(|c| {
            (
                (c.layout.column_span.max(1) as usize).min(ncols),
                c.layout.row_span.max(1) as usize,
            )
        })
        .collect();
    let (placed, _) = crate::layout::place_cells(ncols, &spans);
    let mut widths: Vec<f64> = columns
        .iter()
        .map(|t| match t {
            Track::Fixed(v) => *v,
            _ => 0.0,
        })
        .collect();
    for ((c, &(cs, _)), &(_, col)) in children.iter().zip(&spans).zip(&placed) {
        if cs == 1 && matches!(columns[col], Track::Auto) {
            widths[col] = widths[col].max(min_width(c, font));
        }
    }
    widths.iter().sum::<f64>() + gap * (ncols as f64 - 1.0).max(0.0)
}

/// The shortest `el` can be, `width` wide, without its content
/// overflowing.
fn min_height(el: &Element, width: f64, font: Option<&Font>) -> f64 {
    let short = Size::new(width, 0.0);
    if decided_height(el).is_some() || el.scroll.is_some() || el.scroll_x.is_some() || el.wrap {
        return measure(el, short, font).height;
    }
    let (lo, hi) = (min_size(el), max_size(el));
    let own = clamp_len(el.layout.size.width.unwrap_or(width), lo.width, hi.width);
    let pad = el.layout.padding;
    let inner = (own - pad.horizontal()).max(0.0);
    let content = match &el.kind {
        ElementKind::Stack {
            axis: Axis::Vertical,
            gap,
            children,
            ..
        } => {
            let room = Size::new(inner, 0.0);
            let n = children.len() as f64;
            children
                .iter()
                .map(|c| main_min(c, Axis::Vertical, room, font))
                .sum::<f64>()
                + gap * (n - 1.0).max(0.0)
        }
        ElementKind::Stack {
            axis: Axis::Horizontal,
            gap,
            children,
            ..
        } => {
            // The widths the row gives its children at this width, and the
            // height each needs at its own.
            let room = Size::new(inner, 0.0);
            let mut sizes: Vec<Size> = children.iter().map(|c| measure(c, room, font)).collect();
            let aspect = settle_row_aspect(el, Axis::Horizontal, *gap, inner, children, &mut sizes);
            let items: Vec<_> = children
                .iter()
                .zip(&sizes)
                .map(|(c, m)| flex_item(c, Axis::Horizontal, *m))
                .collect();
            let spans = solve_main_axis(inner, *gap, &items);
            children
                .iter()
                .zip(&spans)
                .zip(sizes.iter().zip(&aspect))
                .map(|((c, span), (m, &settled))| {
                    if settled {
                        m.height
                    } else {
                        min_height(c, span.length, font)
                    }
                })
                .fold(0.0, f64::max)
        }
        ElementKind::Layers {
            children,
            flow: None,
            ..
        } => children
            .iter()
            .map(|c| min_height(c, inner, font))
            .fold(0.0, f64::max),
        _ => return measure(el, short, font).height,
    };
    clamp_len(content + pad.vertical(), lo.height, hi.height)
}

/// The longest `el` has a use for along `axis`: finite only where its
/// whole extent there is fixed (a `.width(..)` or `.max_width(..)`, and
/// so on), down to every child, nothing along it grows, and nothing is
/// centered or placed at the end (which takes any room around it). Text,
/// images and unsized boxes (an empty stack, a spacer) take any length.
fn max_extent(el: &Element, axis: Axis) -> f64 {
    let (lo, hi) = (axis.main(min_size(el)), axis.main(max_size(el)));
    let fixed = match axis {
        Axis::Horizontal => el.layout.size.width.filter(|_| !el.layout.soft_width),
        Axis::Vertical => el.layout.size.height,
    };
    if let Some(length) = fixed {
        return clamp_len(length, lo, hi);
    }
    let pad = el.layout.padding;
    let pad = match axis {
        Axis::Horizontal => pad.horizontal(),
        Axis::Vertical => pad.vertical(),
    };
    let content = if scrolls_along(el, axis) {
        f64::INFINITY
    } else {
        match &el.kind {
            ElementKind::Stack {
                axis: along,
                gap,
                main_align,
                cross_align,
                children,
            } if !el.wrap && !children.is_empty() => {
                if *along == axis {
                    // Children placed at the center or the end use any
                    // room there is around them.
                    if placed_in_room(*main_align) {
                        return clamp_len(f64::INFINITY, lo, hi);
                    }
                    let n = children.len() as f64;
                    children
                        .iter()
                        .map(|c| {
                            // A growing child takes the room there is, up
                            // to its maximum.
                            let grows = c.layout.grow > 0.0
                                || (c.scroll.is_some() && scrolls_along(c, axis));
                            if grows {
                                axis.main(max_size(c))
                            } else {
                                max_extent(c, axis)
                            }
                        })
                        .sum::<f64>()
                        + gap * (n - 1.0).max(0.0)
                } else {
                    // A spacer has no use for any room across (but a
                    // stack of nothing else takes any), a child centered
                    // or at the end any.
                    children
                        .iter()
                        .filter(|c| !is_spacer(c))
                        .map(|c| {
                            if placed_in_room(c.layout.align_self.unwrap_or(*cross_align)) {
                                f64::INFINITY
                            } else {
                                max_extent(c, axis)
                            }
                        })
                        .reduce(f64::max)
                        .unwrap_or(f64::INFINITY)
                }
            }
            ElementKind::Layers {
                children, align, ..
            } if !children.is_empty() => {
                let align = match axis {
                    Axis::Horizontal => align.0,
                    Axis::Vertical => align.1,
                };
                if placed_in_room(align) {
                    f64::INFINITY
                } else {
                    children
                        .iter()
                        .map(|c| max_extent(c, axis))
                        .fold(0.0, f64::max)
                }
            }
            ElementKind::Grid {
                columns,
                rows,
                column_gap,
                row_gap,
                children,
                ..
            } if axis == Axis::Horizontal || rows.len() >= grid_rows(columns.len(), children) => {
                let (tracks, gap) = match axis {
                    Axis::Horizontal => (columns, column_gap),
                    Axis::Vertical => (rows, row_gap),
                };
                // Only pixel tracks are fixed.
                let mut total = 0.0;
                for track in tracks {
                    match track {
                        Track::Fixed(v) => total += v,
                        _ => return f64::INFINITY,
                    }
                }
                let n = tracks.len() as f64;
                if n == 0.0 {
                    f64::INFINITY
                } else {
                    total + gap * (n - 1.0)
                }
            }
            _ => f64::INFINITY,
        }
    };
    clamp_len(content + pad, lo, hi)
}

/// Whether children aligned so are placed within the room they're given
/// (centered, or at the end), rather than packed at the start or
/// stretched: a window centering its content has a use for any size.
fn placed_in_room(align: Align) -> bool {
    matches!(align, Align::Center | Align::End)
}

/// Whether `el` is an empty, undecorated stack: a
/// [`spacer`](crate::widgets::spacer) or a gap.
fn is_spacer(el: &Element) -> bool {
    matches!(&el.kind, ElementKind::Stack { children, .. } if children.is_empty())
        && el.decoration == crate::core::element::BoxStyle::default()
}

/// How many rows a grid of `columns` columns at least has for `children`
/// (more, where children span several cells).
fn grid_rows(columns: usize, children: &[Element]) -> usize {
    let cells: usize = children
        .iter()
        .map(|c| (c.layout.column_span.max(1) as usize) * (c.layout.row_span.max(1) as usize))
        .sum();
    cells.div_ceil(columns.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::element::BoxStyle;
    use crate::geometry::Insets;

    fn fixed(w: f64, h: f64) -> Element {
        Element::boxed(BoxStyle::default()).width(w).height(h)
    }

    #[test]
    fn a_column_of_fixed_widths_is_as_narrow_as_its_widest() {
        let col = Element::stack(
            Axis::Vertical,
            vec![fixed(120.0, 20.0), fixed(200.0, 30.0), fixed(80.0, 10.0)],
        )
        .gap(4.0)
        .padding(Insets::uniform(10.0));
        let (min, max) = content_limits(&col, 50.0, None);
        assert_eq!(min, Size::new(220.0, 20.0 + 30.0 + 10.0 + 8.0 + 20.0));
        // Every child fixed, nothing grows: no use for more height. The
        // column stretches its children across, but they are fixed.
        assert_eq!(max, Size::new(220.0, 88.0));
    }

    #[test]
    fn a_row_sums_its_children_and_a_shrinking_one_counts_at_its_minimum() {
        let row = Element::stack(
            Axis::Horizontal,
            vec![
                fixed(100.0, 20.0),
                fixed(300.0, 20.0).shrink(1.0).min_width(50.0),
                fixed(40.0, 20.0),
            ],
        )
        .gap(10.0);
        // The shrinking child has a fixed width, but gives it up down to
        // its minimum.
        let (min, _) = content_limits(&row, 0.0, None);
        assert_eq!(min.width, 100.0 + 50.0 + 40.0 + 20.0);
    }

    #[test]
    fn a_growing_child_or_an_unsized_box_lifts_the_maximum() {
        let col = Element::stack(
            Axis::Vertical,
            vec![fixed(100.0, 20.0), crate::widgets::spacer()],
        );
        let (min, max) = content_limits(&col, 0.0, None);
        assert_eq!(min, Size::new(100.0, 20.0));
        assert_eq!(max.height, f64::INFINITY);
        // The spacer has no use for any width.
        assert_eq!(max.width, 100.0);
        let capped = Element::stack(
            Axis::Vertical,
            vec![
                fixed(100.0, 20.0),
                crate::widgets::spacer().max_height(50.0).width(10.0),
            ],
        );
        assert_eq!(content_limits(&capped, 0.0, None).1, Size::new(100.0, 70.0));
    }

    #[test]
    fn a_scroll_container_counts_at_its_floor_not_its_content() {
        let rows: Vec<Element> = (0..50).map(|_| fixed(100.0, 30.0)).collect();
        let list = Element::stack(Axis::Vertical, rows).scrollable(crate::core::ScrollId(1));
        let col = Element::stack(Axis::Vertical, vec![fixed(100.0, 40.0), list]);
        let (min, max) = content_limits(&col, 200.0, None);
        assert_eq!(min.height, 40.0 + 100.0);
        assert_eq!(max.height, f64::INFINITY);
    }

    #[test]
    fn bounds_on_the_root_apply() {
        let el = crate::widgets::spacer()
            .min_width(300.0)
            .min_height(200.0)
            .max_width(500.0);
        let (min, max) = content_limits(&el, 0.0, None);
        assert_eq!(min, Size::new(300.0, 200.0));
        assert_eq!(max, Size::new(500.0, f64::INFINITY));
    }

    #[test]
    fn wrapped_text_is_as_narrow_as_its_longest_word_and_taller_for_it() {
        let Some(font) = Font::system_default() else {
            return; // no system font on this host
        };
        let text = "several words that wrap";
        let para = Element::text(text, 14.0, crate::render::Color::rgb(0, 0, 0)).wrap();
        let one_line = measure(&para, Size::new(f64::INFINITY, f64::INFINITY), Some(&font));
        let (narrow, _) = content_limits(&para, 0.0, Some(&font));
        assert!(narrow.width < one_line.width / 2.0);
        assert!(narrow.height > one_line.height);
        // Wide enough for one line, one line tall.
        let (wide, _) = content_limits(&para, one_line.width + 1.0, Some(&font));
        assert_eq!(wide.height, one_line.height);
        // Unwrapped text can't get narrower than its line.
        let line = Element::text(text, 14.0, crate::render::Color::rgb(0, 0, 0));
        assert_eq!(
            content_limits(&line, 0.0, Some(&font)).0.width,
            one_line.width
        );
    }

    #[test]
    fn the_natural_size_is_the_unwrapped_content() {
        let col = Element::stack(Axis::Vertical, vec![fixed(120.0, 20.0), fixed(60.0, 30.0)]);
        assert_eq!(natural_size(&col, None), Size::new(120.0, 50.0));
    }
}
