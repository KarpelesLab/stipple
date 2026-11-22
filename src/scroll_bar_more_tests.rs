//! Scroll bars in the app, continued: a press held on the track or an arrow
//! repeating on the test clock, arrow buttons, a container scrolling both
//! ways (its gutters and corner, left to right and mirrored), a scrolling
//! grid's gutter, skinned bars, and the bars as assistive technologies see
//! them.

use crate::core::scroll_bar::BarPaint;
use crate::core::{Cx, Element, LayoutNode, ScrollId};
use crate::geometry::{Point, Rect, Size};
use crate::layout::LayoutDirection;
use crate::platform::{ButtonState, Event, PointerButton};
use crate::style::{ScrollBarLook, Theme};
use crate::{App, widgets};

fn tree_of<S: 'static>(app: &App<S>) -> &LayoutNode {
    app.panes[0].tree.as_ref().expect("a frame was built")
}

/// The scroll container `id`.
fn container<S: 'static>(app: &App<S>, id: ScrollId) -> &LayoutNode {
    crate::core::find_scroll(tree_of(app), id).expect("the container")
}

fn bar<S: 'static>(app: &App<S>, id: ScrollId) -> BarPaint {
    container(app, id)
        .scroll_bar
        .as_deref()
        .expect("a bar")
        .clone()
}

fn offset<S: 'static>(app: &App<S>, id: ScrollId) -> f64 {
    app.panes[0].scroll_offsets.get(&id).copied().unwrap_or(0.0)
}

fn press<S: 'static>(app: &mut App<S>, at: Point) -> bool {
    app.pointer_event(Event::PointerButton {
        button: PointerButton::Left,
        state: ButtonState::Pressed,
        position: at,
    })
}

fn release<S: 'static>(app: &mut App<S>, at: Point) -> bool {
    app.pointer_event(Event::PointerButton {
        button: PointerButton::Left,
        state: ButtonState::Released,
        position: at,
    })
}

fn move_to<S: 'static>(app: &mut App<S>, at: Point) -> bool {
    app.pointer_event(Event::PointerMoved { position: at })
}

fn center(r: Rect) -> Point {
    Point::new(r.min_x() + r.width() / 2.0, r.min_y() + r.height() / 2.0)
}

/// Advance the test clock by `secs`, and let a press held on a bar repeat
/// as the loop's wake would. Returns whether it scrolled.
fn tick<S: 'static>(app: &mut App<S>, secs: f64) -> bool {
    crate::platform::time::test_clock::advance(std::time::Duration::from_secs_f64(secs));
    app.pane_scroll_bar_repeat(0)
}

/// A plain list of `rows` 30 px rows in a 200 px viewport, with `look`.
fn list_app(rows: usize, look: ScrollBarLook) -> App<()> {
    list_app_in(rows, Theme::light().scroll_bar(look))
}

/// [`list_app`] in `theme`.
fn list_app_in(rows: usize, theme: Theme) -> App<()> {
    App::new((), move |_: &(), cx: &mut Cx<()>| {
        let items = (0..rows)
            .map(|_| Element::boxed(Default::default()).height(30.0))
            .collect();
        widgets::column(vec![widgets::scroll(cx).height(200.0).children(items)])
    })
    .no_font()
    .theme(theme)
    .logical_size(Size::new(300.0, 300.0))
}

const LIST: ScrollId = ScrollId(0);

