//! The system's share sheet: [`share`] hands text, a URL and files to it.
//!
//! | | How | [`ShareOutcome::Shared`] means |
//! |---|---|---|
//! | iOS | `UIActivityViewController`, presented from the top view controller (a popover from [`Share::anchor`] on an iPad) | an activity completed |
//! | Android | `Intent.createChooser` over `ACTION_SEND` (`ACTION_SEND_MULTIPLE` for several files); files go through `MediaStore` (Android 10+) | a target was picked (the chooser's `IntentSender`) |
//! | macOS | `NSSharingServicePicker`, shown from the key window's view | a service was picked |
//! | Web | `navigator.share` | the promise resolved |
//! | Windows, Linux | — ([`ShareOutcome::Unavailable`]) | |
//!
//! See `docs/os-integration.md`.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::geometry::Rect;

/// What to hand to the share sheet: any of a text, a URL and files, with a
/// subject for the targets that take one (mail).
///
/// ```
/// use stipple::platform::Share;
/// let s = Share::new()
///     .text("Scanned with QR Master")
///     .url("https://example.com/item/42")
///     .subject("A code")
///     .file("code.png", vec![0x89, b'P', b'N', b'G'], "image/png");
/// assert_eq!(s.files.len(), 1);
/// assert_eq!(s.files[0].mime, "image/png");
/// ```
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Share {
    /// Plain text; empty for none.
    pub text: String,
    /// A URL; empty for none. Where a target takes one text only (Android's
    /// `EXTRA_TEXT`), it follows the text on a line of its own.
    pub url: String,
    /// The subject line, for mail and the targets that show a title; empty for
    /// none.
    pub subject: String,
    /// Files, each a name, its contents and its MIME type.
    pub files: Vec<ShareFile>,
    /// Where the sheet points from, in the primary window's logical
    /// coordinates: an iPad shows it as a popover from there, macOS's picker
    /// opens there. `None`: the middle of the window.
    pub anchor: Option<Rect>,
}

/// One file to share.
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShareFile {
    /// Its name, with an extension: `code.png`.
    pub name: String,
    /// Its contents.
    pub bytes: Vec<u8>,
    /// Its MIME type: `image/png`. Empty: guessed from the name's extension.
    pub mime: String,
}

impl ShareFile {
    /// A file called `name`, holding `bytes`, of type `mime`.
    pub fn new(name: impl Into<String>, bytes: Vec<u8>, mime: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            bytes,
            mime: mime.into(),
        }
    }

    /// The MIME type, or one guessed from the extension.
    #[cfg_attr(
        not(any(target_os = "android", target_arch = "wasm32", test)),
        allow(dead_code)
    )]
    pub(crate) fn mime_type(&self) -> String {
        if self.mime.is_empty() {
            crate::platform::dialog::mobile::mime_for_name(&self.name).to_string()
        } else {
            self.mime.clone()
        }
    }

    /// The name, made safe as one path component.
    #[cfg_attr(
        not(any(target_os = "ios", target_os = "macos", target_os = "android")),
        allow(dead_code)
    )]
    pub(crate) fn safe_name(&self) -> String {
        let name: String = self
            .name
            .chars()
            .map(|c| {
                if c == '/' || c == '\\' || c.is_control() {
                    '_'
                } else {
                    c
                }
            })
            .collect();
        match name.trim_matches('.') {
            "" => "file".into(),
            _ => name,
        }
    }
}

