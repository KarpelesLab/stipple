//! Layout and paint passes: turn an [`Element`] tree into a retained
//! [`LayoutNode`] tree, then paint that tree into a [`Scene`].
//!
//! Three phases:
//! 1. **measure** — bottom-up natural sizing (content + padding, honoring fixed
//!    overrides; text measures via the active [`Font`]).
//! 2. **layout** — top-down assignment of absolute bounds, using
//!    [`crate::layout::solve_main_axis`] to distribute the main axis and
//!    [`Align`] to position children on the cross axis. Produces a
//!    [`LayoutNode`] tree that survives the frame so pointer events can be
//!    routed against it (see [`crate::core::hit_test`]).
//! 3. **paint** — walk the layout tree, drawing each node's decoration and text.

use crate::core::element::{Align, BoxStyle, Element, ElementKind};
use crate::core::runtime::{
    ActionId, FocusId, LayoutNode, NodeContent, ScrollId, find_action, find_focus, first_text,
};
use crate::geometry::{PhysicalSize, Point, Rect, Size, Vec2};
use crate::layout::{Axis, FlexItem, solve_main_axis};
use crate::platform::native::NativeState;
use crate::render::{Color, Font, Scene};

/// Neutral fill painted into an embedded-content viewport before its content is
/// composited in, so the reserved area reads as a defined surface (a dark slate
/// matching the GPU path's placeholder in `stipple::gpu`).
const VIEWPORT_PLACEHOLDER: Color = Color::rgb(0x20, 0x24, 0x2c);

/// Natural (desired) size of `el` given the `avail` space and active `font`.
pub fn measure(el: &Element, avail: Size, font: Option<&Font>) -> Size {
    let pad = el.layout.padding;
    // A fixed size is the space the content actually gets, so wrapped text inside
    // a `.width(360.0)` panel must measure against 360, not the parent's width.
    let outer = Size::new(
        el.layout.size.width.unwrap_or(avail.width),
        el.layout.size.height.unwrap_or(avail.height),
    );
    let inner = outer.deflate(pad);

    let content = match &el.kind {
        // A viewport sizes purely to its width/height overrides (or grow); it
        // has no intrinsic content size.
        ElementKind::Leaf | ElementKind::Viewport { .. } => Size::ZERO,
        ElementKind::Text { text, size, .. } => match font {
            // Wrapping text takes the available width and grows in height.
            Some(f) if el.wrap && inner.width.is_finite() => {
                let lines = f.wrap(text, *size, inner.width);
                let w = lines
                    .iter()
                    .map(|l| f.measure(l, *size).width)
                    .fold(0.0_f64, f64::max);
                Size::new(w, f.line_height(*size) * lines.len() as f64)
            }
            Some(f) => f.measure(text, *size),
            None => Size::ZERO,
        },
        ElementKind::Stack {
            axis,
            gap,
            children,
            ..
        } => {
            let mut main = 0.0;
            let mut cross: f64 = 0.0;
            for c in children {
                let cs = measure(c, inner, font);
                main += axis.main(cs);
                cross = cross.max(axis.cross(cs));
            }
            if children.len() > 1 {
                main += gap * (children.len() as f64 - 1.0);
            }
            axis.size(main, cross)
        }
    };

    let w = el
        .layout
        .size
        .width
        .unwrap_or(content.width + pad.horizontal());
    let h = el
        .layout
        .size
        .height
        .unwrap_or(content.height + pad.vertical());
    Size::new(w, h)
}