#[test]
fn holding_the_track_pages_until_the_thumb_reaches_the_pointer() {
    // 40 rows of 30 px in 200: a page is 180 px, the largest offset 1,000.
    let mut app = list_app(40, ScrollBarLook::CLASSIC);
    app.render_once();
    let at = Point::new(294.0, 100.0);
    assert!(press(&mut app, at));
    assert_eq!(offset(&app, LIST), 180.0, "a page at once");
    assert!(app.panes[0].scroll_bars.timer.is_some(), "a repeat is due");
    app.render_once();
    // Not before the delay (0.25 s for the classic bar).
    assert!(!tick(&mut app, 0.2));
    assert_eq!(offset(&app, LIST), 180.0);
    assert!(tick(&mut app, 0.06));
    assert_eq!(offset(&app, LIST), 360.0);
    app.render_once();
    // Then every 0.05 s, until the thumb is under the pointer.
    assert!(!tick(&mut app, 0.03), "not yet");
    assert!(tick(&mut app, 0.03));
    assert_eq!(offset(&app, LIST), 540.0);
    app.render_once();
    let thumb = bar(&app, LIST).thumb;
    assert!(thumb.min_y() <= at.y && at.y <= thumb.max_y(), "{thumb:?}");
    for _ in 0..5 {
        assert!(!tick(&mut app, 0.06), "the thumb reached the pointer");
    }
    assert_eq!(offset(&app, LIST), 540.0);
    // Moved further down, it pages on again.
    move_to(&mut app, Point::new(294.0, 190.0));
    assert!(tick(&mut app, 0.06));
    assert_eq!(offset(&app, LIST), 720.0);
    app.render_once();
    // Off the bar it waits.
    move_to(&mut app, Point::new(100.0, 190.0));
    assert!(!tick(&mut app, 0.06));
    assert_eq!(offset(&app, LIST), 720.0);
    release(&mut app, Point::new(100.0, 190.0));
    assert!(app.panes[0].scroll_bars.timer.is_none(), "nothing repeats");
    assert!(!tick(&mut app, 1.0));
    assert_eq!(offset(&app, LIST), 720.0);
}

#[test]
fn arrow_buttons_step_and_repeat_while_held() {
    use crate::platform::native::ArrowDir;
    let mut app = list_app(40, ScrollBarLook::CLASSIC.arrows(true));
    app.render_once();
    let b = bar(&app, LIST);
    let dirs: Vec<_> = b.arrows.iter().map(|a| a.dir).collect();
    assert_eq!(dirs, vec![ArrowDir::Up, ArrowDir::Down]);
    assert_eq!(b.arrows[0].rect, Rect::from_xywh(288.0, 0.0, 12.0, 12.0));
    assert_eq!(b.arrows[1].rect, Rect::from_xywh(288.0, 188.0, 12.0, 12.0));
    // The thumb runs between them.
    assert_eq!(b.thumb.min_y(), 15.0);
    // At the top, the up arrow is faded: there's nowhere to go.
    assert!(b.arrows[0].glyph.a < b.arrows[1].glyph.a);
    let down = center(b.arrows[1].rect);
    assert!(press(&mut app, down));
    assert_eq!(offset(&app, LIST), 40.0, "a step");
    app.render_once();
    assert!(bar(&app, LIST).arrows[1].fill.is_some(), "drawn pressed");
    assert!(!tick(&mut app, 0.2));
    assert!(tick(&mut app, 0.06));
    assert_eq!(offset(&app, LIST), 80.0);
    assert!(tick(&mut app, 0.05));
    assert_eq!(offset(&app, LIST), 120.0);
    release(&mut app, down);
    assert!(!tick(&mut app, 0.5));
    // The up arrow steps back.
    app.render_once();
    let up = center(bar(&app, LIST).arrows[0].rect);
    press(&mut app, up);
    release(&mut app, up);
    assert_eq!(offset(&app, LIST), 80.0);
}

#[test]
fn windows_bars_have_arrows_and_others_none() {
    assert!(ScrollBarLook::WINDOWS.shows_arrows());
    assert!(!ScrollBarLook::CLASSIC.shows_arrows());
    // An overlay or touch bar never shows them.
    assert!(!ScrollBarLook::OVERLAY.arrows(true).shows_arrows());
    assert!(!ScrollBarLook::TOUCH.arrows(true).shows_arrows());
    assert_eq!(
        ScrollBarLook::platform().arrows,
        cfg!(target_os = "windows")
    );
}

/// A 300×200 two-way scroll area over 600×1000 px of content, laid out in
/// `dir`.
fn both_app(dir: LayoutDirection) -> App<()> {
    App::new((), |_: &(), cx: &mut Cx<()>| {
        let content = Element::boxed(Default::default())
            .min_width(600.0)
            .height(1000.0);
        widgets::scroll_both(cx).height(200.0).child(content)
    })
    .no_font()
    .theme(
        Theme::light()
            .scroll_bar(ScrollBarLook::CLASSIC)
            .layout_direction(dir),
    )
    .logical_size(Size::new(300.0, 200.0))
}

