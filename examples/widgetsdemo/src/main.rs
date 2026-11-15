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

    let bar = toolbar(
        &theme,
        vec![
            tool_button(cx, &theme, "New", |s: &mut State| s.note("New document")),
            tool_button(cx, &theme, "Open", |s: &mut State| s.note("Open…")),
            tool_button(cx, &theme, "Save", |s: &mut State| s.note("Saved")),
            tool_separator(&theme),
            tool_toggle(cx, &theme, "B", state.bold, |s: &mut State| {
                s.bold = !s.bold;
            }),
            tool_toggle(cx, &theme, "I", state.italic, |s: &mut State| {
                s.italic = !s.italic;
            }),
            tool_separator(&theme),
            tool_icon_button(
                cx,
                &theme,
                &state.trash,
                "Clear log",
                move |s: &mut State| {
                    let cleared = std::mem::take(&mut s.log);
                    let id = s.toasts.push_for(
                        &undo_handle,
                        Duration::from_secs(6),
                        |s: &mut State| &mut s.toasts,
                        "Log cleared",
                        ToastKind::Info,
                    );
                    s.toasts.action(id, "Undo", move |s: &mut State| {
                        s.log.clone_from(&cleared);
                    });
                },
            ),
            spacer(),
            tool_button(cx, &theme, "Notify", move |s: &mut State| {
                let n = s.log.len();
                s.toasts.push_for(
                    &handle,
                    Duration::from_secs(4),
                    |s: &mut State| &mut s.toasts,
                    format!("Notification #{n}"),
                    ToastKind::Success,
                );
            }),
        ],
    );

    let form = grid(
        vec![Track::px(84.0), Track::fr(1.0)],
        vec![
            heading(&theme, "Settings").span(2, 1),
            label(&theme, "Size"),
            select(cx, &theme, &SIZES, &state.size, |s: &mut State| &mut s.size).width(180.0),
            label(&theme, "Width"),
            number_input(cx, &theme, &state.width, |s: &mut State| &mut s.width),
            label(&theme, "Opacity"),
            number_input(cx, &theme, &state.opacity, |s: &mut State| &mut s.opacity),
            label(&theme, "Format"),
            choice_chips(cx, &theme, FORMATS, state.format, |s: &mut State, i| {
                s.format = i;
            }),
            label(&theme, "Show"),
            filter_chips(cx, &theme, EXTRAS, &state.extras, |s: &mut State, i| {
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
        heading(&theme, "Preview"),
        paragraph(
            &theme,
            format!(
                "{size}, {} px wide at {:.0}% opacity, {style}, as {} with {extras}.",
                state.width.value(),
                state.opacity.value() * 100.0,
                FORMATS[state.format],
            ),
        ),
    ])
    .gap(sp.sm)
    .padding(Insets::uniform(sp.lg))
    .align(Align::Start, Align::Stretch);
    let log = column(
        std::iter::once(heading(&theme, "Log"))
            .chain(state.log.iter().map(|l| label(&theme, l.as_str())))
            .collect(),
    )
    .gap(sp.xs)
    .padding(Insets::uniform(sp.lg));
    let right = split_pane(
        cx,
        &theme,
        &state.preview,
        |s: &mut State| &mut s.preview,
        preview,
        log,
    );

    let body = split_pane(
        cx,
        &theme,
        &state.sidebar,
        |s: &mut State| &mut s.sidebar,
        form,
        right,
    );
    let body = row(vec![body])
        .fill(theme.palette.surface)
        .border(theme.palette.border, 1.0)
        .radius(theme.radius)
        .align(Align::Start, Align::Stretch)
        .clip()
        .grow(1.0);

    toasts(cx, &theme, &state.toasts, |s: &mut State| &mut s.toasts);

    // The editor, the inbox and the about page, each kept as it was while
    // another shows.
    let mut editor = Some(body);
    let pages = page_stack(cx, 3, state.page, |cx, i, _visible| match i {
        0 => editor.take().unwrap_or_else(spacer),
        1 => {
            let rows = (1..=60)
                .map(|n| label(&theme, format!("Message {n}")).padding(Insets::uniform(sp.sm)))
                .collect();
            let list = Element::stack(Axis::Vertical, rows).align(Align::Start, Align::Stretch);
            let id = cx.register_scroll();
            column(vec![list])
                .scrollable(id)
                .fill(theme.palette.surface)
                .radius(theme.radius)
                .grow(1.0)
        }
        _ => column(vec![
            heading(&theme, "Stipple widgets"),
            paragraph(&theme, "Self-drawn widgets over hand-written OS backends."),
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
    let nav = navigation_bar(cx, &theme, items, state.page, |s: &mut State, i| {
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
        .align(Align::Start, Align::Stretch)
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
    let mut app = App::new(State::new(), view)
        .title("Stipple Widgets")
        .theme(theme_from_args())
        .logical_size(Size::new(W, H));
    if let Some(font) = Font::system_default() {
        app = app.font(font);
    }
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
        let mut app = App::new(State::new(), view).logical_size(Size::new(W, H));
        if let Some(font) = Font::system_default() {
            app = app.font(font);
        } else {
            return; // positions depend on text metrics
        }
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
