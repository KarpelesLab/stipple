//! Exercises Stipple's native file dialog (`stipple::platform::dialog`), which on
//! Linux drives `org.freedesktop.portal.FileChooser` over the hand-written D-Bus
//! client — no GTK/Qt, no `zbus`/`dbus` crate.
//!
//! Two modes, so CI can verify the whole round-trip headlessly inside a private
//! `dbus-run-session` (no real portal backend required):
//!
//! - `serve`: run the built-in mock portal — own `org.freedesktop.portal.Desktop`,
//!   answer one `OpenFile` with a request handle, then emit the `Response` signal
//!   carrying a canned `file://` URI.
//! - no args (client): call [`dialog::open_file`] and print the chosen path; exit
//!   non-zero if nothing came back.

#[cfg(target_os = "linux")]
fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    if mode == "serve" {
        // The path the mock portal "returns" to whoever opens a file.
        match stipple::platform::a11y::run_mock_file_portal("file:///tmp/stipple-pick.txt") {
            Ok(()) => println!("mock portal served one request"),
            Err(e) => {
                eprintln!("mock portal error: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    use stipple::platform::dialog::{self, FileDialog};
    match dialog::open_file(&FileDialog::new().with_title("Pick a file")) {
        Some(path) => println!("PICKED: {}", path.display()),
        None => {
            eprintln!("no file chosen (cancelled or no portal)");
            std::process::exit(1);
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    // Elsewhere there's no mock to serve: open the real native dialog
    // (`open`, `save` or `folder`, default `open`) and print what was chosen.
    use stipple::platform::dialog::{self, FileDialog};
    let mode = std::env::args().nth(1).unwrap_or_default();
    let picked = match mode.as_str() {
        "save" => dialog::save_file(&FileDialog::new()),
        "folder" => dialog::pick_folder(&FileDialog::new()),
        _ => dialog::open_file(&FileDialog::new().with_title("Pick a file")),
    };
    match picked {
        Some(path) => println!("PICKED: {}", path.display()),
        None => println!("no file chosen (cancelled, or no native dialog here)"),
    }
}