const DOWN: ScrollId = ScrollId(0);
const ACROSS: ScrollId = ScrollId(1);

#[test]
fn a_two_way_area_keeps_both_gutters_and_a_corner() {
    let mut app = both_app(LayoutDirection::Ltr);
    app.render_once();
    let node = container(&app, DOWN);
    let v = node.scroll_bar.as_deref().expect("a vertical bar");
    let h = node.scroll_bar_x.as_deref().expect("a horizontal bar");
    assert_eq!(v.strip, Rect::from_xywh(288.0, 0.0, 12.0, 188.0));
    assert_eq!(h.strip, Rect::from_xywh(0.0, 188.0, 288.0, 12.0));
    let (corner, _) = v.corner.expect("the corner between them");
    assert_eq!(corner, Rect::from_xywh(288.0, 188.0, 12.0, 12.0));
    // Each way the content scrolls a gutter further, so neither bar covers
    // its last row or column.
    assert_eq!(crate::core::scroll_range_for(node, DOWN, 0.0), 812.0);
    assert_eq!(crate::core::scroll_range_for(node, ACROSS, 0.0), 312.0);
    // Dragging the bottom thumb to its end shows the last column beside
    // the vertical bar.
    let grab = center(h.thumb);
    press(&mut app, grab);
    move_to(&mut app, Point::new(1000.0, grab.y));
    release(&mut app, Point::new(1000.0, grab.y));
    assert_eq!(offset(&app, ACROSS), 312.0);
    app.render_once();
    assert_eq!(container(&app, DOWN).children[0].bounds.max_x(), 288.0);
    // A press on the corner takes nothing.
    assert!(!press(&mut app, Point::new(294.0, 194.0)));
}

#[test]
fn a_two_way_area_mirrors_right_to_left() {
    let mut app = both_app(LayoutDirection::Rtl);
    app.render_once();
    let node = container(&app, DOWN);
    let v = node.scroll_bar.as_deref().unwrap();
    let h = node.scroll_bar_x.as_deref().unwrap();
    // The vertical bar on the left, the corner at the bottom left.
    assert_eq!(v.strip, Rect::from_xywh(0.0, 0.0, 12.0, 188.0));
    assert_eq!(h.strip, Rect::from_xywh(12.0, 188.0, 288.0, 12.0));
    assert_eq!(v.corner.unwrap().0, Rect::from_xywh(0.0, 188.0, 12.0, 12.0));
    // The content starts at the right, and so does its thumb.
    assert_eq!(node.children[0].bounds.max_x(), 300.0);
    assert_eq!(h.thumb.max_x(), 300.0 - 3.0);
    assert_eq!(crate::core::scroll_range_for(node, ACROSS, 0.0), 312.0);
    // Dragged left, it scrolls on: the content moves right.
    let grab = center(h.thumb);
    press(&mut app, grab);
    move_to(&mut app, Point::new(-1000.0, grab.y));
    release(&mut app, Point::new(-1000.0, grab.y));
    assert_eq!(offset(&app, ACROSS), 312.0);
    app.render_once();
    let left = container(&app, DOWN).children[0].bounds.min_x();
    assert_eq!(left, 12.0, "its left edge shows beside the bar");
    // A wheel to the right goes back towards the start.
    app.panes[0].last_pointer = Point::new(150.0, 100.0);
    assert!(app.pane_scroll_by(0, 100.0, 0.0));
    assert_eq!(offset(&app, ACROSS), 212.0);
}

#[test]
fn a_scrolling_grid_keeps_a_gutter() {
    use crate::layout::Track;
    for dir in [LayoutDirection::Ltr, LayoutDirection::Rtl] {
        let mut app = App::new((), |_: &(), cx: &mut Cx<()>| {
            let id = cx.register_scroll();
            let cells = (0..40)
                .map(|_| Element::boxed(Default::default()).height(40.0))
                .collect();
            widgets::grid(vec![Track::Fr(1.0); 2], cells)
                .scrollable(id)
                .height(200.0)
        })
        .no_font()
        .theme(
            Theme::light()
                .scroll_bar(ScrollBarLook::CLASSIC)
                .layout_direction(dir),
        )
        .logical_size(Size::new(300.0, 300.0));
        app.render_once();
        let strip = bar(&app, LIST).strip;
        // Two columns share what the bar leaves.
        for c in container(&app, LIST).children.iter().take(2) {
            assert_eq!(c.bounds.width(), 144.0, "{dir:?}");
            assert!(
                c.bounds.intersection(strip).is_none(),
                "{dir:?}: {:?} {strip:?}",
                c.bounds
            );
        }
    }
}

