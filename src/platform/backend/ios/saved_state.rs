//! Keeping the app's state across process death on iOS: a file in the app's
//! `Library/Caches`, written when the app goes to the background and read
//! (then deleted) at launch.
//!
//! iOS suspends a backgrounded app and later ends it without a word when it
//! needs the memory, so the only moment to save is on the way out:
//! `UIApplicationDidEnterBackgroundNotification` asks the app
//! ([`Event::SaveState`](crate::platform::Event::SaveState)) and the bytes go
//! to the file, as does `UIApplicationWillTerminateNotification` for an app
//! still running in the background. Coming back to the foreground deletes it,
//! so a crash on screen doesn't bring back an old state; a launch reads it and
//! deletes it, and [`take_restored_state`](crate::platform::take_restored_state)
//! hands it to the app.
//!
//! UIKit's own state restoration (`application:shouldSaveSecureApplicationState:`
//! and the restorable-state coder) wasn't used: it is tied to view
//! controllers with restoration identifiers and to scenes' user activities,
//! saves at a moment UIKit picks, and gives nothing a self-test can check;
//! a file written on `didEnterBackground` is what iOS apps do in practice.
//! Unlike it, the file doesn't know whether the user swiped the app away in
//! the app switcher (iOS doesn't tell a suspended app), so the state comes
//! back then too.
//!
//! UIKit-free: unit-tested on every host.

use std::path::{Path, PathBuf};

/// What starts the file, so a stray file isn't taken for the app's state.
const MAGIC: &[u8] = b"stipple-state\0";
const NAME: &str = "stipple-saved-state";

/// Where the file goes: the app's `Library/Caches` (in its container, which
/// is `$HOME`), kept across launches but not backed up.
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(crate) fn dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library").join("Caches"))
}

/// The saved state in `dir`, if any, deleting the file: it is restored once.
pub(crate) fn take(dir: &Path) -> Option<Vec<u8>> {
    let path = dir.join(NAME);
    let bytes = std::fs::read(&path).ok();
    let _ = std::fs::remove_file(&path);
    let bytes = bytes?.strip_prefix(MAGIC)?.to_vec();
    (!bytes.is_empty()).then_some(bytes)
}

/// Keep `bytes` in `dir` (replacing what was there), or delete the file for
/// `None`. Written to a temporary file and renamed, so the process ending
/// half way leaves the old state or the new, never a mix.
pub(crate) fn store(dir: &Path, bytes: Option<&[u8]>) {
    let path = dir.join(NAME);
    let Some(bytes) = bytes.filter(|b| !b.is_empty()) else {
        let _ = std::fs::remove_file(&path);
        return;
    };
    let tmp = dir.join(format!("{NAME}.tmp"));
    let mut data = Vec::with_capacity(MAGIC.len() + bytes.len());
    data.extend_from_slice(MAGIC);
    data.extend_from_slice(bytes);
    let written = std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&tmp, &data))
        .and_then(|()| std::fs::rename(&tmp, &path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_state_is_taken_once() {
        let dir = std::env::temp_dir().join(format!("stipple-ios-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(take(&dir), None, "nothing saved");
        store(&dir, Some(b"screen 2"));
        store(&dir, Some(b"screen 3")); // replaces
        assert_eq!(take(&dir).as_deref(), Some(&b"screen 3"[..]));
        assert_eq!(take(&dir), None, "taken once");
        // Saving nothing deletes what was there.
        store(&dir, Some(b"x"));
        store(&dir, None);
        assert_eq!(take(&dir), None);
        // A file that isn't ours is ignored (and goes).
        std::fs::write(dir.join(NAME), b"something else").unwrap();
        assert_eq!(take(&dir), None);
        assert!(!dir.join(NAME).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
