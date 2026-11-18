//! A log viewer: a [`text_view`] of 10,000 lines of (generated) log that
//! follows new lines as they arrive, with find and selection across lines.
//!
//! - Only the lines on screen are laid out; the cap keeps the last 10,000,
//!   and what's on screen stays put as old lines drop off the top.
//! - It opens at the end and follows it; scroll up to stop following, press
//!   End (or scroll back down) to resume.
//! - Drag across lines to select them, Ctrl/Cmd+C or right-click to copy,
//!   Ctrl/Cmd+A to select all.
//! - Type in Find to highlight the matches; Enter or Next/Previous jump to
//!   them.
//!
//! `cargo run --example logview`

use std::time::Duration;

use stipple::prelude::*;
use stipple::widgets::{FindOptions, TextView, TextViewOptions, text_view};

struct Viewer {
    log: TextView,
    find: TextInput,
    case_sensitive: bool,
    live: bool,
    seq: u64,
}

const LEVELS: [&str; 4] = ["INFO ", "DEBUG", "WARN ", "ERROR"];
const EVENTS: [&str; 8] = [
    "accepted connection from 10.0.4.17:51522",
    "GET /api/v2/items?page=3 200 12ms",
    "cache miss for key session:7f3a",
    "flushed 128 dirty pages to disk",
    "retrying upstream request (attempt 2)",
    "worker 3 finished job #88213 in 412ms",
    "disk usage at 91% on /var/lib/data",
    "connection reset by peer",
];

/// Log line `seq`: a timestamp, a level and an event, picked by a cheap
/// hash so the log looks varied but is the same every run.
fn line(seq: u64) -> String {
    let h = seq.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 32;
    let level = match h % 16 {
        0 => 3,
        1 | 2 => 2,
        3..=6 => 1,
        _ => 0,
    };
    let (s, ms) = (seq / 4, (seq % 4) * 250);
    format!(
        "12:{:02}:{:02}.{ms:03} {} {}",
        (s / 60) % 60,
        s % 60,
        LEVELS[level],
        EVENTS[(h >> 8) as usize % EVENTS.len()]
    )
}

impl Viewer {
    fn new() -> Self {
        let mut log = TextView::new().max_lines(10_000);
        log.append((0..10_000).map(line));
        Viewer {
            log,
            find: TextInput::new(),
            case_sensitive: false,
            live: true,
            seq: 10_000,
        }
    }

    /// A few more lines, as a running service writes them.
    fn tick(&mut self) {
        if !self.live {
            return;
        }
        for _ in 0..3 {
            self.log.push_line(line(self.seq));
            self.seq += 1;
        }
    }

    fn refind(&mut self) {
        let options = FindOptions::default().case_sensitive(self.case_sensitive);
        let query = self.find.text().to_string();
        self.log.find(&query, options);
    }
}

fn view(state: &Viewer, cx: &mut Cx<Viewer>) -> Element {
    let find = text_field(cx, &state.find, |s| &mut s.find)
        .placeholder("Find")
        .on_change(cx, Viewer::refind)
        .on_submit(cx, |s| {
            s.log.find_next();
        })
        .width(220.0);
    let previous = button(cx, "Previous").on_tap(cx, |s| {
        s.log.find_previous();
    });
    let next = button(cx, "Next").on_tap(cx, |s| {
        s.log.find_next();
    });
    let case = checkbox(cx, "Match case", state.case_sensitive, |s| {
        s.case_sensitive = !s.case_sensitive;
        s.refind();
    });
    let live = checkbox(cx, "Live", state.live, |s| s.live = !s.live);
    let clear = button(cx, "Clear").on_tap(cx, |s| s.log.clear());
    let matches = match (state.log.current_match(), state.log.matches().len()) {
        (_, 0) if state.find.text().is_empty() => String::new(),
        (_, 0) => "No matches".to_string(),
        (Some((i, _)), n) => format!("{} of {n}", i + 1),
        (None, n) => format!("{n} matches"),
    };
    let bar = row(vec![
        find,
        previous,
        next,
        label(cx, matches),
        spacer(),
        case,
        live,
        clear,
    ])
    .gap(cx.theme().spacing.sm)
    .align(Align::Center, Align::Center);
    let muted = cx.theme().palette.text_muted;
    let status = label(
        cx,
        RichText::new().colored(
            format!(
                "{} lines, {} dropped off the top",
                state.log.len(),
                state.log.dropped()
            ),
            muted,
        ),
    );
    let log = text_view(
        cx,
        &state.log,
        |s| &mut s.log,
        TextViewOptions::default().stick_to_end(true),
    );
    panel(cx)
        .children(vec![bar, log, status])
        .gap(cx.theme().spacing.sm)
        .padding(Insets::uniform(cx.theme().spacing.md))
}

fn logview_app(theme: Theme) -> App<Viewer> {
    App::new(Viewer::new(), view)
        .title("Stipple Log Viewer")
        .theme(theme)
        .logical_size(Size::new(900.0, 560.0))
}

fn main() {
    let app = logview_app(Theme::light());
    app.handle().every(Duration::from_millis(250), Viewer::tick);
    app.run();
}

// In the browser, `web.rs` builds this file as a library and the page
// runs `main`'s app (on the web, `App::run` hands it to the page).
stipple::web_app!(main);

#[cfg(test)]
mod tests {
    use super::*;
    use stipple::core::Role;

    fn shown(app: &App<Viewer>) -> String {
        let a = app.accessibility_tree().unwrap();
        a.descendants()
            .into_iter()
            .find(|n| n.role == Role::Text && n.name.contains("12:"))
            .map(|n| n.name.clone())
            .unwrap_or_default()
    }

    #[test]
    fn opens_at_the_end_follows_and_finds() {
        let mut app = logview_app(Theme::light()).no_font();
        app.render_once();
        app.render_once();
        assert!(shown(&app).ends_with(&line(9_999)), "{}", shown(&app));
        // New lines arrive: still at the end, the oldest dropped.
        app.handle().update(Viewer::tick);
        assert!(app.run_pending());
        app.render_once();
        assert!(shown(&app).ends_with(&line(10_002)));
        assert_eq!(app.state().log.dropped(), 3);
        // Find, then jump to the first match (near the top).
        app.handle().update(|s| {
            s.find.set_text("job #");
            s.refind();
            s.log.find_next();
        });
        assert!(app.run_pending());
        app.render_once();
        let (_, m) = app.state().log.current_match().unwrap();
        let first = app.state().log.line(m.line).unwrap().to_string();
        assert!(shown(&app).contains(&first));
    }
}
