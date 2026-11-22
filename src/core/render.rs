//! Layout and paint passes: turn an [`Element`] tree into a retained
//! [`LayoutNode`] tree, then paint that tree into a [`Scene`].
//!
//! Three phases:
//! 1. **measure** — bottom-up natural sizing (content + padding, honoring fixed
//!    overrides; text measures via the active [`Font`]).
//! 2. **layout** — top-down assignment of absolute bounds, using
//!    [`crate::layout::solve_main_axis`] to distribute the main axis and
//!    [`Align`] to position children on the cross axis (per line, for wrapping
//!    stacks), or [`crate::layout::solve_tracks`] for grids. Produces a
//!    [`LayoutNode`] tree that survives the frame so pointer events can be
//!    routed against it (see [`crate::core::hit_test`]).
//! 3. **paint** — walk the layout tree, drawing each node's decoration and text.

use crate::core::element::{Align, BoxStyle, Element, ElementKind};
use crate::core::runtime::{
    ActionId, FocusId, LayoutNode, MemoMark, NodeContent, ScrollId, first_text, node_to_window,
};
use crate::geometry::{Affine, PhysicalSize, Point, Rect, Size, Vec2};
use crate::layout::{
    Axis, FlexItem, LayoutDirection, Span, Track, TrackItem, break_lines, clamp_len, place_cells,
    solve_main_axis, solve_tracks, span_extent,
};
use crate::platform::native::NativeState;
use crate::render::{Color, Font, Scene, TextDirection, TextLayout};

mod limits;
pub(crate) use limits::{content_limits, natural_size};

/// Neutral fill painted into an embedded-content viewport before its content is
/// composited in, so the reserved area reads as a defined surface (a dark slate
/// matching the GPU path's placeholder in `stipple::gpu`).
const VIEWPORT_PLACEHOLDER: Color = Color::rgb(0x20, 0x24, 0x2c);

/// Natural (desired) size of `el` given the `avail` space and active `font`.
///
/// The result honors the element's fixed size and its min/max bounds; the
/// space its content measures against is the fixed size (or `avail`), clamped
/// to those bounds.
pub fn measure(el: &Element, avail: Size, font: Option<&Font>) -> Size {
    // A reused memoized subtree measured in this space before.
    let mark = MemoMark::of(el);
    if let Some(size) = mark.and_then(|m| m.measured(avail)) {
        return size;
    }
    let size = measure_element(el, avail, font);
    if let Some(mark) = mark {
        mark.remember_measure(avail, size);
    }
    size
}

/// The room a stack's children measure in, out of the stack's `inner` size:
/// all of it, but no limit across in a sideways scroll container, whose
/// content runs as wide as it likes (a wrapping row of chips stays on one
/// line there).
fn child_room(el: &Element, axis: Axis, inner: Size) -> Size {
    if el.scroll.is_some() && axis == Axis::Horizontal {
        Size::new(f64::INFINITY, inner.height)
    } else {
        inner
    }
}

/// The size `text` takes laid out at `size`, wrapped to `wrap` if set,
/// given `room` across: no more lines than its style's
/// [`max_lines`](Element::max_lines), and no wider than `room` when it ends
/// in an [ellipsis](Element::ellipsis).
fn text_size(
    text: &str,
    size: f64,
    style: &crate::core::TextStyle,
    wrap: Option<f64>,
    room: f64,
    font: Option<&Font>,
) -> Size {
    let mut laid = laid_size(text, size, style, wrap, font);
    if let Some(max) = style.max_lines()
        && let Some(layout) = style.layout(
            font,
            text,
            size,
            TextDirection::Auto,
            wrap,
            LayoutDirection::Ltr,
        )
        && layout.line_count() > max
    {
        laid.height = (0..max).map(|i| layout.line_height(i)).sum();
    }
    if style.ellipsis().is_some() && room.is_finite() {
        laid.width = laid.width.min(room.max(0.0));
    }
    laid
}

/// [`text_size`] with all its lines, as wide as they are.
fn laid_size(
    text: &str,
    size: f64,
    style: &crate::core::TextStyle,
    wrap: Option<f64>,
    font: Option<&Font>,
) -> Size {
    match style.resolve(font).as_ref() {
        // Rich text measures as laid out, its lines as tall as their
        // tallest run or image.
        Some(_) if style.spans.is_some() || style.flow.is_some() => style
            .layout(
                font,
                text,
                size,
                TextDirection::Auto,
                wrap,
                LayoutDirection::Ltr,
            )
            .map_or(Size::ZERO, |l| l.size()),
        // Wrapping text takes the available width and grows in height.
        Some(f) if let Some(width) = wrap => {
            let lines = f.wrap(text, size, width);
            let w = lines
                .iter()
                .map(|l| f.measure(l, size).width)
                .fold(0.0_f64, f64::max);
            Size::new(w, f.line_height(size) * lines.len() as f64)
        }
        Some(f) => f.measure(text, size),
        None => Size::ZERO,
    }
}

/// [`measure`], without a memoized subtree's cache.
fn measure_element(el: &Element, avail: Size, font: Option<&Font>) -> Size {
    let pad = el.layout.padding;
    let (min, max) = (min_size(el), max_size(el));
    // A fixed size is the space the content actually gets, so wrapped text inside
    // a `.width(360.0)` panel must measure against 360, not the parent's width.
    // Likewise a max-width caps (and a min-width widens) that space.
    let outer = Size::new(
        clamp_len(
            el.layout.size.width.unwrap_or(avail.width),
            min.width,
            max.width,
        ),
        clamp_len(
            el.layout.size.height.unwrap_or(avail.height),
            min.height,
            max.height,
        ),
    );
    let inner = outer.deflate(pad);

    let content = match &el.kind {
        // A viewport sizes purely to its width/height overrides (or grow); it
        // has no intrinsic content size.
        ElementKind::Leaf | ElementKind::Viewport { .. } | ElementKind::Canvas { .. } => Size::ZERO,
        ElementKind::Image { source, .. } => {
            // The natural size, unless one axis is fixed: then the other follows
            // the image's aspect ratio.
            let natural = source.size();
            let fixed = |v: Option<f64>, p: f64| v.map(|v| (v - p).max(0.0));
            let w = fixed(el.layout.size.width, pad.horizontal());
            let h = fixed(el.layout.size.height, pad.vertical());
            match (w, h) {
                (Some(w), None) if natural.width > 0.0 => {
                    Size::new(w, w * natural.height / natural.width)
                }
                (None, Some(h)) if natural.height > 0.0 => {
                    Size::new(h * natural.width / natural.height, h)
                }
                _ => natural,
            }
        }
        ElementKind::Text {
            text, size, style, ..
        } => {
            let wrap = (el.wrap && inner.width.is_finite()).then_some(inner.width);
            text_size(text, *size, style, wrap, inner.width, font)
        }
        ElementKind::Stack {
            axis,
            gap,
            children,
            ..
        } => {
            let room = child_room(el, *axis, inner);
            let mut sizes: Vec<Size> = children.iter().map(|c| measure(c, room, font)).collect();
            settle_row_aspect(el, *axis, *gap, axis.main(inner), children, &mut sizes);
            let lines = if el.wrap {
                let mains: Vec<f64> = sizes.iter().map(|s| axis.main(*s)).collect();
                break_lines(axis.main(inner), *gap, &mains)
            } else {
                std::iter::once(0..children.len()).collect()
            };
            // Widest line along the main axis; lines stacked across it.
            let mut main: f64 = 0.0;
            let mut cross = 0.0;
            for (k, line) in lines.iter().enumerate() {
                let n = line.len() as f64;
                let line_main: f64 = sizes[line.clone()].iter().map(|s| axis.main(*s)).sum();
                main = main.max(line_main + gap * (n - 1.0).max(0.0));
                cross += sizes[line.clone()]
                    .iter()
                    .map(|s| axis.cross(*s))
                    .fold(0.0_f64, f64::max);
                if k > 0 {
                    cross += gap;
                }
            }
            axis.size(main, cross)
        }
        ElementKind::Layers {
            children,
            flow: Some(flow),
            ..
        } if children
            .first()
            .is_some_and(|c| matches!(c.kind, ElementKind::Text { .. })) =>
        {
            flow_text(children, flow, inner, font).map_or(Size::ZERO, |t| measure(&t, inner, font))
        }
        ElementKind::Layers { children, .. } => children
            .iter()
            .map(|c| measure(c, inner, font))
            .fold(Size::ZERO, |a, m| {
                Size::new(a.width.max(m.width), a.height.max(m.height))
            }),
        ElementKind::Grid {
            columns,
            rows,
            column_gap,
            row_gap,
            children,
            ..
        } => {
            // fr columns share a finite available width (a grid fills its
            // container's width, like wrapped text); fr rows size to their
            // content here, and share the actual height once laid out.
            let g = grid_geometry(
                columns,
                rows,
                *column_gap,
                *row_gap,
                children,
                inner,
                false,
                font,
            );
            Size::new(tracks_end(&g.columns), tracks_end(&g.rows))
        }
    };

    let natural = Size::new(
        content.width + pad.horizontal(),
        content.height + pad.vertical(),
    );
    // A container that scrolls sideways is no wider than the space there
    // is, however wide its content.
    let natural_width = match el.scroll_x {
        Some(_) if avail.width.is_finite() => natural.width.min(avail.width),
        _ => natural.width,
    };
    let w = el.layout.size.width.unwrap_or(natural_width);
    let h = el.layout.size.height.unwrap_or(natural.height);
    let mut size = Size::new(
        clamp_len(w, min.width, max.width),
        clamp_len(h, min.height, max.height),
    );
    if let Some(ratio) = el.layout.aspect {
        size = aspect_size(el, ratio, size, natural, avail);
    }
    // A scroll container sized by nothing else is as long as its content
    // along its scroll axis, but no longer than the space there is (or it
    // would never scroll), and no shorter than its floor.
    if let Some(axis) = scroll_fill_axis(el) {
        let length = axis.main(natural).min(axis.main(avail));
        let floor = scroll_floor(el, axis, axis.main(natural));
        let length = clamp_len(length, floor, axis.main(max));
        size = axis.size(length, axis.cross(size));
    }
    // One whose content overflows it is as much wider (or taller, sideways)
    // as the gutter its scroll bar keeps, unless its size across is fixed.
    if let Some(axis) = gutter_axis(el)
        && axis.main(natural) > axis.main(size) + 0.5
    {
        let fixed = match axis {
            Axis::Horizontal => el.layout.size.height,
            Axis::Vertical => el.layout.size.width,
        };
        if fixed.is_none() {
            let cross = axis.cross(size) + crate::core::scroll_bar::gutter();
            size = axis.size(
                axis.main(size),
                clamp_len(cross, axis.cross(min), axis.cross(max)),
            );
        }
    }
    size
}

/// The size of `el`, keeping width over height at `ratio`, out of the size
/// it would otherwise measure (`size`, its content's `natural` size) in
/// `avail`: the height follows a fixed width, or else the width it is
/// given (all of `avail`'s, when that is finite); the width follows a fixed
/// height when only that is fixed.
fn aspect_size(el: &Element, ratio: f64, size: Size, natural: Size, avail: Size) -> Size {
    let (min, max) = (min_size(el), max_size(el));
    match (el.layout.size.width, el.layout.size.height) {
        (Some(_), Some(_)) => size,
        (None, Some(_)) => Size::new(
            clamp_len(size.height * ratio, min.width, max.width),
            size.height,
        ),
        (fixed, None) => {
            let w = match fixed {
                Some(_) => size.width,
                None if avail.width.is_finite() => clamp_len(avail.width, min.width, max.width),
                None => clamp_len(
                    natural.width.max(natural.height * ratio),
                    min.width,
                    max.width,
                ),
            };
            Size::new(w, clamp_len(w / ratio, min.height, max.height))
        }
    }
}

/// `el`'s height, when something other than its container decides it: a
/// fixed one, or one following its width by its aspect ratio. A container
/// stretching its children across leaves such a height alone.
fn decided_height(el: &Element) -> Option<f64> {
    el.layout
        .size
        .height
        .or_else(|| el.layout.aspect.map(|_| 0.0))
}

/// `el`'s width, when something other than its container decides it: a
/// fixed one, or one following a fixed height by its aspect ratio.
fn sized_width(el: &Element) -> Option<f64> {
    el.layout
        .size
        .width
        .or_else(|| el.layout.aspect.filter(|_| el.layout.size.height.is_some()))
}

/// [`sized_width`], but not a widget's default width, which a column
/// stretches.
fn decided_width(el: &Element) -> Option<f64> {
    sized_width(el).filter(|_| !el.layout.soft_width)
}

/// Whether `child` of a row keeps an aspect ratio with neither side fixed:
/// it then takes a share of the row's free width (as if it grew), and its
/// height follows the width it gets.
fn row_aspect(child: &Element) -> bool {
    child.layout.aspect.is_some()
        && child.layout.size.width.is_none()
        && child.layout.size.height.is_none()
}

/// The flex item `child` of a stack along `axis` is, measured `m`.
fn flex_item(child: &Element, axis: Axis, m: Size) -> FlexItem {
    let item = FlexItem::fixed(axis.main(m))
        .grow(child.layout.grow)
        .shrink(child.layout.shrink)
        .bounds(axis.main(min_size(child)), axis.main(max_size(child)));
    // A scroll container along this axis, with no length of its own, takes
    // the free space and gives it back when its siblings overflow (down to
    // its floor).
    if scroll_fill_axis(child) == Some(axis) {
        FlexItem {
            grow: if child.layout.grow > 0.0 {
                child.layout.grow
            } else {
                1.0
            },
            shrink: if child.layout.shrink > 0.0 {
                child.layout.shrink
            } else {
                1.0
            },
            min: scroll_floor(child, axis, axis.main(m)),
            ..item
        }
    } else {
        item
    }
}

/// In a row that neither wraps nor scrolls, `main` wide: the children that
/// keep an aspect ratio with neither side fixed share the free width with
/// the growing ones (as growing items with no width of their own), and
/// their `measured` sizes become the width each gets and the height
/// following it. Returns which children those are (none, elsewhere).
fn settle_row_aspect(
    el: &Element,
    axis: Axis,
    gap: f64,
    main: f64,
    children: &[Element],
    measured: &mut [Size],
) -> Vec<bool> {
    let settled: Vec<bool> = children.iter().map(row_aspect).collect();
    if axis != Axis::Horizontal
        || el.wrap
        || el.scroll.is_some()
        || !main.is_finite()
        || !settled.contains(&true)
    {
        return vec![false; children.len()];
    }
    let items: Vec<FlexItem> = children
        .iter()
        .zip(measured.iter())
        .zip(&settled)
        .map(|((c, m), &aspect)| {
            let item = flex_item(c, axis, *m);
            if aspect {
                FlexItem {
                    basis: 0.0,
                    grow: if c.layout.grow > 0.0 {
                        c.layout.grow
                    } else {
                        1.0
                    },
                    ..item
                }
            } else {
                item
            }
        })
        .collect();
    let spans = solve_main_axis(main, gap, &items);
    for ((c, m), (span, &aspect)) in children
        .iter()
        .zip(measured.iter_mut())
        .zip(spans.iter().zip(&settled))
    {
        if let (true, Some(ratio)) = (aspect, c.layout.aspect) {
            let h = clamp_len(span.length / ratio, min_size(c).height, max_size(c).height);
            *m = Size::new(span.length, h);
        }
    }
    settled
}

/// The axis of a scroll container that keeps a gutter for its scroll bar
/// when its content overflows (a stack that doesn't wrap, while the app's
/// bars take one), if `el` is one.
fn gutter_axis(el: &Element) -> Option<Axis> {
    el.scroll?;
    if el.wrap || crate::core::scroll_bar::gutter() <= 0.0 {
        return None;
    }
    match el.kind {
        ElementKind::Stack { axis, .. } => Some(axis),
        _ => None,
    }
}

