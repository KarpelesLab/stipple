use super::*;
use crate::geometry::ScaleFactor;
use crate::render::{Color, Font, RichText, Scene, SoftwareRenderer, SpanStyle};

fn font() -> Option<Font> {
    let f = Font::system_default();
    if f.is_none() {
        eprintln!("skipping: no system font found");
    }
    f
}

const LOREM: &str = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do \
    eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, \
    quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. \
    Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu \
    fugiat nulla pariatur. Excepteur sint occaecat cupidatat non proident, sunt in culpa \
    qui officia deserunt mollit anim id est laborum.";

/// A five-pointed star, opaque on transparent, `side` pixels square.
fn star(side: u32) -> Arc<Pixmap> {
    let s = side as f64;
    let points: Vec<Point> = (0..10)
        .map(|k| {
            let a = std::f64::consts::PI * (k as f64 / 5.0 - 0.5);
            let r = if k % 2 == 0 { s / 2.0 } else { s / 5.0 };
            Point::new(s / 2.0 + r * a.cos(), s / 2.0 + r * a.sin())
        })
        .collect();
    let mut data = vec![0u8; (side * side * 4) as usize];
    for y in 0..side {
        for (a, b) in polygon_row(&points, y as f64 + 0.5) {
            for x in 0..side {
                let cx = x as f64 + 0.5;
                if cx >= a && cx < b {
                    let i = ((y * side + x) * 4) as usize;
                    data[i..i + 4].copy_from_slice(&[200, 40, 40, 255]);
                }
            }
        }
    }
    Arc::new(Pixmap::from_rgba8(PhysicalSize::new(side, side), data))
}

/// The boxes of the layout's characters (each one's advance across its
/// line's height), but spaces.
fn glyph_boxes(layout: &TextLayout<'_>, text: &str) -> Vec<Rect> {
    let mut out = Vec::new();
    for (i, c) in text.char_indices() {
        if c == ' ' || c == '\n' || c == crate::render::rich::OBJECT {
            continue;
        }
        out.extend(layout.selection_rects(i, i + c.len_utf8()));
    }
    out
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.min_x() < b.max_x() - 0.01
        && b.min_x() < a.max_x() - 0.01
        && a.min_y() < b.max_y() - 0.01
        && b.min_y() < a.max_y() - 0.01
}

#[test]
fn intervals_merge_and_subtract() {
    assert_eq!(
        merge(vec![(5.0, 8.0), (0.0, 2.0), (1.0, 3.0), (8.0, 9.0)]),
        vec![(0.0, 3.0), (5.0, 9.0)]
    );
    let free = subtract(&[(0.0, 100.0)], &[(10.0, 20.0), (50.0, 60.0)]);
    assert_eq!(free, vec![(0.0, 10.0), (20.0, 50.0), (60.0, 100.0)]);
    // Blocks reaching past the region's edges.
    let free = subtract(&[(0.0, 100.0)], &[(f64::NEG_INFINITY, 30.0), (90.0, 200.0)]);
    assert_eq!(free, vec![(30.0, 90.0)]);
    assert!(subtract(&[(0.0, 100.0)], &[(-1.0, 101.0)]).is_empty());
}