/// Lay `el` out within `bounds`, producing a retained [`LayoutNode`] tree with
/// absolute bounds, decorations, text content, and action handles.
pub fn layout(el: &Element, bounds: Rect, font: Option<&Font>) -> LayoutNode {
    let content = match &el.kind {
        ElementKind::Text { text, size, color } => NodeContent::Text {
            text: text.clone(),
            size: *size,
            color: *color,
        },
        ElementKind::Viewport { id } => NodeContent::Viewport(*id),
        _ => NodeContent::None,
    };
    let mut node = LayoutNode {
        bounds,
        decoration: el.decoration,
        content,
        action: el.action,
        focus: el.focus,
        drag: el.drag,
        context: el.context,
        caret: el.caret,
        selection: el.selection,
        text_pos: el.text_pos,
        wrap: el.wrap,
        native: el.native,
        scroll: el.scroll,
        clip: el.clip,
        children: Vec::new(),
    };

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
    let avail = inner.size;
    let axis = *axis;

    // Main-axis distribution.
    let measured: Vec<Size> = children.iter().map(|c| measure(c, avail, font)).collect();
    // A scroll container lays its children out at their *natural* main size
    // (so content can overflow the viewport), stacked from the start with no
    // grow/shrink; the offset + clip are applied afterward (see `apply_scroll`).
    let (spans, main_shift) = if el.scroll.is_some() {
        let mut spans = Vec::with_capacity(children.len());
        let mut cursor = 0.0;
        for m in &measured {
            let length = axis.main(*m);
            spans.push(crate::layout::Span {
                offset: cursor,
                length,
            });
            cursor += length + *gap;
        }
        (spans, 0.0)
    } else {
        let items: Vec<FlexItem> = children
            .iter()
            .zip(&measured)
            .map(|(c, m)| FlexItem {
                basis: axis.main(*m),
                grow: c.layout.grow,
            })
            .collect();
        let spans = solve_main_axis(axis.main(avail), *gap, &items);
        // If nothing grows, the block may be shorter than the content area;
        // shift it as a whole per the main-axis alignment.
        let used_main = spans.last().map(|s| s.offset + s.length).unwrap_or(0.0);
        let leftover = (axis.main(avail) - used_main).max(0.0);
        let main_shift = match main_align {
            Align::Start | Align::Stretch => 0.0,
            Align::Center => leftover / 2.0,
            Align::End => leftover,
        };
        (spans, main_shift)
    };

    node.children.reserve(children.len());
    for ((child, m), span) in children.iter().zip(&measured).zip(&spans) {
        let cross_avail = axis.cross(avail);
        let cross_len = match cross_align {
            Align::Stretch => cross_avail,
            _ => axis.cross(*m).min(cross_avail),
        };
        let cross_off = match cross_align {
            Align::Start | Align::Stretch => 0.0,
            Align::Center => (cross_avail - cross_len) / 2.0,
            Align::End => cross_avail - cross_len,
        };
        let child_bounds = child_rect(
            axis,
            inner,
            span.offset + main_shift,
            span.length,
            cross_off,
            cross_len,
        );
        node.children.push(layout(child, child_bounds, font));
    }
    node
}

