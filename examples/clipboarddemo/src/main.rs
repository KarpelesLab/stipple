//! Copy/cut/paste in a caret-aware text field. The field starts with "Clip";
//! CI focuses it, selects all (Ctrl+A), copies (Ctrl+C), moves to the end, and
//! pastes (Ctrl+V) — doubling the text to "ClipClip" — then screenshots before
//! and after to confirm the paste changed the field. The copied text is also
//! pushed to the OS clipboard (verified separately via xclip). Under sway, CI
//! copies out of the field and pastes into it with `wl-copy` / `wl-paste`.
//! Every change to the field is printed as `field: <text>`, so CI can check
//! what a paste (Ctrl+V, or a middle click with the primary selection) put
//! there.
//!
//! Other modes, for CI:
//!
//! - `--roundtrip` skips the window: it writes a string to the system
//!   clipboard with `stipple::platform::clipboard`, reads it back, and prints
//!   the result (exiting non-zero on a mismatch). `--roundtrip-rich` does the
//!   same with text + HTML + an image (Windows and macOS), and leaves them on
//!   the clipboard for the OS's own tools to check.
//! - `--read` skips the window: it reads the system clipboard (every format)
//!   and prints what it found.
//! - `--copy-rich` opens the window; Ctrl+C in the field puts text, HTML and a
//!   400×300 image on the clipboard (large enough that X11 sends it in `INCR`
//!   chunks), for other applications to read while it runs.
//! - `--paste-rich` opens the window, reads the clipboard (every format)
//!   through the app, prints what it got, and exits. `--paste-primary` does
//!   the same with the primary selection.

use stipple::platform::clipboard::{self, ClipboardContent, ClipboardFormats, ClipboardKind};
use stipple::prelude::*;
use stipple::widgets::EditBuffer;

#[derive(Default)]
struct State {
    buffer: EditBuffer,
    /// The text last printed.
    shown: std::cell::RefCell<String>,
    /// `--copy-rich`: Ctrl+C in the field copies [`rich`] content.
    rich: bool,
}

fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let text = state.buffer.text();
    if *state.shown.borrow() != text {
        println!("field: {text}");
        *state.shown.borrow_mut() = text.to_string();
    }
    let theme = *cx.theme();
    let field = if state.rich {
        // A field whose copy puts text, HTML and an image on the clipboard.
        text_field(cx, &theme, text, |s: &mut State, key: &KeyInput| {
            if *key == KeyInput::Copy {
                stipple::core::write_clipboard(ClipboardKind::Clipboard, rich());
                println!("copied rich");
            } else {
                s.buffer.apply(key);
            }
        })
    } else {
        text_editor(cx, &theme, &state.buffer, |s: &mut State| &mut s.buffer)
    }
    .width(560.0)
    .height(64.0);
    panel(
        &theme,
        vec![label(&theme, "Clipboard (Tab to focus):"), field],
    )
    .padding(Insets::uniform(32.0))
    .align(Align::Start, Align::Stretch)
}

/// The rich content `--copy-rich` and `--roundtrip` write: text, HTML and a
/// noisy (barely compressible) opaque image.
fn rich() -> ClipboardContent {
    let (w, h) = (400u32, 300u32);
    let mut x = 0x1234_5678u32;
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..w * h {
        x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
        let [r, g, b, _] = x.to_le_bytes();
        px.extend_from_slice(&[r, g, b, 255]);
    }
    ClipboardContent::text("stipple rich clipboard")
        .with_html("<b>stipple</b> rich <i>clipboard</i>")
        .with_image(Pixmap::from_rgba8(PhysicalSize::new(w, h), px))
}

/// One line describing `content`.
fn describe(content: Option<&ClipboardContent>) -> String {
    let Some(c) = content else {
        return "nothing".into();
    };
    let image = c
        .image
        .as_ref()
        .map(|i| format!("{}x{}", i.size().width, i.size().height));
    format!(
        "text={:?} html={:?} image={}",
        c.text.as_deref().unwrap_or(""),
        c.html.as_deref().unwrap_or(""),
        image.as_deref().unwrap_or("none")
    )
}

/// Set the system clipboard, read it back, and report.
fn roundtrip() {
    let sent = "stipple clipboard \u{2713} round-trip";
    let set = clipboard::set_text(sent);
    let got = clipboard::text();
    println!("clipboard set: {set}");
    println!("clipboard read: {got:?}");
    if got.as_deref() == Some(sent) {
        println!("clipboard roundtrip: ok");
    } else {
        eprintln!("clipboard roundtrip: mismatch (sent {sent:?})");
        std::process::exit(1);
    }
}

/// Write text, HTML and an image to the system clipboard, read them back, and
/// report. (Windows and macOS; elsewhere the process-wide API takes text only.)
fn roundtrip_rich() {
    let content = rich();
    let set = clipboard::write(&content);
    let got = clipboard::read(ClipboardFormats::ALL);
    println!("rich set: {set}");
    println!("rich read: {}", describe(got.as_ref()));
    let got = got.unwrap_or_default();
    let html_ok = got
        .html
        .as_deref()
        .is_some_and(|h| h.contains("<b>stipple</b> rich"));
    if set && got.text == content.text && html_ok && got.image == content.image {
        println!("rich roundtrip: ok");
    } else {
        eprintln!("rich roundtrip: mismatch");
        std::process::exit(1);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    if has("--roundtrip") {
        roundtrip();
        return;
    }
    if has("--roundtrip-rich") {
        roundtrip_rich();
        return;
    }
    if has("--read") {
        println!(
            "read: {}",
            describe(clipboard::read(ClipboardFormats::ALL).as_ref())
        );
        return;
    }
    let mut app = App::new(
        State {
            buffer: EditBuffer::from_text("Clip"),
            rich: has("--copy-rich"),
            ..State::default()
        },
        view,
    )
    .title("Stipple Clipboard")
    .theme(Theme::dark())
    .logical_size(Size::new(640.0, 240.0));
    if let Some(font) = Font::system_default() {
        app = app.font(font);
    }
    let paste = if has("--paste-rich") {
        Some(ClipboardKind::Clipboard)
    } else if has("--paste-primary") {
        Some(ClipboardKind::Primary)
    } else {
        None
    };
    if let Some(kind) = paste {
        // Ask every second until something comes (Wayland only tells the
        // client with keyboard focus about the selection), ten times at most.
        app.handle()
            .every(std::time::Duration::from_secs(1), move |_| {
                stipple::core::request_clipboard(kind, ClipboardFormats::ALL);
            });
        let mut tries = 0;
        app = app.on_clipboard(move |_state, kind, content| {
            tries += 1;
            if content.is_some() || tries == 10 {
                println!("pasted {kind:?}: {}", describe(content.as_ref()));
                std::process::exit(0);
            }
        });
    }
    app.run();
}