/// The room a stack's children are laid out in across, out of `avail`, and
/// their measured sizes: all of it, unless the stack is a scroll container
/// whose content overflows it, which keeps a gutter for its scroll bar (on
/// its trailing side; the bottom, sideways, when the content still fits
/// above it).
fn gutter_layout(
    el: &Element,
    axis: Axis,
    gap: f64,
    avail: Size,
    children: &[Element],
    font: Option<&Font>,
) -> (Size, Vec<Size>) {
    let measure_in = |avail: Size| -> Vec<Size> {
        let room = child_room(el, axis, avail);
        children.iter().map(|c| measure(c, room, font)).collect()
    };
    if gutter_axis(el) == Some(axis) {
        // Measured narrower first: a long list, the usual case, overflows.
        let g = crate::core::scroll_bar::gutter();
        let narrowed = axis.size(axis.main(avail), (axis.cross(avail) - g).max(0.0));
        let measured = measure_in(narrowed);
        let n = measured.len() as f64;
        let content: f64 =
            measured.iter().map(|m| axis.main(*m)).sum::<f64>() + gap * (n - 1.0).max(0.0);
        // A sideways strip whose height leaves no room for the gutter (it
        // is fixed, say) doesn't squeeze its content: its bar is drawn over
        // the content's bottom edge instead.
        let fits = axis == Axis::Vertical
            || measured
                .iter()
                .all(|m| axis.cross(*m) <= axis.cross(narrowed) + 0.5);
        // Content scrolling sideways too, with both gutters kept, scrolls a
        // gutter further down (below it, the horizontal bar's).
        let below = if both_gutters(el) && wider_than(&measured, narrowed.width) {
            g
        } else {
            0.0
        };
        if content + below > axis.main(avail) + 0.5 && fits {
            return (narrowed, measured);
        }
    }
    (avail, measure_in(avail))
}

/// Whether `el` scrolls both ways keeping a gutter for each bar
/// ([`Element::scrollable_both`]), while the app's bars take one.
fn both_gutters(el: &Element) -> bool {
    el.scroll_gutters
        && el.scroll.is_some()
        && el.scroll_x.is_some()
        && crate::core::scroll_bar::gutter() > 0.0
}

/// Whether any of `measured` is wider than `width`.
fn wider_than(measured: &[Size], width: f64) -> bool {
    measured.iter().any(|m| m.width > width + 0.5)
}

/// How much further than its content `el` scrolls sideways and down (see
/// [`LayoutNode::scroll_tail`]), its children `measured` in `avail` out of
/// `inner`: the vertical bar's gutter sideways, where it took one and the
/// content is wider than what's left, and the horizontal bar's down, while
/// the content is wider.
fn scroll_tail(el: &Element, inner: Size, avail: Size, measured: &[Size]) -> (f64, f64) {
    if !both_gutters(el) || !wider_than(measured, avail.width) {
        return (0.0, 0.0);
    }
    let g = crate::core::scroll_bar::gutter();
    let beside = if avail.width < inner.width - 0.5 {
        g
    } else {
        0.0
    };
    (beside, g)
}

/// The shortest a scroll container that fills its parent shrinks to, unless
/// a [`min_height`](Element::min_height) (or width) says otherwise.
const SCROLL_MIN: f64 = 100.0;

/// The axis along which `el` is a scroll container whose length nothing
/// fixes, if it is one: that length then follows its parent (see
/// [`widgets::scroll`](crate::widgets::scroll)).
fn scroll_fill_axis(el: &Element) -> Option<Axis> {
    el.scroll?;
    let axis = match el.kind {
        ElementKind::Stack {
            axis: Axis::Horizontal,
            ..
        } => Axis::Horizontal,
        _ => Axis::Vertical,
    };
    let fixed = match axis {
        Axis::Horizontal => el.layout.size.width,
        Axis::Vertical => el.layout.size.height,
    };
    fixed.is_none().then_some(axis)
}

/// How short scroll container `el` may get along `axis`, its content
/// being `natural` long: its own minimum, or [`SCROLL_MIN`] (less for less
/// content).
fn scroll_floor(el: &Element, axis: Axis, natural: f64) -> f64 {
    let own = match axis {
        Axis::Horizontal => el.layout.min.width,
        Axis::Vertical => el.layout.min.height,
    };
    own.unwrap_or(natural.min(SCROLL_MIN))
}

/// The element's lower size bounds (zero where unset).
fn min_size(el: &Element) -> Size {
    Size::new(
        el.layout.min.width.unwrap_or(0.0),
        el.layout.min.height.unwrap_or(0.0),
    )
}

/// The element's upper size bounds (infinite where unset).
fn max_size(el: &Element) -> Size {
    Size::new(
        el.layout.max.width.unwrap_or(f64::INFINITY),
        el.layout.max.height.unwrap_or(f64::INFINITY),
    )
}

/// Where the last track of an axis ends (its total extent, gaps included).
fn tracks_end(spans: &[Span]) -> f64 {
    spans.last().map(|s| s.offset + s.length).unwrap_or(0.0)
}

/// A grid's resolved tracks and cell assignments.
struct GridGeometry {
    columns: Vec<Span>,
    rows: Vec<Span>,
    /// Per child: `(row, column, row_span, column_span)`.
    cells: Vec<(usize, usize, usize, usize)>,
    /// Per child: natural size measured against its cell's width.
    sizes: Vec<Size>,
}

/// Size a grid's tracks within `inner` and place its children. Columns are
/// sized first (auto columns from each child's natural width), then every child
/// is re-measured at its cell's width — so wrapped text in an `fr` column gets
/// the right height — and the rows are sized from those heights.
#[allow(clippy::too_many_arguments)]
fn grid_geometry(
    columns: &[Track],
    rows: &[Track],
    column_gap: f64,
    row_gap: f64,
    children: &[Element],
    inner: Size,
    rows_definite: bool,
    font: Option<&Font>,
) -> GridGeometry {
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
    let (placed, nrows) = place_cells(ncols, &spans);

    let col_items: Vec<TrackItem> = children
        .iter()
        .zip(&spans)
        .zip(&placed)
        .map(|((c, &(cs, _)), &(_, col))| {
            // Inside fixed columns the child measures against their width.
            let fixed: Option<f64> = columns[col..col + cs]
                .iter()
                .map(|t| match t {
                    Track::Fixed(v) => Some(*v),
                    _ => None,
                })
                .sum();
            let probe = fixed.map(|w| w + column_gap * (cs as f64 - 1.0));
            let width = measure(
                c,
                Size::new(probe.unwrap_or(inner.width), inner.height),
                font,
            )
            .width;
            TrackItem {
                start: col,
                span: cs,
                size: width,
            }
        })
        .collect();
    let col_avail = inner.width.is_finite().then_some(inner.width);
    let col_spans = solve_tracks(columns, col_avail, column_gap, &col_items);

    let sizes: Vec<Size> = children
        .iter()
        .zip(&spans)
        .zip(&placed)
        .map(|((c, &(cs, _)), &(_, col))| {
            let (_, cell_w) = span_extent(&col_spans, col, cs);
            measure(c, Size::new(cell_w, inner.height), font)
        })
        .collect();
    let row_tracks: Vec<Track> = (0..nrows.max(rows.len()))
        .map(|r| rows.get(r).copied().unwrap_or(Track::Auto))
        .collect();
    let row_items: Vec<TrackItem> = sizes
        .iter()
        .zip(&spans)
        .zip(&placed)
        .map(|((s, &(_, rs)), &(row, _))| TrackItem {
            start: row,
            span: rs,
            size: s.height,
        })
        .collect();
    let row_avail = (rows_definite && inner.height.is_finite()).then_some(inner.height);
    let row_spans = solve_tracks(&row_tracks, row_avail, row_gap, &row_items);

    GridGeometry {
        columns: col_spans,
        rows: row_spans,
        cells: placed
            .iter()
            .zip(&spans)
            .map(|(&(r, c), &(cs, rs))| (r, c, rs, cs))
            .collect(),
        sizes,
    }
}

/// Position a child of natural length `natural` in a slot of length `slot`
/// along one axis per `align`, honoring its `min..=max`: `(offset, length)`.
fn align_in(align: Align, natural: f64, slot: f64, min: f64, max: f64) -> (f64, f64) {
    let length = match align {
        Align::Stretch => clamp_len(slot, min, max),
        _ => clamp_len(natural.min(slot), min, max),
    };
    let offset = match align {
        Align::Start | Align::Stretch => 0.0,
        Align::Center => (slot - length) / 2.0,
        Align::End => slot - length,
    };
    (offset, length)
}

/// Lay `el` out within `bounds`, producing a retained [`LayoutNode`] tree with
/// absolute bounds, decorations, text content, and action handles. The tree is
/// laid out left to right, except where an element sets
/// [`Element::layout_direction`]; see [`layout_with_direction`].
pub fn layout(el: &Element, bounds: Rect, font: Option<&Font>) -> LayoutNode {
    layout_with_direction(el, bounds, font, LayoutDirection::Ltr)
}

/// [`layout`] with the root inheriting `direction` (an element's own
/// [`Element::layout_direction`] still wins, for it and its subtree).
///
/// A right-to-left element places its children as a left-to-right one would,
/// then reflects each child's box about the middle of its own: rows run right
/// to left, horizontal `Start` alignment is the right edge, grid columns count
/// from the right, and the left padding inset ends up on the right. Children
/// then lay out their own children the same way, so a whole subtree mirrors.
/// Glyphs, images and vertical positions are never flipped.
pub fn layout_with_direction(
    el: &Element,
    bounds: Rect,
    font: Option<&Font>,
    direction: LayoutDirection,
) -> LayoutNode {
    // A reused memoized subtree laid out at this size before.
    let mark = MemoMark::of(el);
    if let Some(node) = mark.and_then(|m| m.laid_out(bounds, direction)) {
        return node;
    }
    let node = layout_element(el, bounds, font, direction);
    if let Some(mark) = mark {
        mark.remember_layout(direction, &node);
    }
    node
}

/// [`layout_with_direction`], without a memoized subtree's cache.
fn layout_element(
    el: &Element,
    bounds: Rect,
    font: Option<&Font>,
    direction: LayoutDirection,
) -> LayoutNode {
    let dir = el.layout_direction.unwrap_or(direction);
    // Place a child at `rect`, as computed left to right within `bounds`.
    let place = |child: &Element, rect: Rect| {
        let rect = if dir.is_rtl() {
            mirror_x(rect, bounds)
        } else {
            rect
        };
        layout_with_direction(child, rect, font, dir)
    };
    let content = match &el.kind {
        ElementKind::Text {
            text,
            size,
            color,
            direction,
            underline,
            style,
        } => {
            let padding = if dir.is_rtl() {
                el.layout.padding.mirrored()
            } else {
                el.layout.padding
            };
            // How far the lines run past their box: a line too long for it,
            // and lines too many for its height.
            let room = bounds.inset(padding);
            let wrap = (el.wrap && room.width().is_finite()).then_some(room.width());
            // Cut to its lines and its box, when it says so.
            let original = text;
            let (text, style) = match crate::core::elide::elide(
                style,
                font,
                text,
                *size,
                *direction,
                wrap,
                room.width(),
                dir,
            ) {
                Some((shown, style)) => (shown, style),
                None => (text.clone(), style.clone()),
            };
            let laid = text_size(&text, *size, &style, wrap, room.width(), font);
            let overflow = crate::core::runtime::TextOverflow {
                across: (laid.width - room.width()).max(0.0) as f32,
                below: (laid.height - room.height()).max(0.0) as f32,
            };
            // A mnemonic's underline stays where the text still is.
            let underline = underline.filter(|&(_, e)| {
                text.get(..e)
                    .is_some_and(|shown| original.get(..e) == Some(shown))
            });
            NodeContent::Text {
                text,
                size: *size,
                color: *color,
                direction: *direction,
                underline,
                style,
                padding,
                overflow,
            }
        }
        ElementKind::Viewport { id } => NodeContent::Viewport(*id),
        ElementKind::Canvas {
            id,
            paint,
            revision,
        } => NodeContent::Canvas {
            id: *id,
            paint: paint.clone(),
            revision: *revision,
        },
        ElementKind::Image { source, fit } => NodeContent::Image {
            source: source.clone(),
            fit: *fit,
            padding: if dir.is_rtl() {
                el.layout.padding.mirrored()
            } else {
                el.layout.padding
            },
        },
        _ => NodeContent::None,
    };
    let mut node = LayoutNode {
        bounds,
        decoration: el.decoration,
        background: el.background.clone(),
        content,
        action: el.action,
        focus: el.focus,
        drag: el.drag,
        context: el.context,
        drop: el.drop,
        drag_source: el.drag_source,
        pointer: el.pointer,
        caret: el.caret,
        selection: el.selection,
        preedit: el.preedit,
        keyboard: el.keyboard.clone(),
        text_pos: el.text_pos,
        wrap: el.wrap,
        native: el.native,
        native_from: None,
        native_live: None,
        native_a11y: None,
        scroll: el.scroll,
        scroll_x: el.scroll_x.filter(|_| el.scroll.is_some()),
        scroll_tail: (0.0, 0.0),
        scroll_horizontal: el.scroll.is_some()
            && matches!(
                el.kind,
                ElementKind::Stack {
                    axis: Axis::Horizontal,
                    ..
                }
            ),
        clip: el.clip,
        cursor: el.cursor,
        semantics: el.semantics.clone(),
        rtl: dir.is_rtl(),
        transform: el.transform,
        opacity: el.opacity,
        key: el.reactive.as_ref().and_then(|r| r.id),
        ghost: false,
        offstage: el.offstage,
        hit_through: el.hit_through,
        scroll_bar: None,
        scroll_bar_x: None,
        indicator: el.indicator,
        children: Vec::new(),
    };

    if let ElementKind::Grid {
        columns,
        rows,
        column_gap,
        row_gap,
        align,
        children,
    } = &el.kind
    {
        let mut inner = bounds.inset(el.layout.padding);
        // A scrolling grid's rows overflow at natural size instead of sharing
        // the viewport height.
        let geometry = |room: Size| {
            grid_geometry(
                columns,
                rows,
                *column_gap,
                *row_gap,
                children,
                room,
                el.scroll.is_none(),
                font,
            )
        };
        let mut g = geometry(inner.size);
        // One whose rows overflow keeps a gutter on its trailing side for
        // its bar (laid out narrower; mirrored right to left with the rest).
        let gutter = crate::core::scroll_bar::gutter();
        if el.scroll.is_some()
            && gutter > 0.0
            && tracks_end(&g.rows) > inner.height() + 0.5
            && inner.width() > gutter
        {
            inner = Rect::from_xywh(
                inner.min_x(),
                inner.min_y(),
                inner.width() - gutter,
                inner.height(),
            );
            g = geometry(inner.size);
        }
        node.children.reserve(children.len());
        for (k, child) in children.iter().enumerate() {
            let (row, col, rs, cs) = g.cells[k];
            let (cx, cw) = span_extent(&g.columns, col, cs);
            let (cy, ch) = span_extent(&g.rows, row, rs);
            let (min, max, m) = (min_size(child), max_size(child), g.sizes[k]);
            // As in CSS, `Stretch` leaves a child's fixed size alone (it sits at
            // the cell's start); only unsized axes fill the cell.
            let fixed = |a: Align, size: Option<f64>| match (a, size) {
                (Align::Stretch, Some(_)) => Align::Start,
                _ => a,
            };
            // A child's own `justify_self` / `align_self` wins over the
            // grid's (horizontal, vertical) alignment, as in CSS grid.
            let (dx, w) = align_in(
                fixed(
                    child.layout.justify_self.unwrap_or(align.0),
                    sized_width(child),
                ),
                m.width,
                cw,
                min.width,
                max.width,
            );
            let (dy, h) = align_in(
                fixed(
                    child.layout.align_self.unwrap_or(align.1),
                    decided_height(child),
                ),
                m.height,
                ch,
                min.height,
                max.height,
            );
            let rect = Rect::from_xywh(inner.min_x() + cx + dx, inner.min_y() + cy + dy, w, h);
            node.children.push(place(child, rect));
        }
        return node;
    }

    if let ElementKind::Layers {
        children,
        flow: Some(flow),
        ..
    } = &el.kind
        && let Some(placed) = flow_layout(children, flow, bounds.inset(el.layout.padding), font)
    {
        node.children.reserve(children.len());
        for (child, rect) in placed {
            node.children
                .push(layout_with_direction(&child, rect, font, dir));
        }
        return node;
    }

    if let ElementKind::Layers {
        align, children, ..
    } = &el.kind
    {
        let inner = bounds.inset(el.layout.padding);
        node.children.reserve(children.len());
        for child in children {
            let (m, min, max) = (
                measure(child, inner.size, font),
                min_size(child),
                max_size(child),
            );
            // As in a grid cell, a sized child sits per `align`; only unsized
            // axes stretch.
            let fixed = |a: Align, size: Option<f64>| match (a, size) {
                (Align::Stretch, Some(_)) => Align::Start,
                _ => a,
            };
            let (dx, w) = align_in(
                fixed(align.0, sized_width(child)),
                m.width,
                inner.width(),
                min.width,
                max.width,
            );
            let (dy, h) = align_in(
                fixed(align.1, decided_height(child)),
                m.height,
                inner.height(),
                min.height,
                max.height,
            );
            let rect = Rect::from_xywh(inner.min_x() + dx, inner.min_y() + dy, w, h);
            node.children.push(place(child, rect));
        }
        return node;
    }

    let ElementKind::Stack {
        axis,
        gap,
        main_align,
        cross_align,
        children,
    } = &el.kind
    else {
        return node;
    };
    if children.is_empty() {
        return node;
    }

    let inner = bounds.inset(el.layout.padding);
    let axis = *axis;
    let main_avail = axis.main(inner.size);

    // Main-axis distribution. A scroll container whose content overflows
    // keeps a gutter across for its scroll bar (see `core::scroll_bar`).
    let (avail, mut measured) = gutter_layout(el, axis, *gap, inner.size, children, font);
    node.scroll_tail = scroll_tail(el, inner.size, avail, &measured);
    let settled = settle_row_aspect(el, axis, *gap, main_avail, children, &mut measured);
    // A wrapping stack flows children onto lines that fit the main axis; each
    // line is then laid out like an unwrapped stack whose cross extent is the
    // line's tallest child. Lines pack from the cross start, `gap` apart.
    let lines = if el.wrap {
        let mains: Vec<f64> = measured.iter().map(|m| axis.main(*m)).collect();
        break_lines(main_avail, *gap, &mains)
    } else {
        std::iter::once(0..children.len()).collect()
    };

    node.children.reserve(children.len());
    let mut cross_cursor = 0.0;
    for line in lines {
        let line_children = &children[line.clone()];
        let line_measured = &measured[line.clone()];
        let line_settled = &settled[line.clone()];
        // A scroll container lays its children out at their *natural* main size
        // (so content can overflow the viewport), stacked from the start with no
        // grow/shrink; the offset + clip are applied afterward (see
        // `apply_scroll`).
        let (spans, main_shift) = if el.scroll.is_some() {
            let items: Vec<FlexItem> = line_measured
                .iter()
                .map(|m| FlexItem::fixed(axis.main(*m)))
                .collect();
            (solve_main_axis(f64::INFINITY, *gap, &items), 0.0)
        } else {
            let items: Vec<FlexItem> = line_children
                .iter()
                .zip(line_measured)
                .zip(line_settled)
                .map(|((c, m), &settled)| {
                    // A child keeping its aspect ratio already took its
                    // share of the row (see `settle_row_aspect`).
                    if settled {
                        FlexItem::fixed(axis.main(*m))
                    } else {
                        flex_item(c, axis, *m)
                    }
                })
                .collect();
            let spans = solve_main_axis(main_avail, *gap, &items);
            // If nothing grows, the line may be shorter than the content area;
            // shift it as a whole per the main-axis alignment.
            let leftover = (main_avail - tracks_end(&spans)).max(0.0);
            let main_shift = match main_align {
                Align::Start | Align::Stretch => 0.0,
                Align::Center => leftover / 2.0,
                Align::End => leftover,
            };
            (spans, main_shift)
        };

        // An unwrapped stack's single line spans the whole cross extent.
        let line_cross = if el.wrap {
            line_measured
                .iter()
                .map(|m| axis.cross(*m))
                .fold(0.0_f64, f64::max)
        } else {
            axis.cross(avail)
        };
        for ((child, m), span) in line_children.iter().zip(line_measured).zip(&spans) {
            // As in a grid cell, `Stretch` leaves a child sized across alone.
            let fixed_cross = match axis {
                Axis::Horizontal => decided_height(child),
                Axis::Vertical => decided_width(child),
            };
            let align = match (child.layout.align_self.unwrap_or(*cross_align), fixed_cross) {
                (Align::Stretch, Some(_)) => Align::Start,
                (a, _) => a,
            };
            let (cross_off, cross_len) = align_in(
                align,
                axis.cross(*m),
                line_cross,
                axis.cross(min_size(child)),
                axis.cross(max_size(child)),
            );
            let child_bounds = child_rect(
                axis,
                inner,
                span.offset + main_shift,
                span.length,
                cross_cursor + cross_off,
                cross_len,
            );
            node.children.push(place(child, child_bounds));
        }
        cross_cursor += line_cross + *gap;
    }
    node
}