impl Share {
    /// Nothing to share yet: add a [`text`](Self::text), a [`url`](Self::url)
    /// or [`file`](Self::file)s.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the text.
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = text.into();
        self
    }

    /// Set the URL.
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// Set the subject line.
    pub fn subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = subject.into();
        self
    }

    /// Add a file called `name`, holding `bytes`, of MIME type `mime` (empty
    /// to guess it from the name).
    pub fn file(
        mut self,
        name: impl Into<String>,
        bytes: Vec<u8>,
        mime: impl Into<String>,
    ) -> Self {
        self.files.push(ShareFile::new(name, bytes, mime));
        self
    }

    /// Point the sheet from `rect` (logical coordinates in the primary
    /// window): the button that opened it.
    pub fn anchor(mut self, rect: Rect) -> Self {
        self.anchor = Some(rect);
        self
    }

    /// Whether there's nothing to share.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty() && self.url.is_empty() && self.files.is_empty()
    }

    /// The text and the URL as one text, for targets that take one.
    #[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
    pub(crate) fn combined_text(&self) -> String {
        match (self.text.is_empty(), self.url.is_empty()) {
            (_, true) => self.text.clone(),
            (true, false) => self.url.clone(),
            (false, false) => format!("{}\n{}", self.text, self.url),
        }
    }
}

/// How a [`share`] ended.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShareOutcome {
    /// The user shared it: picked a target, or (on iOS) the target finished.
    Shared,
    /// The user closed the sheet without sharing.
    Cancelled,
    /// There's no share sheet here (Windows, Linux, a browser without
    /// `navigator.share`), no window to show it from, or it couldn't take
    /// what was given. Fall back to copying or saving.
    Unavailable,
}

impl ShareOutcome {
    /// The outcome from its code over the platform glue: 0 shared, 1
    /// cancelled, anything else unavailable.
    pub(crate) fn from_code(code: u32) -> Self {
        match code {
            0 => ShareOutcome::Shared,
            1 => ShareOutcome::Cancelled,
            _ => ShareOutcome::Unavailable,
        }
    }

    pub(crate) fn code(self) -> u32 {
        match self {
            ShareOutcome::Shared => 0,
            ShareOutcome::Cancelled => 1,
            ShareOutcome::Unavailable => 2,
        }
    }
}

/// Show the system's share sheet with `share`, and tell `done` how it ended.
///
/// Call it from the UI thread, from the handler of the button the user
/// pressed (browsers only share during a user gesture). It returns at once;
/// `done` runs later, on whichever thread the system answers on (the UI
/// thread on iOS and macOS, the main Java thread on Android), and exactly
/// once. To change the app's state from it, use
/// [`AppHandle::share`](crate::core::AppHandle::share), which runs the answer
/// as a state update.
///
/// Where there's no share sheet (Windows, Linux), `done` gets
/// [`ShareOutcome::Unavailable`] at once. See `docs/os-integration.md` for what
/// each platform does with files.
///
/// ```no_run
/// use stipple::platform::{Share, ShareOutcome, share};
/// share(&Share::new().text("Hello").url("https://example.com/"), |outcome| {
///     if outcome == ShareOutcome::Unavailable {
///         // copy it to the clipboard instead
///     }
/// });
/// ```
pub fn share(share: &Share, done: impl FnOnce(ShareOutcome) + Send + 'static) {
    if share.is_empty() {
        done(ShareOutcome::Unavailable);
        return;
    }
    let id = register(move |code| done(ShareOutcome::from_code(code)));
    if !os_share(id, share) {
        answer(id, ShareOutcome::Unavailable.code());
    }
}

// ---- Answers waited for --------------------------------------------------------

type Waiter = Box<dyn FnOnce(u32) + Send>;

/// Callbacks waiting for an answer from the system (a share sheet, a
/// permission prompt), by id.
static WAITING: Mutex<Option<(u32, HashMap<u32, Waiter>)>> = Mutex::new(None);

/// Keep `f` until [`answer`] is called with the id returned (never 0).
pub(crate) fn register(f: impl FnOnce(u32) + Send + 'static) -> u32 {
    let mut w = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    let (next, map) = w.get_or_insert_with(|| (1, HashMap::new()));
    let id = *next;
    *next = if *next >= 0xffff { 1 } else { *next + 1 };
    map.insert(id, Box::new(f));
    id
}

