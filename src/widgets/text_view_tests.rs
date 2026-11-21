//! `text_view` through the `App`: virtualized lines, following the end,
//! selecting across lines with the pointer, copying, find, and the cap on
//! lines.

use crate::App;
use crate::core::{AccessNode, Cx, KeyInput, Role, clipboard_text, set_clipboard_text};
use crate::geometry::{Point, Size};
use crate::render::Font;
use crate::widgets::{FindOptions, TextView, TextViewOptions, column, text_view};

const LH: f64 = 20.0;

struct Log {
    view: TextView,
}

fn log(lines: usize) -> TextView {
    let mut v = TextView::new();
    v.append((0..lines).map(|i| format!("line {i}")));
    v
}

/// A 400×300 window with a 200px-high text view of `view` at the top.
fn app(view: TextView, stick: bool, font: Option<Font>) -> App<Log> {
    let app = App::new(Log { view }, move |s: &Log, cx: &mut Cx<Log>| {
        let options = TextViewOptions::default()
            .height(200.0)
            .line_height(LH)
            .text_size(14.0)
            .stick_to_end(stick);
        column(vec![text_view(cx, &s.view, |s| &mut s.view, options)])
    })
    .logical_size(Size::new(400.0, 300.0));
    match font {
        Some(f) => app.font(f),
        None => app.no_font(),
    }
}

fn font() -> Option<Font> {
    let f = Font::system_default();
    if f.is_none() {
        eprintln!("skipping: no system font");
    }
    f
}

fn text_node(app: &App<Log>) -> AccessNode {
    let a = app.accessibility_tree().unwrap();
    a.descendants()
        .into_iter()
        .find(|n| n.role == Role::Text)
        .unwrap()
        .clone()
}

/// The lines a screen reader reads: those on screen.
fn shown(app: &App<Log>) -> Vec<String> {
    text_node(app).name.lines().map(String::from).collect()
}

fn update(app: &mut App<Log>, f: impl FnOnce(&mut Log) + Send + 'static) {
    app.handle().update(f);
    assert!(app.run_pending());
    app.render_once();
}

/// The middle of line `i` (on screen at offset 0), `x` from the left.
fn at_line(i: usize, x: f64) -> Point {
    Point::new(x, 1.0 + i as f64 * LH + LH / 2.0)
}

#[test]
fn ten_thousand_lines_lay_out_a_screenful_and_read_as_text() {
    let mut app = app(log(10_000), false, None);
    app.render_once();
    let node = text_node(&app);
    // 200px viewport (less the frame's hairline) of 20px lines.
    assert_eq!(shown(&app).first().map(String::as_str), Some("line 0"));
    assert_eq!(shown(&app).len(), 10);
    let scroll = node.scroll.unwrap();
    assert!(
        (scroll.max - (10_000.0 * LH - 198.0)).abs() < 2.0,
        "{scroll:?}"
    );
    // Only the lines around the viewport were built.
    assert!(node.children.is_empty());
    fn leaves(n: &crate::core::LayoutNode) -> usize {
        let own = matches!(n.content, crate::core::NodeContent::Text { .. }) as usize;
        own + n.children.iter().map(leaves).sum::<usize>()
    }
    let built = leaves(app.panes[0].tree.as_ref().unwrap());
    assert!(built < 20, "{built} lines built");

    // The wheel scrolls into lines never built before.
    app.hover_at(Point::new(100.0, 100.0));
    assert!(app.scroll_by(LH * 5_000.0));
    app.render_once();
    assert_eq!(shown(&app).first().map(String::as_str), Some("line 5000"));
}

#[test]
fn it_follows_appended_lines_until_scrolled_up() {
    let mut app = app(log(100), true, None);
    app.render_once();
    assert_eq!(shown(&app).last().map(String::as_str), Some("line 99"));
    update(&mut app, |s| s.view.append_text("line 100\nline 101\n"));
    assert_eq!(shown(&app).last().map(String::as_str), Some("line 101"));

    app.hover_at(Point::new(100.0, 100.0));
    assert!(app.scroll_by(-LH * 20.0));
    app.render_once();
    let before = shown(&app);
    update(&mut app, |s| s.view.push_line("line 102"));
    assert_eq!(shown(&app), before);
    // End goes back to the end, which it follows again.
    update(&mut app, |s| s.view.scroll_to_end());
    update(&mut app, |s| s.view.push_line("line 103"));
    assert_eq!(shown(&app).last().map(String::as_str), Some("line 103"));
}

#[test]
fn a_capped_log_keeps_what_is_on_screen_as_lines_drop() {
    let mut app = app(log(100).max_lines(100), false, None);
    app.render_once();
    app.hover_at(Point::new(100.0, 100.0));
    assert!(app.scroll_by(LH * 50.0));
    app.render_once();
    let before = shown(&app);
    assert_eq!(before.first().map(String::as_str), Some("line 50"));
    update(&mut app, |s| {
        s.view.append((100..110).map(|i| format!("line {i}")))
    });
    assert_eq!(app.state().view.line(0), Some("line 10"));
    assert_eq!(shown(&app), before);
}

#[test]
fn find_next_scrolls_to_the_match() {
    let mut view = log(5_000);
    view.push_line("the needle");
    view.append((0..5_000).map(|i| format!("more {i}")));
    let mut app = app(view, false, None);
    app.render_once();
    update(&mut app, |s| {
        assert_eq!(s.view.find("NEEDLE", FindOptions::default()), 1);
        s.view.find_next();
    });
    assert!(
        shown(&app).contains(&"the needle".to_string()),
        "{:?}",
        shown(&app)
    );
}

