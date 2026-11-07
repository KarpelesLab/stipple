//! A process-local clipboard mirror.
//!
//! Text editing (copy/cut/paste) reads and writes this in-process string, so it
//! works on its own (within one app, and headlessly) with no platform coupling.
//! The platform layer syncs it with the OS clipboard around copy/paste: it
//! pushes the mirror to the OS after a copy/cut and pulls the OS clipboard into
//! the mirror before a paste (see the app's key handling + `Window::clipboard`).

use std::cell::RefCell;

thread_local! {
    static CLIPBOARD: RefCell<String> = const { RefCell::new(String::new()) };
}

/// The current clipboard text (the in-process mirror).
pub fn clipboard_text() -> String {
    CLIPBOARD.with(|c| c.borrow().clone())
}

/// Replace the clipboard text (the in-process mirror).
pub fn set_clipboard_text(text: &str) {
    CLIPBOARD.with(|c| {
        let mut s = c.borrow_mut();
        s.clear();
        s.push_str(text);
    });
}
