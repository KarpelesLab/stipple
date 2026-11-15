//! A terminal-like app's chrome: a document tab bar, a connection form with a
//! password field and a tab color picker, and message boxes.
//!
//! - Tabs select on a click, close with their × (a busy one asks first, in a
//!   message box) and on a middle click, rename on a double click, reorder
//!   by dragging, and have a context menu (Rename…,
//!   a color, Close Others, Close). "+" opens a new one; past what fits, ‹ ›
//!   scroll and ▾ lists them all. With the bar focused (Tab), ←/→ move
//!   between tabs.
//! - On a desktop, a tab dragged out of the window and let go opens in a
//!   window of its own there (the tab follows the pointer outside the window
//!   on X11 and Windows); dragged onto another window's tab bar, it moves
//!   there. A window whose last tab leaves closes; closing one moves its tabs
//!   back to the first.
//! - "Ring bell" puts a bell on the next tab; selecting it clears it.
//! - "Connect" checks the form: an error box without a password, else an
//!   info box.
//!
//! Message boxes are the platform's own on Windows, macOS, iOS and the web,
//! and drawn in the window elsewhere (and with `--drawn`).
//!
//! Pick the theme with an argument: `light` (default), `dark`, `material`,
//! `material-dark`, `contrast` or `native`.
//!
//! With `--alert-selftest`, the app asks a three-button question as soon as it
//! starts, presses its second button itself after `STIPPLE_ALERT_HOLD_MS`
//! (default 1500), prints the answer and exits. CI runs that on Windows and
//! macOS (screenshotting the native alert while it's up), and drives the
//! drawn one on X11 with xdotool.

use stipple::platform::{WindowAttributes, dialog};
use stipple::prelude::*;

const W: f64 = 760.0;
const H: f64 = 480.0;

const COLORS: [(&str, Color); 5] = [
    ("Red", Color::rgb(0xe5, 0x48, 0x4d)),
    ("Orange", Color::rgb(0xf7, 0x6b, 0x15)),
    ("Green", Color::rgb(0x46, 0xa7, 0x58)),
    ("Blue", Color::rgb(0x00, 0x90, 0xff)),
    ("Purple", Color::rgb(0x6e, 0x56, 0xcf)),
];

struct Session {
    id: u64,
    /// The window it shows in.
    window: WindowId,
    name: String,
    color: Option<Color>,
    /// A process runs in it: closing asks first.
    busy: bool,
    bell: bool,
}

/// A window's tab bar: its id, which of its tabs is selected, and the bar's
/// own state.
struct Win {
    id: WindowId,
    /// Names the window's bar (`sessions-<key>`), so a tab dropped on another
    /// window's bar says where it came from.
    key: u64,
    selected: usize,
    bar: TabBarState,
}

struct State {
    /// Every window's sessions, each window's in its tab order.
    sessions: Vec<Session>,
    wins: Vec<Win>,
    next: u64,
    host: TextInput,
    password: PasswordField,
    tint: ColorPicker,
    log: Vec<String>,
    drawn: bool,
}

impl State {
    fn new(drawn: bool) -> Self {
        let sessions = vec![
            Session {
                window: WindowId::PRIMARY,
                id: 1,
                name: "build".into(),
                color: Some(COLORS[2].1),
                busy: true,
                bell: false,
            },
            Session {
                window: WindowId::PRIMARY,
                id: 2,
                name: "prod-db".into(),
                color: Some(COLORS[0].1),
                busy: false,
                bell: true,
            },
            Session {
                window: WindowId::PRIMARY,
                id: 3,
                name: "~/src".into(),
                color: None,
                busy: false,
                bell: false,
            },
        ];
        Self {
            tint: ColorPicker::new(COLORS[2].1).alpha_strip(false),
            sessions,
            wins: vec![Win {
                id: WindowId::PRIMARY,
                key: 0,
                selected: 0,
                bar: TabBarState::new(),
            }],
            next: 4,
            host: TextInput::from_text("build.example.com"),
            password: PasswordField::new().reveal_button(true),
            log: vec!["Ready.".into()],
            drawn,
        }
    }

    fn note(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
        if self.log.len() > 4 {
            self.log.remove(0);
        }
    }