/// The text of a [`flow_text`](Element::flow_text) container (its first
/// child), carrying the objects it flows around: each other child measured
/// in `inner`, placed as its item says, shaped as its item says (a box
/// rounded as the child's corners by default). `None` when the first child
/// isn't text.
fn flow_text(
    children: &[Element],
    flow: &crate::core::element::FlowBox,
    inner: Size,
    font: Option<&Font>,
) -> Option<Element> {
    let mut text = children.first()?.clone();
    let ElementKind::Text { style, .. } = &mut text.kind else {
        return None;
    };
    let mut spec = crate::render::TextFlow::new(inner.width).options(flow.options);
    for (child, item) in children[1..].iter().zip(&flow.items) {
        let size = measure(child, inner, font);
        let mut item = item.clone();
        if item.shape == crate::render::FlowShape::Rect && child.decoration.radius > 0.0 {
            item.shape = crate::render::FlowShape::RoundRect(child.decoration.radius);
        }
        spec = spec.item(item, size);
    }
    style.flow = Some(std::sync::Arc::new(spec));
    text.wrap = true;
    Some(text)
}

/// A [`flow_text`](Element::flow_text) container's children and where each
/// goes in `inner`: the text across it, the others where the flow placed
/// them (children past the items at the top-left, at their size).
fn flow_layout(
    children: &[Element],
    flow: &crate::core::element::FlowBox,
    inner: Rect,
    font: Option<&Font>,
) -> Option<Vec<(Element, Rect)>> {
    let text = flow_text(children, flow, inner.size, font)?;
    let height = measure(&text, inner.size, font).height.max(inner.height());
    let text_rect = Rect::new(inner.origin, Size::new(inner.width(), height));
    let ElementKind::Text {
        text: string,
        size,
        direction,
        style,
        ..
    } = &text.kind
    else {
        return None;
    };
    let box_ = text_rect.inset(text.layout.padding);
    let layout = style.layout(
        font,
        string,
        *size,
        *direction,
        Some(box_.width()),
        LayoutDirection::Ltr,
    );
    let mut out = Vec::with_capacity(children.len());
    for (k, child) in children.iter().enumerate().skip(1) {
        let rect = layout
            .as_ref()
            .and_then(|l| l.flow_item_rect(k - 1))
            .filter(|_| k - 1 < flow.items.len())
            .map(|r| r.translate(Vec2::new(box_.min_x(), box_.min_y())))
            .unwrap_or_else(|| Rect::new(inner.origin, measure(child, inner.size, font)));
        out.push((child.clone(), rect));
    }
    out.insert(0, (text, text_rect));
    Some(out)
}

/// Overlay focus affordances for the focused element: a `ring` around its
/// bounds, a `selection` highlight behind any selected text range, and a
/// `caret` bar at the text's caret index (or its end when no caret is set).
/// No-op if `focused` isn't in the tree.
#[cfg(test)]
pub(crate) fn paint_focus(
    tree: &LayoutNode,
    focused: FocusId,
    scene: &mut Scene,
    font: Option<&Font>,
    ring: Color,
    caret: Color,
    selection: Color,
) {
    paint_focus_look(
        tree,
        focused,
        scene,
        font,
        (ring, &crate::style::FocusLook::STANDARD, true),
        caret,
        selection,
    );
}

/// [`paint_focus`] with the ring drawn the way `look` says (its width, how
/// far it stands off a button, its glow; see
/// [`Theme::focus`](crate::style::Theme::focus)), if `visible` (the focus
/// came from the keyboard: CSS's `:focus-visible`) or the focused element
/// is a text field, whose ring shows however it got the focus.
pub(crate) fn paint_focus_look(
    tree: &LayoutNode,
    focused: FocusId,
    scene: &mut Scene,
    font: Option<&Font>,
    (ring, look, visible): (Color, &crate::style::FocusLook, bool),
    caret: Color,
    selection: Color,
) {
    let Some((node, to_window, opacity)) = node_to_window(tree, &|n| n.focus == Some(focused))
    else {
        return;
    };
    // A live OS control draws its own focus, caret and selection.
    if node.native_live.is_some() {
        return;
    }
    // Drawn where the node is drawn, if it's transformed or translucent.
    let layer = !to_window.is_identity() || opacity < 1.0;
    if layer {
        scene.push_layer(to_window, opacity);
    }
    paint_focus_node(node, scene, font, (ring, look, visible), caret, selection);
    if layer {
        scene.pop_layer();
    }
}

/// [`paint_focus`] for the focused `node`, in its layout coordinates.
fn paint_focus_node(
    node: &LayoutNode,
    scene: &mut Scene,
    font: Option<&Font>,
    (ring, look, visible): (Color, &crate::style::FocusLook, bool),
    caret: Color,
    selection: Color,
) {
    let os_focus = node
        .native
        .is_some_and(|n| crate::platform::native::draws_focus(n.control));
    // Only text entry shows a caret: not a focusable that also takes taps (a
    // select's button), nor a list, table or tree taking focus as a whole.
    let entry = node
        .semantics
        .as_ref()
        .and_then(|s| s.role)
        .is_none_or(|r| {
            matches!(
                r,
                crate::core::a11y::Role::TextField
                    | crate::core::a11y::Role::SpinButton
                    | crate::core::a11y::Role::PasswordField
            )
        })
        && node.action.is_none();
    // A field being typed in (placing its caret with the pointer) shows its
    // ring whatever moved the focus there.
    if !os_focus && (visible || (entry && node.text_pos.is_some())) {
        // A button's ring stands off it, following its corners, so it
        // shows on a button filled with the ring's color too.
        let tap = crate::core::tab_focus::is_tap_focus(node);
        let (r, out) = if tap {
            (node.decoration.radius, look.offset)
        } else {
            (node.decoration.radius, 0.0)
        };
        let b = node.bounds;
        let grown = |by: f64| {
            Rect::from_xywh(
                b.min_x() - by,
                b.min_y() - by,
                b.width() + 2.0 * by,
                b.height() + 2.0 * by,
            )
        };
        let corner = |by: f64| if r > 0.0 { r + by } else { 0.0 };
        // The glow: rings fading out beyond the ring, drawn first.
        if look.glow > 0.0 {
            let steps = (look.glow / 2.0).ceil().max(1.0) as usize;
            for i in (0..steps).rev() {
                let by = out + look.width / 2.0 + 1.0 + 2.0 * i as f64;
                let fade = 1.0 - i as f64 / steps as f64;
                let a = (f64::from(ring.a) * 0.3 * fade * fade).round() as u8;
                let color = Color { a, ..ring };
                scene.stroke_round_rect(grown(by), corner(by), color, 2.0);
            }
        }
        if tap || r > 0.0 {
            scene.stroke_round_rect(grown(out), corner(out), ring, look.width);
        } else {
            scene.stroke_rect(b, ring, look.width);
        }
    }
    if !entry {
        return;
    }

    let Some(leaf) = first_text(node) else { return };
    let NodeContent::Text {
        text,
        size,
        direction,
        style,
        ..
    } = &leaf.content
    else {
        return;
    };
    let bounds = leaf.text_rect();
    let styled = style.resolve(font);
    let (Some(f), Some(layout)) = (
        styled.as_ref(),
        text_layout(
            style,
            font,
            text,
            *size,
            *direction,
            leaf.wrap,
            bounds.width(),
            leaf.rtl,
        ),
    ) else {
        // No font, no geometry: a caret bar at the start of the box.
        let h = *size;
        scene.fill_rect(
            Rect::from_xywh(bounds.min_x(), bounds.min_y(), 2.0, h),
            caret,
        );
        return;
    };
    let font = f;
    let origin = bounds.origin;

    // Selection highlight: in mixed-direction text one logical range can be
    // several rectangles on a line (under the caret; translucent so the text
    // reads through).
    if let Some((s, e)) = leaf.selection {
        for rect in layout.selection_rects(s, e) {
            scene.fill_rect(rect.translate(Vec2::new(origin.x, origin.y)), selection);
        }
    }

    // Caret bar on its line (or end of text when unset), kept inside the box.
    let line_h = font.line_height(*size);
    let at = layout.caret_position(leaf.caret.unwrap_or(text.len()));
    let cx = (origin.x + at.x).clamp(bounds.min_x(), (bounds.max_x() - 1.0).max(bounds.min_x()));
    scene.fill_rect(Rect::from_xywh(cx, origin.y + at.y, 2.0, line_h), caret);
}

/// The layout a text node is painted with: wrapped to its box when `wrap` is
/// set, with right-to-left paragraphs aligned to the box's right edge. In a
/// right-to-left layout (`rtl`), `Auto` text lines up on the right too.
#[allow(clippy::too_many_arguments)]
pub(crate) fn text_layout<'a>(
    style: &crate::core::TextStyle,
    font: Option<&Font>,
    text: &'a str,
    size: f64,
    direction: TextDirection,
    wrap: bool,
    width: f64,
    rtl: bool,
) -> Option<TextLayout<'a>> {
    let ui = if rtl {
        LayoutDirection::Rtl
    } else {
        LayoutDirection::Ltr
    };
    style
        .layout(font, text, size, direction, wrap.then_some(width), ui)
        .map(|l| l.align_width(width))
}

/// The layout of the text leaf `leaf` (relative to its
/// [`text_rect`](LayoutNode::text_rect)), if it is one and there's a font.
pub(crate) fn leaf_layout<'a>(leaf: &'a LayoutNode, font: Option<&Font>) -> Option<TextLayout<'a>> {
    let NodeContent::Text {
        text,
        size,
        direction,
        style,
        ..
    } = &leaf.content
    else {
        return None;
    };
    let b = leaf.text_rect();
    text_layout(
        style,
        font,
        text,
        *size,
        *direction,
        leaf.wrap,
        b.width(),
        leaf.rtl,
    )
}

/// Resolve a pointer position (logical pixels, absolute) to the nearest caret
/// byte index within `node`'s first text leaf. The `y` picks the line and the
/// `x` the nearer edge of the character under it, in visual order, so clicks
/// land correctly in right-to-left and mixed-direction text. Returns `None` if
/// `node` has no text or no `font`.
pub(crate) fn caret_index_at(
    node: &LayoutNode,
    point: Point,
    font: Option<&Font>,
) -> Option<usize> {
    let leaf = first_text(node)?;
    let layout = leaf_layout(leaf, font)?;
    let b = leaf.text_rect();
    Some(layout.index_at(Point::new(point.x - b.min_x(), point.y - b.min_y())))
}

