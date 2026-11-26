//! A `table` wider than its view, through the `App`: it scrolls sideways
//! with its header following (and staying put vertically), builds only the
//! columns in view, keeps frozen columns in place, moves the cell cursor
//! with Left/Right and scrolls it into view, mirrors right to left, and
//! numbers its cells and brings them into view for assistive technologies.

use crate::App;
use crate::core::{AccessNode, Cx, Element, KeyInput, Role};
use crate::geometry::{Point, Rect, Size};
use crate::layout::LayoutDirection;
use crate::platform::Event;
use crate::platform::ScrollDelta;
use crate::style::{ScrollBarLook, Theme};
use crate::widgets::{Column, SelectionMode, TableSpec, TableState, column, table};

/// 8 columns of 100 px in a 400 px window: the body is 398 wide (inside the
/// frame's hairline), and its rows show in the 386 its vertical bar leaves.
const COLUMNS: usize = 8;
const ROW: f64 = 24.0;

struct Grid {
    table: TableState,
}

fn app(frozen: usize, rtl: bool) -> App<Grid> {
    let theme = Theme::light().scroll_bar(ScrollBarLook::CLASSIC);
    let theme = if rtl {
        theme.layout_direction(LayoutDirection::Rtl)
    } else {
        theme
    };
    let mut app = App::new(
        Grid {
            table: TableState::new(SelectionMode::Single),
        },
        move |s: &Grid, cx: &mut Cx<Grid>| {
            let theme = *cx.theme();
            let cols: Vec<Column> = (0..COLUMNS)
                .map(|c| Column::new(format!("C{c}"), 100.0))
                .collect();
            let grid = table(
                cx,
                &s.table,
                |s| &mut s.table,
                TableSpec::new(&cols, 1_000)
                    .row_height(ROW)
                    .height(240.0)
                    .frozen_columns(frozen),
                |_cx, row, col| Element::text(format!("r{row}c{col}"), 14.0, theme.palette.text),
            );
            column(vec![grid])
        },
    )
    .no_font()
    .theme(theme)
    .logical_size(Size::new(400.0, 320.0));
    // The first frame guesses the body's width; the second is built for it.
    app.render_once();
    app.render_once();
    app
}

fn tree(app: &App<Grid>) -> AccessNode {
    app.accessibility_tree().unwrap()
}

/// The node named `name` with `role`, if built.
fn named(a: &AccessNode, role: Role, name: &str) -> Option<Rect> {
    a.descendants()
        .into_iter()
        .find(|n| n.role == role && n.name == name)
        .map(|n| n.bounds)
}

fn header(a: &AccessNode, col: usize) -> Option<Rect> {
    named(a, Role::ColumnHeader, &format!("C{col}"))
}

fn cell(a: &AccessNode, row: usize, col: usize) -> Option<Rect> {
    named(a, Role::Cell, &format!("r{row}c{col}"))
}

/// The sideways offset, from the table's horizontal scroll bar.
fn sideways(a: &AccessNode) -> f64 {
    a.descendants()
        .into_iter()
        .find_map(|n| n.scroll_bar.filter(|b| b.horizontal))
        .expect("a sideways bar")
        .offset
}

fn wheel(app: &mut App<Grid>, dx: f64, dy: f64) {
    app.hover_at(Point::new(200.0, 150.0));
    assert!(app.pointer_event(Event::Scroll {
        delta: ScrollDelta::pixels(dx, dy),
    }));
    app.render_once();
}