    /// Window `w`'s tab bar (made the first time it's asked for).
    fn win(&mut self, w: WindowId) -> &mut Win {
        if let Some(i) = self.wins.iter().position(|x| x.id == w) {
            return &mut self.wins[i];
        }
        let key = self.wins.iter().map(|x| x.key).max().unwrap_or(0) + 1;
        self.wins.push(Win {
            id: w,
            key,
            selected: 0,
            bar: TabBarState::new(),
        });
        self.wins.last_mut().unwrap()
    }

    fn selected(&self, w: WindowId) -> usize {
        self.wins
            .iter()
            .find(|x| x.id == w)
            .map_or(0, |x| x.selected)
    }

    /// Where window `w`'s sessions are in `sessions`, in its tab order.
    fn in_window(&self, w: WindowId) -> Vec<usize> {
        (0..self.sessions.len())
            .filter(|&i| self.sessions[i].window == w)
            .collect()
    }

    /// Where window `w`'s tab `i` is in `sessions`.
    fn global(&self, w: WindowId, i: usize) -> Option<usize> {
        self.in_window(w).get(i).copied()
    }

    /// Select window `w`'s tab `i`.
    fn select(&mut self, w: WindowId, i: usize) {
        let n = self.in_window(w).len();
        let i = i.min(n.saturating_sub(1));
        self.win(w).selected = i;
        if let Some(g) = self.global(w, i) {
            let s = &mut self.sessions[g];
            s.bell = false;
            let c = s.color;
            self.tint
                .set_color(c.unwrap_or(Color::rgb(0x8b, 0x8d, 0x98)));
        }
    }

    /// Close session `g` (an index in `sessions`).
    fn close(&mut self, g: usize) {
        if g < self.sessions.len() {
            let w = self.sessions[g].window;
            let i = self.in_window(w).iter().position(|&x| x == g).unwrap_or(0);
            let s = self.sessions.remove(g);
            self.note(format!("Closed {}", s.name));
            let mut sel = self.selected(w);
            if sel > i || sel >= self.in_window(w).len() {
                sel = sel.saturating_sub(1);
            }
            self.select(w, sel);
        }
    }

    /// Put `session` in window `w` at its tab `to` (past the last when `to`
    /// is the tab count), and select it.
    fn insert(&mut self, w: WindowId, to: usize, mut session: Session) {
        session.window = w;
        let ids = self.in_window(w);
        let at = match ids.get(to) {
            Some(&g) => g,
            None => ids.last().map_or(self.sessions.len(), |&g| g + 1),
        };
        self.sessions.insert(at, session);
        self.select(w, to.min(ids.len()));
    }

    /// Move window `w`'s tab `from` to its tab `to`.
    fn reorder(&mut self, w: WindowId, from: usize, to: usize) {
        if let Some(g) = self.global(w, from) {
            let moved = self.sessions.remove(g);
            self.insert(w, to, moved);
        }
    }

    /// The window whose bar is `bar` (`sessions-<key>`).
    fn window_of_bar(&self, bar: &str) -> Option<WindowId> {
        let key: u64 = bar.strip_prefix("sessions-")?.parse().ok()?;
        self.wins.iter().find(|x| x.key == key).map(|x| x.id)
    }

    /// The tab `from` of bar `bar` was dropped on window `w`'s bar at `to`.
    fn receive(
        &mut self,
        w: WindowId,
        bar: &str,
        from: usize,
        to: usize,
        handle: &AppHandle<State>,
    ) {
        let Some(src) = self.window_of_bar(bar) else {
            return;
        };
        let Some(g) = self.global(src, from) else {
            return;
        };
        let moved = self.sessions.remove(g);
        self.note(format!("Moved {} to another window", moved.name));
        self.insert(w, to, moved);
        self.left(src, handle);
    }

    /// Window `w`'s tab `i` was dragged out of every window and let go at
    /// `at` on the desktop: open a window there for it.
    fn detach(&mut self, w: WindowId, i: usize, at: Option<Point>, handle: &AppHandle<State>) {
        let Some(g) = self.global(w, i) else {
            return;
        };
        let session = self.sessions.remove(g);
        let mut attrs = WindowAttributes::new()
            .title(format!("{} — Stipple Sessions", session.name))
            .logical_size(Size::new(W, H));
        if let Some(at) = at {
            // The tab bar's first tab under the pointer.
            attrs = attrs.position((at.x - 60.0) as i32, (at.y - 40.0) as i32);
        }
        let id = handle.open_window(attrs, view);
        self.note(format!("{} has a window of its own", session.name));
        self.insert(id, 0, session);
        self.left(w, handle);
    }

