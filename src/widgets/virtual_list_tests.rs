//! `virtual_list` through the `App`: scrolling to a row from code, following
//! the end as rows are appended, filling the parent, plain rows, the
//! near-the-end hook, and keyed scroll containers.

use crate::App;
use crate::core::{AccessNode, Cx, Element, Role};
use crate::geometry::{Point, Size};
use crate::render::Color;
use crate::widgets::{ListState, ScrollAlign, SelectionMode, VirtualList, column, virtual_list};

const ROW: f64 = 24.0;

struct Feed {
    list: ListState,
    rows: usize,
    /// Rows the end hook asked for.
    pages: usize,
    /// A scroll container built before the list, some of the time.
    banner: bool,
}

impl Feed {
    fn new(rows: usize) -> Self {
        Feed {
            list: ListState::new(SelectionMode::Single),
            rows,
            pages: 0,
            banner: false,
        }
    }
}

fn row(i: usize) -> Element {
    Element::text(format!("Row {i}"), 14.0, Color::BLACK)
}

/// A 320×300 window with `spec` built for the feed's rows under a 50px
/// header.
fn app(feed: Feed, spec: fn(&Feed) -> VirtualList<'static, Feed>) -> App<Feed> {
    App::new(feed, move |s: &Feed, cx: &mut Cx<Feed>| {
        let banner = if s.banner {
            let id = cx.register_scroll();
            Element::stack(crate::core::Axis::Vertical, vec![]).scrollable(id)
        } else {
            Element::boxed(Default::default())
        };
        let list = virtual_list(cx, &s.list, |s| &mut s.list, spec(s), |_, r| row(r.index));
        column(vec![
            banner,
            Element::boxed(Default::default()).height(50.0),
            list,
        ])
    })
    .no_font()
    .logical_size(Size::new(320.0, 300.0))
}

fn items(app: &App<Feed>) -> Vec<AccessNode> {
    let a = app.accessibility_tree().unwrap();
    a.descendants()
        .into_iter()
        .filter(|n| n.role == Role::ListItem)
        .cloned()
        .collect()
}

/// The rows that are (at least partly) inside the list's viewport.
fn shown(app: &App<Feed>) -> Vec<String> {
    let a = app.accessibility_tree().unwrap();
    let list = a
        .descendants()
        .into_iter()
        .find(|n| n.role == Role::List)
        .unwrap()
        .clone();
    let view = list.bounds;
    items(app)
        .into_iter()
        .filter(|n| n.bounds.max_y() > view.min_y() + 1.5 && n.bounds.min_y() < view.max_y() - 1.5)
        .map(|n| n.name)
        .collect()
}

fn update(app: &mut App<Feed>, f: impl FnOnce(&mut Feed) + Send + 'static) {
    app.handle().update(f);
    assert!(app.run_pending());
    app.render_once();
}

#[test]
fn a_handler_scrolls_to_a_row_and_aligns_it() {
    let mut app = app(Feed::new(10_000), |s| {
        VirtualList::fixed(s.rows, ROW).height(240.0)
    });
    app.render_once();
    assert_eq!(shown(&app).first().map(String::as_str), Some("Row 0"));

    update(&mut app, |s| {
        s.list.scroll_to_row(5_000, ScrollAlign::Start)
    });
    // Shown in the very frame built for it.
    let rows = shown(&app);
    assert_eq!(
        rows.first().map(String::as_str),
        Some("Row 5000"),
        "{rows:?}"
    );

    update(&mut app, |s| s.list.scroll_to_row(100, ScrollAlign::End));
    assert_eq!(shown(&app).last().map(String::as_str), Some("Row 100"));

    update(&mut app, |s| s.list.scroll_to_row(300, ScrollAlign::Center));
    let rows = shown(&app);
    let mid = rows.iter().position(|r| r == "Row 300").unwrap();
    assert!(mid.abs_diff(rows.len() / 2) <= 1, "{rows:?}");

    // Nearest: a row already in view doesn't move the list.
    update(&mut app, |s| s.list.reveal(302));
    assert_eq!(shown(&app), rows);
    // One just below it scrolls it to the bottom edge.
    update(&mut app, |s| s.list.reveal(320));
    assert_eq!(shown(&app).last().map(String::as_str), Some("Row 320"));
}

#[test]
fn sticking_to_the_end_follows_appends_until_scrolled_up() {
    let mut app = app(Feed::new(100), |s| {
        VirtualList::fixed(s.rows, ROW)
            .height(240.0)
            .stick_to_end(true)
    });
    app.render_once();
    // It opens at the end.
    assert_eq!(shown(&app).last().map(String::as_str), Some("Row 99"));

    update(&mut app, |s| s.rows += 5);
    assert_eq!(shown(&app).last().map(String::as_str), Some("Row 104"));

    // Scrolled up, it stays where it was as rows come in.
    app.hover_at(Point::new(100.0, 150.0));
    assert!(app.scroll_by(-10.0 * ROW));
    app.render_once();
    let before = shown(&app);
    assert!(!before.contains(&"Row 104".to_string()));
    update(&mut app, |s| s.rows += 5);
    assert_eq!(shown(&app), before);

    // Back at the end, it follows again.
    assert!(app.scroll_by(1e6));
    app.render_once();
    update(&mut app, |s| s.rows += 3);
    assert_eq!(shown(&app).last().map(String::as_str), Some("Row 112"));
}