#[test]
fn wide_columns_scroll_sideways_under_a_header_that_follows() {
    let mut app = app(0, false);
    let a = tree(&app);
    // Both bars, each a node; the sideways one ranges over what the 386 px
    // the vertical bar leaves don't show of the 800.
    let bar = a
        .descendants()
        .into_iter()
        .find_map(|n| n.scroll_bar.filter(|b| b.horizontal))
        .unwrap();
    assert!((bar.max - (800.0 - 386.0)).abs() < 0.5, "{bar:?}");
    // Only the columns in view (and one past its edge) are built.
    let built: Vec<_> = (0..COLUMNS).filter(|&c| header(&a, c).is_some()).collect();
    assert_eq!(built, vec![0, 1, 2, 3, 4]);
    assert!(cell(&a, 0, 5).is_none());
    assert_eq!(header(&a, 2).unwrap().min_x(), 201.0);
    let header_y = header(&a, 2).unwrap().min_y();

    // A sideways wheel moves the header and the cells together.
    wheel(&mut app, 150.0, 0.0);
    let a = tree(&app);
    assert_eq!(sideways(&a), 150.0);
    assert_eq!(header(&a, 2).unwrap().min_x(), 51.0);
    assert_eq!(cell(&a, 0, 2).unwrap().min_x(), 51.0);
    assert!(header(&a, 5).is_some() && cell(&a, 0, 5).is_some());
    // The rows stay the view's width (their fill and hover with them).
    let row = a
        .descendants()
        .into_iter()
        .find(|n| n.role == Role::Row && n.cell.is_some_and(|c| c.row == 1))
        .unwrap()
        .bounds;
    assert_eq!((row.min_x(), row.max_x()), (1.0, 387.0));
    assert!(header(&a, 7).is_none(), "not in view yet, not built");

    // Down, the header stays where it is; its columns still line up.
    wheel(&mut app, 0.0, ROW * 10.0);
    let a = tree(&app);
    assert_eq!(header(&a, 2).unwrap().min_y(), header_y);
    assert_eq!(cell(&a, 10, 2).unwrap().min_x(), 51.0);

    // To the end: the last column ends where the vertical bar starts.
    wheel(&mut app, 2_000.0, 0.0);
    let a = tree(&app);
    assert_eq!(sideways(&a), 800.0 - 386.0);
    assert!(header(&a, 0).is_none(), "scrolled out, not built");
    assert_eq!(header(&a, 7).unwrap().max_x() + 6.0, 387.0);
    assert_eq!(cell(&a, 10, 7).unwrap().max_x(), 387.0);
    // Cells say their place among all the columns, built or not.
    let numbered: Vec<_> = a
        .descendants()
        .into_iter()
        .filter(|n| n.role == Role::Cell && n.name.starts_with("r10c"))
        .map(|n| (n.name.clone(), n.cell.unwrap().column))
        .collect();
    assert!(numbered.len() < COLUMNS, "{numbered:?}");
    for (name, column) in numbered {
        assert_eq!(name, format!("r10c{column}"));
    }
    let t = a.descendants().into_iter().find_map(|n| n.table).unwrap();
    assert_eq!((t.rows, t.columns), (1_001, COLUMNS));
}

#[test]
fn frozen_columns_stay_put_and_the_rest_scroll_clear_of_them() {
    let mut app = app(1, false);
    wheel(&mut app, 250.0, 0.0);
    let a = tree(&app);
    assert_eq!(sideways(&a), 250.0);
    // The frozen column and its header stay at the start.
    assert_eq!(header(&a, 0).unwrap().min_x(), 1.0);
    assert_eq!(cell(&a, 0, 0).unwrap().min_x(), 1.0);
    // The others moved by the offset (column 3 starts 300 in).
    assert_eq!(header(&a, 3).unwrap().min_x(), 51.0);
    assert_eq!(cell(&a, 0, 3).unwrap().min_x(), 51.0);
    // A click on the frozen cell lands on it (the scrolled ones are
    // clipped clear of it) and selects its row.
    assert!(app.click_at(Point::new(60.0, 1.0 + ROW + 1.0 + ROW * 2.5)));
    assert!(app.state().table.rows.is_selected(2));
    let t = tree(&app)
        .descendants()
        .into_iter()
        .find_map(|n| n.table)
        .unwrap();
    assert_eq!(t.frozen, 1);
}

#[test]
fn the_cell_cursor_moves_across_and_scrolls_into_view() {
    let mut app = app(1, false);
    assert!(app.click_at(Point::new(60.0, 1.0 + ROW + 1.0 + ROW * 0.5)));
    // Right moves into the next column, keeping the row.
    for _ in 0..6 {
        assert!(app.press_key(KeyInput::Right));
    }
    app.render_once();
    assert_eq!(app.state().table.cursor_column(), Some(6));
    // Column 6 (600..700) shows whole at the end of the 386 px.
    let a = tree(&app);
    assert_eq!(sideways(&a), 700.0 - 386.0);
    assert_eq!(cell(&a, 0, 6).unwrap().max_x(), 387.0);
    assert_eq!(app.state().table.rows.cursor(), Some(0));
    // Back left: column 2 comes in beside the frozen one.
    for _ in 0..4 {
        assert!(app.press_key(KeyInput::Left));
    }
    app.render_once();
    let a = tree(&app);
    assert_eq!(sideways(&a), 100.0);
    assert_eq!(cell(&a, 0, 2).unwrap().min_x(), 101.0);
    // Ctrl+Right to the last column; End to the last row, its cell in view.
    assert!(app.press_key(KeyInput::WordRight));
    assert!(app.press_key(KeyInput::End));
    app.render_once();
    let a = tree(&app);
    assert_eq!(app.state().table.cursor_column(), Some(7));
    assert_eq!(app.state().table.rows.cursor(), Some(999));
    let last = cell(&a, 999, 7).expect("the last cell built");
    assert_eq!(last.max_x(), 387.0);
    // Above the sideways bar's gutter, at the body's bottom.
    assert!(
        last.max_y() <= 1.0 + ROW + 1.0 + 240.0 - 12.0 + 0.5,
        "{last:?}"
    );
}