#[test]
fn shapes_block_their_rows_grown_by_the_margin() {
    let size = Size::new(100.0, 60.0);
    // A rectangle blocks its width in any band it touches, plus the margin.
    assert_eq!(
        FlowShape::Rect.blocked(size, 10.0..20.0, 0.0),
        vec![(0.0, 100.0)]
    );
    assert_eq!(
        FlowShape::Rect.blocked(size, 10.0..20.0, 5.0),
        vec![(-5.0, 105.0)]
    );
    // Within the margin above it, less of the margin reaches the band.
    let above = FlowShape::Rect.blocked(size, -10.0..-4.0, 5.0);
    let e = (25.0f64 - 16.0).sqrt();
    assert!((above[0].0 + e).abs() < 0.01 && (above[0].1 - 100.0 - e).abs() < 0.01);
    assert!(FlowShape::Rect.blocked(size, -10.0..-6.0, 5.0).is_empty());

    // An ellipse: narrow at the top, full width across the middle.
    let top = FlowShape::Ellipse.blocked(size, 0.0..2.0, 0.0);
    let mid = FlowShape::Ellipse.blocked(size, 28.0..32.0, 0.0);
    assert!(top[0].1 - top[0].0 < 60.0, "{top:?}");
    assert!((mid[0].0).abs() < 0.01 && (mid[0].1 - 100.0).abs() < 0.01);
    // With a margin, the middle grows by it on each side.
    let grown = FlowShape::Ellipse.blocked(size, 28.0..32.0, 8.0);
    assert!((grown[0].0 + 8.0).abs() < 0.01 && (grown[0].1 - 108.0).abs() < 0.01);

    // A rounded rectangle's corners free their band's ends.
    let r = FlowShape::RoundRect(20.0).blocked(size, 0.0..1.0, 0.0);
    assert!(r[0].0 > 5.0 && r[0].1 < 95.0, "{r:?}");
    assert_eq!(
        FlowShape::RoundRect(20.0).blocked(size, 25.0..35.0, 0.0),
        vec![(0.0, 100.0)]
    );

    // A V: two prongs at the top, one at the bottom.
    let v = FlowShape::polygon(vec![
        Point::new(0.0, 0.0),
        Point::new(20.0, 0.0),
        Point::new(50.0, 40.0),
        Point::new(80.0, 0.0),
        Point::new(100.0, 0.0),
        Point::new(50.0, 60.0),
    ]);
    let prongs = v.blocked(size, 2.0..4.0, 0.0);
    assert_eq!(prongs.len(), 2, "{prongs:?}");
    assert!(prongs[0].1 < 50.0 && prongs[1].0 > 50.0);
    // The margin closes the gap between them where it's narrower than
    // twice the margin.
    assert_eq!(v.blocked(size, 30.0..32.0, 10.0).len(), 1);
    let low = v.blocked(size, 50.0..52.0, 0.0);
    assert_eq!(low.len(), 1);
    assert!(low[0].0 > 35.0 && low[0].1 < 65.0, "{low:?}");
}

#[test]
fn an_alpha_shape_blocks_its_opaque_pixels_grown_by_the_margin() {
    // A 10×10 image whose only opaque pixels are a 2×2 block at (4, 4),
    // with a half-transparent pixel at (0, 0).
    let mut data = vec![0u8; 400];
    for (x, y) in [(4, 4), (5, 4), (4, 5), (5, 5)] {
        data[(y * 10 + x) * 4 + 3] = 255;
    }
    data[3] = 100;
    let pm = Arc::new(Pixmap::from_rgba8(PhysicalSize::new(10, 10), data));
    // Drawn at twice the size: the block is x 8..12, y 8..12.
    let size = Size::new(20.0, 20.0);
    let shape = FlowShape::alpha(pm.clone(), 0.5);
    assert_eq!(shape.blocked(size, 8.0..12.0, 0.0), vec![(8.0, 12.0)]);
    assert!(shape.blocked(size, 0.0..6.0, 0.0).is_empty());
    assert!(shape.blocked(size, 13.0..20.0, 0.0).is_empty());
    // At a low threshold the half-transparent pixel blocks too.
    let low = FlowShape::alpha(pm.clone(), 0.2);
    assert_eq!(low.blocked(size, 0.0..2.0, 0.0), vec![(0.0, 2.0)]);
    // A margin of 3 grows it 3 px every way.
    let grown = shape.blocked(size, 9.0..11.0, 3.0);
    assert!((grown[0].0 - 5.0).abs() < 0.01 && (grown[0].1 - 15.0).abs() < 0.01);
    assert!(!shape.blocked(size, 13.0..14.0, 3.0).is_empty());
    assert!(shape.blocked(size, 16.0..18.0, 3.0).is_empty());
    // Asking again reuses the profile.
    let before = profiles_cached();
    let _ = shape.blocked(size, 9.0..11.0, 3.0);
    assert_eq!(profiles_cached(), before);
}

