//! macOS: `NSWorkspace` opens URLs; notifications go through `osascript`.
//!
//! The notification frameworks need an app bundle: `UNUserNotificationCenter`
//! raises an exception in a bare executable (no bundle identifier), and the
//! deprecated `NSUserNotificationCenter` hands it `nil`. AppleScript's
//! `display notification` works from any process (macOS shows it as coming
//! from Script Editor), so that's what [`notify`] runs.

use super::Notification;
use crate::platform::PlatformError;
use crate::platform::native::objc::{Id, class, msg, nsstring, with_pool};

pub fn open_url(url: &str) -> Result<(), PlatformError> {
    with_pool(|| {
        let s = nsstring(url);
        let nsurl = unsafe { msg!(class("NSURL"), "URLWithString:", (s: Id) -> Id) };
        if nsurl.is_null() {
            return Err(PlatformError::Os(format!("NSURL refused {url:?}")));
        }
        let ws = unsafe { msg!(class("NSWorkspace"), "sharedWorkspace", () -> Id) };
        let ok = unsafe { msg!(ws, "openURL:", (nsurl: Id) -> bool) };
        if ok {
            Ok(())
        } else {
            Err(PlatformError::Os(format!(
                "NSWorkspace couldn't open {url}"
            )))
        }
    })
}

pub fn notify(n: &Notification) -> Result<(), PlatformError> {
    let script = format!(
        "display notification {} with title {}",
        applescript_string(&n.body),
        applescript_string(&n.title)
    );
    let status = std::process::Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .stdin(std::process::Stdio::null())
        .status()
        .map_err(|e| PlatformError::Os(format!("osascript: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(PlatformError::Os(format!("osascript exited with {status}")))
    }
}

/// `s` as an AppleScript string literal.
fn applescript_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' | '\r' => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