/// [`caret_index_at`] with the rest of what a text-hit handler gets: the
/// pointer in the text's box, the box's width, and its images' boxes.
pub(crate) fn text_hit_at(
    node: &LayoutNode,
    point: Point,
    font: Option<&Font>,
) -> Option<crate::core::TextHit> {
    // Of several texts (lines), the one level with the pointer: the first
    // that reaches below it (so of texts side by side, the first), or the
    // last when the pointer is below them all.
    let mut leaves = Vec::new();
    text_leaves(node, &mut leaves);
    let at = leaves
        .iter()
        .position(|l| l.bounds.max_y() > point.y)
        .unwrap_or(leaves.len().saturating_sub(1));
    let leaf = *leaves.get(at)?;
    let layout = leaf_layout(leaf, font)?;
    let b = leaf.text_rect();
    let local = Point::new(point.x - b.min_x(), point.y - b.min_y());
    Some(
        crate::core::TextHit::new(layout.index_at(local), local, b.width())
            .objects(layout.object_rects())
            .leaf(at),
    )
}

/// The text leaves under `node`, in order.
fn text_leaves<'a>(node: &'a LayoutNode, out: &mut Vec<&'a LayoutNode>) {
    if matches!(node.content, NodeContent::Text { .. }) {
        out.push(node);
    } else {
        for c in &node.children {
            text_leaves(c, out);
        }
    }
}

/// Overlay a `highlight` (typically translucent) on the hovered tappable
/// element, matching its corner radius. No-op if `hovered` isn't in the tree.
pub(crate) fn paint_hover(
    tree: &LayoutNode,
    hovered: ActionId,
    scene: &mut Scene,
    highlight: Color,
) {
    if let Some((node, to_window, opacity)) = node_to_window(tree, &|n| n.action == Some(hovered)) {
        // A labeled control lights up its box, not its label.
        let node = indicator_of(node).unwrap_or(node);
        // An OS-drawn control shows hover itself (see `mark_native_states`),
        // and a skinned one with its frames: a rectangle over a shaped frame
        // would show its corners.
        if node.native.is_some() || node.background.is_some() || opacity <= 0.0 {
            return;
        }
        // Drawn where the node is drawn, if it's transformed or translucent.
        let layer = !to_window.is_identity() || opacity < 1.0;
        if layer {
            scene.push_layer(to_window, opacity);
        }
        if node.decoration.radius > 0.0 {
            scene.fill_round_rect(node.bounds, node.decoration.radius, highlight);
        } else {
            scene.fill_rect(node.bounds, highlight);
        }
        if layer {
            scene.pop_layer();
        }
    }
}

/// The [indicator](crate::core::Element) of labeled control `node` (its
/// box, drawn without a transform), if it has one: found among its
/// descendants that aren't controls of their own.
fn indicator_of(node: &LayoutNode) -> Option<&LayoutNode> {
    node.children
        .iter()
        .filter(|c| c.action.is_none())
        .find_map(|c| {
            if c.indicator && c.transform.is_none() {
                Some(c)
            } else {
                indicator_of(c)
            }
        })
}

/// Tell each OS-drawn control which interaction state to draw itself in.
pub(crate) fn mark_native_states(
    node: &mut LayoutNode,
    hovered: Option<ActionId>,
    pressed: Option<ActionId>,
    focused: Option<FocusId>,
) {
    mark_native_states_in(node, (None, None), hovered, pressed, focused);
}

/// [`mark_native_states`], under the tappable and focusable element `owner`
/// around `node`: a labeled control's box draws in the state of the control.
fn mark_native_states_in(
    node: &mut LayoutNode,
    owner: (Option<ActionId>, Option<FocusId>),
    hovered: Option<ActionId>,
    pressed: Option<ActionId>,
    focused: Option<FocusId>,
) {
    let (action, focus) = if node.indicator {
        (node.action.or(owner.0), node.focus.or(owner.1))
    } else {
        (node.action, node.focus)
    };
    if let Some(native) = &mut node.native {
        native.state = NativeState {
            hovered: action.is_some() && action == hovered,
            pressed: action.is_some() && action == pressed,
            focused: focus.is_some() && focus == focused,
        };
    }
    let owner = if node.action.is_some() {
        (node.action, node.focus)
    } else {
        owner
    };
    for child in &mut node.children {
        mark_native_states_in(child, owner, hovered, pressed, focused);
    }
}

/// Have the OS draw `node`'s chrome. Returns `false` if it can't.
fn paint_native(node: &LayoutNode, scene: &mut Scene) -> bool {
    let Some(deco) = node.native else {
        return false;
    };
    let scale = scene.scale();
    let size = PhysicalSize::new(
        (node.bounds.width() * scale).round() as u32,
        (node.bounds.height() * scale).round() as u32,
    );
    let Some(image) = crate::platform::native::render(deco, size, scale) else {
        return false;
    };
    scene.draw_image(node.bounds, image);
    // Fading from another state's picture: that one over it, going.
    if let Some((from, shows)) = node.native_from
        && shows > 0.0
        && let Some(old) = crate::platform::native::render(from, size, scale)
    {
        scene.push_layer(Affine::IDENTITY, shows);
        scene.draw_image(node.bounds, old);
        scene.pop_layer();
    }
    true
}

/// Paint a laid-out tree into `scene`, parents before children.
///
/// A node with a [transform](LayoutNode::transform) or a group
/// [opacity](LayoutNode::opacity) below 1 is painted, with its subtree, inside
/// a [`Scene::push_layer`] — so it is drawn moved and composited as a group.
/// A fully transparent node isn't painted at all.
pub fn paint(node: &LayoutNode, scene: &mut Scene, font: Option<&Font>) {
    paint_in(node, scene, font, None);
}

/// [`paint`], leaving out whatever is drawn entirely outside `visible` (in
/// the tree's coordinates: usually the window, or the area about to be
/// rasterized). The rows of a long list scrolled out of their container,
/// say, aren't shaped or put in the scene at all. Under a clip (a scroll
/// container, a clipped panel) what is outside the clip is left out too.
/// Conservative: a subtree is kept when the box around everything it draws
/// (through transforms, a little wider for anti-aliasing and glyph
/// overhang) meets the visible area.
pub fn paint_visible(node: &LayoutNode, scene: &mut Scene, font: Option<&Font>, visible: Rect) {
    if drawn_near(node, visible) {
        paint_in(node, scene, font, Some(visible));
    }
}

/// How far past its box [`paint_visible`] assumes a node may draw:
/// anti-aliased borders, and glyphs overhanging their line box.
pub const CULL_MARGIN: f64 = 16.0;

/// Whether anything `node`'s subtree draws (in its parent's coordinates)
/// may land in `visible`.
fn drawn_near(node: &LayoutNode, visible: Rect) -> bool {
    let near = |r: Rect| {
        r.min_x() - CULL_MARGIN < visible.max_x()
            && visible.min_x() < r.max_x() + CULL_MARGIN
            && r.min_y() - CULL_MARGIN < visible.max_y()
            && visible.min_y() < r.max_y() + CULL_MARGIN
    };
    // Its own box is the quick answer; else what its children draw outside
    // it (overflow, transforms).
    near(node.bounds) || near(crate::core::diff::subtree_extent(node, Affine::IDENTITY))
}

/// [`paint`], culled to `visible` (in `node`'s parent's coordinates) if set.
fn paint_in(node: &LayoutNode, scene: &mut Scene, font: Option<&Font>, visible: Option<Rect>) {
    let affine = node.affine();
    let translucent = node.opacity < 1.0;
    // A live OS control shows itself, over the frame (`core::native_host`).
    if node.opacity <= 0.0 || node.native_live.is_some() {
        return;
    }
    if affine.is_none() && !translucent {
        return paint_node(node, scene, font, visible);
    }
    // Under a transform, the visible area in the node's own coordinates (or
    // no culling, when the transform squashes it to nothing).
    let inner = match (visible, affine) {
        (Some(v), Some(a)) => a.inverse().map(|inv| inv.map_rect(v)),
        (v, _) => v,
    };
    scene.push_layer(affine.unwrap_or(Affine::IDENTITY), node.opacity);
    paint_node(node, scene, font, inner);
    scene.pop_layer();
}

/// [`paint`] for one node and its subtree, inside whatever layer it needs.
fn paint_node(node: &LayoutNode, scene: &mut Scene, font: Option<&Font>, visible: Option<Rect>) {
    if !paint_native(node, scene) {
        paint_decoration(
            &node.decoration,
            node.background.as_deref(),
            node.bounds,
            scene,
        );
    }
    match &node.content {
        NodeContent::Text {
            text,
            size,
            color,
            direction,
            underline,
            style,
            ..
        } => {
            // Inside the padding, wrapped to the width left when `wrap` is
            // set; right-to-left paragraphs line up against the right edge.
            let rect = node.text_rect();
            if let Some(f) = style.resolve(font).as_ref()
                && !text.is_empty()
                && let Some(layout) = text_layout(
                    style,
                    font,
                    text,
                    *size,
                    *direction,
                    node.wrap,
                    rect.width(),
                    node.rtl,
                )
            {
                let o = rect.origin;
                // Selected read-only text: the highlight under the glyphs.
                if let (Some((s, e)), Some(highlight), None) =
                    (node.selection, style.highlight, node.caret)
                {
                    for r in layout.selection_rects(s, e) {
                        scene.fill_rect(r.translate(Vec2::new(o.x, o.y)), highlight);
                    }
                }
                let mut paint =
                    crate::render::TextPaint::new(f, *size, *color).decoration(style.decoration);
                if let Some(outline) = style.outline {
                    paint = paint.outline(outline);
                }
                if let Some(&shadow) = style.shadow.as_deref() {
                    paint = paint.shadow(shadow);
                }
                scene.paint_layout(&layout, o, &paint);
                if let Some((start, end)) = *underline {
                    // Just under the baseline, as thick as the strokes.
                    let thick = (size / 14.0).max(1.0);
                    let below = f.baseline(*size) + (size / 10.0).max(1.0);
                    for r in layout.selection_rects(start, end.min(text.len())) {
                        let at = Rect::from_xywh(
                            o.x + r.min_x(),
                            o.y + r.min_y() + below,
                            r.width(),
                            thick,
                        );
                        scene.fill_rect(at, *color);
                    }
                }
            }
        }
        // Reserve the embedded-content area: paint a placeholder and record the
        // viewport command so the app/GPU composites real content into the rect.
        NodeContent::Viewport(id) => {
            scene.fill_viewport(node.bounds, id.0 as u64, VIEWPORT_PLACEHOLDER);
        }
        NodeContent::Image {
            source,
            fit,
            padding,
        } => {
            scene.draw_image_source(node.bounds.inset(*padding), source, *fit);
        }
        // The app draws it, when it is rasterized (see `Scene::draw_canvas`).
        NodeContent::Canvas { paint, id, .. } => {
            scene.draw_canvas_with_id(node.bounds, &paint.0, Some(*id))
        }
        NodeContent::None => {}
    }
    // Clip children to this node's bounds (scroll containers, overlay panels)
    // so overflowing content is masked to the viewport.
    let children = |visible: Option<Rect>, scene: &mut Scene| {
        for child in &node.children {
            if visible.is_none_or(|v| drawn_near(child, v)) {
                paint_in(child, scene, font, visible);
            }
        }
    };
    if node.clip && !node.children.is_empty() {
        // Nothing outside the clip shows (when culling at all).
        let clipped = match visible {
            Some(v) => v.intersection(node.bounds),
            None => None,
        };
        if visible.is_some() && clipped.is_none() {
            return;
        }
        scene.push_clip(node.bounds);
        children(clipped, scene);
        scene.pop_clip();
    } else {
        children(visible, scene);
    }
    // A scroll container's bar, over its content.
    if let Some(bar) = &node.scroll_bar {
        crate::core::scroll_bar::paint(bar, scene);
    }
    if let Some(bar) = &node.scroll_bar_x {
        crate::core::scroll_bar::paint(bar, scene);
    }
}

/// Apply scroll offsets to a laid-out tree: for each scroll container, clamp its
/// stored offset to the content overflow and shift its descendants by that
/// amount, up for a vertical container and sideways for a
/// [horizontal](LayoutNode::scroll_horizontal) one (so the offset is the
/// source of truth, re-applied each frame). Returns nothing; clamps `offsets`
/// in place so the app keeps valid values.
pub(crate) fn apply_scroll(
    node: &mut LayoutNode,
    offsets: &mut std::collections::HashMap<ScrollId, f64>,
) {
    if let Some(id) = node.scroll {
        let max_off = scroll_range(node, 0.0);
        let off = offsets.entry(id).or_insert(0.0);
        *off = off.clamp(0.0, max_off);
        let mut shift = scroll_shift(node, *off);
        if let Some(x) = node.scroll_x {
            let max_x = scroll_range_x(node, 0.0);
            let off = offsets.entry(x).or_insert(0.0);
            *off = off.clamp(0.0, max_x);
            // Right to left, the content starts at the right and moves right.
            if node.rtl {
                shift.dx += *off;
            } else {
                shift.dx -= *off;
            }
        }
        if shift != Vec2::ZERO {
            for child in &mut node.children {
                translate(child, shift);
            }
        }
    }
    for child in &mut node.children {
        apply_scroll(child, offsets);
    }
}

/// How far scroll container `node` can scroll (its largest offset), given its
/// children are already shifted by offset `applied`: the content's extent
/// past the viewport along the container's axis.
pub(crate) fn scroll_range(node: &LayoutNode, applied: f64) -> f64 {
    let b = node.bounds;
    let content = if !node.scroll_horizontal {
        // Content height = furthest child extent below the viewport top.
        let bottom = node
            .children
            .iter()
            .map(|c| c.bounds.max_y())
            .fold(b.min_y(), f64::max);
        // Below it, a container scrolling both ways keeps its horizontal
        // bar's gutter.
        bottom + applied - b.min_y() + node.scroll_tail.1
    } else if node.rtl {
        // Laid out right to left, the content overflows to the left.
        let left = node
            .children
            .iter()
            .map(|c| c.bounds.min_x())
            .fold(b.max_x(), f64::min);
        b.max_x() - (left - applied)
    } else {
        let right = node
            .children
            .iter()
            .map(|c| c.bounds.max_x())
            .fold(b.min_x(), f64::max);
        right + applied - b.min_x()
    };
    let viewport = if node.scroll_horizontal {
        b.width()
    } else {
        b.height()
    };
    (content - viewport).max(0.0)
}

/// How far the sideways scrolling ([`scroll_x`](LayoutNode::scroll_x)) of
/// vertical scroll container `node` can go, its children shifted by
/// `applied` (left; right, right to left): how far they reach past its
/// right edge (its left one), and the vertical bar's gutter beside them
/// for one that keeps both (see [`LayoutNode::scroll_tail`]).
pub(crate) fn scroll_range_x(node: &LayoutNode, applied: f64) -> f64 {
    let b = node.bounds;
    let past = if node.rtl {
        let left = node
            .children
            .iter()
            .map(|c| c.bounds.min_x())
            .fold(b.max_x(), f64::min);
        b.min_x() - (left - applied)
    } else {
        let right = node
            .children
            .iter()
            .map(|c| c.bounds.max_x())
            .fold(b.min_x(), f64::max);
        right + applied - b.max_x()
    };
    if past < 0.5 {
        return 0.0;
    }
    past + node.scroll_tail.0
}

/// Whether container `node`'s offset by `id` counts from the right: a
/// right-to-left sideways strip, or a right-to-left container's sideways
/// scrolling.
pub(crate) fn scrolls_from_right(node: &LayoutNode, id: ScrollId) -> bool {
    node.rtl && (node.scroll_horizontal || node.scroll_x == Some(id))
}