#[test]
fn a_shape_in_the_middle_splits_lines_and_text_runs_on_both_sides() {
    let Some(font) = font() else { return };
    let at = Rect::from_xywh(150.0, 40.0, 100.0, 100.0);
    let flow = TextFlow::new(400.0).item(
        FlowItem::at(at.origin)
            .shape(FlowShape::Ellipse)
            .margin(6.0),
        at.size,
    );
    let layout = font.layout_flow(LOREM, 16.0, &flow);
    assert_eq!(layout.flow_item_rect(0), Some(at));
    // Lines beside the disc come in pairs on one band: left and right.
    let mut beside = 0;
    for i in 0..layout.line_count() {
        let r = layout.line_rect(i).unwrap();
        assert!(r.max_x() <= 400.5, "line {i} overflows: {r:?}");
        if r.min_y() > at.min_y() + 10.0 && r.max_y() < at.max_y() - 10.0 {
            beside += 1;
            assert!(
                r.max_x() <= at.min_x() || r.min_x() >= at.max_x(),
                "line {i} crosses the disc: {r:?}"
            );
        }
    }
    assert!(beside >= 6, "{beside} lines beside the disc");
    // No character box crosses the disc (grown by its margin).
    for b in glyph_boxes(&layout, LOREM) {
        let mid = FlowShape::Ellipse.blocked(
            at.size,
            b.min_y() - at.min_y()..b.max_y() - at.min_y(),
            6.0,
        );
        for (l, r) in mid {
            let (l, r) = (l + at.min_x(), r + at.min_x());
            assert!(
                b.max_x() <= l + 0.01 || b.min_x() >= r - 0.01,
                "{b:?} in {l}..{r}"
            );
        }
    }
    // Every byte is laid out once, in order.
    let mut at_byte = 0;
    for i in 0..layout.line_count() {
        let r = layout.line_range(i).unwrap();
        assert!(r.start >= at_byte);
        at_byte = r.end;
    }
    assert_eq!(at_byte, LOREM.len());
    // The flow is as tall as its text, which runs below the disc.
    assert!(layout.size().height > at.max_y());
    assert_eq!(layout.size().width, 400.0);
}

#[test]
fn wrap_sides_choose_where_text_runs() {
    let Some(font) = font() else { return };
    let at = Rect::from_xywh(100.0, 30.0, 80.0, 80.0);
    let lines_beside = |side: WrapSide| {
        let flow = TextFlow::new(400.0).item(FlowItem::at(at.origin).wrap(side), at.size);
        let layout = font.layout_flow(LOREM, 14.0, &flow);
        let (mut left, mut right) = (0, 0);
        for i in 0..layout.line_count() {
            let r = layout.line_rect(i).unwrap();
            if r.max_y() > at.min_y() + 1.0 && r.min_y() < at.max_y() - 1.0 {
                if r.max_x() <= at.min_x() + 0.01 {
                    left += 1;
                } else if r.min_x() >= at.max_x() - 0.01 {
                    right += 1;
                } else {
                    panic!("{side:?}: line {i} crosses the box: {r:?}");
                }
            }
        }
        (left, right)
    };
    let (l, r) = lines_beside(WrapSide::Both);
    assert!(l > 0 && r > 0);
    let (l, r) = lines_beside(WrapSide::Left);
    assert!(l > 0 && r == 0);
    let (l, r) = lines_beside(WrapSide::Right);
    assert!(l == 0 && r > 0);
    // More room on the right (220 px) than the left (100).
    let (l, r) = lines_beside(WrapSide::Largest);
    assert!(l == 0 && r > 0);
    assert_eq!(lines_beside(WrapSide::Clear), (0, 0));
}