/// Answer waiter `id` with `value`, once; later answers are ignored. Safe
/// from any thread.
pub(crate) fn answer(id: u32, value: u32) {
    let f = WAITING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
        .and_then(|(_, map)| map.remove(&id));
    if let Some(f) = f {
        f(value);
    }
}

/// Write `files` into a directory of their own under the temporary
/// directory, for share request `id`, and return their paths (those that
/// could be written). The directories of earlier shares go first: by the
/// next share, whoever took the files has read them.
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub(crate) fn write_files(id: u32, files: &[ShareFile]) -> Vec<std::path::PathBuf> {
    let tmp = std::env::temp_dir();
    let prefix = format!("stipple-share-{}-", std::process::id());
    if let Ok(dir) = std::fs::read_dir(&tmp) {
        for e in dir.flatten() {
            if e.file_name().to_string_lossy().starts_with(&prefix) {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    if files.is_empty() {
        return Vec::new();
    }
    let dir = tmp.join(format!("{prefix}{id}"));
    if std::fs::create_dir_all(&dir).is_err() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (i, f) in files.iter().enumerate() {
        // Two files of the same name go in subdirectories, keeping their names.
        let mut path = dir.join(f.safe_name());
        if out.contains(&path) {
            let sub = dir.join(i.to_string());
            let _ = std::fs::create_dir_all(&sub);
            path = sub.join(f.safe_name());
        }
        if std::fs::write(&path, &f.bytes).is_ok() {
            out.push(path);
        }
    }
    out
}

// ---- Per-platform dispatch ------------------------------------------------------

#[cfg(target_os = "ios")]
fn os_share(id: u32, share: &Share) -> bool {
    super::ios::share(id, share)
}

#[cfg(target_os = "macos")]
fn os_share(id: u32, share: &Share) -> bool {
    super::macos::share(id, share)
}

#[cfg(target_os = "android")]
fn os_share(id: u32, share: &Share) -> bool {
    crate::platform::backend::android::share(id, share.clone())
}

#[cfg(target_arch = "wasm32")]
fn os_share(id: u32, share: &Share) -> bool {
    crate::platform::backend::web::share(id, share);
    true
}

#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_arch = "wasm32"
)))]
fn os_share(_id: u32, _share: &Share) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn waiters_answer_once() {
        let got = Arc::new(AtomicU32::new(99));
        let g = got.clone();
        let id = register(move |v| g.store(v, Ordering::SeqCst));
        assert_ne!(id, 0);
        answer(id, 1);
        answer(id, 0);
        assert_eq!(got.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn text_and_url_combine() {
        assert_eq!(Share::new().text("a").url("b").combined_text(), "a\nb");
        assert_eq!(Share::new().url("b").combined_text(), "b");
        assert_eq!(Share::new().text("a").combined_text(), "a");
        assert!(Share::new().is_empty());
        assert!(Share::new().subject("s").is_empty());
    }

    #[test]
    fn file_names_and_types() {
        let f = ShareFile::new("../a/b.png", vec![], "");
        assert_eq!(f.safe_name(), ".._a_b.png");
        assert_eq!(f.mime_type(), "image/png");
        assert_eq!(ShareFile::new("..", vec![], "x/y").safe_name(), "file");
        assert_eq!(ShareFile::new("..", vec![], "x/y").mime_type(), "x/y");
    }

    #[test]
    fn outcome_codes_round_trip() {
        for o in [
            ShareOutcome::Shared,
            ShareOutcome::Cancelled,
            ShareOutcome::Unavailable,
        ] {
            assert_eq!(ShareOutcome::from_code(o.code()), o);
        }
    }

    #[test]
    fn empty_or_unsupported_shares_answer_at_once() {
        let got = Arc::new(AtomicU32::new(99));
        let g = got.clone();
        share(&Share::new(), move |o| g.store(o.code(), Ordering::SeqCst));
        assert_eq!(got.load(Ordering::SeqCst), 2);
    }
}
