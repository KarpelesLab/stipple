//! Keeping the app's state across process death: the system may end a
//! backgrounded app to free memory, then start it again where the user left
//! it, handing back what the app saved.
//!
//! The system asks with [`Event::SaveState`](crate::platform::Event::SaveState);
//! the handler answers with [`save_state`], bytes the backend keeps where the
//! system keeps such things (Android's saved-instance `Bundle`, a file in the
//! app's caches on iOS, the tab's `sessionStorage` on the web). When the app
//! starts again from them, [`take_restored_state`] hands them back, once. The
//! [`App`](crate::App) does both from
//! [`App::save_state`](crate::App::save_state) and
//! [`App::restore_state`](crate::App::restore_state). See
//! `docs/window-lifecycle.md`.

use std::cell::RefCell;
use std::sync::Mutex;

/// The bytes the process was started with, until taken.
static RESTORED: Mutex<Option<Vec<u8>>> = Mutex::new(None);

thread_local! {
    /// What the handler of an [`Event::SaveState`] saved.
    static SAVED: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
}

/// Call while handling an [`Event::SaveState`](crate::platform::Event::SaveState)
/// with the bytes to keep. Has no effect at any other time. Keep it small, a
/// few kilobytes: Android carries the saved state in a binder transaction,
/// which fails past a megabyte for the whole process.
pub fn save_state(bytes: Vec<u8>) {
    SAVED.with(|s| *s.borrow_mut() = Some(bytes));
}

/// The state saved before the system ended the process, if it was started
/// again from it. Returns it once; `None` on a fresh start.
pub fn take_restored_state() -> Option<Vec<u8>> {
    RESTORED.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// The backend was started with `bytes` (a backend's own, from the system).
#[cfg_attr(
    not(any(target_os = "android", target_os = "ios", target_arch = "wasm32")),
    allow(dead_code)
)]
pub(crate) fn set_restored_state(bytes: Option<Vec<u8>>) {
    *RESTORED.lock().unwrap_or_else(|e| e.into_inner()) = bytes;
}

/// Hand [`Event::SaveState`](crate::platform::Event::SaveState) to `handler`.
/// Returns what it saved (see [`save_state`]).
#[cfg_attr(
    not(any(target_os = "android", target_os = "ios", target_arch = "wasm32", test)),
    allow(dead_code)
)]
pub(crate) fn deliver_save(handler: impl FnOnce(crate::platform::Event)) -> Option<Vec<u8>> {
    SAVED.with(|s| s.borrow_mut().take());
    handler(crate::platform::Event::SaveState);
    SAVED.with(|s| s.borrow_mut().take())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Event;

    #[test]
    fn a_save_is_collected_only_while_asked() {
        save_state(b"stray".to_vec()); // outside a delivery: dropped
        let saved = deliver_save(|e| {
            assert_eq!(e, Event::SaveState);
            save_state(b"state".to_vec());
        });
        assert_eq!(saved.as_deref(), Some(&b"state"[..]));
        assert_eq!(deliver_save(|_| {}), None, "nothing saved: nothing kept");
    }
}
