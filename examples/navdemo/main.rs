//! A phone app's pages and sheets: a history of scans that drills into a
//! scan's details (`nav_stack`, each page with its `app_bar`), a sheet with a
//! scan's result (`bottom_sheet` on its content's height, draggable up to the
//! full height) and one with filters (on half the window).
//!
//! Back, a gamepad's B, iOS's swipe from the left edge and Android's
//! predictive back gesture pop the details page, following the finger; with
//! a sheet up they close the sheet first, as Escape and a tap on the scrim
//! do. Drag a sheet by its handle (or, on a touch screen, by its list pulled
//! down at its top) to resize it or throw it away. The chips on the first
//! page switch between the platforms' page transitions.
//!
//! ```sh
//! cargo run --example navdemo            # or: dark, material, native
//! ```

use stipple::prelude::*;
use stipple::widgets::{
    BottomSheet, NavStack, PageTransition, SheetDetent, app_bar, bottom_sheet, nav_stack,
};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Page {
    History,
    Details(usize),
}

struct State {
    nav: NavStack<Page>,
    /// The sheet showing a scan's result, and which scan.
    result: BottomSheet,
    shown: usize,
    filters: BottomSheet,
    only_links: bool,
    only_today: bool,
    /// The chip picked: the theme's transition, or one of the three.
    look: usize,
}

const SCANS: [(&str, &str); 12] = [
    ("https://example.com/menu", "Link"),
    ("WIFI:S:Cafe;T:WPA;P:espresso;;", "Wi-Fi"),
    ("Hello from Stipple", "Text"),
    ("+1 555 0100", "Phone"),
    ("https://example.org/ticket/42", "Link"),
    ("BEGIN:VCARD…", "Contact"),
    ("geo:48.8566,2.3522", "Location"),
    ("mailto:hello@example.com", "Email"),
    ("978-3-16-148410-0", "ISBN"),
    ("https://example.net/coupon", "Link"),
    ("Order #10293", "Text"),
    ("SMSTO:555 0100:Hi", "SMS"),
];

const LOOKS: [&str; 4] = ["Theme's", "Slide (iOS)", "Shared axis", "Fade"];

fn history(state: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    let filter = Button::new("Filters")
        .variant(Variant::Ghost)
        .view(cx)
        .on_tap(cx, |s| s.filters.open());
    let bar = app_bar(cx, "History").actions(vec![filter]).view(cx);
    let looks = choice_chips(cx, LOOKS, state.look, |s, i| {
        s.look = i;
        s.nav.set_transition(match i {
            1 => Some(PageTransition::Slide),
            2 => Some(PageTransition::SharedAxis),
            3 => Some(PageTransition::Fade),
            _ => None,
        });
    });
    let rows = SCANS
        .iter()
        .enumerate()
        .filter(|(_, (_, kind))| !state.only_links || *kind == "Link")
        .map(|(i, (text, kind))| {
            row(vec![
                column(vec![label(cx, *text), label(cx, *kind)]).grow(1.0),
                label(cx, "›"),
            ])
            .padding(Insets::symmetric(theme.spacing.lg, theme.spacing.md))
            .align(Align::Center, Align::Center)
            .on_tap(cx, move |s| s.nav.push(Page::Details(i)))
            .role(Role::Button)
            .a11y_label(*text)
        })
        .collect();
    column(vec![
        bar,
        looks.padding(Insets::symmetric(theme.spacing.lg, theme.spacing.sm)),
        scroll(cx).children(rows).grow(1.0),
    ])
    .grow(1.0)
}

fn details(cx: &mut Cx<State>, i: usize) -> Element {
    let theme = *cx.theme();
    let (text, kind) = SCANS[i];
    let bar = app_bar(cx, kind)
        .back_label("History")
        .on_back(|s| {
            s.nav.pop();
        })
        .view(cx);
    let lines = (1..=20)
        .map(|n| label(cx, format!("Scanned {n} day(s) ago")))
        .collect();
    column(vec![
        bar,
        column(vec![
            heading(cx, text),
            label(cx, format!("{kind} · scan #{}", i + 1)),
            button(cx, "Show result").on_tap(cx, move |s| {
                s.shown = i;
                s.result.open();
            }),
        ])
        .gap(theme.spacing.md)
        .padding(Insets::uniform(theme.spacing.lg)),
        scroll(cx)
            .child(column(lines).gap(theme.spacing.sm))
            .padding(Insets::symmetric(theme.spacing.lg, 0.0))
            .grow(1.0),
    ])
    .grow(1.0)
}

fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let pages = nav_stack(
        cx,
        &state.nav,
        |s| &mut s.nav,
        |cx, page| match *page {
            Page::History => history(state, cx),
            Page::Details(i) => details(cx, i),
        },
    );
    // Declared after the stack, so Back closes them before it pops.
    let (text, kind) = SCANS[state.shown];
    bottom_sheet(cx, &state.result, |s| &mut s.result)
        .id("result")
        .title(kind)
        .detents(vec![SheetDetent::Fit, SheetDetent::Full])
        .children(vec![
            paragraph(cx, text),
            row(vec![
                button(cx, "Open").on_tap(cx, |s| s.result.dismiss()),
                Button::new("Copy")
                    .variant(Variant::Secondary)
                    .view(cx)
                    .on_tap(cx, |s| s.result.dismiss()),
                Button::new("Share")
                    .variant(Variant::Secondary)
                    .view(cx)
                    .on_tap(cx, |s| s.result.dismiss()),
            ])
            .gap(8.0),
        ])
        .open(cx);
    bottom_sheet(cx, &state.filters, |s| &mut s.filters)
        .id("filters")
        .title("Filters")
        .detents(vec![SheetDetent::Half, SheetDetent::Full])
        .children(vec![
            switch(cx, "Links only", state.only_links, |s| {
                s.only_links = !s.only_links
            }),
            switch(cx, "Today only", state.only_today, |s| {
                s.only_today = !s.only_today
            }),
            label(cx, "Drag the handle up for more room."),
        ])
        .open(cx);
    pages
}

fn main() {
    let theme = match std::env::args().nth(1).as_deref() {
        Some("dark") => Theme::dark(),
        Some("material") => Theme::material3_light(),
        Some("native") => Theme::native(),
        _ => Theme::light(),
    };
    let state = State {
        nav: NavStack::new(Page::History),
        result: BottomSheet::new(),
        shown: 0,
        filters: BottomSheet::new(),
        only_links: false,
        only_today: false,
        look: 0,
    };
    App::new(state, view)
        .title("Stipple Navigation")
        .theme(theme)
        .logical_size(Size::new(360.0, 640.0))
        .run();
}

// In the browser, `web.rs` builds this file as a library and the page
// runs `main`'s app (on the web, `App::run` hands it to the page).
stipple::web_app!(main);