    /// A tab left window `w`: select another; close the window when it was
    /// the last (but the first window, which stays).
    fn left(&mut self, w: WindowId, handle: &AppHandle<State>) {
        let n = self.in_window(w).len();
        if n == 0 && w != WindowId::PRIMARY {
            self.wins.retain(|x| x.id != w);
            handle.close_window(w);
        } else {
            let sel = self.selected(w).min(n.saturating_sub(1));
            self.select(w, sel);
        }
    }

    /// Window `w` closed: its tabs go back to the first window.
    fn window_closed(&mut self, w: WindowId) {
        if w == WindowId::PRIMARY {
            return;
        }
        for s in self.sessions.iter_mut().filter(|s| s.window == w) {
            s.window = WindowId::PRIMARY;
        }
        self.wins.retain(|x| x.id != w);
    }

    fn index_of(&self, id: u64) -> Option<usize> {
        self.sessions.iter().position(|s| s.id == id)
    }

    fn message(&self, mb: MessageBox) -> MessageBox {
        mb.title("Stipple Sessions").drawn(self.drawn)
    }
}

/// Ask before closing a busy session; close an idle one at once.
fn request_close(s: &mut State, i: usize, handle: &AppHandle<State>) {
    let Some(session) = s.sessions.get(i) else {
        return;
    };
    if !session.busy {
        s.close(i);
        return;
    }
    let id = session.id;
    let mb = s.message(
        MessageBox::new(format!("Close “{}”?", session.name))
            .detail("A process is still running in it; closing the tab ends it.")
            .kind(MessageKind::Warning)
            .buttons(["Close", "Cancel"])
            .cancel(1),
    );
    handle.message(mb, move |s: &mut State, r| {
        if r.button == Some(0)
            && let Some(i) = s.index_of(id)
        {
            s.close(i);
        }
    });
}

/// Ask for session `i`'s new name, in a message box with a text field.
fn ask_rename(s: &mut State, i: usize, handle: &AppHandle<State>) {
    let Some(session) = s.sessions.get(i) else {
        return;
    };
    let id = session.id;
    let mb = s.message(
        MessageBox::new("Rename tab")
            .kind(MessageKind::Question)
            .buttons(["Rename", "Cancel"])
            .cancel(1)
            .input(session.name.clone(), false),
    );
    handle.message(mb, move |st: &mut State, r| {
        if let (Some(0), Some(text)) = (r.button, r.input)
            && let Some(i) = st.index_of(id)
            && !text.trim().is_empty()
        {
            st.sessions[i].name = text.trim().to_string();
        }
    });
}

fn tab_menu(s: &Session, handle: &AppHandle<State>) -> Menu<State> {
    let id = s.id;
    let rename = {
        let handle = handle.clone();
        move |st: &mut State| {
            if let Some(i) = st.index_of(id) {
                ask_rename(st, i, &handle);
            }
        }
    };
    let mut colors = Menu::new("Color");
    for (label, color) in COLORS {
        colors = colors.item(
            MenuItem::new(label, move |st: &mut State| {
                if let Some(i) = st.index_of(id) {
                    st.sessions[i].color = Some(color);
                }
            })
            .checked(s.color == Some(color)),
        );
    }
    colors = colors
        .separator()
        .item(MenuItem::new("None", move |st: &mut State| {
            if let Some(i) = st.index_of(id) {
                st.sessions[i].color = None;
            }
        }));
    let close = {
        let handle = handle.clone();
        move |st: &mut State| {
            if let Some(i) = st.index_of(id) {
                request_close(st, i, &handle);
            }
        }
    };
    Menu::new("")
        .item(MenuItem::new("Rename…", rename))
        .submenu(colors)
        .separator()
        .item(MenuItem::new("Close Others", move |st: &mut State| {
            let Some(w) = st.index_of(id).map(|g| st.sessions[g].window) else {
                return;
            };
            st.sessions.retain(|s| s.id == id || s.window != w);
            st.select(w, 0);
        }))
        .item(MenuItem::new("Close", close))
}

fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    let sp = theme.spacing;
    let handle = cx.handle();

    let w = cx.window_id();
    let key = state.wins.iter().find(|x| x.id == w).map_or(0, |x| x.key);
    let selected = state.selected(w);
    let tabs = state
        .sessions
        .iter()
        .filter(|s| s.window == w)
        .map(|s| {
            Tab::new(s.id, s.name.clone())
                .color(s.color)
                .indicator(if s.bell {
                    Some(TabIndicator::Bell)
                } else if s.busy {
                    Some(TabIndicator::Dot)
                } else {
                    None
                })
                .context_menu(tab_menu(s, &handle))
        })
        .collect();
    let h = handle.clone();
    let mut bar = TabBar::new(format!("sessions-{key}"), tabs)
        .selected(selected)
        .on_select(move |s: &mut State, i| s.select(w, i))
        .on_close(move |s: &mut State, i| {
            if let Some(g) = s.global(w, i) {
                request_close(s, g, &h)
            }
        })
        .on_rename({
            let h = handle.clone();
            move |s: &mut State, i| {
                if let Some(g) = s.global(w, i) {
                    ask_rename(s, g, &h)
                }
            }
        })
        .on_reorder(move |s: &mut State, from, to| s.reorder(w, from, to))
        .on_new(move |s: &mut State| {
            let id = s.next;
            s.next += 1;
            let n = s.in_window(w).len();
            s.insert(
                w,
                n,
                Session {
                    id,
                    window: w,
                    name: format!("shell {id}"),
                    color: None,
                    busy: false,
                    bell: false,
                },
            );
        });
    // Windows of their own where there are several windows.
    if cfg!(not(any(
        target_arch = "wasm32",
        target_os = "ios",
        target_os = "android"
    ))) {
        let (h1, h2) = (handle.clone(), handle.clone());
        bar = bar
            .on_detach(move |s: &mut State, i, at| s.detach(w, i, at, &h1))
            .on_receive(move |s: &mut State, from_bar, from, to| {
                s.receive(w, from_bar, from, to, &h2)
            });
    }
    let bar_state = state
        .wins
        .iter()
        .find(|x| x.id == w)
        .map_or(&state.wins[0].bar, |x| &x.bar);
    let bar = tab_bar(cx, bar, bar_state, move |s| &mut s.win(w).bar);

    let current = state.global(w, selected).map(|g| &state.sessions[g]);
    let title = current.map_or("No session".to_string(), |s| s.name.clone());
    let form = grid(
        vec![Track::px(110.0), Track::fr(1.0)],
        vec![
            label(cx, "Host"),
            text_field(cx, &state.host, |s| &mut s.host).width(260.0),
            label(cx, "Password"),
            password_field(cx, &state.password, |s| &mut s.password).width(260.0),
            label(cx, "Tab color"),
            row(vec![color_button(
                cx,
                &state.tint,
                |s| &mut s.tint,
                move |s, c| {
                    if let Some(g) = s.global(w, s.selected(w)) {
                        s.sessions[g].color = Some(c);
                    }
                },
            )]),
        ],
    )
    .gap(sp.md)
    .align(Align::Start, Align::Center);

    let h = handle.clone();
    let connect = button(cx, "Connect").on_tap(cx, move |s| {
        let mb = if s.password.is_empty() {
            MessageBox::new("A password is needed")
                .detail(format!("Enter the password for {}.", s.host.text()))
                .kind(MessageKind::Error)
        } else {
            MessageBox::new(format!("Connected to {}", s.host.text()))
                .detail(format!(
                    "{} characters sent, never shown.",
                    s.password.text().chars().count()
                ))
                .kind(MessageKind::Info)
        };
        let mb = s.message(mb);
        h.message(mb, |s: &mut State, r| {
            s.note(format!("Message box answered: {:?}", r.button));
        });
        s.password.clear();
    });
    let ring = Button::new("Ring bell")
        .variant(Variant::Secondary)
        .view(cx)
        .on_tap(cx, move |s| {
            let ids = s.in_window(w);
            if ids.len() > 1 {
                let g = ids[(s.selected(w) + 1) % ids.len()];
                s.sessions[g].bell = true;
                let name = s.sessions[g].name.clone();
                s.note(format!("Bell in {name}"));
            }
        });
    let actions = row(vec![connect, ring]).gap(sp.sm);

    let log = column(
        state
            .log
            .iter()
            .map(|l| {
                Element::text(
                    l.clone(),
                    theme.typography.caption,
                    theme.palette.text_muted,
                )
            })
            .collect(),
    )
    .gap(2.0);

    let body = column(vec![
        heading(cx, title),
        form,
        actions,
        spacer().grow(1.0),
        log,
    ])
    .gap(sp.lg)
    .padding(Insets::uniform(sp.lg))
    .fill(theme.palette.surface)
    .grow(1.0);

    column(vec![bar, body])
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