#[test]
fn floats_go_to_their_edges_and_stack() {
    let Some(font) = font() else { return };
    let flow = TextFlow::new(360.0)
        .item(FlowItem::float_left().margin(8.0), Size::new(60.0, 50.0))
        .item(FlowItem::float_right().margin(8.0), Size::new(90.0, 40.0))
        .item(FlowItem::float_left(), Size::new(60.0, 30.0));
    let layout = font.layout_flow(LOREM, 14.0, &flow);
    let a = layout.flow_item_rect(0).unwrap();
    let b = layout.flow_item_rect(1).unwrap();
    let c = layout.flow_item_rect(2).unwrap();
    assert_eq!((a.min_x(), a.min_y()), (0.0, 0.0));
    assert_eq!((b.max_x(), b.min_y()), (360.0, 0.0));
    // The second left float sits beside the first, margin apart.
    assert_eq!((c.min_x(), c.min_y()), (68.0, 0.0));
    for i in 0..layout.line_count() {
        let r = layout.line_rect(i).unwrap();
        for f in [a, b, c] {
            assert!(!overlaps(r, f), "line {i} {r:?} over {f:?}");
        }
        if r.min_y() < 30.0 {
            assert!(r.min_x() >= 128.0 && r.max_x() <= 262.0 + 0.01, "{r:?}");
        }
    }
    // Text goes back to the full width below the floats.
    let last = layout.line_rect(layout.line_count() - 1).unwrap();
    assert!(last.min_y() >= 50.0 && last.min_x() == 0.0);

    // A float anchored later in the text floats at the line holding it.
    let anchor = LOREM.find("Duis").unwrap();
    let flow = TextFlow::new(360.0).item(
        FlowItem::float_right().anchor(anchor),
        Size::new(80.0, 80.0),
    );
    let layout = font.layout_flow(LOREM, 14.0, &flow);
    let f = layout.flow_item_rect(0).unwrap();
    let line = (0..layout.line_count())
        .find(|&i| layout.line_range(i).unwrap().contains(&anchor))
        .unwrap();
    assert_eq!(f.min_y(), layout.line_rect(line).unwrap().min_y());
    assert!(f.min_y() > 0.0);
}

#[test]
fn right_to_left_text_fills_the_right_side_first() {
    let Some(font) = font().filter(|f| {
        f.chain()
            .shape("א", 16.0)
            .ok()
            .and_then(|g| g.first().map(|g| g.glyph_id != 0))
            .unwrap_or(false)
    }) else {
        return;
    };
    let text = "שלום עולם זהו טקסט ארוך בעברית שזורם מסביב לצורה שבאמצע העמוד ".repeat(4);
    let at = Rect::from_xywh(140.0, 0.0, 80.0, 60.0);
    let flow = TextFlow::new(360.0).item(FlowItem::at(at.origin), at.size);
    let layout = font.layout_flow(&text, 14.0, &flow);
    assert!(layout.line_is_rtl(0));
    // The first line (in text order) is right of the box, the second left
    // of it, on the same band.
    let (a, b) = (layout.line_rect(0).unwrap(), layout.line_rect(1).unwrap());
    assert_eq!(a.min_y(), b.min_y());
    assert!(
        a.min_x() >= at.max_x() && b.max_x() <= at.min_x(),
        "{a:?} {b:?}"
    );
    // Each piece sits against its room's right edge.
    assert!((a.max_x() - 360.0).abs() < 0.5 && (b.max_x() - at.min_x()).abs() < 0.5);
    // Clicking a piece finds its text.
    let r0 = layout.line_range(0).unwrap();
    let i = layout.index_at(Point::new(a.max_x() - 3.0, a.min_y() + 2.0));
    assert!(r0.contains(&i) || i == r0.start, "{i} not in {r0:?}");
    let r1 = layout.line_range(1).unwrap();
    let j = layout.index_at(Point::new(b.min_x() + 3.0, b.min_y() + 2.0));
    assert!(j >= r1.start && j <= r1.end, "{j} not in {r1:?}");
}

