//! Handing things to the rest of the desktop: opening URLs and files in the
//! default app, desktop notifications, and the event plumbing behind tray
//! icons ([`Window::set_tray`](crate::platform::Window::set_tray)).
//!
//! | | [`open_url`] | [`notify`] | tray icon |
//! |---|---|---|---|
//! | Linux | the XDG portal's `OpenURI` over D-Bus, else `xdg-open` | `org.freedesktop.Notifications` over D-Bus | `StatusNotifierItem` + `com.canonical.dbusmenu` over D-Bus |
//! | Windows | `ShellExecuteW` | a `Shell_NotifyIconW` balloon (a toast on Windows 10 and 11) | `Shell_NotifyIconW` + a popup `HMENU` |
//! | macOS | `NSWorkspace openURL:` | `osascript`'s `display notification` | `NSStatusItem` + `NSMenu` |
//! | iOS | `UIApplication openURL:` | `UNUserNotificationCenter` (asking permission first) | — |
//! | Android | an `ACTION_VIEW` `Intent` over JNI | `NotificationManager` (asking for `POST_NOTIFICATIONS` first on Android 13+) | — |
//! | Web | `window.open` | the `Notification` API | — |
//!
//! See `docs/os-integration.md`.

use crate::platform::PlatformError;
use crate::platform::event::Event;
use std::path::Path;
use std::sync::Mutex;

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
mod linux;
#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
pub(crate) mod sni;

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod macos;

#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
pub(crate) mod ios;

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod windows;

/// A desktop notification: a title and an optional body.
///
/// ```
/// use stipple::platform::Notification;
/// let n = Notification::new("Download complete").body("notes.txt is ready");
/// assert_eq!(n.title, "Download complete");
/// assert_eq!(n.body, "notes.txt is ready");
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Notification {
    pub title: String,
    pub body: String,
}

impl Notification {
    /// A notification titled `title`, with no body.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: String::new(),
        }
    }

    /// Set the body text.
    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }
}

/// Open `url` with the user's default handler for its scheme: a browser for
/// `https:`, the mail client for `mailto:`, the default app for a `file:` URL.
///
/// The URL must start with a scheme (`https:`, `file:`…); anything else is
/// refused, so a string from the outside can't pass itself off as an option to
/// the program that opens it. Where opening is asynchronous (Linux, Android,
/// the web) `Ok` means the request was handed over, not that an app opened.
///
/// ```no_run
/// stipple::platform::open_url("https://example.com/").unwrap();
/// ```
pub fn open_url(url: &str) -> Result<(), PlatformError> {
    if !has_scheme(url) || url.chars().any(char::is_control) {
        return Err(PlatformError::Os(format!("not a URL: {url:?}")));
    }
    os_open_url(url)
}

/// Open the file or folder at `path` with its default app, as
/// [`open_url`] does for its `file:` URL.
pub fn open_path(path: impl AsRef<Path>) -> Result<(), PlatformError> {
    let path = path.as_ref();
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| PlatformError::Os(e.to_string()))?
            .join(path)
    };
    open_url(&file_url(&abs))
}

/// Show `notification` as a desktop notification.
///
/// Where it's delivered asynchronously (Linux, iOS, Android, the web) `Ok`
/// means it was handed over; a failure is reported on stderr (the console on
/// the web). On iOS and Android 13 and later the first call asks the user's
/// permission, and notifications wait for the answer; once refused, `notify`
/// fails. On Android the app's manifest must declare
/// `android.permission.POST_NOTIFICATIONS`.
pub fn notify(notification: &Notification) -> Result<(), PlatformError> {
    os_notify(notification)
}

/// Whether the app's tray icon is currently shown (the last
/// [`Window::set_tray`](crate::platform::Window::set_tray) through the `App`
/// succeeded).
pub fn tray_shown() -> bool {
    TRAY_SHOWN.load(std::sync::atomic::Ordering::Acquire)
}

static TRAY_SHOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn set_tray_shown(shown: bool) {
    TRAY_SHOWN.store(shown, std::sync::atomic::Ordering::Release);
}

