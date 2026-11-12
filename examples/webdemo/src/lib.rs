//! Stipple on the web: an ordinary [`App`], made a web app by
//! [`stipple::web_app!`]. The library provides the exports, and
//! `web/stipple.js` (in the repository) the page side; no `wasm-bindgen`.
//!
//! It shows what the web backend does: a click counter, a spinner that turns
//! (driven by `requestAnimationFrame`) while a timer "loads" data, a repeating
//! timer, a text editor with clipboard support (and input methods), opening
//! and saving a file, and a checkbox. Its theme follows the page's
//! `prefers-color-scheme`, and screen readers see it through the ARIA overlay
//! `web/stipple.js` keeps.
//!
//! A theme change (the page's color scheme flipping) cross-fades, and snaps
//! under `prefers-reduced-motion: reduce`.
//!
//! The counter has a context menu (right-click, or a long press on a touch
//! screen) that adds ten.
//!
//! `index.html#selftest` drives it with scripted DOM events and reports what
//! it saw (see the page). CI runs that in headless Chrome. The page reads the
//! state through [`webdemo_probe`].

use std::cell::Cell;

use stipple::platform::dialog::{self, FileDialog};
use stipple::prelude::*;

#[derive(Debug, Default)]
pub struct State {
    clicks: u32,
    ticks: u32,
    loaded: bool,
    notes: EditBuffer,
    file: Option<(String, usize)>,
    checked: bool,
}

thread_local! {
    /// Frames built while loading, and after (the selftest's spinner check).
    static FRAMES_LOADING: Cell<u32> = const { Cell::new(0) };
    static FRAMES_IDLE: Cell<u32> = const { Cell::new(0) };
    /// The state, mirrored for [`webdemo_probe`].
    static PROBE: Cell<[u32; 8]> = const { Cell::new([0; 8]) };
}

fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    let handle = cx.handle();
    let counter = if state.loaded {
        &FRAMES_IDLE
    } else {
        &FRAMES_LOADING
    };
    counter.with(|c| c.set(c.get() + 1));
    PROBE.with(|p| {
        p.set([
            state.clicks,
            state.ticks,
            u32::from(state.loaded),
            state.file.as_ref().map_or(0, |f| f.1 as u32),
            state.notes.text().len() as u32,
            u32::from(theme.is_dark()),
            u32::from(state.checked),
            u32::from(cx.reduced_motion()),
        ])
    });

    let status = if state.loaded {
        label(&theme, "Loaded.")
    } else {
        row(vec![spinner(cx, &theme), label(&theme, "Loading…")])
            .gap(theme.spacing.sm)
            .align(Align::Start, Align::Center)
    };
    let notes = text_editor(cx, &theme, &state.notes, |s: &mut State| &mut s.notes)
        .width(420.0)
        .height(44.0);
    let open = button_labeled(&theme, "Open…").on_tap(cx, move |_: &mut State| {
        let handle = handle.clone();
        dialog::open_file_contents(&FileDialog::new().with_accept(".txt,text/*"), move |file| {
            if let Some(file) = file {
                let text = String::from_utf8_lossy(&file.bytes).into_owned();
                handle.update(move |s: &mut State| {
                    s.file = Some((file.name, file.bytes.len()));
                    s.notes = EditBuffer::from_text(text.lines().next().unwrap_or(""));
                });
            }
        });
    });
    let save = button_labeled(&theme, "Save").on_tap(cx, |s: &mut State| {
        let opts = FileDialog::new().with_file_name("notes.txt");
        dialog::save_file_contents(&opts, s.notes.text().as_bytes());
    });
    let file = match &state.file {
        Some((name, len)) => format!("Opened {name} ({len} bytes)"),
        None => "No file opened".to_string(),
    };
    column(vec![
        text_field_row(&theme, notes),
        row(vec![
            button_labeled(&theme, format!("Clicked {} times", state.clicks))
                .on_tap(cx, |s: &mut State| s.clicks += 1)
                // Right-click, or a long press on a touch screen.
                .context_menu(
                    cx,
                    Menu::new("").item(MenuItem::new("Add ten", |s: &mut State| s.clicks += 10)),
                ),
            open,
            save,
        ])
        .gap(theme.spacing.sm),
        status,
        label(&theme, format!("Ticks: {}", state.ticks)),
        label(&theme, file),
        row(vec![
            checkbox(cx, &theme, state.checked, |s: &mut State| {
                s.checked = !s.checked
            }),
            label(&theme, "A checkbox"),
        ])
        .gap(theme.spacing.sm)
        .align(Align::Start, Align::Center),
    ])
    .gap(theme.spacing.md)
    .padding(Insets::uniform(theme.spacing.lg))
}

fn text_field_row(theme: &Theme, field: Element) -> Element {
    column(vec![label(theme, "Notes (type, copy, paste):"), field]).gap(theme.spacing.xs)
}

/// The app. `web_app!` runs it in the page; natively it renders headless.
pub fn app() -> App<State> {
    let app = App::new(State::default(), view)
        .title("Stipple on the web")
        // The page's prefers-color-scheme, followed as it changes.
        .theme(Theme::native());
    let handle = app.handle();
    // Pretend to load something for 1.5 s, while the spinner turns.
    handle.after(Duration::from_millis(1500), |s: &mut State| s.loaded = true);
    // Tick twice a second for 2.5 s, then stop (so the app goes idle, and the
    // screenshot CI takes is the same every time).
    let ticker = handle.every(Duration::from_millis(500), |s: &mut State| s.ticks += 1);
    let stop = handle.clone();
    handle.after(Duration::from_millis(2600), move |_: &mut State| {
        stop.cancel(ticker)
    });
    app
}

stipple::web_app!(app);

/// The state for the page's selftest: 0 clicks, 1 timer ticks, 2 loaded, 3 the
/// opened file's size, 4 the notes' length in bytes, 5 frames built while
/// loading, 6 frames built since, 7 whether the theme is dark, 8 whether the
/// checkbox is checked, 9 whether motion is reduced (`prefers-reduced-motion`).
#[unsafe(no_mangle)]
pub extern "C" fn webdemo_probe(what: u32) -> u32 {
    match what {
        5 => FRAMES_LOADING.with(Cell::get),
        6 => FRAMES_IDLE.with(Cell::get),
        7..=9 => PROBE.with(|p| p.get()[what as usize - 2]),
        i => PROBE.with(|p| p.get().get(i as usize).copied().unwrap_or(0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_and_counts_clicks() {
        let mut app = app().logical_size(Size::new(480.0, 320.0));
        let frame = app.render_once();
        assert_eq!((frame.size().width, frame.size().height), (480, 320));
        assert_eq!(webdemo_probe(0), 0);
    }
}