#[test]
fn with_no_height_it_fills_the_rest_of_the_column() {
    let mut app = app(Feed::new(1_000), |s| VirtualList::fixed(s.rows, ROW));
    app.render_once();
    app.render_once();
    let a = app.accessibility_tree().unwrap();
    let list = a
        .descendants()
        .into_iter()
        .find(|n| n.role == Role::List)
        .unwrap();
    // 300px window, 50px header.
    assert!(
        (list.bounds.min_y() - 50.0).abs() < 0.5,
        "{:?}",
        list.bounds
    );
    assert!(
        (list.bounds.max_y() - 300.0).abs() < 0.5,
        "{:?}",
        list.bounds
    );
    let scroll = list.scroll.unwrap();
    assert!((scroll.viewport - 250.0).abs() < 0.5, "{scroll:?}");
    // The rows cover the whole viewport, and not many more are built.
    let rows = shown(&app);
    assert_eq!(rows.len(), (248.0 / ROW).ceil() as usize);
    assert!(items(&app).len() <= rows.len() + 2 * crate::widgets::DEFAULT_OVERSCAN + 1);

    // Following the end works out the height the layout gives it, too.
    let mut app = self::app(Feed::new(1_000), |s| {
        VirtualList::fixed(s.rows, ROW).stick_to_end(true)
    });
    app.render_once();
    app.render_once();
    assert_eq!(shown(&app).last().map(String::as_str), Some("Row 999"));
}

#[test]
fn plain_rows_have_no_frame_and_select_nothing() {
    let mut app = app(Feed::new(50), |s| {
        VirtualList::fixed(s.rows, ROW).height(240.0).plain(true)
    })
    // An overlay bar takes no width, so the rows span the list on every
    // platform (a classic bar would leave them 12 px short).
    .theme({
        let mut theme = crate::Theme::light();
        theme.scroll_bar = crate::style::ScrollBarLook::OVERLAY;
        theme
    });
    app.render_once();
    let first = items(&app).into_iter().next().unwrap();
    // Edge to edge: no frame inset, no padding.
    assert_eq!(first.bounds.min_x(), 0.0);
    assert_eq!(first.bounds.min_y(), 50.0);
    assert_eq!(first.bounds.width(), 320.0);
    app.click_at(Point::new(100.0, 50.0 + ROW * 1.5));
    app.render_once();
    assert_eq!(app.state().list.selected_count(), 0);
    assert!(items(&app).iter().all(|n| !n.selected));
}

#[test]
fn the_end_hook_runs_once_per_row_count_near_the_end() {
    let mut app = app(Feed::new(20), |s| {
        VirtualList::fixed(s.rows, ROW)
            .height(240.0)
            .on_end_reached(100.0, |s: &mut Feed| s.pages += 1)
    });
    app.render_once();
    // 480px of rows in a 240px viewport: 240px from the end.
    assert!(!app.run_pending());
    app.hover_at(Point::new(100.0, 150.0));
    assert!(app.scroll_by(150.0));
    app.render_once();
    assert!(app.run_pending());
    assert_eq!(app.state().pages, 1);
    // Still near the end, same rows: it doesn't run again.
    assert!(app.scroll_by(10.0));
    app.render_once();
    assert!(!app.run_pending());
    // The page came in: near its end again, it runs again.
    update(&mut app, |s| s.rows += 20);
    assert!(!app.run_pending());
    assert!(app.scroll_by(1e6));
    app.render_once();
    assert!(app.run_pending());
    assert_eq!(app.state().pages, 2);
}

#[test]
fn a_keyed_list_keeps_its_offset_when_containers_come_and_go_before_it() {
    let spec: fn(&Feed) -> VirtualList<'static, Feed> = |s| {
        VirtualList::fixed(s.rows, ROW)
            .height(240.0)
            .scroll_key("feed")
    };
    let mut app = app(Feed::new(1_000), spec);
    app.render_once();
    update(&mut app, |s| s.list.scroll_to_row(500, ScrollAlign::Start));
    assert_eq!(shown(&app).first().map(String::as_str), Some("Row 500"));
    // Another scroll container now comes first: the list's id doesn't move.
    update(&mut app, |s| s.banner = true);
    assert_eq!(shown(&app).first().map(String::as_str), Some("Row 500"));
}