#[test]
fn right_to_left_starts_at_the_right_with_the_frozen_columns_there() {
    let mut app = app(1, true);
    let a = tree(&app);
    // Column 0 is at the right edge, column 1 left of it.
    assert_eq!(header(&a, 0).unwrap().max_x(), 399.0);
    assert_eq!(cell(&a, 0, 0).unwrap().max_x(), 399.0);
    assert_eq!(cell(&a, 0, 1).unwrap().max_x(), 299.0);
    // A wheel scrolling left goes on into the table: the columns move
    // right, the frozen one doesn't.
    wheel(&mut app, -150.0, 0.0);
    let a = tree(&app);
    assert_eq!(sideways(&a), 150.0);
    assert_eq!(cell(&a, 0, 0).unwrap().max_x(), 399.0);
    assert_eq!(header(&a, 2).unwrap().max_x(), 349.0);
    assert_eq!(cell(&a, 0, 2).unwrap().max_x(), 349.0);
    // Left moves to the next column, right to left; the cursor's cell is
    // scrolled clear of the frozen column.
    assert!(app.click_at(Point::new(350.0, 1.0 + ROW + 1.0 + ROW * 0.5)));
    for _ in 0..2 {
        assert!(app.press_key(KeyInput::Left));
    }
    assert!(app.press_key(KeyInput::Right));
    app.render_once();
    assert_eq!(app.state().table.cursor_column(), Some(1));
    let a = tree(&app);
    assert_eq!(sideways(&a), 0.0);
    assert_eq!(cell(&a, 0, 1).unwrap().max_x(), 299.0);
}

#[test]
fn a_cell_off_to_the_side_is_brought_into_view_before_it_is_activated() {
    let mut app = app(1, false);
    // To the end, then activate column 4 (built, just left of the view).
    wheel(&mut app, 2_000.0, 0.0);
    let a = tree(&app);
    assert_eq!(sideways(&a), 800.0 - 386.0);
    let platform = crate::to_platform_a11y(&a);
    let path = path_to(&platform, &|n| n.name == "r3c4").expect("built");
    let events = platform.activate_events(&path).unwrap();
    for e in events {
        app.pointer_event(e);
    }
    app.render_once();
    let a = tree(&app);
    // Scrolled so it shows whole beside the frozen column (not under it),
    // and the click selected its row.
    assert_eq!(sideways(&a), 300.0);
    assert_eq!(cell(&a, 3, 4).unwrap().min_x(), 101.0);
    assert!(app.state().table.rows.is_selected(3));
}

fn path_to(
    n: &crate::platform::access::A11yNode,
    pred: &dyn Fn(&crate::platform::access::A11yNode) -> bool,
) -> Option<Vec<usize>> {
    if pred(n) {
        return Some(Vec::new());
    }
    n.children.iter().enumerate().find_map(|(i, c)| {
        path_to(c, pred).map(|mut p| {
            p.insert(0, i);
            p
        })
    })
}

#[test]
fn a_column_resizes_while_scrolled_sideways() {
    let mut app = app(1, false);
    wheel(&mut app, 150.0, 0.0);
    // Column 3 (300..400) ends at 1 + 400 - 150; its handle is the 6 px
    // left of that.
    let edge = 1.0 + 400.0 - 150.0 - 3.0;
    let y = 1.0 + ROW / 2.0;
    assert!(app.drag_at_point(Point::new(edge, y)));
    assert!(app.drag_at_point(Point::new(edge + 20.0, y)));
    assert!(app.drag_at_point(Point::new(edge + 40.0, y)));
    app.end_drag();
    app.render_once();
    let a = tree(&app);
    assert_eq!(sideways(&a), 150.0);
    assert_eq!(cell(&a, 0, 3).unwrap().width(), 140.0);
    assert_eq!(header(&a, 4).unwrap().min_x(), 1.0 + 440.0 - 150.0);
    assert_eq!(cell(&a, 0, 4).unwrap().min_x(), 1.0 + 440.0 - 150.0);
}

#[test]
fn a_table_its_columns_fit_keeps_their_width() {
    let mut app = App::new(
        Grid {
            table: TableState::new(SelectionMode::Single),
        },
        |s: &Grid, cx: &mut Cx<Grid>| {
            let cols = [Column::new("A", 100.0), Column::new("B", 100.0)];
            let grid = table(
                cx,
                &s.table,
                |s| &mut s.table,
                TableSpec::new(&cols, 1_000).row_height(ROW).height(240.0),
                |cx, row, col| crate::widgets::label(cx, format!("r{row}c{col}")),
            );
            crate::widgets::row(vec![grid])
                .align(crate::core::Align::Start, crate::core::Align::Start)
        },
    )
    .no_font()
    .theme(Theme::light().scroll_bar(ScrollBarLook::CLASSIC))
    .logical_size(Size::new(600.0, 320.0));
    app.render_once();
    app.render_once();
    let a = tree(&app);
    let table = a
        .descendants()
        .into_iter()
        .find(|n| n.role == Role::Table)
        .unwrap();
    // The columns, the frame's hairlines and the vertical bar: no sideways bar.
    assert_eq!(table.bounds.width(), 200.0 + 2.0 + 12.0);
    assert!(
        a.descendants()
            .iter()
            .all(|n| n.scroll_bar.is_none_or(|b| !b.horizontal))
    );
}
