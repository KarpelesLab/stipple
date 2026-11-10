//! The system clipboard: text, HTML and images.
//!
//! [`text`] reads the clipboard's text and [`set_text`] replaces it; [`read`]
//! and [`write`] do the same with a [`ClipboardContent`], which holds several
//! representations at once (plain text, HTML, an image). Apps rarely call
//! these directly: the `App` already syncs its in-process mirror
//! (`stipple::core::clipboard`) with the OS around each copy, cut and paste,
//! through [`Window::clipboard`] / [`Window::set_clipboard`], and the backends
//! below implement those by calling into this module.
//!
//! - **Windows**: `OpenClipboard` + `CF_UNICODETEXT`, with the text in a
//!   `GlobalAlloc(GMEM_MOVEABLE)` block. Line endings are converted between
//!   `\n` and Windows' `\r\n`. HTML goes in the registered `HTML Format`
//!   (`CF_HTML`, see [`format::cf_html_encode`]); images in `CF_DIBV5` and the
//!   registered `PNG` format, and are read from `PNG`, `CF_DIBV5` or `CF_DIB`.
//! - **macOS**: `NSPasteboard.generalPasteboard`, types `public.utf8-plain-text`
//!   (`NSPasteboardTypeString`), `public.html` and `public.png`.
//! - **iOS**: `UIPasteboard.generalPasteboard`'s `string`.
//! - **Android**: `ClipboardManager`, over JNI through the activity's
//!   `JavaVM`: `getPrimaryClip()` coerced to text, and
//!   `setPrimaryClip(ClipData.newPlainText(..))`. Needs a running activity.
//! - **Web**: [`set_text`] calls `navigator.clipboard.writeText` and also hands
//!   the text to the `copy` / `cut` event that follows Ctrl/Cmd+C or X. The
//!   browser only lets a page read the clipboard from a `paste` event, so
//!   [`text`] returns what the last one carried; Ctrl/Cmd+V reaches the app
//!   through that event, with the text.
//!   [`write`] also hands HTML to the `copy` event; [`read`] returns the HTML
//!   the last paste carried, too. Images aren't supported.
//! - **iOS, Android**: text only.
//! - **Elsewhere**: these return `None` / `false`. On Linux the clipboard
//!   belongs to the display connection, so the backends implement it on their
//!   window ([`Window::read_clipboard`] / [`Window::write_clipboard`]): Wayland
//!   through `wl_data_device` (and the primary selection through
//!   `zwp_primary_selection_device_manager_v1`), X11 through the `CLIPBOARD`
//!   and `PRIMARY` selections.
//!
//! [`ClipboardKind::Primary`] is the X11/Wayland select-to-copy,
//! middle-click-to-paste selection. Elsewhere the `App` keeps it in process.
//!
//! [`Window::clipboard`]: crate::platform::Window::clipboard
//! [`Window::set_clipboard`]: crate::platform::Window::set_clipboard
//! [`Window::read_clipboard`]: crate::platform::Window::read_clipboard
//! [`Window::write_clipboard`]: crate::platform::Window::write_clipboard
//! [`text`]: crate::platform::clipboard::text
//! [`set_text`]: crate::platform::clipboard::set_text

pub mod format;

use crate::render::Pixmap;

/// Which selection a clipboard call is about.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ClipboardKind {
    /// The clipboard proper: Ctrl/Cmd+C, X and V.
    #[default]
    Clipboard,
    /// The X11/Wayland `PRIMARY` selection: whatever text was last selected,
    /// pasted with the middle button. Only X11 and Wayland have one; elsewhere
    /// the `App` keeps it in process.
    Primary,
}

/// What a clipboard holds: any of plain text, HTML and an image, written
/// together so each reader takes the representation it understands.
///
/// ```
/// # use stipple::platform::clipboard::ClipboardContent;
/// let c = ClipboardContent::text("bold").with_html("<b>bold</b>");
/// assert_eq!(c.text.as_deref(), Some("bold"));
/// assert!(c.image.is_none() && !c.is_empty());
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClipboardContent {
    /// Plain text.
    pub text: Option<String>,
    /// HTML: a fragment (`<b>bold</b>`) or a whole document.
    pub html: Option<String>,
    /// An image, as straight-alpha RGBA (it travels as PNG, or a DIB on
    /// Windows). [`Pixmap`] converts to and from `oxideav` frames.
    pub image: Option<Pixmap>,
}

