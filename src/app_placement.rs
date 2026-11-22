//! Windows that come back where the user left them
//! ([`App::remember_window`](crate::App::remember_window),
//! [`WindowAttributes::remember`]).
//!
//! The placement ([`WindowPlacement`]) is kept as a small text file per key
//! in the app's config directory (`platform::dirs().config`, under
//! `windows/`). The window opens at it (within its size limits, and kept on
//! a monitor that is still there by the backend), and every change the
//! window reports is written back, at most once a second while it moves,
//! and once more when the window goes away.

use std::path::PathBuf;

use crate::app_limits::{RESIZABLE_WINDOWS, fit};
use crate::platform::time::Instant;
use crate::platform::{WindowAttributes, WindowInfo, WindowPlacement};
use std::time::Duration;

/// The least time between two writes while the window moves.
const WRITE_EVERY: Duration = Duration::from_secs(1);

/// What a pane keeps of its remembered placement.
#[derive(Default)]
pub(crate) struct Memory {
    /// The key it is saved under, once the window opened with one.
    key: Option<String>,
    /// The placement last written (or read).
    saved: Option<WindowPlacement>,
    /// A placement not written yet (it came too soon after the last).
    pending: Option<WindowPlacement>,
    last_write: Option<Instant>,
}

impl Memory {
    /// Before the window opens: bring `attrs` to the placement saved under
    /// its key, if any, within its own size limits.
    pub(crate) fn restore(&mut self, attrs: &mut WindowAttributes) {
        if !RESIZABLE_WINDOWS {
            return;
        }
        let Some(key) = attrs.remember.clone() else {
            return;
        };
        self.key = Some(key.clone());
        let Some(placement) = load(&key) else {
            return;
        };
        crate::app_dbg(format_args!("window {key:?} restored to {placement:?}"));
        *attrs = placement.apply(std::mem::take(attrs));
        attrs.logical_size = fit(attrs.logical_size, (attrs.min_size, attrs.max_size));
        self.saved = Some(placement);
    }

    /// The window now is as `info` says: save its placement if it changed
    /// (not while it is minimized, when it doesn't say whether it was
    /// maximized).
    pub(crate) fn note(&mut self, info: &WindowInfo) {
        if self.key.is_none() || info.minimized {
            return;
        }
        let now = WindowPlacement::of(info);
        if info.inner_size.width < 1.0 || Some(now) == self.saved {
            self.pending = None;
            return;
        }
        self.pending = Some(now);
        let due = self.last_write.is_none_or(|t| t.elapsed() >= WRITE_EVERY);
        if due {
            self.flush();
        }
    }

    /// Write the placement not written yet, if any.
    pub(crate) fn flush(&mut self) {
        let (Some(key), Some(placement)) = (self.key.as_deref(), self.pending.take()) else {
            return;
        };
        self.last_write = Some(Instant::now());
        match save(key, &placement) {
            Ok(()) => self.saved = Some(placement),
            Err(e) => crate::app_dbg(format_args!("window {key:?} not saved: {e}")),
        }
    }
}

impl Drop for Memory {
    fn drop(&mut self) {
        self.flush();
    }
}

#[cfg(test)]
thread_local! {
    /// Where tests keep placements, instead of the app's config directory.
    static TEST_DIR: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Keep this thread's placements in `dir` (tests).
#[cfg(test)]
pub(crate) fn set_test_dir(dir: Option<PathBuf>) {
    TEST_DIR.with(|d| *d.borrow_mut() = dir);
}

/// The directory placements are kept in.
fn dir() -> Option<PathBuf> {
    #[cfg(test)]
    {
        TEST_DIR.with(|d| d.borrow().clone())
    }
    #[cfg(not(test))]
    {
        crate::platform::dirs()
            .ok()
            .map(|d| d.config.join("windows"))
    }
}

/// `key` as a file name: letters, digits, `-`, `_` and `.` kept, anything
/// else `_`, never empty or hidden.
pub(crate) fn file_name(key: &str) -> String {
    let safe: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe = safe.trim_start_matches('.');
    if safe.is_empty() {
        "window.placement".into()
    } else {
        format!("{safe}.placement")
    }
}

fn load(key: &str) -> Option<WindowPlacement> {
    let text = std::fs::read_to_string(dir()?.join(file_name(key))).ok()?;
    WindowPlacement::from_text(&text)
}

fn save(key: &str, placement: &WindowPlacement) -> std::io::Result<()> {
    let dir = dir().ok_or_else(|| std::io::Error::other("no config directory"))?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(file_name(key));
    // Written beside it, then moved over it: never half a file.
    let tmp = path.with_extension("placement.tmp");
    std::fs::write(&tmp, placement.to_text())?;
    std::fs::rename(&tmp, &path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_become_safe_file_names() {
        assert_eq!(file_name("main"), "main.placement");
        assert_eq!(file_name("../etc/passwd"), "_etc_passwd.placement");
        assert_eq!(file_name("a b/c"), "a_b_c.placement");
        assert_eq!(file_name(""), "window.placement");
        assert_eq!(file_name("..."), "window.placement");
    }
}
