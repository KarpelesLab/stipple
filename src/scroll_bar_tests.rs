//! Scroll bars in the app: drawn for the containers whose content overflows
//! (the virtualized table's and tree's included), dragged, paged, hovered,
//! and overlay bars showing while their container scrolls.

use crate::core::scroll_bar::BarPaint;
use crate::core::{Cx, Element, LayoutNode, ScrollId};
use crate::geometry::{Point, Rect, Size};
use crate::platform::{ButtonState, Event, PointerButton};
use crate::style::{ScrollBarLook, Theme};
use crate::widgets::{
    Column, SelectionMode, TableSpec, TableState, TreeModel, TreeSpec, TreeState, table, tree,
};
use crate::{App, widgets};

fn tree_of<S: 'static>(app: &App<S>) -> &LayoutNode {
    app.panes[0].tree.as_ref().expect("a frame was built")
}

/// The scroll container `id` and its bar this frame.
fn bar<S: 'static>(app: &App<S>, id: ScrollId) -> (&LayoutNode, Option<&BarPaint>) {
    let node = crate::core::find_scroll(tree_of(app), id).expect("the container");
    (node, node.scroll_bar.as_deref())
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

struct Table {
    table: TableState,
    taps: usize,
}

/// A 10,000-row table (24 px rows, a 240 px body) in a 400×320 window, with
/// the classic bar.
fn table_app() -> App<Table> {
    App::new(
        Table {
            table: TableState::new(SelectionMode::Single),
            taps: 0,
        },
        |state: &Table, cx: &mut Cx<Table>| {
            let theme = *cx.theme();
            let cols = [Column::new("Name", 120.0), Column::new("Value", 80.0)];
            let grid = table(
                cx,
                &state.table,
                |s| &mut s.table,
                TableSpec::new(&cols, 10_000).row_height(24.0).height(240.0),
                |_cx, row, col| Element::text(format!("{row}:{col}"), 14.0, theme.palette.text),
            );
            widgets::column(vec![grid])
        },
    )
    .no_font()
    .theme(Theme::light().scroll_bar(ScrollBarLook::CLASSIC))
    .logical_size(Size::new(400.0, 320.0))
}

/// The table body's container.
const BODY: ScrollId = ScrollId(0);

#[test]
fn the_virtualized_table_has_a_bar_for_all_its_rows_beside_its_header() {
    let mut app = table_app();
    app.render_once();
    let (node, bar) = bar(&app, BODY);
    let bar = bar.expect("10,000 rows overflow the body");
    let body = node.bounds;
    assert_eq!(body.height(), 240.0);
    // The bar is the body's: on its right edge, from its top (under the
    // header and its rule), not over the header.
    let (track, _) = bar.track.expect("a classic bar has a track");
    assert_eq!(track.min_y(), body.min_y());
    assert_eq!(track.max_x(), body.max_x());
    assert_eq!(track.width(), 12.0);
    assert!(track.min_y() >= 26.0, "below the header: {track:?}");
    // Rows leave the gutter to the bar.
    let row = &node.children[0].children[1];
    assert!(row.bounds.max_x() <= track.min_x(), "{:?}", row.bounds);
    // 240 px of 240,000: the thumb is at its 24 px floor, at the top.
    assert_eq!(bar.thumb.height(), 24.0);
    assert_eq!(bar.thumb.min_y(), body.min_y() + 3.0);
}

#[test]
fn dragging_the_thumb_scrolls_proportionally() {
    let mut app = table_app();
    app.render_once();
    let (node, b) = bar(&app, BODY);
    let (body, thumb) = (node.bounds, b.unwrap().thumb);
    let grab = center(thumb);
    assert!(press(&mut app, grab));
    // The press is the bar's: nothing taps, the row under it isn't selected.
    assert_eq!(app.state().table.rows.cursor(), None);
    app.render_once();
    assert_eq!(bar(&app, BODY).1.unwrap().thumb.width(), 8.0, "grown");
    // Half the thumb's travel down: half the content.
    let travel = body.height() - 6.0 - thumb.height();
    let max = 10_000.0 * 24.0 - 240.0;
    assert!(move_to(
        &mut app,
        Point::new(grab.x + 30.0, grab.y + travel / 2.0)
    ));
    assert!((offset(&app, BODY) - max / 2.0).abs() < 1e-6);
    app.render_once();
    // The rows there were built, and the thumb followed the pointer.
    let t = bar(&app, BODY).1.unwrap().thumb;
    assert!((center(t).y - (grab.y + travel / 2.0)).abs() < 1e-6);
    // Past the end: the last row.
    move_to(&mut app, Point::new(grab.x, 1000.0));
    assert_eq!(offset(&app, BODY), max);
    assert!(release(&mut app, Point::new(grab.x, 1000.0)));
    app.render_once();
    assert_eq!(bar(&app, BODY).1.unwrap().thumb.width(), 6.0, "at rest");
    // Moves after the release don't scroll.
    move_to(&mut app, grab);
    assert_eq!(offset(&app, BODY), max);
    let _ = app.state().taps;
}

