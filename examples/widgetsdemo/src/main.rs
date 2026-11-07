//! Select, number input, split pane, toolbar and toasts in one window.
//!
//! A toolbar runs along the top; below it a split pane holds a settings form
//! (a select and two number inputs) on the left and, on the right, a second,
//! vertical split with a preview above an event log. Drag either divider, or
//! focus one (Tab) and use the arrow keys. "Notify" raises a toast that
//! dismisses itself after four seconds; the welcome toast stays until its ×.
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

const SIZES: [&str; 5] = ["Small", "Medium", "Large", "Extra large", "Huge"];

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
    log: Vec<String>,
    trash: Arc<Svg>,
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
            log: vec!["Ready.".into()],
            trash: Arc::new(Svg::parse(ICON).expect("the bundled icon parses")),
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
            tool_icon_button(cx, &theme, &state.trash, "Clear log", |s: &mut State| {
                s.log.clear();
            }),
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
    let preview = column(vec![
        heading(&theme, "Preview"),
        paragraph(
            &theme,
            format!(
                "{size}, {} px wide at {:.0}% opacity, {style}.",
                state.width.value(),
                state.opacity.value() * 100.0
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

    column(vec![bar, body])
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
