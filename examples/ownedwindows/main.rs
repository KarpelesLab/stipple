//! Windows that belong to another (`WindowAttributes::parent`): a
//! Preferences window that is modal to the main one (a sheet on macOS), an
//! owned tool window, and a message box asked for one window
//! (`MessageBox::window`). The find bar's field takes focus as it opens
//! (`Element::autofocus`), and closing it gives focus back to the "terminal"
//! (`AppHandle::focus`).
//!
//! With `--selftest` it opens the modal window, checks that a message box
//! can target it, closes it and exits, printing `ownedwindows: …` lines.

use std::time::Duration;

use stipple::platform::WindowAttributes;
use stipple::platform::dialog::MessageBox;
use stipple::prelude::*;

struct State {
    prefs: Option<WindowId>,
    finding: bool,
    query: TextInput,
    terminal: TextInput,
    log: String,
    font_size: TextInput,
}

fn say(line: std::fmt::Arguments<'_>) {
    use std::io::Write;
    println!("ownedwindows: {line}");
    let _ = std::io::stdout().flush();
}

/// The Preferences window: a field, and Enter or "Done" closes it.
fn prefs_view(s: &State, cx: &mut Cx<State>) -> Element {
    let h = cx.handle();
    let win = cx.window_id();
    let ask = {
        let h = h.clone();
        move |_: &mut State| {
            h.message(
                MessageBox::new("Reset preferences?")
                    .buttons(["Reset", "Cancel"])
                    .cancel(1)
                    .window(win),
                |s: &mut State, r| {
                    if r.button == Some(0) {
                        s.font_size.set_text("13");
                    }
                },
            )
        }
    };
    column(vec![
        label(cx, "Font size"),
        text_field(cx, &s.font_size, |s| &mut s.font_size).autofocus(),
        row(vec![
            button(cx, "Reset…").on_tap(cx, ask),
            button(cx, "Done").on_tap(cx, move |s| {
                s.prefs = None;
                h.close_window(win);
            }),
        ])
        .gap(8.0),
    ])
    .gap(12.0)
    .padding(Insets::uniform(20.0))
    .grow(1.0)
}

fn open_prefs(s: &mut State, h: &AppHandle<State>, parent: WindowId) {
    if s.prefs.is_some() {
        return;
    }
    let id = h.open_window(
        WindowAttributes::new()
            .title("Preferences")
            .logical_size(Size::new(320.0, 180.0))
            .resizable(false)
            .parent(parent)
            .modal(true)
            .sheet(true),
        prefs_view,
    );
    s.prefs = Some(id);
}

fn view(s: &State, cx: &mut Cx<State>) -> Element {
    let h = cx.handle();
    let win = cx.window_id();
    if s.finding {
        let field = text_field(cx, &s.query, |s| &mut s.query)
            .placeholder("Find")
            .autofocus()
            .width(240.0);
        let close = {
            let h = h.clone();
            move |s: &mut State| {
                s.finding = false;
                h.focus(win, "terminal");
            }
        };
        let bar = row(vec![field, button(cx, "Close").on_tap(cx, close)])
            .gap(8.0)
            .padding(Insets::uniform(8.0));
        cx.overlay(OverlaySpec::new(bar, Anchor::At(Point::new(260.0, 8.0))));
    }
    let prefs = {
        let h = h.clone();
        move |s: &mut State| open_prefs(s, &h, win)
    };
    let tool = {
        let h = h.clone();
        move |_: &mut State| {
            h.open_window(
                WindowAttributes::new()
                    .title("Tool")
                    .logical_size(Size::new(200.0, 120.0))
                    .parent(win),
                |_: &State, cx: &mut Cx<State>| {
                    column(vec![label(cx, "An owned window")])
                        .padding(Insets::uniform(16.0))
                        .grow(1.0)
                },
            );
        }
    };
    let ask = move |_: &mut State| {
        h.message(
            MessageBox::new("Host key changed").window(win),
            |s: &mut State, _| s.log = "answered".into(),
        )
    };
    column(vec![
        text_area(cx, &s.terminal, |s| &mut s.terminal)
            .key("terminal")
            .grow(1.0),
        row(vec![
            button(cx, "Preferences…").on_tap(cx, prefs),
            button(cx, "Tool window").on_tap(cx, tool),
            button(cx, "Find").on_tap(cx, |s| s.finding = true),
            button(cx, "Ask here").on_tap(cx, ask),
        ])
        .gap(8.0),
        label(cx, &s.log),
    ])
    .gap(12.0)
    .padding(Insets::uniform(16.0))
    .grow(1.0)
}

fn selftest(handle: &AppHandle<State>) {
    let step = Duration::from_millis(1200);
    let h = handle.clone();
    handle.after(step, move |s: &mut State| {
        open_prefs(s, &h, WindowId::PRIMARY);
        say(format_args!("opened {:?}", s.prefs));
    });
    let h = handle.clone();
    handle.after(step * 2, move |s: &mut State| {
        say(format_args!("windows={}", h.windows().len()));
        if let Some(id) = s.prefs.take() {
            h.close_window(id);
        }
    });
    let h = handle.clone();
    handle.after(step * 3, move |_: &mut State| {
        say(format_args!("windows={}", h.windows().len()));
        h.exit();
    });
}

fn main() {
    let app = App::new(
        State {
            prefs: None,
            finding: false,
            query: TextInput::new(),
            terminal: TextInput::new(),
            log: String::new(),
            font_size: TextInput::new(),
        },
        view,
    )
    .title("Owned windows")
    .logical_size(Size::new(560.0, 360.0))
    .on_close_requested(|s: &mut State, id| {
        // The Preferences window closing by its close box.
        if s.prefs == Some(id) {
            s.prefs = None;
        }
        CloseResponse::Close
    });
    if std::env::args().any(|a| a == "--selftest") {
        selftest(&app.handle());
    }
    app.run();
    say(format_args!("event loop exited"));
}