#[test]
fn a_taller_run_grows_its_band_and_still_clears_the_shape() {
    let Some(font) = font() else { return };
    // A box whose top is just below the first line of small text: a big
    // run on that line makes it reach the box, so the line must leave room.
    let small = font.line_height(12.0);
    let at = Rect::from_xywh(0.0, small + 2.0, 150.0, 60.0);
    let text = RichText::new()
        .push("small words then a ")
        .styled("HUGE", SpanStyle::new().size(40.0))
        .push(" run and more small words after it to fill the lines below.");
    let flow = TextFlow::new(300.0).item(FlowItem::at(at.origin), at.size);
    let layout = text.layout_flow(&font, 12.0, &flow);
    let huge = text.text().find("HUGE").unwrap();
    let line = (0..layout.line_count())
        .find(|&i| layout.line_range(i).unwrap().contains(&huge))
        .unwrap();
    let r = layout.line_rect(line).unwrap();
    assert!(r.height() > font.line_height(40.0) - 0.5);
    assert!(!overlaps(r, at), "{r:?} over {at:?}");
    for b in glyph_boxes(&layout, text.text()) {
        assert!(!overlaps(b, at), "{b:?} over {at:?}");
    }
}

#[test]
fn hit_testing_and_carets_follow_the_pieces() {
    let Some(font) = font() else { return };
    let at = Rect::from_xywh(160.0, 0.0, 80.0, 200.0);
    let flow = TextFlow::new(400.0).item(FlowItem::at(at.origin), at.size);
    let layout = font.layout_flow(LOREM, 14.0, &flow);
    let (a, b) = (layout.line_rect(0).unwrap(), layout.line_rect(1).unwrap());
    assert_eq!(a.min_y(), b.min_y());
    assert!(a.max_x() <= at.min_x() && b.min_x() >= at.max_x());
    let r1 = layout.line_range(1).unwrap();
    // The start of the right piece is drawn at its left edge, and a click
    // there finds it.
    let p = layout.caret_position(r1.start);
    assert!(
        (p.x - b.min_x()).abs() < 0.5 && p.y == b.min_y(),
        "{p:?} {b:?}"
    );
    assert_eq!(
        layout.index_at(Point::new(b.min_x() + 0.5, b.min_y() + 3.0)),
        r1.start
    );
    // A click over the shape lands at the nearer piece's end.
    let i = layout.index_at(Point::new(at.min_x() + 5.0, a.min_y() + 3.0));
    assert_eq!(i, layout.line_range(0).unwrap().end);
    // A selection across both pieces highlights both.
    let rects = layout.selection_rects(0, r1.start + 3);
    assert!(rects.iter().any(|r| r.max_x() <= at.min_x() + 0.01));
    assert!(rects.iter().any(|r| r.min_x() >= at.max_x() - 0.01));
}

