//! Owned windows on Windows: a window that belongs to another
//! ([`WindowAttributes::parent`](crate::platform::WindowAttributes::parent))
//! is created with it as its owner, so it stays above it, minimizes with it
//! and has no taskbar button of its own; it opens centered on it. A modal
//! one disables its owner (`EnableWindow`) until it closes, as
//! `DialogBoxIndirectParamW` does: clicks on the owner beep and bring the
//! dialog forward.

use std::cell::RefCell;

use super::{GetWindowRect, Hwnd, Rect, SetForegroundWindow};

#[link(name = "user32")]
unsafe extern "system" {
    fn EnableWindow(hwnd: Hwnd, enable: i32) -> i32;
    fn IsWindow(hwnd: Hwnd) -> i32;
}

thread_local! {
    /// The modal windows open, with the owner each disables.
    static MODAL: RefCell<Vec<(usize, usize)>> = const { RefCell::new(Vec::new()) };
}

/// The owner window `attrs` names, if it is still open.
pub(super) fn owner_of(attrs: &crate::platform::WindowAttributes) -> Option<Hwnd> {
    let hwnd = attrs.parent?.0 as usize as Hwnd;
    (unsafe { IsWindow(hwnd) } != 0).then_some(hwnd)
}

/// Where a `width`×`height` (physical pixels) window goes to be centered on
/// `owner`.
pub(super) fn centered_on(owner: Hwnd, width: i32, height: i32) -> (i32, i32) {
    let mut r = Rect {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    unsafe { GetWindowRect(owner, &mut r) };
    (
        r.left + (r.right - r.left - width) / 2,
        r.top + (r.bottom - r.top - height) / 2,
    )
}

/// Disable `owner` while modal window `hwnd` is open.
pub(super) fn begin_modal(hwnd: Hwnd, owner: Hwnd) {
    MODAL.with(|m| m.borrow_mut().push((hwnd as usize, owner as usize)));
    unsafe { EnableWindow(owner, 0) };
}

/// Window `hwnd` is about to be destroyed: if it was modal, enable its owner
/// again (before the destroy, so the owner, not some other app, is
/// activated next) unless another modal window still blocks it.
pub(super) fn closing(hwnd: Hwnd) {
    let owner = MODAL.with(|m| {
        let mut m = m.borrow_mut();
        let i = m.iter().position(|(w, _)| *w == hwnd as usize)?;
        let (_, owner) = m.remove(i);
        let still = m.iter().any(|(_, o)| *o == owner);
        (!still).then_some(owner)
    });
    if let Some(owner) = owner {
        let owner = owner as Hwnd;
        unsafe {
            EnableWindow(owner, 1);
            SetForegroundWindow(owner);
        }
    }
}
