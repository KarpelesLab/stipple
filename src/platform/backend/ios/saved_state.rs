//! Keeping the app's state across process death on iOS: a file in the app's
//! `Library/Caches`, written when the app goes to the background and read
//! (then deleted) at launch, which belongs to the scene session that saved
//! it.
//!
//! iOS suspends a backgrounded app and later ends it without a word when it
//! needs the memory, so the only moment to save is on the way out:
//! `UIApplicationDidEnterBackgroundNotification` asks the app
//! ([`Event::SaveState`](crate::platform::Event::SaveState)) and the bytes go
//! to the file, as does `UIApplicationWillTerminateNotification` for an app
//! still running in the background. Coming back to the foreground deletes it,
//! so a crash on screen doesn't bring back an old state.
//!
//! What tells a system kill from the user swiping the app away in the app
//! switcher is the scene session ([`scene`](super::scene)): the file names
//! the `UISceneSession.persistentIdentifier` it was saved under. After a
//! system kill UIKit reconnects the same session, and the state comes back
//! ([`take_restored_state`](crate::platform::take_restored_state) hands it to
//! the app); after a swipe-away the session is gone, so the next launch
//! connects a new one and the file is dropped, and
//! `application:didDiscardSceneSessions:` deletes it as soon as UIKit says
//! so. That's iOS's own rule for state restoration (and Android's for saved
//! instance state): a user's quit starts afresh. An app without scenes (no
//! `UIApplicationSceneManifest` in its `Info.plist`) saves under the empty
//! session, and as iOS doesn't tell it about a swipe-away, its state comes
//! back then too.
//!
//! UIKit's own state restoration (the scene's `stateRestorationActivity`, or
//! the older restoration identifiers on view controllers) wasn't used for the
//! bytes: UIKit writes it at a moment it picks, while a file written on
//! `didEnterBackground` is there before iOS may end the app, and a self-test
//! can check it.
//!
//! UIKit-free: unit-tested on every host.

use std::path::{Path, PathBuf};

/// What starts the file, so a stray file isn't taken for the app's state.
/// The session's identifier follows, then a newline, then the bytes.
const MAGIC: &[u8] = b"stipple-state\x01";
const NAME: &str = "stipple-saved-state";

/// Where the file goes: the app's `Library/Caches` (in its container, which
/// is `$HOME`), kept across launches but not backed up.
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(crate) fn dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library").join("Caches"))
}

/// The file's session and bytes, if it is ours.
fn read(dir: &Path) -> Option<(String, Vec<u8>)> {
    let data = std::fs::read(dir.join(NAME)).ok()?;
    let rest = data.strip_prefix(MAGIC)?;
    let nl = rest.iter().position(|&b| b == b'\n')?;
    let session = std::str::from_utf8(&rest[..nl]).ok()?.to_string();
    Some((session, rest[nl + 1..].to_vec()))
}

/// The session the saved state in `dir` belongs to, if there is one.
pub(crate) fn stored_session(dir: &Path) -> Option<String> {
    read(dir).map(|(session, _)| session)
}

/// The state in `dir` saved by `session`, if any, deleting the file whoever
/// saved it: it is restored once, and only in the session that saved it.
pub(crate) fn take(dir: &Path, session: &str) -> Option<Vec<u8>> {
    let found = read(dir);
    let _ = std::fs::remove_file(dir.join(NAME));
    let (saved_by, bytes) = found?;
    (saved_by == session && !bytes.is_empty()).then_some(bytes)
}

/// Keep `bytes` in `dir` for `session` (replacing what was there), or delete
/// the file for `None`. Written to a temporary file and renamed, so the
/// process ending half way leaves the old state or the new, never a mix.
pub(crate) fn store(dir: &Path, session: &str, bytes: Option<&[u8]>) {
    let path = dir.join(NAME);
    let Some(bytes) = bytes.filter(|b| !b.is_empty()) else {
        let _ = std::fs::remove_file(&path);
        return;
    };
    let tmp = dir.join(format!("{NAME}.tmp"));
    let session = session.replace('\n', "");
    let mut data = Vec::with_capacity(MAGIC.len() + session.len() + 1 + bytes.len());
    data.extend_from_slice(MAGIC);
    data.extend_from_slice(session.as_bytes());
    data.push(b'\n');
    data.extend_from_slice(bytes);
    let written = std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&tmp, &data))
        .and_then(|()| std::fs::rename(&tmp, &path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// UIKit discarded `sessions` (the user swiped them away): delete the state
/// saved by one of them. Returns whether there was some.
pub(crate) fn discard(dir: &Path, sessions: &[String]) -> bool {
    match stored_session(dir) {
        Some(saved_by) if sessions.contains(&saved_by) => {
            let _ = std::fs::remove_file(dir.join(NAME));
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("stipple-ios-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn stored_state_is_taken_once() {
        let dir = temp("state");
        assert_eq!(take(&dir, "A"), None, "nothing saved");
        store(&dir, "A", Some(b"screen 2"));
        store(&dir, "A", Some(b"screen 3")); // replaces
        assert_eq!(stored_session(&dir).as_deref(), Some("A"));
        assert_eq!(take(&dir, "A").as_deref(), Some(&b"screen 3"[..]));
        assert_eq!(take(&dir, "A"), None, "taken once");
        // Saving nothing deletes what was there.
        store(&dir, "A", Some(b"x"));
        store(&dir, "A", None);
        assert_eq!(take(&dir, "A"), None);
        // A file that isn't ours is ignored (and goes), as is the format
        // from before sessions.
        std::fs::write(dir.join(NAME), b"something else").unwrap();
        assert_eq!(take(&dir, ""), None);
        assert!(!dir.join(NAME).exists());
        std::fs::write(dir.join(NAME), b"stipple-state\0screen 2").unwrap();
        assert_eq!(take(&dir, ""), None);
        // Without scenes: the empty session.
        store(&dir, "", Some(b"legacy"));
        assert_eq!(take(&dir, "").as_deref(), Some(&b"legacy"[..]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_new_session_starts_afresh() {
        // The user swiped the app away: the next launch is a new session,
        // which doesn't get the old one's state, and the file goes.
        let dir = temp("new-session");
        store(&dir, "A", Some(b"screen 2"));
        assert_eq!(take(&dir, "B"), None);
        assert!(!dir.join(NAME).exists());
        assert_eq!(take(&dir, "A"), None, "dropped, not kept for later");
        // Bytes holding newlines survive.
        store(&dir, "B", Some(b"line 1\nline 2\n"));
        assert_eq!(take(&dir, "B").as_deref(), Some(&b"line 1\nline 2\n"[..]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn discarding_the_session_deletes_its_state() {
        let dir = temp("discard");
        store(&dir, "A", Some(b"screen 2"));
        // Another session going leaves it.
        assert!(!discard(&dir, &["B".into()]));
        assert_eq!(stored_session(&dir).as_deref(), Some("A"));
        assert!(discard(&dir, &["B".into(), "A".into()]));
        assert_eq!(take(&dir, "A"), None);
        assert!(!discard(&dir, &["A".into()]), "nothing left");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
