//! Window lifecycle: confirming a close, following window focus, and asking
//! for the user's attention, as a terminal does with running processes and
//! its bell.
//!
//! - Closing the window while "work is running" is cancelled the first time,
//!   and a banner asks to close again. The second request starts a check
//!   that takes a moment (standing in for asking a daemon whether processes
//!   still run), and once it answers the app closes the window itself with
//!   `AppHandle::close_window`.
//! - The view shows whether the window has keyboard focus (a solid or hollow
//!   caret). A second after it loses focus, the "bell" rings: a beep, an
//!   attention request and a Dock badge counting the bells, cleared when the
//!   window is focused again.
//!
//! Each step is printed (`lifecycle: ...`), which CI checks.

use stipple::platform::WindowId;
use stipple::prelude::*;

#[derive(Default)]
struct State {
    /// Work is running: closing needs a confirmation.
    busy: bool,
    /// The window a close was cancelled for, while the banner asks again.
    confirm: Option<WindowId>,
    /// Waiting for the close check to answer.
    checking: bool,
    focused: bool,
    /// Bells rung while the window was away.
    bells: u32,
}

fn say(what: std::fmt::Arguments<'_>) {
    println!("lifecycle: {what}");
}

fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    let handle = cx.handle();
    let focused = cx.window_focused();
    // A terminal-like caret: solid while focused, hollow while not.
    let caret = Element::boxed(BoxStyle::default())
        .border(theme.palette.primary, 2.0)
        .width(12.0)
        .height(22.0);
    let caret = if focused {
        caret.fill(theme.palette.primary)
    } else {
        caret
    };
    let status = row(vec![
        caret,
        label(
            &theme,
            if focused {
                "Window focused"
            } else {
                "Window not focused"
            },
        ),
    ])
    .gap(theme.spacing.sm)
    .align(Align::Start, Align::Center);
    let busy = row(vec![
        checkbox(cx, &theme, state.busy, |s: &mut State| s.busy = !s.busy),
        label(&theme, "Work is running (closing asks first)"),
    ])
    .gap(theme.spacing.sm)
    .align(Align::Start, Align::Center);
    let beep = button_labeled(&theme, "Beep").on_tap(cx, {
        let handle = handle.clone();
        move |_| handle.beep()
    });
    let attention = button_labeled(&theme, "Ask for attention in 2 s").on_tap(cx, {
        let handle = handle.clone();
        move |_| {
            let handle2 = handle.clone();
            handle.after(Duration::from_secs(2), move |_| {
                handle2.request_attention(UserAttention::Critical)
            });
        }
    });
    let mut children = vec![
        heading(&theme, "Lifecycle"),
        status,
        label(&theme, format!("Bells while away: {}", state.bells)),
        busy,
        row(vec![beep, attention]).gap(theme.spacing.sm),
    ];
    if let Some(id) = state.confirm {
        let text = if state.checking {
            "Checking whether the work can stop…"
        } else {
            "Work is still running. Close again to close anyway."
        };
        let close = button_labeled(&theme, "Close anyway").on_tap(cx, {
            let handle = handle.clone();
            move |_| handle.close_window(id)
        });
        let keep = button_labeled(&theme, "Keep open").on_tap(cx, |s: &mut State| {
            s.confirm = None;
        });
        children.push(panel(
            &theme,
            vec![
                label(&theme, text),
                row(vec![close, keep]).gap(theme.spacing.sm),
            ],
        ));
    }
    column(children)
        .gap(theme.spacing.md)
        .padding(Insets::uniform(theme.spacing.lg))
}

fn main() {
    let state = State {
        busy: true,
        focused: true,
        ..Default::default()
    };
    let mut app = App::new(state, view)
        .title("Stipple lifecycle")
        .logical_size(Size::new(420.0, 340.0));
    if let Some(font) = Font::system_default() {
        app = app.font(font);
    }
    let handle = app.handle();
    let on_close = handle.clone();
    let on_focus = handle.clone();
    app.on_close_requested(move |s, id| {
        if !s.busy {
            say(format_args!("close accepted"));
            return CloseResponse::Close;
        }
        if s.confirm.is_none() {
            say(format_args!("close vetoed"));
            s.confirm = Some(id);
            return CloseResponse::Cancel;
        }
        // Asked again: check (a moment, off the UI thread), then close
        // ourselves.
        say(format_args!("close confirmed, checking"));
        s.checking = true;
        let handle = on_close.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            say(format_args!("check done, closing"));
            handle.close_window(id);
        });
        CloseResponse::Cancel
    })
    .on_window_focus(move |s, _, focused| {
        say(format_args!("focus {focused}"));
        s.focused = focused;
        if focused {
            s.bells = 0;
            on_focus.set_badge(None);
            return;
        }
        // The bell rings a second after the window loses focus.
        let handle = on_focus.clone();
        on_focus.after(Duration::from_secs(1), move |s: &mut State| {
            if s.focused {
                return;
            }
            s.bells += 1;
            say(format_args!("bell {}", s.bells));
            handle.beep();
            handle.request_attention(UserAttention::Informational);
            handle.set_badge(Some(&s.bells.to_string()));
        });
    })
    .run();
    say(format_args!("event loop exited"));
}
