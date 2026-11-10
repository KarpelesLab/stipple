//! An element that takes the keyboard the way a terminal does: every key, raw
//! (`Element::on_raw_key`), Tab included (`captures_tab`), with the input
//! method switched on for it (`ime_input`) and the left Option key as Meta on
//! macOS.
//!
//! Each key prints a line on stdout (`rawkeys: Pressed F5 phys=F5 …`), which
//! the CI jobs check: on X11, `xdotool` types F-keys, Ctrl+letters and Tab;
//! under sway, `wtype` holds a key down and the repeats must arrive. The
//! menu bar's Ctrl+T shortcut (Cmd+T on macOS) still runs its command: menu
//! shortcuts come before the raw handler.

use stipple::prelude::*;

#[derive(Default)]
struct Term {
    /// The last few keys, newest last.
    lines: Vec<String>,
    /// Text an input method or dead key committed (through `on_key`).
    committed: String,
}

const KEEP: usize = 5;

fn mods(m: Modifiers) -> String {
    [
        (m.ctrl, "ctrl+"),
        (m.alt, "alt+"),
        (m.shift, "shift+"),
        (m.meta, "meta+"),
    ]
    .iter()
    .filter(|(on, _)| *on)
    .map(|(_, s)| *s)
    .collect()
}

fn describe(ev: &KeyEvent) -> String {
    let mods = mods(ev.modifiers);
    format!(
        "{:?} {:?} phys={} mods={} sides={:#04x} text={:?} ign={:?} repeat={} loc={:?}",
        ev.state,
        ev.logical,
        ev.physical.name().unwrap_or("?"),
        if mods.is_empty() { "-" } else { &mods },
        ev.sides.0,
        ev.text,
        ev.text_ignoring_modifiers,
        ev.repeat,
        ev.location,
    )
}

fn view(state: &Term, cx: &mut Cx<Term>) -> Element {
    let theme = *cx.theme();
    let mut lines: Vec<Element> = state.lines.iter().map(|l| label(&theme, l)).collect();
    lines.push(label(&theme, format!("> {}", state.committed)));
    let rows = lines.len();
    let terminal = column(lines)
        .padding(Insets::uniform(8.0))
        .fill(theme.palette.surface)
        .width(760.0)
        .height(360.0)
        .on_raw_key(cx, |s: &mut Term, ev: &KeyEvent| {
            println!("rawkeys: {}", describe(ev));
            // The window shows the last few presses (text is slow to draw).
            if ev.is_press() {
                s.lines
                    .push(format!("{}{:?}", mods(ev.modifiers), ev.logical));
                if s.lines.len() > KEEP {
                    s.lines.remove(0);
                }
            }
            Handled::Yes
        })
        // Text from an input method or a dead key comes through `on_key`.
        .on_key(cx, |s: &mut Term, input: &KeyInput| {
            println!("rawkeys: on_key {input:?}");
            if let KeyInput::Text(t) = input {
                s.committed.push_str(t);
            }
        })
        .captures_tab(true)
        .ime_cursor_area(Rect::from_xywh(
            8.0,
            8.0 + 20.0 * (rows - 1) as f64,
            2.0,
            18.0,
        ))
        .option_as_meta(OptionAsMeta::Left);
    panel(
        &theme,
        vec![
            label(&theme, "Raw keys (Tab to focus, then type):"),
            terminal,
        ],
    )
    .padding(Insets::uniform(16.0))
    .align(Align::Start, Align::Stretch)
}

fn main() {
    let mut app = App::new(Term::default(), view)
        .title("Stipple Raw Keys")
        .theme(Theme::dark())
        .logical_size(Size::new(800.0, 440.0))
        .menu(|_: &Term| {
            MenuBar::new().menu(
                Menu::new("&Shell").item(
                    MenuItem::new("New Tab", |_: &mut Term| println!("rawkeys: menu new tab"))
                        .shortcut(Shortcut::cmd('t')),
                ),
            )
        });
    if let Some(font) = Font::system_default() {
        app = app.font(font);
    }
    app.run();
}
