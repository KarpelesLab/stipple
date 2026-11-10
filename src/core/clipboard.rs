//! A process-local clipboard mirror, and clipboard requests from handlers.
//!
//! Text editing (copy/cut/paste) reads and writes this in-process string, so it
//! works on its own (within one app, and headlessly) with no platform coupling.
//! The platform layer syncs it with the OS clipboard around copy/paste: it
//! pushes the mirror to the OS after a copy/cut and pulls the OS clipboard into
//! the mirror before a paste (see the app's key handling + `Window::clipboard`).
//!
//! The primary selection (select to copy, middle-click to paste) has a mirror
//! of its own, which the `App` keeps equal to the focused text field's
//! selection; where the platform has a primary selection (X11, Wayland) the
//! `App` also owns it there.
//!
//! Handlers that aren't text fields (a terminal, a canvas) put rich content on
//! the clipboard with [`write_clipboard`] and ask for it with
//! [`request_clipboard`]; the `App` carries both out after the handler returns
//! and hands what it read to the function set with `App::on_clipboard`.

use std::cell::RefCell;

use crate::core::runtime::{FocusId, LayoutNode, NodeContent, find_focus, first_text};
use crate::platform::clipboard::{ClipboardContent, ClipboardFormats, ClipboardKind};

/// A clipboard operation queued by a handler, for the `App` to carry out.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ClipboardRequest {
    Write(ClipboardKind, ClipboardContent),
    Read(ClipboardKind, ClipboardFormats),
}

thread_local! {
    static CLIPBOARD: RefCell<String> = const { RefCell::new(String::new()) };
    static PRIMARY: RefCell<String> = const { RefCell::new(String::new()) };
    static REQUESTS: RefCell<Vec<ClipboardRequest>> = const { RefCell::new(Vec::new()) };
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

/// The primary selection's text, as last selected in this app (the in-process
/// mirror).
pub fn primary_text() -> String {
    PRIMARY.with(|c| c.borrow().clone())
}

/// Replace the primary selection's text (the in-process mirror only; see
/// [`write_clipboard`] to reach the system's).
pub fn set_primary_text(text: &str) {
    PRIMARY.with(|c| {
        let mut s = c.borrow_mut();
        s.clear();
        s.push_str(text);
    });
}

/// Put `content` on the clipboard (or the primary selection), every
/// representation it has: text, HTML, an image. Call it from any handler; the
/// running `App` hands it to the system once the handler returns. The text,
/// if any, also replaces the in-process mirror at once.
///
/// ```
/// # use stipple::core::write_clipboard;
/// # use stipple::platform::clipboard::{ClipboardContent, ClipboardKind};
/// write_clipboard(
///     ClipboardKind::Clipboard,
///     ClipboardContent::text("bold").with_html("<b>bold</b>"),
/// );
/// assert_eq!(stipple::core::clipboard_text(), "bold");
/// ```
pub fn write_clipboard(kind: ClipboardKind, content: ClipboardContent) {
    if let Some(text) = &content.text {
        match kind {
            ClipboardKind::Clipboard => set_clipboard_text(text),
            ClipboardKind::Primary => set_primary_text(text),
        }
    }
    REQUESTS.with(|r| r.borrow_mut().push(ClipboardRequest::Write(kind, content)));
}

/// Ask for the clipboard's (or the primary selection's) content in `formats`.
/// The running `App` reads it once the handler returns, which may take a
/// moment when another application owns it, and passes it to the function set
/// with `App::on_clipboard` (`None` when there was nothing in those formats).
pub fn request_clipboard(kind: ClipboardKind, formats: ClipboardFormats) {
    REQUESTS.with(|r| r.borrow_mut().push(ClipboardRequest::Read(kind, formats)));
}

/// Whether a handler queued a write to the clipboard proper (which then
/// replaces the plain-text copy the `App` would make).
pub(crate) fn write_queued() -> bool {
    REQUESTS.with(|r| {
        r.borrow()
            .iter()
            .any(|q| matches!(q, ClipboardRequest::Write(ClipboardKind::Clipboard, _)))
    })
}

/// Take the queued requests, oldest first.
pub(crate) fn take_requests() -> Vec<ClipboardRequest> {
    REQUESTS.with(|r| std::mem::take(&mut *r.borrow_mut()))
}

/// What the in-process mirror holds for `kind`, in `formats` (text only).
pub(crate) fn mirror(kind: ClipboardKind, formats: ClipboardFormats) -> Option<ClipboardContent> {
    let text = match kind {
        ClipboardKind::Clipboard => clipboard_text(),
        ClipboardKind::Primary => primary_text(),
    };
    (formats.text && !text.is_empty()).then(|| ClipboardContent::text(text))
}

/// The text selected in the focused text field, if it has a selection.
pub(crate) fn selection_text(tree: &LayoutNode, focused: Option<FocusId>) -> Option<String> {
    let leaf = first_text(find_focus(tree, focused?)?)?;
    let (Some((s, e)), NodeContent::Text { text, .. }) = (leaf.selection, &leaf.content) else {
        return None;
    };
    // Only an editable leaf (one with a caret) has a selection of its own.
    leaf.caret?;
    let (s, e) = (s.min(e), s.max(e).min(text.len()));
    (s < e)
        .then(|| text.get(s..e).map(str::to_string))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_update_the_mirrors_and_queue() {
        take_requests();
        write_clipboard(ClipboardKind::Primary, ClipboardContent::text("sel"));
        request_clipboard(ClipboardKind::Clipboard, ClipboardFormats::ALL);
        assert_eq!(primary_text(), "sel");
        assert_eq!(
            take_requests(),
            vec![
                ClipboardRequest::Write(ClipboardKind::Primary, ClipboardContent::text("sel")),
                ClipboardRequest::Read(ClipboardKind::Clipboard, ClipboardFormats::ALL),
            ]
        );
        assert!(take_requests().is_empty());
        assert_eq!(
            mirror(ClipboardKind::Primary, ClipboardFormats::TEXT),
            Some(ClipboardContent::text("sel"))
        );
        let image_only = ClipboardFormats {
            image: true,
            ..ClipboardFormats::default()
        };
        assert_eq!(mirror(ClipboardKind::Primary, image_only), None);
    }
}
