//! A terminal-like app's chrome: a document tab bar, a connection form with a
//! password field and a tab color picker, and message boxes.
//!
//! - Tabs select on a click, close with their × (a busy one asks first, in a
//!   message box) and on a middle click, rename on a double click, reorder
//!   by dragging, and have a context menu (Rename…,
//!   a color, Close Others, Close). "+" opens a new one; past what fits, ‹ ›
//!   scroll and ▾ lists them all. With the bar focused (Tab), ←/→ move
//!   between tabs.
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

use stipple::platform::dialog;
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
    name: String,
    color: Option<Color>,
    /// A process runs in it: closing asks first.
    busy: bool,
    bell: bool,
}

struct State {
    sessions: Vec<Session>,
    selected: usize,
    bar: TabBarState,
    next: u64,
    host: EditBuffer,
    password: PasswordField,
    tint: ColorPicker,
    log: Vec<String>,
    drawn: bool,
}

impl State {
    fn new(drawn: bool) -> Self {
        let sessions = vec![
            Session {
                id: 1,
                name: "build".into(),
                color: Some(COLORS[2].1),
                busy: true,
                bell: false,
            },
            Session {
                id: 2,
                name: "prod-db".into(),
                color: Some(COLORS[0].1),
                busy: false,
                bell: true,
            },
            Session {
                id: 3,
                name: "~/src".into(),
                color: None,
                busy: false,
                bell: false,
            },
        ];
        Self {
            tint: ColorPicker::new(COLORS[2].1).with_alpha(false),
            sessions,
            selected: 0,
            bar: TabBarState::new(),
            next: 4,
            host: EditBuffer::from_text("build.example.com"),
            password: PasswordField::new().with_reveal_button(true),
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

    fn select(&mut self, i: usize) {
        self.selected = i.min(self.sessions.len().saturating_sub(1));
        if let Some(s) = self.sessions.get_mut(self.selected) {
            s.bell = false;
            let c = s.color;
            self.tint
                .set_color(c.unwrap_or(Color::rgb(0x8b, 0x8d, 0x98)));
        }
    }

    fn close(&mut self, i: usize) {
        if i < self.sessions.len() {
            let s = self.sessions.remove(i);
            self.note(format!("Closed {}", s.name));
            if self.selected > i || self.selected >= self.sessions.len() {
                self.selected = self.selected.saturating_sub(1);
            }
            let sel = self.selected;
            self.select(sel);
        }
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
            st.sessions.retain(|s| s.id == id);
            st.select(0);
        }))
        .item(MenuItem::new("Close", close))
}

fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    let sp = theme.spacing;
    let handle = cx.handle();

    let tabs = state
        .sessions
        .iter()
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
    let bar = TabBar::new("sessions", tabs)
        .selected(state.selected)
        .on_select(|s: &mut State, i| s.select(i))
        .on_close(move |s: &mut State, i| request_close(s, i, &h))
        .on_rename({
            let h = handle.clone();
            move |s: &mut State, i| ask_rename(s, i, &h)
        })
        .on_reorder(|s: &mut State, from, to| {
            let moved = s.sessions.remove(from);
            s.sessions.insert(to, moved);
            s.selected = to;
        })
        .on_new(|s: &mut State| {
            let id = s.next;
            s.next += 1;
            s.sessions.push(Session {
                id,
                name: format!("shell {id}"),
                color: None,
                busy: false,
                bell: false,
            });
            let last = s.sessions.len() - 1;
            s.select(last);
        });
    let bar = tab_bar(cx, &theme, bar, &state.bar, |s: &mut State| &mut s.bar);

    let current = state.sessions.get(state.selected);
    let title = current.map_or("No session".to_string(), |s| s.name.clone());
    let form = grid(
        vec![Track::px(110.0), Track::fr(1.0)],
        vec![
            label(&theme, "Host"),
            text_editor(cx, &theme, &state.host, |s: &mut State| &mut s.host).width(260.0),
            label(&theme, "Password"),
            password_field(cx, &theme, &state.password, |s: &mut State| &mut s.password)
                .width(260.0),
            label(&theme, "Tab color"),
            row(vec![color_button(
                cx,
                &theme,
                &state.tint,
                |s: &mut State| &mut s.tint,
                |s: &mut State, c| {
                    let i = s.selected;
                    if let Some(session) = s.sessions.get_mut(i) {
                        session.color = Some(c);
                    }
                },
            )]),
        ],
    )
    .gap(sp.md)
    .align(Align::Start, Align::Center);

    let h = handle.clone();
    let connect = button_labeled(&theme, "Connect").on_tap(cx, move |s: &mut State| {
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
    let ring =
        button_variant(&theme, "Ring bell", Variant::Secondary).on_tap(cx, |s: &mut State| {
            let n = s.sessions.len();
            if n > 1 {
                let i = (s.selected + 1) % n;
                s.sessions[i].bell = true;
                let name = s.sessions[i].name.clone();
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
        heading(&theme, title),
        form,
        actions,
        spacer().grow(1.0),
        log,
    ])
    .gap(sp.lg)
    .padding(Insets::uniform(sp.lg))
    .fill(theme.palette.surface)
    .align(Align::Start, Align::Stretch)
    .grow(1.0);

    column(vec![bar, body])
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
    let mut app = App::new(State::new(drawn), view)
        .title("Stipple Sessions")
        .theme(theme_from_args())
        .logical_size(Size::new(W, H));
    if let Some(font) = Font::system_default() {
        app = app.font(font);
    }
    if args.iter().any(|a| a == "--alert-selftest") {
        alert_selftest(&app.handle(), drawn);
    }
    app.run();
    println!("sessionsdemo: event loop exited");
}

#[cfg(test)]
mod tests {
    use super::*;
    use stipple::core::Role;

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
        let tab = find(&app, |n| n.role == Role::Tab && n.name == "build").expect("the build tab");
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
        let cancel =
            find(&app, |n| n.role == Role::Button && n.name == "Cancel").expect("the drawn alert");
        assert!(app.click_at(cancel.center()));
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
        let swatch = find(&app, |n| n.name == "#0090ff").expect("a blue swatch");
        assert!(app.click_at(swatch.center()));
        assert_eq!(
            app.state().sessions[0].color,
            Some(Color::rgb(0, 0x90, 0xff))
        );
    }

    #[test]
    fn connect_without_a_password_shows_an_error() {
        let Some(mut app) = app() else { return };
        let connect = find(&app, |n| n.name == "Connect").unwrap();
        assert!(app.click_at(connect.center()));
        app.run_pending();
        app.render_once();
        assert!(
            find(&app, |n| n.role == Role::Dialog
                && n.name == "Stipple Sessions")
            .is_some()
        );
        app.press_key(KeyInput::Escape);
        assert_eq!(
            app.state().log.last().unwrap(),
            "Message box answered: Some(0)"
        );
    }
}
