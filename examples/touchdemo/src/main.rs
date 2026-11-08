//! A text field above a list of rows: on a touch screen a finger drag scrolls
//! the list (and flings it), a tap on a row taps it, and the text field brings
//! up the keyboard. With a mouse the wheel scrolls, as everywhere.
//!
//! It logs what happens, on stdout and, on iOS, to `touchdemo.log` in the
//! app's tmp directory, which CI reads back from the simulator:
//!
//! - `touchdemo: scrolled to N` as the list's offset changes;
//! - `touchdemo: row N tapped`;
//! - `touchdemo: text "..."` as the field's text changes.
//!
//! **Self-test (iOS).** Launched with `TOUCHDEMO_SELFTEST` set (`xcrun simctl
//! launch` passes it on as `SIMCTL_CHILD_TOUCHDEMO_SELFTEST=1`), the app drives
//! itself, since a simulator can't script touches or typing: it taps the text
//! field, types through the view's `UIKeyInput` methods (`insertText:` and
//! `deleteBackward`, sent through objc dispatch), drags the list with a
//! synthetic finger through the backend's touch path, then taps outside the
//! field. It logs the keyboard state at each step and ends with
//! `touchdemo: selftest passed` or `touchdemo: selftest FAILED: ...`.

use std::sync::atomic::{AtomicU64, Ordering};
use stipple::prelude::*;

const ROWS: usize = 30;
const PAD: f64 = 16.0;
const FIELD_H: f64 = 44.0;
const LIST_H: f64 = 400.0;

#[derive(Default)]
struct State {
    name: String,
    tapped: Vec<usize>,
}

/// The list's scroll offset in the frame last built (`f64` bits).
static OFFSET: AtomicU64 = AtomicU64::new(0);

fn log(line: &str) {
    println!("{line}");
    if cfg!(target_os = "ios") {
        use std::io::Write;
        let path = std::env::temp_dir().join("touchdemo.log");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{line}");
        }
    }
}

fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    let field = text_field(cx, &theme, &state.name, |s: &mut State, key| {
        edit_string(&mut s.name, key);
        log(&format!("touchdemo: text {:?}", s.name));
    })
    .height(FIELD_H);
    let rows = (0..ROWS)
        .map(|i| {
            button_labeled(&theme, format!("Row {i}"))
                .height(48.0)
                .on_tap(cx, move |s: &mut State| {
                    s.tapped.push(i);
                    log(&format!("touchdemo: row {i} tapped"));
                })
        })
        .collect();
    // A scroll container built by hand (as `scroll` does) to read its offset.
    let id = cx.register_scroll();
    let offset = cx.scroll_offset(id);
    if OFFSET.swap(offset.to_bits(), Ordering::Relaxed) != offset.to_bits() {
        log(&format!("touchdemo: scrolled to {offset:.0}"));
    }
    let list = Element::stack(
        Axis::Vertical,
        vec![column(rows).gap(4.0).align(Align::Start, Align::Stretch)],
    )
    .height(LIST_H)
    .scrollable(id)
    .align(Align::Start, Align::Stretch);
    column(vec![
        field,
        label(&theme, format!("Rows tapped: {}", state.tapped.len())),
        list,
    ])
    .gap(12.0)
    .align(Align::Start, Align::Stretch)
    .padding(Insets::uniform(PAD))
    .grow(1.0)
}

fn main() {
    let mut app = App::new(State::default(), view)
        .title("Stipple Touch")
        .logical_size(Size::new(360.0, 560.0));
    if let Some(font) = Font::system_default() {
        app = app.font(font);
    }
    #[cfg(target_os = "ios")]
    if std::env::var_os("TOUCHDEMO_SELFTEST").is_some() {
        selftest::schedule(&app.handle());
    }
    app.run();
}

#[cfg(target_os = "ios")]
mod selftest {
    use super::*;
    use stipple::platform::TouchPhase;
    use stipple::platform::backend::ios::simulate;

    fn at(ms: u64, handle: &AppHandle<State>, f: impl FnOnce(&mut State) + Send + 'static) {
        handle.after(Duration::from_millis(ms), f);
    }

    fn tap(x: f64, y: f64) {
        simulate::touch(1, TouchPhase::Started, Point::new(x, y));
        simulate::touch(1, TouchPhase::Ended, Point::new(x, y));
    }

    pub fn schedule(handle: &AppHandle<State>) {
        let _ = std::fs::remove_file(std::env::temp_dir().join("touchdemo.log"));
        log("touchdemo: selftest starting");
        // Tap the text field: it takes focus, and the view the keyboard.
        at(1500, handle, |_| {
            log(&format!(
                "touchdemo: keyboard before {}",
                simulate::keyboard_status()
            ));
            tap(120.0, PAD + FIELD_H / 2.0);
        });
        // Type through UIKeyInput: "Hi!", then delete the "!".
        at(2500, handle, |_| {
            log(&format!(
                "touchdemo: keyboard focused {}",
                simulate::keyboard_status()
            ));
            simulate::insert_text("Hi");
            simulate::insert_text("!");
            simulate::delete_backward();
        });
        // Drag the list up 300 points in 160 ms with a second finger id, one
        // move per frame, so it scrolls and then flings.
        let (x, y0, y1, steps) = (120.0, 450.0, 150.0, 10u64);
        at(3500, handle, move |_| {
            simulate::touch(2, TouchPhase::Started, Point::new(x, y0));
        });
        for i in 1..=steps {
            let y = y0 + (y1 - y0) * i as f64 / steps as f64;
            at(3500 + 16 * i, handle, move |_| {
                simulate::touch(2, TouchPhase::Moved, Point::new(x, y));
            });
        }
        at(3500 + 16 * steps + 8, handle, move |_| {
            simulate::touch(2, TouchPhase::Ended, Point::new(x, y1));
        });
        // Tap beside the label, on nothing focusable: focus and the keyboard go.
        at(6000, handle, |_| tap(300.0, PAD + FIELD_H + 12.0 + 8.0));
        at(7000, handle, |s| {
            let keyboard = simulate::keyboard_status();
            log(&format!("touchdemo: keyboard after {keyboard}"));
            let offset = f64::from_bits(OFFSET.load(Ordering::Relaxed));
            let mut failures = Vec::new();
            if s.name != "Hi" {
                failures.push(format!("text {:?}, not \"Hi\"", s.name));
            }
            // 300 points less the slop, then the fling's run.
            if offset < 250.0 {
                failures.push(format!("scrolled only {offset:.0}"));
            }
            if !s.tapped.is_empty() {
                failures.push(format!("rows tapped {:?}", s.tapped));
            }
            if !keyboard.contains("first_responder=false") {
                failures.push("the keyboard stayed".into());
            }
            if failures.is_empty() {
                log(&format!("touchdemo: selftest passed (offset {offset:.0})"));
            } else {
                log(&format!(
                    "touchdemo: selftest FAILED: {}",
                    failures.join("; ")
                ));
            }
        });
    }
}
