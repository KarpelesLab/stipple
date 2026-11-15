//! A tab strip that switches content, plus a right-click context menu — showing
//! the `tabs` widget and the `on_context` (secondary-click) handler driving an
//! overlay menu at the click position.

use stipple::prelude::*;

#[derive(Default)]
struct State {
    /// Which tab is selected.
    tab: usize,
    /// Where the context menu is open (logical px), or `None` when closed.
    menu_at: Option<Point>,
}

const TABS: [&str; 3] = ["Profile", "Settings", "About"];

/// The body for the selected tab: a heading, a big color swatch (so the tab
/// switch is obvious in a screenshot), and a hint. Right-clicking anywhere on it
/// opens the context menu.
fn content(state: &State, theme: &Theme, cx: &mut Cx<State>) -> Element {
    let (title, color) = match state.tab {
        0 => ("Profile", Color::rgb(0xe0, 0x6c, 0x4f)),
        1 => ("Settings", Color::rgb(0x4f, 0x9d, 0xe0)),
        _ => ("About", Color::rgb(0x66, 0xbb, 0x6a)),
    };
    let swatch = Element::boxed(BoxStyle::filled(color).radius(theme.radius)).height(180.0);
    column(vec![
        heading(cx, title),
        swatch,
        label(cx, "Right-click anywhere here for a menu"),
    ])
    .gap(theme.spacing.md)
    .grow(1.0)
    // Secondary click anywhere over the body opens the context menu there.
    .on_context(cx, |s, at: Point| s.menu_at = Some(at))
}

fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();

    let bar = tabs(cx, &TABS, state.tab, |s, i| {
        s.tab = i;
        s.menu_at = None;
    });
    let body = content(state, &theme, cx);

    // When open, declare the context menu as an overlay anchored at the click.
    // Picking an item, or pressing outside, closes it.
    if let Some(at) = state.menu_at {
        let menu = Menu::new("Go")
            .item(MenuItem::new("Go to Profile", |s: &mut State| s.tab = 0))
            .item(MenuItem::new("Go to About", |s: &mut State| s.tab = 2))
            .item(MenuItem::new("Dismiss", |_: &mut State| {}));
        open_menu(cx, at, menu, |s| s.menu_at = None);
    }

    panel(cx).children(vec![bar, body])
}

fn main() {
    let app = App::new(State::default(), view)
        .title("Stipple Tabs")
        .theme(Theme::dark())
        .logical_size(Size::new(360.0, 440.0));
    app.run();
}

// In the browser, `src/web.rs` builds this file as a library and the page
// runs `main`'s app (on the web, `App::run` hands it to the page).
stipple::web_app!(main);