/// Ask a question as soon as the app runs, press its second button after a
/// while, print the answer and quit.
fn alert_selftest(handle: &AppHandle<State>, drawn: bool) {
    let hold = std::env::var("STIPPLE_ALERT_HOLD_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1500);
    let asker = handle.clone();
    handle.after(Duration::from_millis(300), move |s: &mut State| {
        let mb = s.message(
            MessageBox::new("Stipple alert self-test")
                .detail("Answer with a button; the self-test presses “No”.")
                .kind(MessageKind::Question)
                .buttons(["Yes", "No", "Cancel"])
                .cancel(2),
        );
        let exit = asker.clone();
        asker.message(mb, move |_: &mut State, r| {
            println!("sessionsdemo: answered button={:?}", r.button);
            exit.exit();
        });
        println!("sessionsdemo: alert asked (drawn={drawn})");
    });
    handle.after(Duration::from_millis(300 + hold), |_: &mut State| {
        let pressed = dialog::press_native_button(1);
        println!("sessionsdemo: pressed native button: {pressed}");
    });
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let drawn = args.iter().any(|a| a == "--drawn");
    let app = App::new(State::new(drawn), view)
        .on_close_requested(|s: &mut State, w| {
            s.window_closed(w);
            CloseResponse::Close
        })
        .title("Stipple Sessions")
        .theme(theme_from_args())
        .logical_size(Size::new(W, H));
    if args.iter().any(|a| a == "--alert-selftest") {
        alert_selftest(&app.handle(), drawn);
    }
    app.run();
    println!("sessionsdemo: event loop exited");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where CI (visual.yml) clicks: the "build" tab's ×, the color button,
    /// and the drawn alert's "Cancel".
    const CLOSE_BUILD_AT: Point = Point::new(207.0, 21.0);
    const COLOR_AT: Point = Point::new(160.0, 197.0);

    fn app() -> Option<App<State>> {
        let font = Font::system_default()?; // positions depend on text metrics
        let mut app = App::new(State::new(true), view)
            .logical_size(Size::new(W, H))
            .font(font);
        app.render_once();
        Some(app)
    }

    fn find(app: &App<State>, f: impl Fn(&stipple::core::AccessNode) -> bool) -> Option<Rect> {
        let tree = app.accessibility_tree()?;
        tree.descendants()
            .into_iter()
            .find(|n| f(n))
            .map(|n| n.bounds)
    }

    #[test]
    fn closing_the_busy_tab_asks_first() {
        let Some(mut app) = app() else { return };
        let tab = app.find(Role::Tab, "build").expect("the build tab").bounds;
        // The × sits at the tab's end.
        let close = Rect::from_xywh(
            tab.max_x() - 24.0,
            tab.min_y() + 6.0,
            14.0,
            tab.height() - 9.0,
        );
        assert!(close.contains(CLOSE_BUILD_AT), "{tab:?}");
        assert!(app.click_at(CLOSE_BUILD_AT));
        app.run_pending();
        app.render_once();
        assert!(app.click(Role::Button, "Cancel"), "the drawn alert");
        assert_eq!(app.state().sessions.len(), 3);
        // Now press Close (Enter, the default).
        assert!(app.click_at(CLOSE_BUILD_AT));
        app.run_pending();
        app.render_once();
        app.press_key(KeyInput::Enter);
        assert_eq!(app.state().sessions.len(), 2);
        assert_eq!(app.state().sessions[0].name, "prod-db");
    }

    #[test]
    fn the_color_button_sits_where_ci_clicks_and_recolors_the_tab() {
        let Some(mut app) = app() else { return };
        let button = find(&app, |n| n.name.starts_with("Color #")).expect("the color button");
        assert!(button.contains(COLOR_AT), "{button:?}");
        assert!(app.click_at(COLOR_AT));
        assert!(app.state().tint.is_open());
        app.render_once();
        let swatch = app.find_text("#0090ff").expect("a blue swatch");
        assert!(app.click_at(swatch.bounds.center()));
        assert_eq!(
            app.state().sessions[0].color,
            Some(Color::rgb(0, 0x90, 0xff))
        );
    }

    #[test]
    fn connect_without_a_password_shows_an_error() {
        let Some(mut app) = app() else { return };
        let connect = app.find_text("Connect").unwrap();
        assert!(app.click_at(connect.bounds.center()));
        app.run_pending();
        app.render_once();
        assert!(app.find(Role::Dialog, "Stipple Sessions").is_some());
        app.press_key(KeyInput::Escape);
        assert_eq!(
            app.state().log.last().unwrap(),
            "Message box answered: Some(0)"
        );
    }

    #[test]
    fn a_tab_dragged_out_gets_a_window_and_comes_back_when_it_closes() {
        let Some(mut app) = app() else { return };
        let tab = app.find(Role::Tab, "~/src").expect("the ~/src tab").bounds;
        let from = tab.center();
        app.pointer_press(from);
        app.pointer_drag_to(Point::new(from.x + 10.0, from.y));
        app.render_once();
        // Let go below the window.
        let out = Point::new(from.x, H + 120.0);
        app.pointer_drag_to(out);
        app.pointer_release(out);
        let st = app.state();
        let moved = st.sessions.iter().find(|s| s.name == "~/src").unwrap();
        let w = moved.window;
        assert_ne!(w, WindowId::PRIMARY, "a window of its own");
        assert_eq!(st.in_window(WindowId::PRIMARY).len(), 2);
        assert_eq!(st.in_window(w).len(), 1);
        assert_eq!(st.log.last().unwrap(), "~/src has a window of its own");
        // Its window closing brings it back.
        app.handle().update(move |s: &mut State| s.window_closed(w));
        app.run_pending();
        assert_eq!(app.state().in_window(WindowId::PRIMARY).len(), 3);
    }

    #[test]
    fn tabs_move_between_windows_bars() {
        let mut st = State::new(true);
        let handle = AppHandle::new();
        // Detach "prod-db" (the primary window's second tab).
        st.detach(WindowId::PRIMARY, 1, None, &handle);
        let w = st
            .sessions
            .iter()
            .find(|s| s.name == "prod-db")
            .unwrap()
            .window;
        let names = |st: &State, w| -> Vec<String> {
            st.in_window(w)
                .into_iter()
                .map(|g| st.sessions[g].name.clone())
                .collect()
        };
        assert_eq!(names(&st, WindowId::PRIMARY), ["build", "~/src"]);
        // Drop the primary's "~/src" on the new window's bar, before its tab.
        st.receive(w, "sessions-0", 1, 0, &handle);
        assert_eq!(names(&st, w), ["~/src", "prod-db"]);
        assert_eq!(st.selected(w), 0);
        // And back, past "build": the new window keeps one.
        st.receive(
            WindowId::PRIMARY,
            &format!("sessions-{}", st.wins[1].key),
            0,
            1,
            &handle,
        );
        assert_eq!(names(&st, WindowId::PRIMARY), ["build", "~/src"]);
        assert_eq!(names(&st, w), ["prod-db"]);
        // Moving its last tab away closes it.
        st.receive(
            WindowId::PRIMARY,
            &format!("sessions-{}", st.wins[1].key),
            0,
            0,
            &handle,
        );
        assert_eq!(names(&st, WindowId::PRIMARY), ["prod-db", "build", "~/src"]);
        assert_eq!(st.wins.len(), 1);
    }
}

// In the browser, `web.rs` builds this file as a library and the page
// runs `main`'s app (on the web, `App::run` hands it to the page).
stipple::web_app!(main);