/// How far `node` can scroll by `id`: along its axis for its scroll, or
/// sideways for its [`scroll_x`](LayoutNode::scroll_x).
pub(crate) fn scroll_range_for(node: &LayoutNode, id: ScrollId, applied: f64) -> f64 {
    if node.scroll_x == Some(id) {
        scroll_range_x(node, applied)
    } else {
        scroll_range(node, applied)
    }
}

/// How scroll container `node`'s children move at offset `off`: up, left, or
/// right when it is laid out right to left.
pub(crate) fn scroll_shift(node: &LayoutNode, off: f64) -> Vec2 {
    match (node.scroll_horizontal, node.rtl) {
        (false, _) => Vec2::new(0.0, -off),
        (true, false) => Vec2::new(-off, 0.0),
        (true, true) => Vec2::new(off, 0.0),
    }
}

/// Shift a node and all its descendants by `by` (logical pixels).
pub(crate) fn translate(node: &mut LayoutNode, by: Vec2) {
    node.bounds = node.bounds.translate(by);
    for child in &mut node.children {
        translate(child, by);
    }
}

/// Scroll the vertical scroll containers around focused node `focused`,
/// innermost first, just far enough to show `field` (its box) or, when that
/// doesn't fit, `caret`, with a small margin: what keeps a text field in view
/// while it has focus. `offsets` takes the new offsets and the tree is moved
/// to match (as [`apply_scroll`] does).
pub(crate) fn reveal(
    tree: &mut LayoutNode,
    focused: FocusId,
    caret: Rect,
    field: Rect,
    offsets: &mut std::collections::HashMap<ScrollId, f64>,
) {
    fn path(node: &LayoutNode, focused: FocusId, at: &mut Vec<usize>) -> bool {
        if node.focus == Some(focused) {
            return true;
        }
        for (i, child) in node.children.iter().enumerate() {
            at.push(i);
            if path(child, focused, at) {
                return true;
            }
            at.pop();
        }
        false
    }
    let mut at = Vec::new();
    if !path(tree, focused, &mut at) {
        return;
    }
    const MARGIN: f64 = 8.0;
    let (mut caret, mut field) = (caret, field);
    for depth in (0..at.len()).rev() {
        let mut node = &mut *tree;
        for &i in &at[..depth] {
            node = &mut node.children[i];
        }
        let Some(id) = node.scroll.filter(|_| !node.scroll_horizontal) else {
            continue;
        };
        let view = node.bounds;
        // The whole field when it fits, else its caret's line.
        let want = if field.height() + 2.0 * MARGIN <= view.height() {
            field
        } else {
            caret
        };
        let margin = MARGIN.min(((view.height() - want.height()) / 2.0).max(0.0));
        let delta = if want.min_y() - margin < view.min_y() {
            want.min_y() - margin - view.min_y()
        } else if want.max_y() + margin > view.max_y() {
            want.max_y() + margin - view.max_y()
        } else {
            continue;
        };
        let off = offsets.get(&id).copied().unwrap_or(0.0);
        let new = (off + delta).clamp(0.0, scroll_range(node, off));
        let moved = new - off;
        if moved.abs() < f64::EPSILON {
            continue;
        }
        offsets.insert(id, new);
        let by = Vec2::new(0.0, -moved);
        for child in &mut node.children {
            translate(child, by);
        }
        caret = caret.translate(by);
        field = field.translate(by);
    }
}

/// `rect` reflected left to right within `within`: the same size, as far from
/// `within`'s right edge as it was from its left.
fn mirror_x(rect: Rect, within: Rect) -> Rect {
    let span = Span {
        offset: rect.min_x() - within.min_x(),
        length: rect.width(),
    }
    .mirrored(within.width());
    Rect::from_xywh(
        within.min_x() + span.offset,
        rect.min_y(),
        span.length,
        rect.height(),
    )
}

fn child_rect(
    axis: Axis,
    content: Rect,
    main_off: f64,
    main_len: f64,
    cross_off: f64,
    cross_len: f64,
) -> Rect {
    match axis {
        Axis::Horizontal => Rect::from_xywh(
            content.min_x() + main_off,
            content.min_y() + cross_off,
            main_len,
            cross_len,
        ),
        Axis::Vertical => Rect::from_xywh(
            content.min_x() + cross_off,
            content.min_y() + main_off,
            cross_len,
            main_len,
        ),
    }
}