#[test]
fn a_floating_image_in_rich_text_keeps_text_off_its_opaque_pixels() {
    let Some(font) = font() else { return };
    let img = star(120);
    let text = RichText::new()
        .float(
            FloatImage::new(img.clone())
                .right()
                .alpha(0.5)
                .margin(2.0)
                .alt("a star"),
        )
        .push(LOREM)
        .push(" ")
        .push(LOREM);
    assert!(text.plain_text().starts_with("a starLorem"));
    assert_eq!(text.floats().count(), 1);
    let width = 320.0;
    let layout = text.layout(&font, 15.0, TextDirection::Auto, Some(width));
    let rects = layout.float_rects();
    assert_eq!(rects.len(), 1);
    let star_at = rects[0];
    assert_eq!(star_at, Rect::from_xywh(200.0, 0.0, 120.0, 120.0));
    // Lines run into the star's transparent corners: some line beside it
    // reaches past its box's left edge.
    let into = (0..layout.line_count()).any(|i| {
        let r = layout.line_rect(i).unwrap();
        r.min_y() < star_at.max_y() && r.max_x() > star_at.min_x() + 5.0
    });
    assert!(into, "no text over the star's transparent pixels");
    // But no character box covers an opaque pixel.
    let opaque = |x: f64, y: f64| {
        let (px, py) = ((x - star_at.min_x()) as i64, (y - star_at.min_y()) as i64);
        (0..120).contains(&px)
            && (0..120).contains(&py)
            && img.pixel(px as u32, py as u32).is_some_and(|p| p[3] > 127)
    };
    for b in glyph_boxes(&layout, text.text()) {
        let mut y = b.min_y().floor() + 0.5;
        while y < b.max_y() {
            let mut x = b.min_x().floor() + 0.5;
            while x < b.max_x() {
                assert!(!opaque(x, y), "{b:?} covers the star at ({x}, {y})");
                x += 1.0;
            }
            y += 1.0;
        }
    }
    // Drawn, the star is there and no ink is on it.
    let size = layout.size();
    let mut scene = Scene::new(size);
    scene.fill_rect(Rect::new(Point::ORIGIN, size), Color::WHITE);
    scene.fill_layout(&layout, Point::ORIGIN, Color::rgb(0, 0, 255));
    let pm = SoftwareRenderer::new().render(scene, ScaleFactor::IDENTITY);
    if let Ok(dir) = std::env::var("STIPPLE_FLOW_PNG") {
        let _ = pm.save_png(format!("{dir}/flow-star.png"));
    }
    let mut red = 0;
    for y in 0..120u32 {
        for x in 0..120u32 {
            if img.pixel(x, y).is_some_and(|p| p[3] > 250) {
                let p = pm.pixel(200 + x, y).unwrap();
                assert!(p[2] < 120, "ink at ({}, {y}) over the star: {p:?}", 200 + x);
                red += 1;
            }
        }
    }
    assert!(red > 1000);
}

#[test]
fn an_inserted_float_lands_between_runs() {
    let pm = star(16);
    let t = RichText::new()
        .push("ab")
        .bold("cd")
        .insert_float(3, FloatImage::new(pm).left());
    assert_eq!(t.text(), "abc\u{FFFC}d");
    let floats: Vec<usize> = t.floats().map(|(at, _)| at).collect();
    assert_eq!(floats, vec![3]);
    // The bold run is split around it.
    let (_, spans) = t.into_parts();
    let spans = spans.unwrap();
    let ranges: Vec<_> = spans.iter().map(|s| s.range.clone()).collect();
    assert_eq!(ranges, vec![0..2, 2..3, 3..6, 6..7]);
}

#[test]
fn moving_a_shape_refills_only_the_lines_it_touches() {
    let Some(font) = font() else { return };
    let text = format!("{LOREM}\n{LOREM}\n{LOREM}");
    let lay = |y: f64| {
        let flow =
            TextFlow::new(400.0).item(FlowItem::at(Point::new(150.0, y)), Size::new(60.0, 40.0));
        let l = font.layout_flow(&text, 14.0, &flow);
        (0..l.line_count())
            .map(|i| (l.line_range(i).unwrap(), l.line_rect(i).unwrap()))
            .collect::<Vec<_>>()
    };
    let a = lay(200.0);
    let b = lay(210.0);
    // The lines above the shape are the same.
    let above = a.iter().take_while(|(_, r)| r.max_y() < 190.0).count();
    assert!(above > 5);
    assert_eq!(a[..above], b[..above]);
    // Laying the same out again gives the same lines.
    assert_eq!(lay(210.0), b);
}
