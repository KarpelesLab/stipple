//! The focus ring shows for the keyboard, not after a tap or a click (CSS's
//! `:focus-visible`), except on a text field, and it is drawn with the layer
//! its element is in: under a sheet or a popup over it, never on top.

use crate::App;
use crate::core::{Cx, Element, KeyInput, Role};
use crate::geometry::{Rect, Size};
use crate::render::{Color, Pixmap};
use crate::style::Theme;
use crate::widgets::{
    BottomSheet, NavIcon, NavItem, NavigationBar, TextInput, bottom_sheet, button, column, spacer,
    text_field,
};

const RING: Color = Color::rgb(255, 0, 255);
const W: f64 = 360.0;
const H: f64 = 640.0;

const ICON: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"><path d="M3 3h18v18H3z" fill="currentColor"/></svg>"#;

fn theme() -> Theme {
    let mut theme = Theme::light();
    theme.palette.focus_ring = RING;
    theme
}

/// How many pixels along `rect`'s edges (where a ring around it is drawn)
/// are the ring's color.
fn ring_pixels(frame: &Pixmap, rect: Rect) -> usize {
    let near = |p: Option<[u8; 4]>| {
        p.is_some_and(|p| {
            [RING.r, RING.g, RING.b]
                .iter()
                .zip(p)
                .all(|(a, b)| a.abs_diff(b) < 40)
        })
    };
    let mut n = 0;
    let (x0, x1) = (rect.min_x() as i64 - 3, rect.max_x() as i64 + 3);
    let (y0, y1) = (rect.min_y() as i64 - 3, rect.max_y() as i64 + 3);
    for x in x0..x1 {
        for y in (y0..y0 + 6).chain(y1 - 6..y1) {
            if x >= 0 && y >= 0 && near(frame.pixel(x as u32, y as u32)) {
                n += 1;
            }
        }
    }
    n
}

#[derive(Default)]
struct Phone {
    tab: usize,
    sheet: BottomSheet,
    name: TextInput,
}

fn view(s: &Phone, cx: &mut Cx<Phone>) -> Element {
    bottom_sheet(cx, &s.sheet, |s| &mut s.sheet)
        .child(button(cx, "Close").on_tap(cx, |s| s.sheet.dismiss()))
        .open(cx);
    let items = ["Scan", "History", "Create"]
        .map(|label| NavItem::new(label, NavIcon::template(ICON)))
        .to_vec();
    let bar = NavigationBar::new(items, s.tab, |s: &mut Phone, i| {
        s.tab = i;
        if i == 2 {
            s.sheet.open();
        }
    });
    column(vec![
        text_field(cx, &s.name, |s| &mut s.name),
        spacer(),
        bar.view(cx).a11y_label("Pages"),
    ])
    .grow(1.0)
}

fn app() -> App<Phone> {
    crate::anim::set_reduced_motion(Some(true));
    App::new(Phone::default(), view)
        .theme(theme())
        .no_font()
        .logical_size(Size::new(W, H))
}

/// The navigation bar's box: its focusable element.
fn bar(app: &mut App<Phone>) -> Rect {
    app.render_once();
    let tree = app.panes[0].tree.as_ref().unwrap();
    let mut found = None;
    fn find(n: &crate::core::LayoutNode, out: &mut Option<Rect>) {
        if n.focus.is_some() && n.text_pos.is_none() && n.bounds.min_y() > 300.0 {
            *out = Some(n.bounds);
        }
        for c in &n.children {
            find(c, out);
        }
    }
    find(tree, &mut found);
    found.expect("the bar takes focus")
}

#[test]
fn a_tap_on_the_navigation_bar_leaves_no_ring_and_the_keys_bring_it() {
    let mut app = app();
    let bar = bar(&mut app);
    // Tap "History", the middle third.
    let at = crate::geometry::Point::new(W / 2.0, bar.center().y);
    assert!(app.click_at(at));
    assert_eq!(app.state().tab, 1);
    assert!(app.focused().is_some(), "the tap focused the bar");
    let (frame, _) = app.render_frame();
    assert_eq!(ring_pixels(&frame, bar), 0, "no ring after a tap");
    // The arrow keys move the selection, and show where the focus is.
    assert!(app.press_key(KeyInput::Left));
    assert_eq!(app.state().tab, 0);
    let (frame, _) = app.render_frame();
    assert!(ring_pixels(&frame, bar) > 20, "the ring, from the keyboard");
    // A tap hides it again.
    app.click_at(at);
    let (frame, _) = app.render_frame();
    assert_eq!(ring_pixels(&frame, bar), 0);
}

#[test]
fn the_ring_is_drawn_under_a_sheet_over_its_element() {
    let mut app = app();
    let bar = bar(&mut app);
    // Focus the bar with the keyboard: the ring shows.
    app.focus_next();
    app.focus_next();
    let (frame, _) = app.render_frame();
    assert!(ring_pixels(&frame, bar) > 20, "focused from the keyboard");
    // Enter on "Create" opens a sheet over the bar: the ring stays under
    // it (focus moved into the sheet, or the ring is covered).
    app.press_key(KeyInput::Right);
    app.press_key(KeyInput::Right);
    assert!(app.state().sheet.is_open());
    let (frame, _) = app.render_frame();
    assert_eq!(ring_pixels(&frame, bar), 0, "no ring drawn over the sheet");
}

#[test]
fn a_clicked_text_field_shows_its_ring() {
    let mut app = app();
    app.render_once();
    let field = app.find(Role::TextField, "").unwrap().bounds;
    app.click_at(field.center());
    assert!(app.focused().is_some());
    let (frame, _) = app.render_frame();
    assert!(
        ring_pixels(&frame, field) > 20,
        "a field shows where typing goes"
    );
}