/// An element's box: its fill, then its background image, then its border.
fn paint_decoration(
    deco: &BoxStyle,
    background: Option<&crate::render::BackgroundImage>,
    bounds: Rect,
    scene: &mut Scene,
) {
    if let Some(fill) = deco.fill {
        if deco.radius > 0.0 {
            scene.fill_round_rect(bounds, deco.radius, fill);
        } else {
            scene.fill_rect(bounds, fill);
        }
    }
    if let Some(background) = background {
        scene.draw_background(bounds, background);
    }
    if let Some((color, width)) = deco.border {
        if deco.radius > 0.0 {
            scene.stroke_round_rect(bounds, deco.radius, color, width);
        } else {
            scene.stroke_rect(bounds, color, width);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::element::BoxStyle;
    use crate::render::Color;

    /// A vertical scroll container of `rows` 20-pixel rows, with nothing
    /// fixing its height.
    fn list(rows: usize) -> Element {
        let rows = (0..rows)
            .map(|_| Element::boxed(BoxStyle::default()).height(20.0))
            .collect();
        Element::stack(Axis::Vertical, rows).scrollable(ScrollId(0))
    }

    fn bar() -> Element {
        Element::boxed(BoxStyle::default()).height(50.0)
    }

    #[test]
    fn a_scroll_fills_the_height_its_column_leaves() {
        let window = Rect::from_xywh(0.0, 0.0, 200.0, 400.0);
        for rows in [2, 100] {
            let root = Element::stack(Axis::Vertical, vec![bar(), list(rows), bar()]);
            let laid = layout(&root, window, None);
            let heights: Vec<f64> = laid.children.iter().map(|c| c.bounds.height()).collect();
            assert_eq!(heights, [50.0, 300.0, 50.0], "{rows} rows");
        }
    }

    #[test]
    fn a_scroll_crowded_out_keeps_its_floor() {
        let window = Rect::from_xywh(0.0, 0.0, 200.0, 150.0);
        let root = Element::stack(Axis::Vertical, vec![bar(), list(100), bar()]);
        let laid = layout(&root, window, None);
        assert_eq!(laid.children[1].bounds.height(), SCROLL_MIN);
        // Less content, a lower floor; a min height, another.
        let root = Element::stack(Axis::Vertical, vec![bar(), list(2), bar(), bar()]);
        let laid = layout(&root, window, None);
        assert_eq!(laid.children[1].bounds.height(), 40.0);
        let root = Element::stack(
            Axis::Vertical,
            vec![bar(), list(100).min_height(60.0), bar()],
        );
        let laid = layout(&root, window, None);
        assert_eq!(laid.children[1].bounds.height(), 60.0);
    }

    #[test]
    fn a_scroll_with_nothing_to_fill_is_as_tall_as_its_content_and_the_room() {
        let room = Size::new(200.0, 300.0);
        assert_eq!(measure(&list(2), room, None).height, 40.0);
        assert_eq!(measure(&list(100), room, None).height, 300.0);
        assert_eq!(
            measure(&list(100).max_height(120.0), room, None).height,
            120.0
        );
        assert_eq!(measure(&list(100).height(80.0), room, None).height, 80.0);
        // In an auto-sized panel: the panel hugs it.
        let panel = Element::stack(Axis::Vertical, vec![bar(), list(2)]);
        assert_eq!(measure(&panel, room, None).height, 90.0);
        // A fixed height doesn't grow.
        let root = Element::stack(Axis::Vertical, vec![list(100).height(80.0), bar()]);
        let laid = layout(&root, Rect::from_xywh(0.0, 0.0, 200.0, 400.0), None);
        assert_eq!(laid.children[0].bounds.height(), 80.0);
    }

    #[test]
    fn a_vertical_scroll_in_a_row_does_not_grow_across() {
        let root = Element::stack(
            Axis::Horizontal,
            vec![list(2).width(50.0), bar().width(30.0)],
        );
        let laid = layout(&root, Rect::from_xywh(0.0, 0.0, 400.0, 100.0), None);
        assert_eq!(laid.children[0].bounds.width(), 50.0);
        // A sideways one does.
        let strip = Element::stack(
            Axis::Horizontal,
            vec![Element::boxed(BoxStyle::default()).width(30.0)],
        )
        .scrollable(ScrollId(1));
        let root = Element::stack(Axis::Horizontal, vec![strip, bar().width(30.0)]);
        let laid = layout(&root, Rect::from_xywh(0.0, 0.0, 400.0, 100.0), None);
        assert_eq!(laid.children[0].bounds.width(), 370.0);
    }

    /// Wrapped text inside a fixed-width box wraps to that box, so the box's
    /// measured height must count every wrapped line.
    #[test]
    fn fixed_width_constrains_wrapped_text_height() {
        let Some(font) = crate::render::Font::system_default() else {
            return; // no system font available on this host
        };
        let text = "several words that need to wrap inside a rather narrow fixed width box";
        let para = || Element::text(text, 14.0, Color::rgb(0, 0, 0)).wrap();
        let boxed = Element::stack(Axis::Vertical, vec![para()]).width(120.0);
        let wide = Size::new(4000.0, 4000.0);
        let lines = font.wrap(text, 14.0, 120.0).len();
        assert!(lines > 1);
        let h = measure(&boxed, wide, Some(&font)).height;
        assert_eq!(h, font.line_height(14.0) * lines as f64);
    }

    #[test]
    fn measure_sums_children_with_gap_and_padding() {
        let child = || Element::boxed(BoxStyle::default()).width(20.0).height(10.0);
        let stack = Element::stack(Axis::Vertical, vec![child(), child(), child()])
            .gap(5.0)
            .padding(crate::geometry::Insets::uniform(4.0));
        // main (vertical): 3*10 + 2*5 + 2*4 = 48; cross (width): 20 + 2*4 = 28
        let size = measure(&stack, Size::new(1000.0, 1000.0), None);
        assert_eq!(size, Size::new(28.0, 48.0));
    }

    #[test]
    fn caret_index_at_resolves_pointer_to_byte_index() {
        let Some(font) = Font::system_default() else {
            eprintln!("skipping: no system font found");
            return;
        };
        // A text leaf "hello" at x origin 10.
        let el = Element::text("hello", 16.0, Color::BLACK);
        let node = layout(&el, Rect::from_xywh(10.0, 0.0, 200.0, 20.0), Some(&font));
        let y = node.bounds.min_y() + 1.0;
        // Far left → index 0; far right → end (5).
        assert_eq!(
            caret_index_at(&node, Point::new(0.0, y), Some(&font)),
            Some(0)
        );
        assert_eq!(
            caret_index_at(&node, Point::new(9.0, y), Some(&font)),
            Some(0)
        );
        assert_eq!(
            caret_index_at(&node, Point::new(10_000.0, y), Some(&font)),
            Some(5)
        );
        // A point near the middle lands on an interior boundary (1..=4).
        let mid = node.bounds.min_x() + font.measure("hel", 16.0).width;
        let i = caret_index_at(&node, Point::new(mid, y), Some(&font)).unwrap();
        assert!((1..=4).contains(&i), "mid index {i} out of range");
        // Without a font, no resolution is possible.
        assert_eq!(caret_index_at(&node, Point::new(mid, y), None), None);
    }

    #[test]
    fn viewport_reserves_its_rect_and_paints_a_viewport_command() {
        use crate::core::runtime::{ViewportId, collect_viewports, viewport_at};
        use crate::render::DrawCmd;

        let el = Element::viewport(ViewportId(3)).width(120.0).height(80.0);
        // Sizes to its overrides, not to content.
        assert_eq!(
            measure(&el, Size::new(1000.0, 1000.0), None),
            Size::new(120.0, 80.0)
        );

        let tree = layout(&el, Rect::from_xywh(20.0, 10.0, 120.0, 80.0), None);
        assert!(matches!(tree.content, NodeContent::Viewport(ViewportId(3))));

        // collect_viewports / viewport_at locate it by id + bounds for the compositor.
        let mut found = Vec::new();
        collect_viewports(&tree, &mut found);
        assert_eq!(
            found,
            vec![(ViewportId(3), Rect::from_xywh(20.0, 10.0, 120.0, 80.0))]
        );
        assert_eq!(
            viewport_at(&tree, Point::new(30.0, 20.0)).map(|(id, _)| id),
            Some(ViewportId(3))
        );
        assert_eq!(viewport_at(&tree, Point::new(0.0, 0.0)), None);

        // Painting records a Viewport draw command carrying the id.
        let mut scene = Scene::new(Size::new(200.0, 120.0));
        paint(&tree, &mut scene, None);
        assert!(
            scene
                .commands()
                .iter()
                .any(|c| matches!(c, DrawCmd::Viewport { id: 3, .. }))
        );
    }

    #[test]
    fn grow_child_fills_main_axis() {
        // A row with one fixed 40px box and one grow=1 box in 200px width.
        let fixed = Element::boxed(BoxStyle::default()).width(40.0).height(10.0);
        let flex = Element::boxed(BoxStyle {
            fill: Some(Color::WHITE),
            ..BoxStyle::default()
        })
        .grow(1.0);
        let row =
            Element::stack(Axis::Horizontal, vec![fixed, flex]).align(Align::Start, Align::Stretch);

        let tree = layout(&row, Rect::from_xywh(0.0, 0.0, 200.0, 20.0), None);
        // The flex child occupies the leftover: 200 - 40 = 160px, at x=40,
        // stretched to the full 20px cross height.
        assert_eq!(
            tree.children[1].bounds,
            Rect::from_xywh(40.0, 0.0, 160.0, 20.0)
        );

        let mut scene = Scene::new(Size::new(200.0, 20.0));
        paint(&tree, &mut scene, None);
        assert_eq!(scene.len(), 1); // only the filled flex box paints
    }

    fn block(w: f64, h: f64) -> Element {
        Element::boxed(BoxStyle::default()).width(w).height(h)
    }

    /// An unsized box with a minimum size: natural size `w`×`h`, but free to
    /// stretch (a fixed-size box keeps its size in a grid cell).
    fn cell(w: f64, h: f64) -> Element {
        Element::boxed(BoxStyle::default())
            .min_width(w)
            .min_height(h)
    }

    fn rects(node: &LayoutNode) -> Vec<Rect> {
        node.children.iter().map(|c| c.bounds).collect()
    }

    #[test]
    fn measure_clamps_to_min_and_max() {
        let big = Size::new(1000.0, 1000.0);
        // Content 20×10, but min-width 50 and max-height 5.
        let el = Element::stack(Axis::Vertical, vec![block(20.0, 10.0)])
            .min_width(50.0)
            .max_height(5.0);
        assert_eq!(measure(&el, big, None), Size::new(50.0, 5.0));
        // A fixed size is clamped too.
        let el = block(300.0, 300.0).max_width(120.0).min_height(400.0);
        assert_eq!(measure(&el, big, None), Size::new(120.0, 400.0));
    }

    #[test]
    fn max_width_constrains_wrapped_text() {
        let Some(font) = Font::system_default() else {
            return;
        };
        let text = "several words that need to wrap inside a rather narrow maximum width";
        let el = Element::text(text, 14.0, Color::BLACK)
            .wrap()
            .max_width(120.0);
        let lines = font.wrap(text, 14.0, 120.0).len();
        assert!(lines > 1);
        let s = measure(&el, Size::new(4000.0, 4000.0), Some(&font));
        assert!(s.width <= 120.0);
        assert_eq!(s.height, font.line_height(14.0) * lines as f64);
    }

    #[test]
    fn rows_center_and_columns_stretch_by_default() {
        // A row centers its children across.
        let row = Element::stack(Axis::Horizontal, vec![block(20.0, 10.0), block(20.0, 30.0)]);
        let tree = layout(&row, Rect::from_xywh(0.0, 0.0, 100.0, 40.0), None);
        assert_eq!(
            rects(&tree),
            vec![
                Rect::from_xywh(0.0, 15.0, 20.0, 10.0),
                Rect::from_xywh(20.0, 5.0, 20.0, 30.0),
            ]
        );
        // A column stretches what has no width (a row in it spans it, so a
        // growing child pushes), leaves a sized child alone, and a child can
        // opt out with `align_self`.
        let col = Element::stack(
            Axis::Vertical,
            vec![
                Element::stack(
                    Axis::Horizontal,
                    vec![
                        block(10.0, 10.0),
                        Element::boxed(BoxStyle::default()).grow(1.0),
                        block(10.0, 10.0),
                    ],
                ),
                block(30.0, 10.0),
                cell(30.0, 10.0).align_self(Align::Center),
                cell(30.0, 10.0).align_self(Align::End),
            ],
        );
        let tree = layout(&col, Rect::from_xywh(0.0, 0.0, 100.0, 100.0), None);
        assert_eq!(
            tree.children[0].bounds,
            Rect::from_xywh(0.0, 0.0, 100.0, 10.0)
        );
        assert_eq!(
            tree.children[0].children[2].bounds,
            Rect::from_xywh(90.0, 0.0, 10.0, 10.0)
        );
        assert_eq!(
            tree.children[1].bounds,
            Rect::from_xywh(0.0, 10.0, 30.0, 10.0)
        );
        assert_eq!(
            tree.children[2].bounds,
            Rect::from_xywh(35.0, 20.0, 30.0, 10.0)
        );
        assert_eq!(
            tree.children[3].bounds,
            Rect::from_xywh(70.0, 30.0, 30.0, 10.0)
        );
        // `align_self(Stretch)` in a centering column fills it.
        let col = Element::stack(
            Axis::Vertical,
            vec![cell(30.0, 10.0).align_self(Align::Stretch)],
        )
        .align(Align::Start, Align::Center);
        let tree = layout(&col, Rect::from_xywh(0.0, 0.0, 100.0, 100.0), None);
        assert_eq!(
            tree.children[0].bounds,
            Rect::from_xywh(0.0, 0.0, 100.0, 10.0)
        );
    }

    #[test]
    fn layout_grow_stops_at_max_width() {
        let row = Element::stack(
            Axis::Horizontal,
            vec![
                Element::boxed(BoxStyle::default())
                    .grow(1.0)
                    .max_width(50.0),
                Element::boxed(BoxStyle::default()).grow(1.0),
            ],
        )
        .gap(10.0);
        let tree = layout(&row, Rect::from_xywh(0.0, 0.0, 210.0, 20.0), None);
        assert_eq!(tree.children[0].bounds.width(), 50.0);
        // (An empty box in a row sits at its middle.)
        assert_eq!(
            tree.children[1].bounds,
            Rect::from_xywh(60.0, 10.0, 150.0, 0.0)
        );
    }

    #[test]
    fn layout_shrink_stops_at_min_width() {
        // 150 + 150 in 200: both shrink, but the first stops at 120.
        let row = Element::stack(
            Axis::Horizontal,
            vec![
                block(150.0, 10.0).shrink(1.0).min_width(120.0),
                block(150.0, 10.0).shrink(1.0),
            ],
        );
        let tree = layout(&row, Rect::from_xywh(0.0, 0.0, 200.0, 10.0), None);
        assert_eq!(
            rects(&tree),
            vec![
                Rect::from_xywh(0.0, 0.0, 120.0, 10.0),
                Rect::from_xywh(120.0, 0.0, 80.0, 10.0)
            ]
        );
    }

    #[test]
    fn stretch_honors_cross_max_and_min() {
        let col = Element::stack(
            Axis::Vertical,
            vec![
                Element::boxed(BoxStyle::default())
                    .height(10.0)
                    .max_width(80.0),
                Element::boxed(BoxStyle::default()).height(10.0),
            ],
        )
        .align(Align::Start, Align::Stretch);
        let tree = layout(&col, Rect::from_xywh(0.0, 0.0, 200.0, 100.0), None);
        assert_eq!(tree.children[0].bounds.width(), 80.0);
        assert_eq!(tree.children[1].bounds.width(), 200.0);
        // Centered, a child wider than the column still keeps its min width.
        let col = Element::stack(Axis::Vertical, vec![block(10.0, 10.0).min_width(300.0)])
            .align(Align::Start, Align::Center);
        let tree = layout(&col, Rect::from_xywh(0.0, 0.0, 200.0, 100.0), None);
        assert_eq!(
            tree.children[0].bounds,
            Rect::from_xywh(-50.0, 0.0, 300.0, 10.0)
        );
    }

    #[test]
    fn wrapping_row_measures_lines() {
        // Five 40×20 blocks, gap 10, in 150 wide: 3 per line (140), then 2.
        let row = Element::stack(
            Axis::Horizontal,
            (0..5).map(|_| block(40.0, 20.0)).collect(),
        )
        .gap(10.0)
        .wrap();
        assert_eq!(
            measure(&row, Size::new(150.0, 1000.0), None),
            Size::new(140.0, 50.0)
        );
        // With unlimited width everything fits on one line.
        assert_eq!(
            measure(&row, Size::new(f64::INFINITY, 1000.0), None),
            Size::new(240.0, 20.0)
        );
        // Padding is outside the wrapping width.
        let padded = row.padding(crate::geometry::Insets::uniform(5.0));
        assert_eq!(
            measure(&padded, Size::new(160.0, 1000.0), None),
            Size::new(150.0, 60.0)
        );
    }

    #[test]
    fn wrapping_row_lays_out_lines_with_per_line_alignment() {
        // Line 1: 40×20, 40×30 (cross 30); line 2: 40×10. Width 100, gap 10.
        let row = Element::stack(
            Axis::Horizontal,
            vec![block(40.0, 20.0), block(40.0, 30.0), block(40.0, 10.0)],
        )
        .gap(10.0)
        .wrap()
        .align(Align::Center, Align::End);
        let tree = layout(&row, Rect::from_xywh(0.0, 0.0, 100.0, 200.0), None);
        assert_eq!(
            rects(&tree),
            vec![
                // line 1 is 90 wide → shifted 5; children end-aligned in 30.
                Rect::from_xywh(5.0, 10.0, 40.0, 20.0),
                Rect::from_xywh(55.0, 0.0, 40.0, 30.0),
                // line 2 starts at 30 + 10; 40 wide → shifted 30.
                Rect::from_xywh(30.0, 40.0, 40.0, 10.0),
            ]
        );
    }

    #[test]
    fn wrapping_row_grows_and_stretches_within_each_line() {
        let row = Element::stack(
            Axis::Horizontal,
            vec![
                block(40.0, 20.0),
                cell(40.0, 10.0).grow(1.0),
                cell(40.0, 10.0).grow(1.0),
            ],
        )
        .gap(10.0)
        .wrap()
        .align(Align::Start, Align::Stretch);
        let tree = layout(&row, Rect::from_xywh(0.0, 0.0, 100.0, 200.0), None);
        assert_eq!(
            rects(&tree),
            vec![
                Rect::from_xywh(0.0, 0.0, 40.0, 20.0),
                Rect::from_xywh(50.0, 0.0, 50.0, 20.0),
                Rect::from_xywh(0.0, 30.0, 100.0, 10.0),
            ]
        );
    }

    #[test]
    fn grid_places_cells_in_fixed_auto_fr_columns() {
        let g = Element::grid(
            vec![Track::px(50.0), Track::Auto, Track::fr(1.0)],
            vec![
                cell(10.0, 10.0),
                cell(30.0, 20.0),
                cell(5.0, 5.0),
                cell(10.0, 40.0),
            ],
        )
        .gap(10.0)
        .padding(crate::geometry::Insets::uniform(5.0));
        // Width fills the 300 available; rows: 20, then 40.
        assert_eq!(
            measure(&g, Size::new(300.0, 1000.0), None),
            Size::new(300.0, 5.0 + 20.0 + 10.0 + 40.0 + 5.0)
        );
        let tree = layout(&g, Rect::from_xywh(0.0, 0.0, 300.0, 80.0), None);
        // fr column: 290 - 50 - 30 - 20 = 190, at 5 + 50 + 10 + 30 + 10 = 105.
        assert_eq!(
            rects(&tree),
            vec![
                Rect::from_xywh(5.0, 5.0, 50.0, 20.0),
                Rect::from_xywh(65.0, 5.0, 30.0, 20.0),
                Rect::from_xywh(105.0, 5.0, 190.0, 20.0),
                Rect::from_xywh(5.0, 35.0, 50.0, 40.0),
            ]
        );
    }

    #[test]
    fn grid_aligns_cells_and_honors_bounds() {
        let g = Element::grid(
            vec![Track::px(100.0), Track::px(100.0)],
            vec![block(20.0, 10.0), block(20.0, 30.0).max_width(60.0)],
        )
        .align(Align::Center, Align::End);
        let tree = layout(&g, Rect::from_xywh(0.0, 0.0, 200.0, 30.0), None);
        assert_eq!(
            rects(&tree),
            vec![
                Rect::from_xywh(40.0, 20.0, 20.0, 10.0),
                Rect::from_xywh(140.0, 0.0, 20.0, 30.0),
            ]
        );
        // Stretched, the max width still caps the cell's child.
        let g = Element::grid(
            vec![Track::px(100.0)],
            vec![Element::boxed(BoxStyle::default()).max_width(60.0)],
        )
        .rows(vec![Track::px(30.0)]);
        let tree = layout(&g, Rect::from_xywh(0.0, 0.0, 100.0, 30.0), None);
        assert_eq!(
            tree.children[0].bounds,
            Rect::from_xywh(0.0, 0.0, 60.0, 30.0)
        );
    }

    #[test]
    fn grid_cells_align_themselves() {
        // Cells stretch by default; a child's own `align_self` (vertical)
        // and `justify_self` (horizontal) place it in its cell instead.
        let g = Element::grid(
            vec![Track::px(100.0), Track::px(100.0), Track::px(100.0)],
            vec![
                cell(20.0, 10.0),
                cell(20.0, 10.0)
                    .align_self(Align::End)
                    .justify_self(Align::Center),
                cell(20.0, 10.0).align_self(Align::Center),
            ],
        )
        .rows(vec![Track::px(40.0)]);
        let tree = layout(&g, Rect::from_xywh(0.0, 0.0, 300.0, 40.0), None);
        assert_eq!(
            rects(&tree),
            vec![
                Rect::from_xywh(0.0, 0.0, 100.0, 40.0),
                Rect::from_xywh(140.0, 30.0, 20.0, 10.0),
                Rect::from_xywh(200.0, 15.0, 100.0, 10.0),
            ]
        );
        // Over a grid aligning its cells, `Stretch` fills the cell again,
        // except on an axis the child has a fixed size on.
        let g = Element::grid(
            vec![Track::px(100.0), Track::px(100.0)],
            vec![
                cell(20.0, 10.0)
                    .align_self(Align::Stretch)
                    .justify_self(Align::Stretch),
                block(20.0, 10.0).justify_self(Align::Stretch),
            ],
        )
        .rows(vec![Track::px(40.0)])
        .align(Align::End, Align::Start);
        let tree = layout(&g, Rect::from_xywh(0.0, 0.0, 200.0, 40.0), None);
        assert_eq!(
            rects(&tree),
            vec![
                Rect::from_xywh(0.0, 0.0, 100.0, 40.0),
                Rect::from_xywh(100.0, 0.0, 20.0, 10.0),
            ]
        );
        // Right to left, `justify_self(Start)` is the cell's right edge.
        let g = Element::grid(
            vec![Track::px(100.0)],
            vec![block(20.0, 10.0).justify_self(Align::Start)],
        )
        .align(Align::End, Align::Start);
        let tree = rtl(&g, Rect::from_xywh(0.0, 0.0, 100.0, 10.0));
        assert_eq!(rects(&tree), vec![Rect::from_xywh(80.0, 0.0, 20.0, 10.0)]);
        // In layers neither has an effect: their `align` places every child.
        let layers = |child: Element| {
            let el = Element::layers(vec![child]).align(Align::End, Align::End);
            rects(&layout(&el, Rect::from_xywh(0.0, 0.0, 100.0, 40.0), None))
        };
        assert_eq!(
            layers(
                block(20.0, 10.0)
                    .align_self(Align::Start)
                    .justify_self(Align::Start)
            ),
            layers(block(20.0, 10.0))
        );
    }

    #[test]
    fn grid_fr_rows_size_to_content_then_share_the_laid_out_height() {
        let cells = || vec![cell(10.0, 10.0), cell(10.0, 30.0)];
        let g = Element::grid(vec![Track::Auto], cells())
            .rows(vec![Track::fr(1.0), Track::fr(1.0)])
            .row_gap(10.0);
        // Indefinite: the 1fr unit fits the taller row → 30 + 10 + 30.
        assert_eq!(
            measure(&g, Size::new(500.0, 500.0), None),
            Size::new(10.0, 70.0)
        );
        // Laid out in 110: (110 - 10) / 2 = 50 each.
        let tree = layout(&g, Rect::from_xywh(0.0, 0.0, 10.0, 110.0), None);
        assert_eq!(
            rects(&tree),
            vec![
                Rect::from_xywh(0.0, 0.0, 10.0, 50.0),
                Rect::from_xywh(0.0, 60.0, 10.0, 50.0),
            ]
        );
    }

    #[test]
    fn grid_spans_cells() {
        // A header spanning both columns, then a 2-row sidebar beside two cells.
        let g = Element::grid(
            vec![Track::px(40.0), Track::px(60.0)],
            vec![
                cell(10.0, 10.0).span(2, 1),
                cell(10.0, 10.0).span(1, 2),
                cell(10.0, 15.0),
                cell(10.0, 15.0),
            ],
        )
        .gap(5.0);
        let tree = layout(&g, Rect::from_xywh(0.0, 0.0, 105.0, 100.0), None);
        assert_eq!(
            rects(&tree),
            vec![
                Rect::from_xywh(0.0, 0.0, 105.0, 10.0),
                Rect::from_xywh(0.0, 15.0, 40.0, 35.0),
                Rect::from_xywh(45.0, 15.0, 60.0, 15.0),
                Rect::from_xywh(45.0, 35.0, 60.0, 15.0),
            ]
        );
    }

    #[test]
    fn grid_wrapped_text_measures_at_its_column_width() {
        let Some(font) = Font::system_default() else {
            return;
        };
        let text = "a paragraph long enough to wrap inside one of two equal grid columns";
        let g = Element::grid(
            vec![Track::fr(1.0), Track::fr(1.0)],
            vec![
                Element::text(text, 14.0, Color::BLACK).wrap(),
                block(10.0, 10.0),
            ],
        );
        let lines = font.wrap(text, 14.0, 150.0).len();
        assert!(lines > 1);
        let h = measure(&g, Size::new(300.0, 1000.0), Some(&font)).height;
        assert_eq!(h, font.line_height(14.0) * lines as f64);
    }

    #[test]
    fn grid_and_wrap_trees_route_input_focus_and_accessibility() {
        use crate::core::a11y::accessibility_tree;
        use crate::core::runtime::{ActionId, FocusId, collect_focusables, focus_at, hit_test};

        let mut tappable = cell(40.0, 20.0);
        tappable.action = Some(ActionId(7));
        let mut field = Element::text("name", 14.0, Color::BLACK);
        field.focus = Some(FocusId(1));
        let mut field2 = block(30.0, 20.0);
        field2.focus = Some(FocusId(2));
        let wrapped = Element::stack(Axis::Horizontal, vec![block(80.0, 20.0), field2])
            .wrap()
            .gap(4.0);
        let g = Element::grid(
            vec![Track::px(100.0), Track::px(100.0)],
            vec![tappable, field, wrapped],
        );
        let tree = layout(&g, Rect::from_xywh(0.0, 0.0, 200.0, 100.0), None);

        // The tappable cell stretches over (0,0)-(100,20).
        assert_eq!(hit_test(&tree, Point::new(90.0, 10.0)), Some(ActionId(7)));
        assert_eq!(hit_test(&tree, Point::new(150.0, 10.0)), None);
        // The wrapped row's second item moved to a new line in its 100px cell.
        assert_eq!(focus_at(&tree, Point::new(10.0, 50.0)), Some(FocusId(2)));
        let mut order = Vec::new();
        collect_focusables(&tree, &mut order);
        assert_eq!(order, vec![FocusId(1), FocusId(2)]);
        let a11y = accessibility_tree(&tree, None);
        assert!(a11y.count() >= 3);
    }

    #[test]
    fn image_measures_to_natural_size_and_keeps_aspect() {
        use crate::geometry::{Insets, PhysicalSize};
        use crate::render::{DrawCmd, ImageFit, Pixmap};
        use std::sync::Arc;

        let pm = Arc::new(Pixmap::new(PhysicalSize::new(40, 20)));
        let avail = Size::new(1000.0, 1000.0);
        assert_eq!(
            measure(&Element::image(&pm), avail, None),
            Size::new(40.0, 20.0)
        );
        // One fixed axis scales the other; both fixed wins outright.
        assert_eq!(
            measure(&Element::image(&pm).width(80.0), avail, None),
            Size::new(80.0, 40.0)
        );
        assert_eq!(
            measure(&Element::image(&pm).height(10.0), avail, None),
            Size::new(20.0, 10.0)
        );
        assert_eq!(
            measure(&Element::image(&pm).width(30.0).height(30.0), avail, None),
            Size::new(30.0, 30.0)
        );
        // Padding adds around the natural size.
        let padded = Element::image(&pm).padding(Insets::uniform(5.0));
        assert_eq!(measure(&padded, avail, None), Size::new(50.0, 30.0));

        // Painting places the image inside the padding, fitted (contain).
        let el = Element::image(&pm)
            .padding(Insets::uniform(5.0))
            .fit(ImageFit::Contain);
        let tree = layout(&el, Rect::from_xywh(0.0, 0.0, 50.0, 50.0), None);
        let mut scene = Scene::new(Size::new(50.0, 50.0));
        paint(&tree, &mut scene, None);
        let rects: Vec<Rect> = scene
            .commands()
            .iter()
            .filter_map(|c| match c {
                DrawCmd::Image { rect, .. } => Some(*rect),
                _ => None,
            })
            .collect();
        assert_eq!(rects, vec![Rect::from_xywh(5.0, 15.0, 40.0, 20.0)]);
    }

    fn rtl(el: &Element, bounds: Rect) -> LayoutNode {
        layout_with_direction(el, bounds, None, LayoutDirection::Rtl)
    }

    #[test]
    fn rtl_row_runs_right_to_left_from_the_start_padding() {
        use crate::geometry::Insets;
        // Padding left 5, right 15: right to left the 5 is the start side, so
        // it lands on the right.
        let row = Element::stack(Axis::Horizontal, vec![block(40.0, 10.0), block(60.0, 10.0)])
            .gap(10.0)
            .padding(Insets::new(0.0, 15.0, 0.0, 5.0));
        let ltr = layout(&row, Rect::from_xywh(0.0, 0.0, 200.0, 10.0), None);
        assert_eq!(
            rects(&ltr),
            vec![
                Rect::from_xywh(5.0, 0.0, 40.0, 10.0),
                Rect::from_xywh(55.0, 0.0, 60.0, 10.0)
            ]
        );
        assert!(!ltr.rtl && ltr.children.iter().all(|c| !c.rtl));
        // Mirrored inside a box that doesn't start at zero.
        let tree = rtl(&row, Rect::from_xywh(100.0, 0.0, 200.0, 10.0));
        assert_eq!(
            rects(&tree),
            vec![
                Rect::from_xywh(255.0, 0.0, 40.0, 10.0),
                Rect::from_xywh(185.0, 0.0, 60.0, 10.0)
            ]
        );
        assert!(tree.rtl && tree.children.iter().all(|c| c.rtl));
    }

    #[test]
    fn rtl_flips_start_and_end_alignment() {
        // Main axis: End packs a row against the left edge.
        let row = Element::stack(Axis::Horizontal, vec![block(40.0, 10.0), block(20.0, 10.0)])
            .align(Align::End, Align::Start);
        let tree = rtl(&row, Rect::from_xywh(0.0, 0.0, 200.0, 10.0));
        assert_eq!(rects(&tree)[1], Rect::from_xywh(0.0, 0.0, 20.0, 10.0));
        assert_eq!(rects(&tree)[0], Rect::from_xywh(20.0, 0.0, 40.0, 10.0));
        // Cross axis of a column: Start is the right edge; vertical order stays.
        let col = Element::stack(Axis::Vertical, vec![block(40.0, 10.0), block(20.0, 10.0)])
            .align(Align::Start, Align::Start);
        let tree = rtl(&col, Rect::from_xywh(0.0, 0.0, 100.0, 50.0));
        assert_eq!(
            rects(&tree),
            vec![
                Rect::from_xywh(60.0, 0.0, 40.0, 10.0),
                Rect::from_xywh(80.0, 10.0, 20.0, 10.0)
            ]
        );
        // Center stays centered.
        let centered = col.clone().align(Align::Start, Align::Center);
        assert_eq!(
            rects(&rtl(&centered, Rect::from_xywh(0.0, 0.0, 100.0, 50.0)))[0],
            Rect::from_xywh(30.0, 0.0, 40.0, 10.0)
        );
    }

    #[test]
    fn rtl_grow_and_wrap_mirror_per_line() {
        // A growing child keeps its width, from the right.
        let row = Element::stack(
            Axis::Horizontal,
            vec![
                block(40.0, 10.0),
                Element::boxed(BoxStyle::default()).grow(1.0),
            ],
        );
        let tree = rtl(&row, Rect::from_xywh(0.0, 0.0, 200.0, 10.0));
        assert_eq!(
            rects(&tree),
            vec![
                Rect::from_xywh(160.0, 0.0, 40.0, 10.0),
                Rect::from_xywh(0.0, 5.0, 160.0, 0.0)
            ]
        );
        // Three 40-wide items in 100: two per line, each line from the right.
        let wrapped = Element::stack(
            Axis::Horizontal,
            vec![block(40.0, 10.0), block(40.0, 10.0), block(40.0, 10.0)],
        )
        .wrap()
        .gap(10.0);
        let tree = rtl(&wrapped, Rect::from_xywh(0.0, 0.0, 100.0, 100.0));
        assert_eq!(
            rects(&tree),
            vec![
                Rect::from_xywh(60.0, 0.0, 40.0, 10.0),
                Rect::from_xywh(10.0, 0.0, 40.0, 10.0),
                Rect::from_xywh(60.0, 20.0, 40.0, 10.0)
            ]
        );
    }

    #[test]
    fn rtl_grid_counts_columns_from_the_right() {
        let g = Element::grid(
            vec![Track::px(50.0), Track::fr(1.0), Track::px(30.0)],
            vec![
                cell(0.0, 10.0),
                cell(0.0, 10.0),
                cell(0.0, 10.0),
                cell(0.0, 10.0).span(2, 1),
            ],
        )
        .gap(10.0);
        let tree = rtl(&g, Rect::from_xywh(0.0, 0.0, 200.0, 100.0));
        let r = rects(&tree);
        assert_eq!(r[0], Rect::from_xywh(150.0, 0.0, 50.0, 10.0));
        assert_eq!(r[1], Rect::from_xywh(40.0, 0.0, 100.0, 10.0));
        assert_eq!(r[2], Rect::from_xywh(0.0, 0.0, 30.0, 10.0));
        // The spanning cell covers the two rightmost columns of the next row.
        assert_eq!(r[3], Rect::from_xywh(40.0, 20.0, 160.0, 10.0));
    }

    #[test]
    fn layout_direction_is_inherited_and_overridable() {
        // An inner row forced left to right sits where the mirrored parent
        // puts it but keeps its own children in left-to-right order.
        let inner = Element::stack(Axis::Horizontal, vec![block(10.0, 10.0), block(20.0, 10.0)])
            .layout_direction(LayoutDirection::Ltr);
        let outer = Element::stack(Axis::Horizontal, vec![block(50.0, 10.0), inner]);
        let tree = rtl(&outer, Rect::from_xywh(0.0, 0.0, 200.0, 10.0));
        let inner = &tree.children[1];
        assert_eq!(inner.bounds, Rect::from_xywh(120.0, 0.0, 30.0, 10.0));
        assert!(!inner.rtl);
        assert_eq!(
            rects(inner),
            vec![
                Rect::from_xywh(120.0, 0.0, 10.0, 10.0),
                Rect::from_xywh(130.0, 0.0, 20.0, 10.0)
            ]
        );
        // And the other way: a mirrored subtree inside a left-to-right tree.
        let sub = Element::stack(Axis::Horizontal, vec![block(10.0, 10.0)])
            .width(100.0)
            .layout_direction(LayoutDirection::Rtl);
        let tree = layout(
            &Element::stack(Axis::Horizontal, vec![sub]),
            Rect::from_xywh(0.0, 0.0, 300.0, 10.0),
            None,
        );
        assert_eq!(
            tree.children[0].children[0].bounds,
            Rect::from_xywh(90.0, 0.0, 10.0, 10.0)
        );
    }

    #[test]
    fn ltr_layout_is_unchanged_by_the_direction_plumbing() {
        use crate::geometry::Insets;
        let el = Element::grid(
            vec![Track::Auto, Track::fr(1.0)],
            vec![
                block(30.0, 10.0),
                Element::stack(
                    Axis::Horizontal,
                    vec![
                        block(10.0, 10.0),
                        Element::boxed(BoxStyle::default()).grow(1.0),
                    ],
                )
                .padding(Insets::new(1.0, 2.0, 3.0, 4.0)),
            ],
        )
        .padding(Insets::new(0.0, 0.0, 0.0, 7.0));
        let b = Rect::from_xywh(3.0, 4.0, 150.0, 60.0);
        fn all(node: &LayoutNode, out: &mut Vec<(Rect, bool)>) {
            out.push((node.bounds, node.rtl));
            for c in &node.children {
                all(c, out);
            }
        }
        let (mut a, mut d) = (Vec::new(), Vec::new());
        all(&layout(&el, b, None), &mut a);
        all(
            &layout_with_direction(&el, b, None, LayoutDirection::Ltr),
            &mut d,
        );
        assert_eq!(a, d);
        assert!(a.iter().all(|(_, rtl)| !rtl));
        // Left padding 7: the first column starts at x = 3 + 7.
        assert_eq!(a[1].0.min_x(), 10.0);
    }

    #[test]
    fn rtl_images_swap_their_side_padding() {
        use crate::geometry::{Insets, PhysicalSize};
        use crate::render::Pixmap;
        let pm = std::sync::Arc::new(Pixmap::new(PhysicalSize::new(40, 20)));
        let el = Element::image(&pm).padding(Insets::new(0.0, 0.0, 0.0, 10.0));
        let tree = rtl(&el, Rect::from_xywh(0.0, 0.0, 50.0, 20.0));
        match &tree.content {
            NodeContent::Image { padding, .. } => {
                assert_eq!((padding.left, padding.right), (0.0, 10.0));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rtl_auto_text_lines_up_on_the_right() {
        let Some(font) = crate::render::Font::system_default() else {
            return;
        };
        let el = Element::text("abc", 16.0, Color::BLACK).width(200.0);
        let b = Rect::from_xywh(0.0, 0.0, 200.0, 20.0);
        let w = font.measure("abc", 16.0).width;
        let caret_x = |tree: &LayoutNode, i: usize| {
            text_layout(
                &crate::core::TextStyle::default(),
                Some(&font),
                "abc",
                16.0,
                TextDirection::Auto,
                false,
                200.0,
                tree.rtl,
            )
            .unwrap()
            .caret_position(i)
            .x
        };
        let ltr = layout(&el, b, Some(&font));
        assert!(caret_x(&ltr, 0).abs() < 0.01);
        let mirrored = layout_with_direction(&el, b, Some(&font), LayoutDirection::Rtl);
        assert!(mirrored.rtl);
        // Still a left-to-right paragraph ("abc" reads left to right), but
        // against the right edge.
        assert!((caret_x(&mirrored, 0) - (200.0 - w)).abs() < 0.01);
        assert!((caret_x(&mirrored, 3) - 200.0).abs() < 0.01);
        // A click right of the text lands at its end.
        let hit = caret_index_at(&mirrored, Point::new(199.0, 5.0), Some(&font));
        assert_eq!(hit, Some(3));
        // An explicit left-to-right direction keeps its side.
        let forced = el.clone().direction(TextDirection::Ltr);
        let forced = layout_with_direction(&forced, b, Some(&font), LayoutDirection::Rtl);
        assert_eq!(
            caret_index_at(&forced, Point::new(1.0, 5.0), Some(&font)),
            Some(0)
        );
    }

    // --- Transforms and group opacity -------------------------------------

    use crate::geometry::{ScaleFactor, Transform};
    use crate::render::SoftwareRenderer;

    const RED: Color = Color::rgb(255, 0, 0);

    /// Lay `el` out over a black `w`×`h` window and render it at 1x.
    fn render_el(el: &Element, w: f64, h: f64, font: Option<&Font>) -> crate::render::Pixmap {
        let tree = layout(el, Rect::from_xywh(0.0, 0.0, w, h), font);
        let mut scene = Scene::new(Size::new(w, h));
        paint(&tree, &mut scene, font);
        SoftwareRenderer::new()
            .background(Color::BLACK)
            .render(scene, ScaleFactor::IDENTITY)
    }

    /// A 100×100 window centering an 80×20 bar (drawn at (10, 40)).
    fn centered(bar: Element) -> Element {
        Element::stack(Axis::Vertical, vec![bar])
            .align(Align::Center, Align::Center)
            .width(100.0)
            .height(100.0)
    }

    fn tappable_bar() -> Element {
        let mut bar = block(80.0, 20.0).fill(RED);
        bar.action = Some(ActionId(1));
        bar
    }

    #[test]
    fn a_rotated_element_is_drawn_and_hit_where_it_turned_to() {
        use crate::core::runtime::hit_test;
        let el = centered(tappable_bar().transform(Transform::IDENTITY.rotate_degrees(90.0)));
        // Turned about its center (50, 50): now upright, over (40..60, 10..90).
        let out = render_el(&el, 100.0, 100.0, None);
        assert_eq!(out.pixel(50, 15), Some([255, 0, 0, 255]));
        assert_eq!(out.pixel(50, 85), Some([255, 0, 0, 255]));
        assert_eq!(out.pixel(15, 50), Some([0, 0, 0, 255]));

        let tree = layout(&el, Rect::from_xywh(0.0, 0.0, 100.0, 100.0), None);
        assert_eq!(hit_test(&tree, Point::new(50.0, 15.0)), Some(ActionId(1)));
        assert_eq!(
            hit_test(&tree, Point::new(15.0, 50.0)),
            None,
            "where it was laid out"
        );
    }

    #[test]
    fn a_scaled_element_is_hit_where_it_is_drawn() {
        use crate::core::runtime::{cursor_at, hit_test};
        use crate::platform::CursorIcon;
        // Half size about its center: drawn over (30..70, 45..55).
        let el = centered(tappable_bar().transform(Transform::IDENTITY.scale(0.5)));
        let tree = layout(&el, Rect::from_xywh(0.0, 0.0, 100.0, 100.0), None);
        assert_eq!(hit_test(&tree, Point::new(35.0, 50.0)), Some(ActionId(1)));
        assert_eq!(hit_test(&tree, Point::new(20.0, 50.0)), None);
        assert_eq!(hit_test(&tree, Point::new(50.0, 42.0)), None);
        assert_eq!(
            cursor_at(&tree, Point::new(35.0, 50.0)),
            CursorIcon::Pointer
        );
        assert_eq!(
            cursor_at(&tree, Point::new(20.0, 50.0)),
            CursorIcon::Default
        );

        let out = render_el(&el, 100.0, 100.0, None);
        assert_eq!(out.pixel(35, 50), Some([255, 0, 0, 255]));
        assert_eq!(out.pixel(20, 50), Some([0, 0, 0, 255]));

        // Scaled to nothing, it can't be hit at all.
        let gone = centered(tappable_bar().transform(Transform::IDENTITY.scale(0.0)));
        let tree = layout(&gone, Rect::from_xywh(0.0, 0.0, 100.0, 100.0), None);
        assert_eq!(hit_test(&tree, Point::new(50.0, 50.0)), None);
    }

    #[test]
    fn a_transformed_parent_carries_its_children_and_their_clip() {
        use crate::core::runtime::{hit_test, local_point};
        // A clipping 40×40 panel at (0, 0) holding a tappable child that
        // overflows it; the panel is moved 50 px right and 50 px down.
        let mut child = block(80.0, 20.0);
        child.action = Some(ActionId(2));
        let panel = Element::stack(Axis::Vertical, vec![child])
            .width(40.0)
            .height(40.0)
            .clip()
            .transform(Transform::IDENTITY.translate(50.0, 50.0));
        let root = Element::stack(Axis::Vertical, vec![panel]);
        let tree = layout(&root, Rect::from_xywh(0.0, 0.0, 100.0, 100.0), None);
        assert_eq!(hit_test(&tree, Point::new(60.0, 60.0)), Some(ActionId(2)));
        assert_eq!(
            hit_test(&tree, Point::new(10.0, 10.0)),
            None,
            "the old place"
        );
        // Past the moved clip, the overflowing child isn't there to hit.
        assert_eq!(hit_test(&tree, Point::new(95.0, 60.0)), None);
        // A point read in the child's own layout space.
        assert_eq!(
            local_point(
                &tree,
                &|n| n.action == Some(ActionId(2)),
                Point::new(60.0, 60.0)
            ),
            Some(Point::new(10.0, 10.0))
        );
    }

    #[test]
    fn nested_transforms_compose() {
        use crate::core::runtime::{hit_test, node_to_window};
        // A parent turned 180° about its center holding a child scaled 2×
        // about its top-left corner.
        let mut child =
            block(10.0, 10.0).transform(Transform::IDENTITY.scale(2.0).origin(0.0, 0.0));
        child.action = Some(ActionId(3));
        let parent = Element::stack(Axis::Vertical, vec![child])
            .width(100.0)
            .height(100.0)
            .transform(Transform::IDENTITY.rotate_degrees(180.0));
        let tree = layout(&parent, Rect::from_xywh(0.0, 0.0, 100.0, 100.0), None);
        // The child covers (0..20, 0..20) in the parent, so (80..100, 80..100)
        // once the parent is turned over.
        assert_eq!(hit_test(&tree, Point::new(85.0, 85.0)), Some(ActionId(3)));
        assert_eq!(hit_test(&tree, Point::new(5.0, 5.0)), None);
        let (_, m, _) = node_to_window(&tree, &|n| n.action == Some(ActionId(3))).unwrap();
        let r = m.map_rect(Rect::from_xywh(0.0, 0.0, 10.0, 10.0));
        assert!((r.min_x() - 80.0).abs() < 1e-9 && (r.max_y() - 100.0).abs() < 1e-9);
    }

    /// Group opacity composites the subtree as one: where two children overlap
    /// (the second pulled over the first by a transform) the overlap is the
    /// same color as the rest — no seam — and a parent's own fill doesn't show
    /// through its child.
    #[test]
    fn group_opacity_shows_no_overlap_seam() {
        let a = block(30.0, 20.0).fill(RED);
        let b = block(30.0, 20.0)
            .fill(RED)
            .transform(Transform::IDENTITY.translate(-15.0, 0.0));
        let group = Element::stack(Axis::Horizontal, vec![a, b]).opacity(0.5);
        let out = render_el(&group, 100.0, 20.0, None);
        let alone = out.pixel(5, 10).unwrap();
        let overlap = out.pixel(20, 10).unwrap();
        assert_eq!(alone, overlap, "no seam where the children overlap");
        assert!(
            (120..=136).contains(&alone[0]),
            "half red over black: {alone:?}"
        );
        assert_eq!(out.pixel(50, 10), Some([0, 0, 0, 255]), "nothing past them");

        // A white panel with a red child, at half opacity: inside the child it
        // is half red over black, not red over half-white.
        let panel = Element::stack(Axis::Vertical, vec![block(20.0, 20.0).fill(RED)])
            .padding(crate::geometry::Insets::uniform(10.0))
            .fill(Color::WHITE)
            .opacity(0.5);
        let out = render_el(&panel, 100.0, 100.0, None);
        let [r, g, b, _] = out.pixel(20, 20).unwrap();
        assert!(
            (120..=136).contains(&r) && g < 4 && b < 4,
            "{:?}",
            (r, g, b)
        );
        let [r, g, _, _] = out.pixel(5, 5).unwrap();
        assert!(
            (120..=136).contains(&r) && (120..=136).contains(&g),
            "half white"
        );

        // Fully transparent: not painted at all.
        let gone = Element::stack(Axis::Vertical, vec![block(20.0, 20.0).fill(RED)]).opacity(0.0);
        let out = render_el(&gone, 40.0, 40.0, None);
        assert_eq!(out.pixel(10, 10), Some([0, 0, 0, 255]));
    }

    /// Text scales and turns with its element: scaled glyphs are drawn larger
    /// (from their outlines, not stretched bitmaps), and turned text runs down.
    #[test]
    fn text_scales_and_rotates_with_its_element() {
        let Some(font) = Font::system_default() else {
            eprintln!("skipping: no system font found");
            return;
        };
        // The size of the box around the ink, and how many pixels are inked.
        let ink = |el: &Element| {
            let out = render_el(el, 200.0, 200.0, Some(&font));
            let (mut x0, mut y0, mut x1, mut y1, mut n) = (u32::MAX, u32::MAX, 0, 0, 0u32);
            for y in 0..200 {
                for x in 0..200 {
                    if out.pixel(x, y).unwrap()[0] > 128 {
                        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
                        n += 1;
                    }
                }
            }
            (x1.saturating_sub(x0), y1.saturating_sub(y0), n)
        };
        let word = |t: Transform| {
            let text = Element::text("Wide", 16.0, Color::WHITE).transform(t);
            Element::stack(Axis::Vertical, vec![text])
                .align(Align::Center, Align::Center)
                .width(200.0)
                .height(200.0)
        };
        let (w1, h1, n1) = ink(&word(Transform::IDENTITY));
        let (w2, h2, n2) = ink(&word(Transform::IDENTITY.scale(2.0)));
        assert!(w1 > h1 && n1 > 0, "the word is wider than tall");
        assert!((w2 as f64 / w1 as f64 - 2.0).abs() < 0.25, "{w1} -> {w2}");
        assert!((h2 as f64 / h1 as f64 - 2.0).abs() < 0.35, "{h1} -> {h2}");
        assert!(n2 > n1 * 3, "about four times the ink: {n1} -> {n2}");
        let (w3, h3, _) = ink(&word(Transform::IDENTITY.rotate_degrees(90.0)));
        assert!(h3 > w3, "turned on its side: {w3}x{h3}");
        assert!(
            w3.abs_diff(h1) <= 2 && h3.abs_diff(w1) <= 2,
            "{w1}x{h1} -> {w3}x{h3}"
        );
    }

    /// Text with padding is drawn inside it (a pill's label), measures with
    /// it, and a click maps to a caret index from where the glyphs are.
    #[test]
    fn text_paints_inside_its_padding() {
        use crate::geometry::Insets;
        let Some(font) = Font::system_default() else {
            eprintln!("skipping: no system font found");
            return;
        };
        // The top-left corner of the ink.
        let ink_origin = |el: Element| {
            let el = Element::stack(Axis::Vertical, vec![el])
                .align(Align::Start, Align::Start)
                .width(200.0)
                .height(100.0);
            let out = render_el(&el, 200.0, 100.0, Some(&font));
            let (mut x0, mut y0) = (u32::MAX, u32::MAX);
            for y in 0..100 {
                for x in 0..200 {
                    if out.pixel(x, y).unwrap()[0] > 128 {
                        (x0, y0) = (x0.min(x), y0.min(y));
                    }
                }
            }
            (x0, y0)
        };
        let word = || Element::text("Pill", 16.0, Color::WHITE);
        let pad = Insets::new(10.0, 4.0, 6.0, 30.0);
        let (x, y) = ink_origin(word());
        let (px, py) = ink_origin(word().padding(pad));
        assert_eq!((px - x, py - y), (30, 10), "{x},{y} -> {px},{py}");

        // Measured with the padding around the text.
        let avail = Size::new(1000.0, 1000.0);
        let plain = measure(&word(), avail, Some(&font));
        let padded = measure(&word().padding(pad), avail, Some(&font));
        assert_eq!(padded, Size::new(plain.width + 34.0, plain.height + 16.0));

        // A right-to-left layout keeps the start padding at the start (right).
        let rtl_tree = rtl(
            &Element::stack(Axis::Vertical, vec![word().padding(pad)]),
            Rect::from_xywh(0.0, 0.0, 200.0, 100.0),
        );
        let leaf = &rtl_tree.children[0];
        let (b, t) = (leaf.bounds, leaf.text_rect());
        assert_eq!((t.min_x() - b.min_x(), b.max_x() - t.max_x()), (4.0, 30.0));

        // A click left of the glyphs (in the padding) lands before the
        // first letter; one past them, after the last.
        let tree = layout(
            &word().padding(pad),
            Rect::from_xywh(0.0, 0.0, plain.width + 34.0, plain.height + 16.0),
            Some(&font),
        );
        let mid = 10.0 + plain.height / 2.0;
        assert_eq!(
            caret_index_at(&tree, Point::new(31.0, mid), Some(&font)),
            Some(0)
        );
        assert_eq!(
            caret_index_at(&tree, Point::new(29.0 + plain.width, mid), Some(&font)),
            Some(4)
        );
    }

    #[test]
    fn focus_and_hover_overlays_follow_the_transform() {
        let mut bar = tappable_bar().transform(Transform::IDENTITY.rotate_degrees(90.0));
        bar.focus = Some(FocusId(1));
        let el = centered(bar);
        let tree = layout(&el, Rect::from_xywh(0.0, 0.0, 100.0, 100.0), None);
        let mut scene = Scene::new(Size::new(100.0, 100.0));
        paint_hover(&tree, ActionId(1), &mut scene, Color::WHITE);
        paint_focus(
            &tree,
            FocusId(1),
            &mut scene,
            None,
            Color::WHITE,
            Color::WHITE,
            Color::WHITE,
        );
        let out = SoftwareRenderer::new()
            .background(Color::BLACK)
            .render(scene, ScaleFactor::IDENTITY);
        assert_eq!(out.pixel(50, 15), Some([255, 255, 255, 255]), "upright");
        assert_eq!(
            out.pixel(15, 50),
            Some([0, 0, 0, 255]),
            "not where laid out"
        );
    }

    // --- Per-element fonts and decorations ---------------------------------

    /// Pixels brighter than mid-gray in `out`.
    fn lit(out: &crate::render::Pixmap) -> usize {
        let s = out.size();
        (0..s.height)
            .flat_map(|y| (0..s.width).map(move |x| (x, y)))
            .filter(|&(x, y)| out.pixel(x, y).unwrap()[0] > 128)
            .count()
    }

    #[test]
    fn text_elements_draw_in_their_own_weight_and_font() {
        let Some(font) = Font::system_default() else {
            return;
        };
        let word = || Element::text("Heavy", 20.0, Color::WHITE);
        let plain = render_el(&word(), 200.0, 40.0, Some(&font));
        let bold = render_el(&word().bold(), 200.0, 40.0, Some(&font));
        assert!(lit(&bold) > lit(&plain) * 11 / 10);
        // A bold element also measures with the bold face (wider or equal).
        let (pw, bw) = (
            measure(&word(), Size::new(500.0, 50.0), Some(&font)).width,
            measure(&word().bold(), Size::new(500.0, 50.0), Some(&font)).width,
        );
        assert!(bw >= pw);
        // An element's own font replaces the app's, also without an app font.
        let own = render_el(&word().font(font.bold()), 200.0, 40.0, None);
        assert_eq!(lit(&own), lit(&bold));
        assert_eq!(lit(&render_el(&word(), 200.0, 40.0, None)), 0);
    }

    #[test]
    fn container_text_styles_fill_in_what_children_leave_unset() {
        use crate::render::FontWeight;
        let Some(font) = Font::system_default() else {
            return;
        };
        let el = Element::stack(
            Axis::Vertical,
            vec![
                Element::text("a", 12.0, Color::WHITE),
                Element::text("b", 12.0, Color::WHITE).weight(FontWeight::LIGHT),
            ],
        )
        .bold()
        .italic()
        .font(font.clone())
        .strikethrough();
        let ElementKind::Stack { children, .. } = &el.kind else {
            unreachable!()
        };
        let style = |i: usize| match &children[i].kind {
            ElementKind::Text { style, .. } => style.clone(),
            _ => unreachable!(),
        };
        assert_eq!(style(0).weight, Some(FontWeight::BOLD));
        assert_eq!(style(1).weight, Some(FontWeight::LIGHT), "the child's own");
        assert_eq!(style(0).italic, Some(true));
        assert_eq!(style(1).font.as_ref(), Some(&font));
        assert!(style(0).decoration.strikethrough);
        let resolved = style(0).resolve(None).unwrap();
        assert_eq!(resolved, font.styled(crate::render::FontStyle::BOLD_ITALIC));
    }

    #[test]
    fn decorated_text_draws_its_lines_across_the_text() {
        let Some(font) = Font::system_default() else {
            return;
        };
        // White text with a red underline and strikethrough over black.
        let deco = crate::render::Decoration::NONE
            .underline(crate::render::UnderlineStyle::Single)
            .strikethrough()
            .color(RED);
        let el = Element::text("Lined text", 20.0, Color::WHITE).decorate(deco);
        let out = render_el(&el, 200.0, 40.0, Some(&font));
        let width = font.measure("Lined text", 20.0).width as u32;
        let red_rows: Vec<u32> = (0..40)
            .filter(|&y| {
                (2..width - 2).all(|x| {
                    let [r, g, b, _] = out.pixel(x, y).unwrap();
                    r > 200 && g < 60 && b < 60
                })
            })
            .collect();
        // Two lines: the underline below the baseline, the strikethrough
        // above it.
        let baseline = font.baseline(20.0);
        let m = font.metrics(20.0);
        assert!(red_rows.len() >= 2, "{red_rows:?}");
        let (top, bottom) = (red_rows[0] as f64, *red_rows.last().unwrap() as f64);
        assert!(
            (top - (baseline + m.strikeout_position)).abs() <= 1.5,
            "{red_rows:?}"
        );
        assert!(
            (bottom - (baseline + m.underline_position)).abs() <= 1.5,
            "{red_rows:?}"
        );
        // Nothing red past the text's end.
        assert!((width + 4..200).all(|x| out.pixel(x, bottom as u32).unwrap()[0] < 50));
    }
}
