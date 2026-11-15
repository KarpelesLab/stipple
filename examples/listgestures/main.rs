//! An inbox with the list gestures phone apps use: swipe a message left to
//! delete or archive it (all the way across deletes at once, and the rows
//! below close the gap), right to mark it read or unread, and pull the list
//! down from its top to check for new mail.
//!
//! - On a touch screen: drag a row sideways (a vertical drag still scrolls
//!   the list), or pull the list down at its top.
//! - With a mouse: drag a row sideways, or swipe two fingers sideways on a
//!   trackpad; there's no pull, so the toolbar has a Refresh button, as a
//!   desktop app would.
//! - From the keyboard: Tab to the list, arrows to a message, and the Menu
//!   key (or Shift+F10) for its actions and Refresh.
//!
//! `--selftest` drives it off-screen with simulated touches (a swipe that
//! deletes, a partial swipe that opens a row, a pull that refreshes) and
//! prints `listgestures: selftest ok`.

use std::time::Duration;

use stipple::platform::{Touch, TouchPhase};
use stipple::prelude::*;
use stipple::widgets::{RefreshState, SwipeAction, SwipeActions, SwipeState};

const SENDERS: [(&str, &str); 10] = [
    ("Ada", "Notes on the analytical engine"),
    ("Grace", "The bug in relay 70"),
    ("Linus", "Re: [PATCH] fix the scheduler"),
    ("Barbara", "Abstraction and data types"),
    ("Ken", "A shell that fits on a page"),
    ("Margaret", "Priority displays, again"),
    ("Dennis", "Pointers to pointers"),
    ("Frances", "Compiler optimizations"),
    ("Edsger", "Goto, considered"),
    ("Radia", "Spanning trees for bridges"),
];

#[derive(Clone, Debug, PartialEq)]
struct Message {
    id: u32,
    from: &'static str,
    subject: &'static str,
    read: bool,
}

struct State {
    messages: Vec<Message>,
    next: u32,
    archived: u32,
    list: ListState,
    swipe: SwipeState,
    refresh: RefreshState,
    /// How long a refresh takes (none in the self-test).
    fetch: Duration,
}

impl State {
    fn new() -> Self {
        let mut s = Self {
            messages: Vec::new(),
            next: 0,
            archived: 0,
            list: ListState::new(SelectionMode::Single),
            swipe: SwipeState::new(),
            refresh: RefreshState::new(),
            fetch: Duration::from_millis(1200),
        };
        for _ in 0..24 {
            s.arrive();
        }
        s.messages.reverse();
        s
    }

    /// A new message at the top.
    fn arrive(&mut self) {
        let (from, subject) = SENDERS[self.next as usize % SENDERS.len()];
        self.messages.insert(
            0,
            Message {
                id: self.next,
                from,
                subject,
                read: self.next.is_multiple_of(3),
            },
        );
        self.next += 1;
    }

    fn remove(&mut self, id: u32) {
        self.messages.retain(|m| m.id != id);
        self.list.truncate(self.messages.len());
    }

    fn toggle_read(&mut self, id: u32) {
        if let Some(m) = self.messages.iter_mut().find(|m| m.id == id) {
            m.read = !m.read;
        }
    }
}

/// Check for mail: the indicator spins until the "server" answers.
fn refresh(s: &mut State, handle: &AppHandle<State>) {
    if s.fetch.is_zero() {
        s.arrive();
        s.refresh.finish();
        return;
    }
    handle.after(s.fetch, |s: &mut State| {
        s.arrive();
        s.arrive();
        s.refresh.finish();
    });
}

fn row_of(cx: &mut Cx<State>, s: &State, m: &Message) -> Element {
    let theme = *cx.theme();
    let p = theme.palette;
    let id = m.id;
    let actions = SwipeActions::new()
        .trailing(SwipeAction::new("Delete", move |s: &mut State| s.remove(id)).destructive())
        .trailing(SwipeAction::new("Archive", move |s: &mut State| {
            s.remove(id);
            s.archived += 1;
        }))
        .leading(
            SwipeAction::new(
                if m.read { "Unread" } else { "Read" },
                move |s: &mut State| s.toggle_read(id),
            )
            .color(Color::rgb(0x2f, 0x7d, 0xf6)),
        );
    let dot = Element::boxed(BoxStyle::default())
        .width(8.0)
        .height(8.0)
        .radius(4.0)
        .fill(if m.read {
            Color::TRANSPARENT
        } else {
            p.primary
        });
    let text = column(vec![
        label(cx, m.from),
        Element::text(m.subject, theme.typography.caption, p.text_muted),
    ])
    .gap(2.0)
    .grow(1.0);
    swipe_row(cx, &s.swipe, |s| &mut s.swipe, id, actions).child(
        row(vec![dot, text])
            .gap(theme.spacing.sm)
            .align(Align::Start, Align::Center),
    )
}

