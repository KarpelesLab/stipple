//! `Element::aspect_ratio` through the `App`: a picture as wide as its
//! column and square, thumbnails sharing a row or filling grid cells, and a
//! fixed height deciding the width.

use crate::App;
use crate::core::{BoxStyle, Cx, Element, Role};
use crate::geometry::{Insets, Rect, Size};
use crate::layout::Track;
use crate::render::Color;
use crate::widgets::{column, row};

/// A named box keeping `ratio`.
fn picture(name: &str, ratio: f64) -> Element {
    Element::boxed(BoxStyle::filled(Color::rgb(0, 0, 0)))
        .role(Role::Group)
        .a11y_label(name)
        .aspect_ratio(ratio)
}

fn bounds_of<S: 'static>(app: &mut App<S>, name: &str) -> Rect {
    app.render_once();
    let tree = app.accessibility_tree().expect("a frame");
    tree.descendants()
        .into_iter()
        .find(|n| n.name == name)
        .unwrap_or_else(|| panic!("no {name:?}"))
        .bounds
}

fn app_of(view: impl Fn(&(), &mut Cx<()>) -> Element + 'static, size: Size) -> App<()> {
    App::new((), view).no_font().logical_size(size)
}

#[test]
fn a_square_is_as_wide_as_its_column() {
    let view = |_: &(), _: &mut Cx<()>| {
        column(vec![
            picture("qr", 1.0),
            Element::boxed(BoxStyle::default()).height(20.0),
        ])
        .padding(Insets::uniform(10.0))
    };
    let mut app = app_of(view, Size::new(300.0, 600.0));
    let qr = bounds_of(&mut app, "qr");
    assert_eq!((qr.min_x(), qr.min_y()), (10.0, 10.0));
    assert_eq!((qr.width(), qr.height()), (280.0, 280.0));
    // Narrower window, smaller square: no window size in the view.
    let mut app = app_of(view, Size::new(200.0, 600.0));
    let qr = bounds_of(&mut app, "qr");
    assert_eq!((qr.width(), qr.height()), (180.0, 180.0));
}

#[test]
fn widescreen_with_a_max_width_stays_sixteen_by_nine() {
    let mut app = app_of(
        |_, _| column(vec![picture("video", 16.0 / 9.0).max_width(320.0)]),
        Size::new(800.0, 600.0),
    );
    let v = bounds_of(&mut app, "video");
    assert_eq!(v.width(), 320.0);
    assert_eq!(v.height(), 180.0);
}

#[test]
fn a_fixed_width_or_height_decides_the_other_side() {
    let mut app = app_of(
        |_, _| {
            column(vec![
                picture("wide", 2.0).width(100.0),
                row(vec![picture("tall", 0.5).height(60.0)]),
            ])
        },
        Size::new(400.0, 400.0),
    );
    let wide = bounds_of(&mut app, "wide");
    assert_eq!((wide.width(), wide.height()), (100.0, 50.0));
    let tall = bounds_of(&mut app, "tall");
    assert_eq!((tall.width(), tall.height()), (30.0, 60.0));
}

#[test]
fn thumbnails_share_a_row_and_their_height_follows() {
    let mut app = app_of(
        |_, _| {
            column(vec![
                row(vec![
                    picture("a", 1.0),
                    Element::boxed(BoxStyle::default()).width(40.0),
                    picture("b", 1.0),
                ])
                .gap(10.0),
                Element::boxed(BoxStyle::default())
                    .role(Role::Group)
                    .a11y_label("below")
                    .height(10.0),
            ])
        },
        Size::new(300.0, 400.0),
    );
    // (300 - 40 - 2 × 10) / 2 = 120 each.
    let a = bounds_of(&mut app, "a");
    let b = bounds_of(&mut app, "b");
    assert_eq!((a.width(), a.height()), (120.0, 120.0));
    assert_eq!((b.min_x(), b.width(), b.height()), (180.0, 120.0, 120.0));
    // The row is as tall as its squares: what comes next sits under them.
    let below = bounds_of(&mut app, "below");
    assert_eq!(below.min_y(), 120.0);
}

#[test]
fn grid_cells_keep_the_ratio_of_their_column_width() {
    let mut app = app_of(
        |_, _| {
            Element::grid(
                vec![Track::fr(1.0), Track::fr(1.0), Track::fr(1.0)],
                (0..6)
                    .map(|i| picture(&format!("t{i}"), 4.0 / 3.0))
                    .collect(),
            )
            .column_gap(0.0)
            .row_gap(0.0)
        },
        Size::new(300.0, 400.0),
    );
    let t0 = bounds_of(&mut app, "t0");
    assert_eq!((t0.width(), t0.height()), (100.0, 75.0));
    let t4 = bounds_of(&mut app, "t4");
    assert_eq!((t4.min_x(), t4.min_y()), (100.0, 75.0));
}

#[test]
fn a_bad_ratio_is_ignored() {
    let mut app = app_of(
        |_, _| column(vec![picture("x", 0.0).height(30.0), picture("y", f64::NAN)]),
        Size::new(100.0, 100.0),
    );
    assert_eq!(bounds_of(&mut app, "x").height(), 30.0);
    assert_eq!(bounds_of(&mut app, "y").height(), 0.0);
}
