//! Owned windows: a window that belongs to another
//! ([`WindowAttributes::parent`](crate::platform::WindowAttributes::parent))
//! names the parent's toplevel with `xdg_toplevel.set_parent`, so the
//! compositor keeps it above the parent (and may center it there, or move
//! and minimize the two together). It is also a dialog, modal or not, where
//! the compositor has `xdg-dialog-v1` (`dialog.rs`); the app enforces a
//! modal one itself either way.

use std::cell::RefCell;
use std::io;

use super::{WlConn, arg_u32};

/// `xdg_toplevel.set_parent(parent: object<xdg_toplevel>?)`.
const XDG_TOPLEVEL_SET_PARENT: u16 = 1;

thread_local! {
    /// The open windows' `wl_surface` (their `WindowId`) and `xdg_toplevel`.
    static TOPLEVELS: RefCell<Vec<(u32, u32)>> = const { RefCell::new(Vec::new()) };
}

/// Note that window `surface` has toplevel `toplevel`.
pub(super) fn note(surface: u32, toplevel: u32) {
    TOPLEVELS.with(|t| {
        let mut t = t.borrow_mut();
        t.retain(|(s, _)| *s != surface);
        t.push((surface, toplevel));
    });
}

/// Forget window `surface`, which closed.
pub(super) fn forget(surface: u32) {
    TOPLEVELS.with(|t| t.borrow_mut().retain(|(s, _)| *s != surface));
}

/// Make `toplevel` belong to the window whose surface is `parent`, if it is
/// still open.
pub(super) fn set_parent(conn: &mut WlConn, toplevel: u32, parent: u32) -> io::Result<()> {
    let Some(parent) = TOPLEVELS.with(|t| {
        t.borrow()
            .iter()
            .find(|(s, _)| *s == parent)
            .map(|(_, top)| *top)
    }) else {
        return Ok(());
    };
    let mut a = Vec::new();
    arg_u32(&mut a, parent);
    conn.send(toplevel, XDG_TOPLEVEL_SET_PARENT, &a)
}