#[test]
fn a_skin_dresses_the_track_and_thumb() {
    use crate::render::{NineSlice, Pixmap};
    use crate::style::{ScrollBarSkin, Skin, StateSkin};
    use std::sync::Arc;
    let solid = |rgba: [u8; 4]| {
        let px = Arc::new(Pixmap::from_rgba8(
            crate::geometry::PhysicalSize::new(3, 3),
            rgba.repeat(9),
        ));
        NineSlice::new(px, crate::geometry::Insets::uniform(1.0))
    };
    let skin = Skin::new()
        .scroll_bar(
            ScrollBarSkin::new()
                .track(solid([255, 0, 0, 255]))
                .thumb(StateSkin::new(solid([0, 0, 255, 255]))),
        )
        .leak();
    let theme = Theme::light().scroll_bar(ScrollBarLook::CLASSIC).skin(skin);
    let mut app = list_app_in(40, theme);
    let frame = app.render_once();
    let b = bar(&app, LIST);
    assert!(matches!(b.dress, crate::core::scroll_bar::Dress::Skin(_)));
    // The thumb's box spans the strip's width.
    assert_eq!(b.thumb_box.width(), 12.0);
    let px = |x: u32, y: u32| frame.pixel(x, y).unwrap();
    assert_eq!(px(294, 150), [255, 0, 0, 255], "the track's image");
    assert_eq!(px(294, 10), [0, 0, 255, 255], "the thumb's");
}

#[test]
fn bars_are_scroll_bars_to_assistive_technologies() {
    use crate::core::Role;
    let mut app = both_app(LayoutDirection::Ltr);
    app.render_once();
    app.panes[0].scroll_offsets.insert(DOWN, 406.0);
    app.panes[0].dirty = true;
    app.render_once();
    let a = app.accessibility_tree().unwrap();
    let bars: Vec<_> = a
        .descendants()
        .into_iter()
        .filter(|n| n.role == Role::ScrollBar)
        .collect();
    assert_eq!(bars.len(), 2, "one each way");
    let v = bars[0].scroll_bar.unwrap();
    assert!(!v.horizontal);
    assert_eq!((v.offset, v.max), (406.0, 812.0));
    assert_eq!(bars[0].bounds, Rect::from_xywh(288.0, 0.0, 12.0, 188.0));
    let h = bars[1].scroll_bar.unwrap();
    assert!(h.horizontal);
    assert_eq!((h.offset, h.max), (0.0, 312.0));
    // They come after the content.
    let area = a
        .descendants()
        .into_iter()
        .find(|n| n.scroll.is_some())
        .unwrap();
    assert_eq!(area.children.last().map(|n| n.role), Some(Role::ScrollBar));
    // Through the platform mirror: setting the vertical bar's value (as
    // AT-SPI's `CurrentValue` and UIA's `SetValue` do) scrolls the area.
    let mirror = crate::to_platform_a11y(&a);
    fn first_bar(n: &crate::platform::A11yNode) -> Option<&crate::platform::A11yNode> {
        if n.role == crate::platform::A11yRole::ScrollBar {
            return Some(n);
        }
        n.children.iter().find_map(first_bar)
    }
    let node = first_bar(&mirror).unwrap().clone();
    let s = node.scroll_bar.unwrap();
    for event in node.scroll_by_events(s.to(100.0)).unwrap() {
        app.pointer_event(event);
    }
    assert_eq!(offset(&app, DOWN), 100.0);
    // The web's overlay says so in ARIA.
    let json = crate::platform::access::aria_json(&mirror);
    assert!(json.contains("\"r\":\"scrollbar\""), "{json}");
    assert!(json.contains("\"sb\":[406,812,0]"), "{json}");
}