#[test]
fn dragging_selects_across_lines_and_copy_takes_them() {
    let Some(font) = font() else { return };
    let mut app = app(log(1_000), false, Some(font));
    app.render_once();
    // From the start of line 2 to well past the end of line 4.
    assert!(app.text_press_at(at_line(2, 4.0)));
    assert!(app.text_drag_at(at_line(4, 390.0)));
    app.end_text_select();
    app.render_once();
    assert_eq!(
        app.state().view.selected_text().as_deref(),
        Some("line 2\nline 3\nline 4")
    );
    // The view took the focus: Ctrl/Cmd+C copies.
    let node = text_node(&app);
    assert!(node.focused);
    set_clipboard_text("");
    assert!(app.press_key(KeyInput::Copy));
    assert_eq!(clipboard_text(), "line 2\nline 3\nline 4");

    // Dragging below the view selects into lines scrolled in.
    assert!(app.text_press_at(at_line(8, 4.0)));
    for k in 1..=5 {
        assert!(app.text_drag_at(Point::new(50.0, 230.0 + k as f64)));
        app.render_once();
    }
    app.end_text_select();
    let sel = app.state().view.selection().unwrap();
    assert_eq!(sel.0.line, 8);
    assert!(sel.1.line >= 11, "{sel:?}");
    assert!(shown(&app).contains(&format!("line {}", sel.1.line)));

    // Select all, then Escape deselects.
    assert!(app.press_key(KeyInput::SelectAll));
    assert_eq!(
        app.state().view.selected_text().unwrap().lines().count(),
        1_000
    );
    assert!(app.press_key(KeyInput::Escape));
    assert_eq!(app.state().view.selection(), None);
}

#[test]
fn matches_and_the_selection_are_painted_behind_the_text() {
    let Some(font) = font() else { return };
    let mut app = app(log(50), false, Some(font));
    let theme = crate::style::Theme::light();
    let plain = app.render_once();
    // In line 3's text, at the top of its line, above the glyphs: the
    // surface.
    // A system font's tall glyphs ("l") can reach the top of the line, so
    // take, within the match, the column where the found frame is brightest:
    // background there, not glyph, in every frame alike.
    let x0 = (1.0 + theme.spacing.sm + 1.0) as u32;
    let y = (1.0 + 3.0 * LH + 2.0) as u32;
    update(&mut app, |s| {
        s.view
            .find("line 3", FindOptions::default().case_sensitive(true));
        s.view.find_next();
    });
    let frame = app.render_once();
    let x = (x0..x0 + 30)
        .max_by_key(|&x| {
            let p = frame.pixel(x, y).unwrap();
            p[0] as u32 + p[1] as u32 + p[2] as u32
        })
        .unwrap();
    let surface = plain.pixel(x, y).unwrap();
    let found = frame.pixel(x, y).unwrap();
    assert_ne!(found, surface);
    // The current match, on the warning color (an amber).
    assert!(found[0] > found[2] + 100, "{found:?}");

    update(&mut app, |s| {
        s.view.clear_find();
        s.view.select_line(3);
    });
    let selected = app.render_once().pixel(x, y).unwrap();
    assert_ne!(selected, surface);
    assert_ne!(selected, found);
}

#[test]
fn regex_find_goes_to_matches_and_reports_a_bad_pattern() {
    let mut view = log(3_000);
    view.push_line("GET /index.html 200 12ms");
    view.append((0..3_000).map(|i| format!("more {i}")));
    view.push_line("get /api/users 404 7ms");
    let mut app = app(view, false, None);
    app.render_once();
    update(&mut app, |s| {
        let options = FindOptions::default().regex(true);
        // Case-insensitive by default, anchored within a line.
        assert_eq!(s.view.find(r"^get /\S+ [45]\d\d", options), 1);
        assert_eq!(s.view.matches()[0].line, 6_001);
        assert_eq!(s.view.matches()[0].range, 0..18);
        assert_eq!(s.view.find(r"^get\s+\S+", options), 2);
        assert_eq!(s.view.find(r"^GET", options.case_sensitive(true)), 1);
        // The first match is scrolled to.
        assert_eq!(s.view.find(r"\d+ms$", options), 2);
        s.view.find_next();
    });
    assert!(
        shown(&app).contains(&"GET /index.html 200 12ms".to_string()),
        "{:?}",
        shown(&app)
    );
    // Lines appended later are searched too.
    update(&mut app, |s| s.view.push_line("POST /x 201 3ms"));
    assert_eq!(app.state().view.matches().len(), 3);
    // A bad pattern finds nothing and says why; a good one clears it.
    update(&mut app, |s| {
        let options = FindOptions::default().regex(true);
        assert_eq!(s.view.find("(ms", options), 0);
        assert!(s.view.matches().is_empty());
        assert_eq!(s.view.find_error().map(|e| e.offset), Some(0));
        assert_eq!(
            s.view.try_find("a{2,1}", options).unwrap_err().message,
            "a repetition's {min,max} has max below min"
        );
        assert_eq!(s.view.try_find("ms", options), Ok(3));
        assert_eq!(s.view.find_error(), None);
        // Not a regex: the parenthesis is a parenthesis.
        assert_eq!(s.view.find("(ms", FindOptions::default()), 0);
        assert_eq!(s.view.find_error(), None);
    });
}