/// Overlay focus affordances for the focused element: a `ring` around its
/// bounds, a `selection` highlight behind any selected text range, and a
/// `caret` bar at the text's caret index (or its end when no caret is set).
/// No-op if `focused` isn't in the tree.
pub fn paint_focus(
    tree: &LayoutNode,
    focused: FocusId,
    scene: &mut Scene,
    font: Option<&Font>,
    ring: Color,
    caret: Color,
    selection: Color,
) {
    let Some(node) = find_focus(tree, focused) else {
        return;
    };
    let os_focus = node
        .native
        .is_some_and(|n| crate::platform::native::draws_focus(n.control));
    if !os_focus {
        scene.stroke_rect(node.bounds, ring, 2.0);
    }

    let Some(leaf) = first_text(node) else { return };
    let NodeContent::Text { text, size, .. } = &leaf.content else {
        return;
    };
    let bounds = leaf.bounds;
    let size = *size;
    let line_h = font.map(|f| f.line_height(size)).unwrap_or(size);
    // Width of a text slice (0 without a font).
    let width = |slice: &str| font.map(|f| f.measure(slice, size).width).unwrap_or(0.0);
    // Map a byte index to its (x, line) on screen.
    let pos = |i: usize| -> (f64, usize) {
        let i = i.min(text.len());
        let ls = text[..i].rfind('\n').map(|n| n + 1).unwrap_or(0);
        let line = text[..ls].bytes().filter(|&b| b == b'\n').count();
        (bounds.min_x() + width(&text[ls..i]), line)
    };

    // Selection highlight, drawn as one rectangle per spanned line (under the
    // caret; translucent so the text reads through).
    if let Some((s, e)) = leaf.selection
        && e > s
    {
        let mut ls = 0usize;
        let mut line = 0usize;
        loop {
            let le = text[ls..].find('\n').map(|n| ls + n).unwrap_or(text.len());
            // Intersect [s, e) with this line, including the trailing '\n' when
            // the selection continues onto the next line.
            let nl = if le < text.len() { 1 } else { 0 };
            let sel_s = s.max(ls);
            let sel_e = e.min(le + nl);
            if sel_e > sel_s && sel_s <= le {
                let x0 = bounds.min_x() + width(&text[ls..sel_s.min(le)]);
                let x1 = bounds.min_x() + width(&text[ls..sel_e.min(le)]);
                let extra = if sel_e > le { 6.0 } else { 0.0 }; // hint the newline
                let y = bounds.min_y() + line as f64 * line_h;
                scene.fill_rect(Rect::from_xywh(x0, y, (x1 - x0) + extra, line_h), selection);
            }
            if le >= text.len() {
                break;
            }
            ls = le + 1;
            line += 1;
        }
    }

    // Caret bar on its line (or end of text when unset).
    let (cx, cline) = pos(leaf.caret.unwrap_or(text.len()));
    let cx = (cx + 1.0).min(bounds.max_x().max(bounds.min_x() + 1.0));
    let cy = bounds.min_y() + cline as f64 * line_h;
    scene.fill_rect(Rect::from_xywh(cx, cy, 2.0, line_h), caret);
}

/// Resolve a pointer position (logical pixels, absolute) to the nearest caret
/// byte index within `node`'s first text leaf. The `y` picks the line and the
/// `x` the column within it; returns the boundary whose x is closest. Returns
/// `None` if `node` has no text or no `font`.
pub fn caret_index_at(node: &LayoutNode, point: Point, font: Option<&Font>) -> Option<usize> {
    let leaf = first_text(node)?;
    let NodeContent::Text { text, size, .. } = &leaf.content else {
        return None;
    };
    let font = font?;
    let line_h = font.line_height(*size).max(1.0);
    let line = ((point.y - leaf.bounds.min_y()) / line_h).floor().max(0.0) as usize;

    // Byte range [ls, le) of the chosen line (clamped to the last line if
    // `line` overruns the line count).
    let (mut ls, mut le) = (0usize, text.len());
    let mut start = 0usize;
    for (i, part) in text.split('\n').enumerate() {
        let end = start + part.len();
        ls = start;
        le = end;
        if i == line {
            break;
        }
        start = end + 1; // skip the '\n'
    }

    let local = point.x - leaf.bounds.min_x();
    if local <= 0.0 {
        return Some(ls);
    }
    // Pick the char boundary in [ls, le] whose x is nearest the pointer.
    let line_text = &text[ls..le];
    let mut best = ls;
    let mut best_dist = f64::INFINITY;
    for (off, _) in line_text
        .char_indices()
        .map(|(i, _)| (i, ()))
        .chain(std::iter::once((line_text.len(), ())))
    {
        let w = font.measure(&line_text[..off], *size).width;
        let d = (w - local).abs();
        if d < best_dist {
            best_dist = d;
            best = ls + off;
        }
    }
    Some(best)
}

/// Overlay a `highlight` (typically translucent) on the hovered tappable
/// element, matching its corner radius. No-op if `hovered` isn't in the tree.
pub fn paint_hover(tree: &LayoutNode, hovered: ActionId, scene: &mut Scene, highlight: Color) {
    if let Some(node) = find_action(tree, hovered) {
        // An OS-drawn control shows hover itself (see `mark_native_states`).
        if node.native.is_some() {
            return;
        }
        if node.decoration.radius > 0.0 {
            scene.fill_round_rect(node.bounds, node.decoration.radius, highlight);
        } else {
            scene.fill_rect(node.bounds, highlight);
        }
    }
}

