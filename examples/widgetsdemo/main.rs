//! Select, number input, chips, split pane, toolbar and toasts in one window.
//!
//! A toolbar runs along the top; below it a split pane holds a settings form
//! (a select, two number inputs, a group of choice chips and one of filter
//! chips, which the arrow keys move through) on the left and, on the right, a second,
//! vertical split with a preview above an event log. Drag either divider, or
//! focus one (Tab) and use the arrow keys. "Notify" raises a toast that
//! dismisses itself after four seconds; the welcome toast stays until its ×
//! (or a swipe sideways). The trash button clears the log, with an "Undo" on
//! the toast it raises. Narrow the window below 600 px and the toasts become
//! a snackbar across the bottom. A navigation bar along the bottom switches
//! between that editor, a long inbox and an about page; each keeps its state
//! (the inbox its scroll offset) while another shows.
//!
//! Pick the theme with an argument: `light` (default), `dark`, `material`,
//! `material-dark`, `contrast` or `native`.
//!
//! CI (X11/Xvfb) screenshots it, then clicks the select open and checks the
//! list appeared.

use std::sync::Arc;

use stipple::prelude::*;
use stipple::widgets::{NumberInput, Select, SplitPane, ToastKind, Toasts};

const W: f64 = 720.0;
const H: f64 = 480.0;

/// Navigation icons, drawn in `currentColor` so the bar tints them.
const EDIT: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"><path d="M4 17.5V20h2.5L17.8 8.7l-2.5-2.5zM20.7 5.8l-2.5-2.5a1 1 0 0 0-1.4 0l-1.3 1.3 3.9 3.9 1.3-1.3a1 1 0 0 0 0-1.4z" fill="currentColor"/></svg>"#;
const INBOX: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"><path d="M5 4h14l2 9v7H3v-7zm1.6 2-1.4 7H9a3 3 0 0 0 6 0h3.8l-1.4-7z" fill="currentColor"/></svg>"#;
const INFO: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"><path d="M12 2a10 10 0 1 1 0 20 10 10 0 0 1 0-20zm-1 8v7h2v-7zm0-4v2h2V6z" fill="currentColor"/></svg>"#;

const SIZES: [&str; 5] = ["Small", "Medium", "Large", "Extra large", "Huge"];
const FORMATS: [&str; 3] = ["PNG", "SVG", "PDF"];
const EXTRAS: [&str; 3] = ["Grid", "Margins", "Labels"];

const ICON: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 18 18">
<path d="M3 5h12M7 5V3h4v2M5 5l1 10h6l1-10" fill="none" stroke="#8a8f98" stroke-width="1.6" stroke-linejoin="round"/>
</svg>"##;

struct State {
    size: Select,
    width: NumberInput,
    opacity: NumberInput,
    sidebar: SplitPane,
    preview: SplitPane,
    toasts: Toasts,
    bold: bool,
    italic: bool,
    format: usize,
    extras: [bool; 3],
    log: Vec<String>,
    trash: Arc<Svg>,
    /// The destination the navigation bar shows.
    page: usize,
    /// Inbox messages not opened yet.
    unread: u32,
}

impl State {
    fn new() -> Self {
        let mut toasts = Toasts::new();
        toasts.push("Welcome! Drag the dividers to resize.", ToastKind::Info);
        Self {
            size: Select::new(Some(1)),
            width: NumberInput::new(640.0).range(100.0, 4000.0).step(10.0),
            opacity: NumberInput::new(0.8).range(0.0, 1.0).step(0.05),
            sidebar: SplitPane::horizontal(0.42).min_sizes(220.0, 160.0),
            preview: SplitPane::vertical(0.55).min_sizes(60.0, 60.0),
            toasts,
            bold: true,
            italic: false,
            format: 0,
            extras: [true, false, false],
            log: vec!["Ready.".into()],
            trash: Arc::new(Svg::parse(ICON).expect("the bundled icon parses")),
            page: 0,
            unread: 3,
        }
    }

    fn note(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
        if self.log.len() > 6 {
            self.log.remove(0);
        }
    }
}

fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    let sp = theme.spacing;
    let handle = cx.handle();
    let undo_handle = handle.clone();

    let bar = toolbar(cx).children(vec![
        tool_button(cx, "New").on_tap(cx, |s| s.note("New document")),
        tool_button(cx, "Open").on_tap(cx, |s| s.note("Open…")),
        tool_button(cx, "Save").on_tap(cx, |s| s.note("Saved")),
        tool_separator(cx),
        ToolButton::new("B")
            .selected(state.bold)
            .view(cx)
            .on_tap(cx, |s| s.bold = !s.bold),
        ToolButton::new("I")
            .selected(state.italic)
            .view(cx)
            .on_tap(cx, |s| s.italic = !s.italic),
        tool_separator(cx),
        ToolButton::from_icon(&state.trash, "Clear log")
            .view(cx)
            .on_tap(cx, move |s| {
                let cleared = std::mem::take(&mut s.log);
                let id = s.toasts.push("Log cleared", ToastKind::Info);
                s.toasts.dismiss_after(
                    id,
                    Duration::from_secs(6),
                    &undo_handle,
                    |s: &mut State| &mut s.toasts,
                );
                s.toasts.action(id, "Undo", move |s: &mut State| {
                    s.log.clone_from(&cleared);
                });
            }),
        spacer(),
        tool_button(cx, "Notify").on_tap(cx, move |s| {
            let n = s.log.len();
            let id = s
                .toasts
                .push(format!("Notification #{n}"), ToastKind::Success);
            s.toasts
                .dismiss_after(id, Duration::from_secs(4), &handle, |s| &mut s.toasts);
        }),
    ]);

    let form = grid(
        vec![Track::px(84.0), Track::fr(1.0)],
        vec![
            heading(cx, "Settings").span(2, 1),
            label(cx, "Size"),
            select(cx, &SIZES, &state.size, |s| &mut s.size).width(180.0),
            label(cx, "Width"),
            number_input(cx, &state.width, |s| &mut s.width),
            label(cx, "Opacity"),
            number_input(cx, &state.opacity, |s| &mut s.opacity),
            label(cx, "Format"),
            choice_chips(cx, FORMATS, state.format, |s, i| {
                s.format = i;
            }),
            label(cx, "Show"),
            filter_chips(cx, EXTRAS, &state.extras, |s, i| {
                s.extras[i] = !s.extras[i];
            }),
        ],
    )
    .gap(sp.md)
    .align(Align::Start, Align::Center)
    .padding(Insets::uniform(sp.lg));

    let size = state.size.selected().map_or("none", |i| SIZES[i]);
    let style = match (state.bold, state.italic) {
        (true, true) => "bold italic",
        (true, false) => "bold",
        (false, true) => "italic",
        (false, false) => "regular",
    };
    let extras: Vec<&str> = EXTRAS
        .iter()
        .zip(state.extras)
        .filter_map(|(name, on)| on.then_some(*name))
        .collect();
    let extras = if extras.is_empty() {
        "nothing else".to_string()
    } else {
        extras.join(", ").to_lowercase()
    };
    let preview = column(vec![
        heading(cx, "Preview"),
        paragraph(
            cx,
            format!(
                "{size}, {} px wide at {:.0}% opacity, {style}, as {} with {extras}.",
                state.width.value(),
                state.opacity.value() * 100.0,
                FORMATS[state.format],
            ),
        ),
    ])
    .gap(sp.sm)
    .padding(Insets::uniform(sp.lg));
    let log = column(
        std::iter::once(heading(cx, "Log"))
            .chain(state.log.iter().map(|l| label(cx, l.as_str())))
            .collect(),
    )
    .gap(sp.xs)
    .padding(Insets::uniform(sp.lg));
    let right = split_pane(cx, &state.preview, |s| &mut s.preview).panes(preview, log);

    let body = split_pane(cx, &state.sidebar, |s| &mut s.sidebar).panes(form, right);
    let body = row(vec![body])
        .fill(theme.palette.surface)
        .border(theme.palette.border, 1.0)
        .radius(theme.radius)
        .align(Align::Start, Align::Stretch)
        .clip()
        .grow(1.0);

    toasts(cx, &state.toasts, |s| &mut s.toasts);

    // The editor, the inbox and the about page, each kept as it was while
    // another shows.
    let mut editor = Some(body);
    let pages = page_stack(cx, 3, state.page, |cx, i, _visible| match i {
        0 => editor.take().unwrap_or_else(spacer),
        1 => {
            let rows = (1..=60)
                .map(|n| label(cx, format!("Message {n}")).padding(Insets::uniform(sp.sm)))
                .collect();
            let list = Element::stack(Axis::Vertical, rows);
            let id = cx.register_scroll();
            column(vec![list])
                .scrollable(id)
                .fill(theme.palette.surface)
                .radius(theme.radius)
                .grow(1.0)
        }
        _ => column(vec![
            heading(cx, "Stipple widgets"),
            paragraph(cx, "Self-drawn widgets over hand-written OS backends."),
        ])
        .gap(sp.sm)
        .padding(Insets::uniform(sp.lg)),
    });
    let items = vec![
        NavItem::new("Editor", NavIcon::template(EDIT)),
        NavItem::new("Inbox", NavIcon::template(INBOX))
            .badge((state.unread > 0).then_some(NavBadge::Count(state.unread))),
        NavItem::new("About", NavIcon::template(INFO)),
    ];
    let nav = navigation_bar(cx, items, state.page, |s, i| {
        s.page = i;
        if i == 1 {
            s.unread = 0;
        }
    })
    .radius(theme.radius)
    .clip();

    column(vec![bar, pages, nav])
        .gap(sp.md)
        .padding(Insets::uniform(sp.md))
        .fill(theme.palette.background)
        .grow(1.0)
}

fn theme_from_args() -> Theme {
    match std::env::args().nth(1).as_deref() {
        Some("dark") => Theme::dark(),
        Some("material") => Theme::material3_light(),
        Some("material-dark") => Theme::material3_dark(),
        Some("contrast") => Theme::light().high_contrast(),
        Some("native") => Theme::native(),
        _ => Theme::light(),
    }
}

fn main() {
    let app = App::new(State::new(), view)
        .title("Stipple Widgets")
        .theme(theme_from_args())
        .logical_size(Size::new(W, H));
    app.run();
}

#[cfg(test)]
mod tests {
    use super::*;
    use stipple::core::Role;

    /// Where the size select's button sits: CI (visual.yml) clicks here.
    const SELECT_AT: Point = Point::new(200.0, 127.0);

    #[test]
    fn the_select_sits_where_ci_clicks() {
        if Font::system_default().is_none() {
            return; // positions depend on text metrics
        }
        let mut app = App::new(State::new(), view).logical_size(Size::new(W, H));
        app.render_once();
        let tree = app.accessibility_tree().unwrap();
        let combo = tree
            .descendants()
            .into_iter()
            .find(|n| n.role == Role::ComboBox)
            .unwrap()
            .bounds;
        assert!(combo.contains(SELECT_AT), "{combo:?}");
        assert!(app.click_at(SELECT_AT));
        assert!(app.state().size.is_open());
    }
}

// In the browser, `web.rs` builds this file as a library and the page
// runs `main`'s app (on the web, `App::run` hands it to the page).
stipple::web_app!(main);
