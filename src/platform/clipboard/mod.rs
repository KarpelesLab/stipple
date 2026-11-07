//! The system clipboard (plain text).
//!
//! [`text`] reads the clipboard and [`set_text`] replaces it. Apps rarely call
//! these directly: the `App` already syncs its in-process mirror
//! (`stipple::core::clipboard`) with the OS around each copy, cut and paste,
//! through [`Window::clipboard`] / [`Window::set_clipboard`], and the backends
//! below implement those by calling into this module.
//!
//! - **Windows**: `OpenClipboard` + `CF_UNICODETEXT`, with the text in a
//!   `GlobalAlloc(GMEM_MOVEABLE)` block. Line endings are converted between
//!   `\n` and Windows' `\r\n`.
//! - **macOS**: `NSPasteboard.generalPasteboard`, type `public.utf8-plain-text`
//!   (`NSPasteboardTypeString`).
//! - **iOS**: `UIPasteboard.generalPasteboard`'s `string`.
//! - **Elsewhere**: these return `None` / `false`. On Linux the clipboard
//!   belongs to the display connection, so the X11 backend implements it on its
//!   window (the `CLIPBOARD` selection); Wayland, Android and the web don't
//!   have it yet.
//!
//! [`Window::clipboard`]: crate::platform::Window::clipboard
//! [`Window::set_clipboard`]: crate::platform::Window::set_clipboard

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
#[path = "windows.rs"]
mod os;

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
#[path = "macos.rs"]
mod os;

#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[path = "ios.rs"]
mod os;

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "ios")))]
mod os {
    pub fn text() -> Option<String> {
        None
    }
    pub fn set_text(_text: &str) -> bool {
        false
    }
}

/// Read the system clipboard's text. `None` when it holds no text, or when
/// this platform has no process-wide clipboard API (see the module docs).
pub fn text() -> Option<String> {
    os::text()
}

/// Replace the system clipboard's contents with `text`. Returns whether the OS
/// accepted it (`false` where there is no process-wide clipboard API).
pub fn set_text(text: &str) -> bool {
    os::set_text(text)
}

/// Windows line endings for the clipboard: every `\n` not already preceded by
/// `\r` becomes `\r\n`.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
fn to_crlf(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 16);
    let mut prev = '\0';
    for c in text.chars() {
        if c == '\n' && prev != '\r' {
            out.push('\r');
        }
        out.push(c);
        prev = c;
    }
    out
}

/// The inverse of [`to_crlf`]: `\r\n` becomes `\n` (a lone `\r` is kept).
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
fn from_crlf(text: &str) -> String {
    text.replace("\r\n", "\n")
}

#[cfg(test)]
mod tests {
    use super::{from_crlf, to_crlf};

    #[test]
    fn line_endings_round_trip() {
        assert_eq!(to_crlf("a\nb\r\nc"), "a\r\nb\r\nc");
        assert_eq!(from_crlf("a\r\nb\rc"), "a\nb\rc");
        for s in ["", "one", "a\nb", "\n\n", "x\r\ny"] {
            assert_eq!(from_crlf(&to_crlf(s)), s.replace("\r\n", "\n"));
        }
    }
}