/// Tell each OS-drawn control which interaction state to draw itself in.
pub fn mark_native_states(
    node: &mut LayoutNode,
    hovered: Option<ActionId>,
    pressed: Option<ActionId>,
    focused: Option<FocusId>,
) {
    if let Some(native) = &mut node.native {
        native.state = NativeState {
            hovered: node.action.is_some() && node.action == hovered,
            pressed: node.action.is_some() && node.action == pressed,
            focused: node.focus.is_some() && node.focus == focused,
        };
    }
    for child in &mut node.children {
        mark_native_states(child, hovered, pressed, focused);
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
    match crate::platform::native::render(deco, size, scale) {
        Some(image) => {
            scene.draw_image(node.bounds, image);
            true
        }
        None => false,
    }
}

/// Paint a laid-out tree into `scene`, parents before children.
pub fn paint(node: &LayoutNode, scene: &mut Scene, font: Option<&Font>) {
    if !paint_native(node, scene) {
        paint_decoration(&node.decoration, node.bounds, scene);
    }
    match &node.content {
        NodeContent::Text { text, size, color } => {
            if let Some(f) = font {
                if node.wrap {
                    // Wrap to the laid-out width; fill_text renders the joined lines.
                    let wrapped = f.wrap(text, *size, node.bounds.width()).join("\n");
                    scene.fill_text(f, &wrapped, node.bounds.origin, *size, *color);
                } else {
                    scene.fill_text(f, text, node.bounds.origin, *size, *color);
                }
            }
        }
        // Reserve the embedded-content area: paint a placeholder and record the
        // viewport command so the app/GPU composites real content into the rect.
        NodeContent::Viewport(id) => {
            scene.fill_viewport(node.bounds, id.0 as u64, VIEWPORT_PLACEHOLDER);
        }
        NodeContent::None => {}
    }
    // Clip children to this node's bounds (scroll containers, overlay panels)
    // so overflowing content is masked to the viewport.
    if node.clip && !node.children.is_empty() {
        scene.push_clip(node.bounds);
        for child in &node.children {
            paint(child, scene, font);
        }
        scene.pop_clip();
    } else {
        for child in &node.children {
            paint(child, scene, font);
        }
    }
}

/// Apply scroll offsets to a laid-out tree: for each scroll container, clamp its
/// stored offset to the content overflow and shift its descendants up by that
/// amount (so the offset is the source of truth, re-applied each frame). Returns
/// nothing; clamps `offsets` in place so the app keeps valid values.
pub fn apply_scroll(node: &mut LayoutNode, offsets: &mut std::collections::HashMap<ScrollId, f64>) {
    if let Some(id) = node.scroll {
        // Content height = furthest child extent below the viewport top; viewport
        // height = this node's bounds height.
        let top = node.bounds.min_y();
        let content_bottom = node
            .children
            .iter()
            .map(|c| c.bounds.max_y())
            .fold(top, f64::max);
        let content_h = content_bottom - top;
        let max_off = (content_h - node.bounds.height()).max(0.0);
        let off = offsets.entry(id).or_insert(0.0);
        *off = off.clamp(0.0, max_off);
        let dy = *off;
        if dy > 0.0 {
            for child in &mut node.children {
                translate(child, -dy);
            }
        }
    }
    for child in &mut node.children {
        apply_scroll(child, offsets);
    }
}

/// Shift a node and all its descendants vertically by `dy` (logical pixels).
fn translate(node: &mut LayoutNode, dy: f64) {
    node.bounds = node.bounds.translate(Vec2::new(0.0, dy));
    for child in &mut node.children {
        translate(child, dy);
    }
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

fn paint_decoration(deco: &BoxStyle, bounds: Rect, scene: &mut Scene) {
    if let Some(fill) = deco.fill {
        if deco.radius > 0.0 {
            scene.fill_round_rect(bounds, deco.radius, fill);
        } else {
            scene.fill_rect(bounds, fill);
        }
    }
    if let Some((color, width)) = deco.border {
        scene.stroke_rect(bounds, color, width);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::element::BoxStyle;
    use crate::render::Color;

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
}