/// Events raised away from the backend's own loop (a tray menu pick heard on
/// the D-Bus thread), delivered ahead of the next [`Event::Wake`].
static PENDING: Mutex<Vec<Event>> = Mutex::new(Vec::new());

/// Queue `event` for the event loop and wake it. Safe from any thread.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn post(event: Event) {
    if let Ok(mut p) = PENDING.lock() {
        p.push(event);
    }
    crate::platform::wake::wake();
}

/// The events [`post`]ed since the last call.
pub(crate) fn take_posted() -> Vec<Event> {
    PENDING
        .lock()
        .map(|mut p| std::mem::take(&mut *p))
        .unwrap_or_default()
}

/// Whether `s` starts with a URI scheme (RFC 3986: a letter, then letters,
/// digits, `+`, `-` or `.`, then `:`). A one-letter "scheme" is taken for a
/// Windows drive (`C:\…`) and refused.
fn has_scheme(s: &str) -> bool {
    let Some((scheme, _)) = s.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    scheme.len() > 1
        && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// The `file://` URL of absolute `path`, percent-encoding everything but
/// unreserved characters and `/`. Windows paths (`C:\a b`) become
/// `file:///C:/a%20b`.
pub(crate) fn file_url(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://");
    if !s.starts_with('/') {
        out.push('/');
    }
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char)
            }
            // Keep a drive letter's colon readable.
            b':' if cfg!(windows) => out.push(':'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ---- Per-platform dispatch ---------------------------------------------------

#[cfg(target_os = "linux")]
use linux::{notify as os_notify, open_url as os_open_url};

#[cfg(target_os = "macos")]
use macos::{notify as os_notify, open_url as os_open_url};

#[cfg(target_os = "windows")]
use windows::{notify as os_notify, open_url as os_open_url};

#[cfg(target_os = "ios")]
use ios::{notify as os_notify, open_url as os_open_url};

#[cfg(target_os = "android")]
fn os_open_url(url: &str) -> Result<(), PlatformError> {
    if crate::platform::backend::android::open_url(url) {
        Ok(())
    } else {
        Err(PlatformError::Os("no activity could open the URL".into()))
    }
}

#[cfg(target_arch = "wasm32")]
fn os_open_url(url: &str) -> Result<(), PlatformError> {
    crate::platform::backend::web::open_url(url);
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn os_notify(n: &Notification) -> Result<(), PlatformError> {
    crate::platform::backend::web::notify(&n.title, &n.body);
    Ok(())
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "ios",
    target_os = "android",
    target_arch = "wasm32"
)))]
fn os_open_url(_url: &str) -> Result<(), PlatformError> {
    Err(PlatformError::NoBackend("opening URLs"))
}

#[cfg(target_os = "android")]
use crate::platform::backend::android::notify as os_notify;

#[cfg(not(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "ios",
    target_os = "android",
    target_arch = "wasm32"
)))]
fn os_notify(_n: &Notification) -> Result<(), PlatformError> {
    Err(PlatformError::NoBackend("notifications"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_need_a_scheme() {
        assert!(has_scheme("https://example.com"));
        assert!(has_scheme("mailto:a@b.c"));
        assert!(has_scheme("x-stipple+test.1:go"));
        assert!(!has_scheme("example.com"));
        assert!(!has_scheme("-o/etc/passwd"));
        assert!(!has_scheme("C:\\Users"));
        assert!(!has_scheme("1http://x"));
        assert!(!has_scheme(""));
        assert!(open_url("--help").is_err());
        assert!(open_url("https://a\n-b").is_err());
    }

    #[test]
    fn file_urls_are_percent_encoded() {
        assert_eq!(
            file_url(Path::new("/tmp/a b/ü.txt")),
            "file:///tmp/a%20b/%C3%BC.txt"
        );
        assert_eq!(file_url(Path::new("/x#y?z")), "file:///x%23y%3Fz");
    }

    #[test]
    fn posted_events_are_taken_once() {
        post(Event::TrayActivated);
        let got = take_posted();
        assert!(got.contains(&Event::TrayActivated));
        assert!(!take_posted().contains(&Event::TrayActivated));
    }
}