#[test]
fn pressing_the_track_pages() {
    let mut app = table_app();
    app.render_once();
    let (node, b) = bar(&app, BODY);
    let (body, track) = (node.bounds, b.unwrap().track.unwrap().0);
    let below = Point::new(center(track).x, body.max_y() - 10.0);
    assert!(press(&mut app, below));
    release(&mut app, below);
    // A viewport less a tenth of it (24 px, under the 40 px cap).
    assert_eq!(offset(&app, BODY), 216.0);
    app.render_once();
    assert!(press(&mut app, below));
    release(&mut app, below);
    assert_eq!(offset(&app, BODY), 432.0);
    // Above the thumb pages back.
    app.render_once();
    let above = Point::new(center(track).x, body.min_y() + 2.0);
    press(&mut app, above);
    release(&mut app, above);
    assert_eq!(offset(&app, BODY), 216.0);
}

#[test]
fn hovering_the_bar_grows_and_darkens_the_thumb() {
    let mut app = table_app();
    app.render_once();
    let (_, b) = bar(&app, BODY);
    let rest = b.unwrap().clone();
    assert!(move_to(&mut app, center(rest.thumb)));
    app.render_once();
    let hover = bar(&app, BODY).1.unwrap().clone();
    assert_eq!(hover.thumb.width(), 8.0);
    assert!(hover.thumb_color.a > rest.thumb_color.a);
    // Off the bar, back to rest.
    assert!(move_to(&mut app, Point::new(50.0, 100.0)));
    app.render_once();
    assert_eq!(bar(&app, BODY).1.unwrap(), &rest);
}

/// `n` roots, of which root 0 has 5,000 children.
struct Nodes(u64);

impl TreeModel for Nodes {
    fn children(&self, parent: Option<u64>) -> Vec<u64> {
        match parent {
            None => (0..self.0).collect(),
            Some(0) => (1000..6000).collect(),
            Some(_) => Vec::new(),
        }
    }
}

#[test]
fn the_tree_bar_spans_its_expanded_rows() {
    let mut app = App::new(
        TreeState::new(),
        |state: &TreeState, cx: &mut Cx<TreeState>| {
            let theme = *cx.theme();
            let model = Nodes(4);
            let view = tree(
                cx,
                state,
                |s| s,
                TreeSpec::new(&model).row_height(20.0).height(200.0),
                |_cx, row| Element::text(row.id.to_string(), 14.0, theme.palette.text),
            );
            widgets::column(vec![view])
        },
    )
    .no_font()
    .theme(Theme::light().scroll_bar(ScrollBarLook::CLASSIC))
    .logical_size(Size::new(300.0, 300.0));
    app.render_once();
    // Four rows fit: no bar.
    assert!(bar(&app, ScrollId(0)).1.is_none());
    app.state.set_expanded(0, true);
    app.panes[0].dirty = true;
    app.render_once();
    let (node, b) = bar(&app, ScrollId(0));
    let g = crate::core::scroll_bar::geometry(node, 0.0, &ScrollBarLook::CLASSIC).unwrap();
    // 5,004 rows of 20 px.
    assert_eq!(g.max, 5004.0 * 20.0 - 200.0);
    assert_eq!(b.unwrap().thumb.height(), 24.0);
}

/// A plain list of `rows` 30 px rows in a 200 px viewport, with `look`.
fn list_app(rows: usize, look: ScrollBarLook) -> App<()> {
    App::new((), move |_: &(), cx: &mut Cx<()>| {
        let items = (0..rows)
            .map(|_| Element::boxed(Default::default()).height(30.0))
            .collect();
        widgets::scroll(cx).height(200.0).children(items)
    })
    .no_font()
    .theme(Theme::light().scroll_bar(look))
    .logical_size(Size::new(300.0, 300.0))
}

#[test]
fn a_list_that_fits_keeps_its_width_and_has_no_bar() {
    let mut app = list_app(5, ScrollBarLook::CLASSIC);
    app.render_once();
    let (node, b) = bar(&app, ScrollId(0));
    assert!(b.is_none());
    assert_eq!(node.children[0].bounds.width(), 300.0);
}

#[test]
fn an_overlay_bar_shows_while_scrolling_and_keeps_no_gutter() {
    let mut app = list_app(40, ScrollBarLook::OVERLAY);
    app.render_once();
    let (node, b) = bar(&app, ScrollId(0));
    assert_eq!(node.children[0].bounds.width(), 300.0, "no gutter");
    assert!(b.is_none(), "hidden at rest");
    app.panes[0].last_pointer = Point::new(100.0, 100.0);
    assert!(app.scroll_by(60.0));
    app.render_once();
    let b = bar(&app, ScrollId(0)).1.expect("shown while scrolling");
    assert!(
        b.track.is_none(),
        "an overlay bar's track shows under the pointer"
    );
    assert_eq!(b.thumb.width(), 7.0);
    assert!(app.panes[0].animating, "frames go on until it fades");
    // The pointer over its edge brings the track, and the thumb grows.
    assert!(move_to(&mut app, Point::new(295.0, 150.0)));
    app.render_once();
    let b = bar(&app, ScrollId(0)).1.unwrap();
    assert!(b.track.is_some());
    assert_eq!(b.thumb.width(), 11.0);
}

#[test]
fn a_touch_bar_takes_no_press() {
    let mut app = list_app(40, ScrollBarLook::TOUCH);
    app.render_once();
    app.panes[0].last_pointer = Point::new(100.0, 100.0);
    app.scroll_by(30.0);
    app.render_once();
    let thumb = bar(&app, ScrollId(0))
        .1
        .expect("shown while scrolling")
        .thumb;
    assert_eq!(thumb.width(), 3.0);
    press(&mut app, center(thumb));
    move_to(&mut app, Point::new(center(thumb).x, 190.0));
    assert_eq!(offset(&app, ScrollId(0)), 30.0);
}
