//! Linux: opening URLs through the XDG desktop portal (`xdg-open` as the
//! fallback) and notifications through `org.freedesktop.Notifications`, both
//! over the hand-written D-Bus client in [`crate::platform::a11y`].

use super::Notification;
use crate::platform::PlatformError;
use crate::platform::a11y::DBus;
use std::time::Duration;

/// How long to wait for a D-Bus service (possibly started on demand) to answer.
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

pub fn open_url(url: &str) -> Result<(), PlatformError> {
    let url = url.to_string();
    std::thread::Builder::new()
        .name("stipple-open-url".into())
        .spawn(move || {
            // The portal's OpenURI refuses `file:` URLs (those go through
            // OpenFile, with a file descriptor); xdg-open handles them, and is
            // itself a portal client inside a sandbox.
            if !url.starts_with("file:") {
                match portal_open_uri(&url) {
                    Ok(()) => return,
                    Err(e) => debug(format_args!("portal OpenURI failed ({e}); trying xdg-open")),
                }
            }
            if let Err(e) = run_opener(&url) {
                eprintln!("stipple: couldn't open {url}: {e}");
            }
        })
        .map_err(|e| PlatformError::Os(e.to_string()))?;
    Ok(())
}

pub fn notify(n: &Notification) -> Result<(), PlatformError> {
    let n = n.clone();
    std::thread::Builder::new()
        .name("stipple-notify".into())
        .spawn(move || {
            if let Err(e) = send_notification(&n) {
                eprintln!("stipple: notification not shown: {e}");
            }
        })
        .map_err(|e| PlatformError::Os(e.to_string()))?;
    Ok(())
}

/// `org.freedesktop.portal.OpenURI.OpenURI(parent_window, uri, options)`.
fn portal_open_uri(url: &str) -> Result<(), String> {
    let mut bus = DBus::connect_session()?;
    bus.set_read_timeout(Some(REPLY_TIMEOUT));
    let mut w = W::default();
    w.str(""); // no parent window
    w.str(url);
    w.array(8, |_| {}); // options: a{sv}
    bus.call_with_body(
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.OpenURI",
        "OpenURI",
        "ssa{sv}",
        &w.buf,
    )?;
    debug(format_args!("portal OpenURI {url}"));
    Ok(())
}

/// Run `xdg-open url` (or `gio open url`), reaping it when it exits.
fn run_opener(url: &str) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    let spawn = |program: &str, args: &[&str]| {
        Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
    };
    let mut child = match spawn("xdg-open", &[url]) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => spawn("gio", &["open", url])?,
        other => other?,
    };
    let status = child.wait()?;
    debug(format_args!("xdg-open {url}: {status}"));
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "opener exited with {status}"
        )))
    }
}

/// `org.freedesktop.Notifications.Notify`.
fn send_notification(n: &Notification) -> Result<u32, String> {
    let mut bus = DBus::connect_session()?;
    bus.set_read_timeout(Some(REPLY_TIMEOUT));
    let mut w = W::default();
    w.str(&app_name());
    w.u32(0); // replaces_id
    w.str(""); // app_icon
    w.str(&n.title);
    w.str(&n.body);
    w.array(4, |_| {}); // actions: as
    w.array(8, |_| {}); // hints: a{sv}
    w.i32(-1); // expire_timeout: the server's default
    let reply = bus.call_with_body(
        "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
        "Notify",
        "susssasa{sv}i",
        &w.buf,
    )?;
    crate::platform::a11y::read_u32(&reply, &mut 0)
}

/// The program's name, which notification servers show as the sender.
pub(crate) fn app_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "stipple".into())
}

pub(crate) fn debug(args: std::fmt::Arguments) {
    if std::env::var_os("STIPPLE_DEBUG_SHELL").is_some() {
        eprintln!("stipple shell: {args}");
    }
}

/// A D-Bus message body being marshalled (little-endian). Alignment is
/// relative to the start of the body, which is where a message's body starts.
#[derive(Default)]
pub(crate) struct W {
    pub(crate) buf: Vec<u8>,
}

impl W {
    pub(crate) fn pad(&mut self, n: usize) {
        while !self.buf.len().is_multiple_of(n) {
            self.buf.push(0);
        }
    }
    pub(crate) fn u32(&mut self, v: u32) {
        self.pad(4);
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub(crate) fn i32(&mut self, v: i32) {
        self.pad(4);
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub(crate) fn bool(&mut self, v: bool) {
        self.u32(u32::from(v));
    }
    /// A STRING or OBJECT_PATH.
    pub(crate) fn str(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.buf.extend_from_slice(s.as_bytes());
        self.buf.push(0);
    }
    /// A SIGNATURE.
    pub(crate) fn sig(&mut self, s: &str) {
        self.buf.push(s.len() as u8);
        self.buf.extend_from_slice(s.as_bytes());
        self.buf.push(0);
    }
    /// A VARIANT: its signature, then the value `f` writes.
    pub(crate) fn variant(&mut self, sig: &str, f: impl FnOnce(&mut W)) {
        self.sig(sig);
        f(self);
    }
    /// An ARRAY whose elements align to `align` (8 for structs and dict
    /// entries); `f` writes the elements.
    pub(crate) fn array(&mut self, align: usize, f: impl FnOnce(&mut W)) {
        self.pad(4);
        let at = self.buf.len();
        self.buf.extend_from_slice(&[0; 4]);
        self.pad(align);
        let start = self.buf.len();
        f(self);
        let len = (self.buf.len() - start) as u32;
        self.buf[at..at + 4].copy_from_slice(&len.to_le_bytes());
    }
    /// A STRUCT or DICT_ENTRY: 8-aligned, then the fields `f` writes.
    pub(crate) fn strukt(&mut self, f: impl FnOnce(&mut W)) {
        self.pad(8);
        f(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::a11y::{read_string, read_u32};

    #[test]
    fn marshals_a_notify_body() {
        let mut w = W::default();
        w.str("app");
        w.u32(7);
        w.array(4, |_| {});
        w.array(8, |w| {
            w.strukt(|w| {
                w.str("urgency");
                w.variant("y", |w| w.buf.push(1));
            })
        });
        w.i32(-1);
        let b = &w.buf;
        let mut off = 0;
        assert_eq!(read_string(b, &mut off).unwrap(), "app");
        assert_eq!(read_u32(b, &mut off).unwrap(), 7);
        assert_eq!(read_u32(b, &mut off).unwrap(), 0); // empty `as`
        let dict_len = read_u32(b, &mut off).unwrap() as usize;
        off = off.next_multiple_of(8);
        assert_eq!(read_string(b, &mut off).unwrap(), "urgency");
        assert_eq!(&b[off..off + 3], b"\x01y\0");
        // key (4 + 7 + NUL = 12) + signature (3) + byte (1).
        assert_eq!(dict_len, 16);
        off += 4;
        assert_eq!(read_u32(b, &mut off).unwrap() as i32, -1);
        assert_eq!(off, b.len());
    }
}