fn view(s: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    let handle = cx.handle();
    let toolbar_handle = handle.clone();
    let status = if s.refresh.is_refreshing() {
        "Checking for mail…".to_string()
    } else {
        format!("{} messages, {} archived", s.messages.len(), s.archived)
    };
    let top = row(vec![
        heading(cx, "Inbox").grow(1.0),
        button(cx, "Refresh").on_tap(cx, move |s: &mut State| {
            if !s.refresh.is_refreshing() {
                s.refresh.begin();
                refresh(s, &toolbar_handle);
            }
        }),
    ])
    .gap(theme.spacing.sm)
    .align(Align::Start, Align::Center);
    let list = pull_to_refresh(
        cx,
        &s.refresh,
        |s| &mut s.refresh,
        move |s| refresh(s, &handle),
    )
    .child(virtual_list(
        cx,
        &s.list,
        |s| &mut s.list,
        VirtualList::fixed(s.messages.len(), 60.0).height(460.0),
        |cx, r| row_of(cx, s, &s.messages[r.index]),
    ));
    column(vec![top, list, label(cx, status)])
        .gap(theme.spacing.md)
        .padding(Insets::uniform(16.0))
}

fn app(state: State) -> App<State> {
    App::new(state, view)
        .title("Stipple List Gestures")
        .logical_size(Size::new(380.0, 620.0))
}

fn selftest() -> Result<(), String> {
    let ensure = |ok: bool, what: &str| if ok { Ok(()) } else { Err(what.to_string()) };
    stipple::anim::set_reduced_motion(Some(true));
    let mut state = State::new();
    state.fetch = Duration::ZERO;
    let mut app = app(state);
    let _ = app.render_frame();
    // Where a message's sender is shown (window y).
    let row_y = |app: &mut App<State>, i: usize| -> Result<f64, String> {
        let from = app.state().messages[i].from;
        let node = app.find_text(from).ok_or(format!("{from} isn't shown"))?;
        Ok(node.bounds.center().y)
    };
    let finger = |app: &mut App<State>, phase, x: f64, y: f64| {
        app.touch(Touch::new(1, phase, Point::new(x, y)));
        let _ = app.render_frame();
    };

    // All the way across to the left: the second message is deleted.
    let second = app.state().messages[1].id;
    let y = row_y(&mut app, 1)?;
    finger(&mut app, TouchPhase::Started, 350.0, y);
    finger(&mut app, TouchPhase::Moved, 330.0, y);
    finger(&mut app, TouchPhase::Moved, 150.0, y);
    finger(&mut app, TouchPhase::Moved, 40.0, y);
    finger(&mut app, TouchPhase::Ended, 40.0, y);
    ensure(
        app.state().messages.iter().all(|m| m.id != second),
        "a full swipe deletes",
    )?;

    // Part of the way: the row opens on its buttons.
    let first = app.state().messages[0].id;
    let y = row_y(&mut app, 0)?;
    finger(&mut app, TouchPhase::Started, 300.0, y);
    finger(&mut app, TouchPhase::Moved, 290.0, y);
    finger(&mut app, TouchPhase::Moved, 190.0, y);
    std::thread::sleep(Duration::from_millis(150));
    finger(&mut app, TouchPhase::Ended, 190.0, y);
    ensure(app.state().swipe.is_open(first), "a partial swipe opens")?;
    ensure(app.state().messages.len() == 23, "and runs nothing")?;

    // Pulled down at the top: a new message arrives.
    let before = app.state().next;
    let y = row_y(&mut app, 2)?;
    finger(&mut app, TouchPhase::Started, 190.0, y);
    finger(&mut app, TouchPhase::Moved, 190.0, y + 20.0);
    finger(&mut app, TouchPhase::Moved, 190.0, y + 200.0);
    finger(&mut app, TouchPhase::Ended, 190.0, y + 200.0);
    ensure(app.state().next == before + 1, "a pull refreshes")?;
    ensure(!app.state().refresh.is_refreshing(), "and finishes")?;
    stipple::anim::set_reduced_motion(None);
    Ok(())
}

fn main() {
    if std::env::args().any(|a| a == "--selftest") {
        match selftest() {
            Ok(()) => println!("listgestures: selftest ok"),
            Err(e) => {
                println!("listgestures: selftest FAILED: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    app(State::new()).run();
}

// In the browser, `web.rs` builds this file as a library and the page
// runs `main`'s app (on the web, `App::run` hands it to the page).
stipple::web_app!(main);