impl ClipboardContent {
    /// Plain text only.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            ..Self::default()
        }
    }

    /// An image only.
    pub fn image(image: Pixmap) -> Self {
        Self {
            image: Some(image),
            ..Self::default()
        }
    }

    /// Add (or replace) the HTML representation.
    pub fn with_html(mut self, html: impl Into<String>) -> Self {
        self.html = Some(html.into());
        self
    }

    /// Add (or replace) the plain-text representation.
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// Add (or replace) the image.
    pub fn with_image(mut self, image: Pixmap) -> Self {
        self.image = Some(image);
        self
    }

    /// Whether it holds nothing at all.
    pub fn is_empty(&self) -> bool {
        self.text.is_none() && self.html.is_none() && self.image.is_none()
    }

    /// Keep only the representations `formats` asks for.
    pub fn only(mut self, formats: ClipboardFormats) -> Self {
        if !formats.text {
            self.text = None;
        }
        if !formats.html {
            self.html = None;
        }
        if !formats.image {
            self.image = None;
        }
        self
    }
}

/// The representations to read from the clipboard. Reading only what is
/// needed matters: an image can be megabytes another app has to encode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ClipboardFormats {
    pub text: bool,
    pub html: bool,
    pub image: bool,
}

impl ClipboardFormats {
    /// Plain text only (what a paste into a text field needs).
    pub const TEXT: Self = Self {
        text: true,
        html: false,
        image: false,
    };
    /// Every representation.
    pub const ALL: Self = Self {
        text: true,
        html: true,
        image: true,
    };
}

/// The answer to [`Window::read_clipboard`](crate::platform::Window::read_clipboard).
#[derive(Clone, Debug, PartialEq)]
pub enum ClipboardRead {
    /// The content, there and then (`None` when the clipboard is empty or
    /// holds none of the formats asked for).
    Ready(Option<ClipboardContent>),
    /// The owner has been asked; the content arrives later as
    /// [`Event::Clipboard`](crate::platform::Event::Clipboard). X11 answers
    /// this way when another client owns the selection.
    Pending,
}

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

#[cfg(target_arch = "wasm32")]
mod os {
    use super::{ClipboardContent, ClipboardFormats};
    pub fn text() -> Option<String> {
        crate::platform::backend::web::clipboard_read()
    }
    pub fn set_text(text: &str) -> bool {
        crate::platform::backend::web::clipboard_write(text, None);
        true
    }
    pub fn read(formats: ClipboardFormats) -> Option<ClipboardContent> {
        let c = ClipboardContent {
            text: text(),
            html: crate::platform::backend::web::clipboard_read_html(),
            image: None,
        }
        .only(formats);
        (!c.is_empty()).then_some(c)
    }
    pub fn write(content: &ClipboardContent) -> bool {
        let text = content.text.as_deref().unwrap_or("");
        crate::platform::backend::web::clipboard_write(text, content.html.as_deref());
        content.text.is_some() || content.html.is_some()
    }
}

#[cfg(target_os = "android")]
mod os {
    pub fn text() -> Option<String> {
        crate::platform::backend::android::clipboard_text()
    }
    pub fn set_text(text: &str) -> bool {
        crate::platform::backend::android::set_clipboard_text(text)
    }
}

#[cfg(not(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "ios",
    target_arch = "wasm32",
    target_os = "android"
)))]
mod os {
    pub fn text() -> Option<String> {
        None
    }
    pub fn set_text(_text: &str) -> bool {
        false
    }
}

/// Text-only platforms read and write through `text` / `set_text`.
#[cfg(not(any(target_os = "windows", target_os = "macos", target_arch = "wasm32")))]
mod rich {
    use super::{ClipboardContent, ClipboardFormats, os};
    pub fn read(formats: ClipboardFormats) -> Option<ClipboardContent> {
        formats
            .text
            .then(os::text)
            .flatten()
            .map(ClipboardContent::text)
    }
    pub fn write(content: &ClipboardContent) -> bool {
        content.text.as_deref().is_some_and(os::set_text)
    }
}
#[cfg(any(target_os = "windows", target_os = "macos", target_arch = "wasm32"))]
use os as rich;

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

/// Read the system clipboard in the representations `formats` asks for.
/// `None` when it holds none of them, or where there is no process-wide
/// clipboard API (see the module docs). Images are read on Windows and macOS,
/// HTML on Windows, macOS and the web.
pub fn read(formats: ClipboardFormats) -> Option<ClipboardContent> {
    rich::read(formats)
}

/// Replace the system clipboard's contents with every representation in
/// `content`. Returns whether the OS accepted it. Platforms that only take
/// text write `content.text`.
pub fn write(content: &ClipboardContent) -> bool {
    rich::write(content)
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
