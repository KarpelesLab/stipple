//! Owned windows: a window that belongs to another
//! ([`WindowAttributes::parent`](crate::platform::WindowAttributes::parent))
//! is transient for it (ICCCM `WM_TRANSIENT_FOR`) and typed as a dialog
//! (`_NET_WM_WINDOW_TYPE_DIALOG`), so the window manager keeps it above its
//! parent and usually centers it there; a modal one also says
//! `_NET_WM_STATE_MODAL`, which window managers use to keep it above, and
//! focus going to, the parent while it is open.

use std::io;

use super::{Conn, intern_atom, set_property_atoms, set_property32};

/// `WM_TRANSIENT_FOR` and the `WINDOW` type are predefined atoms.
const ATOM_WM_TRANSIENT_FOR: u32 = 68;
const ATOM_WINDOW: u32 = 33;

/// The atoms this module uses, interned at startup (0 until then).
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Atoms {
    window_type: u32,
    type_dialog: u32,
    /// `_NET_WM_STATE_MODAL`, for the window's initial `_NET_WM_STATE`.
    pub(super) state_modal: u32,
}

impl Atoms {
    pub(super) fn intern(conn: &mut Conn) -> io::Result<Self> {
        Ok(Self {
            window_type: intern_atom(conn, b"_NET_WM_WINDOW_TYPE")?,
            type_dialog: intern_atom(conn, b"_NET_WM_WINDOW_TYPE_DIALOG")?,
            state_modal: intern_atom(conn, b"_NET_WM_STATE_MODAL")?,
        })
    }
}

/// Make `window` (not mapped yet) a dialog transient for `parent`.
pub(super) fn set_parent(conn: &mut Conn, window: u32, parent: u32) -> io::Result<()> {
    set_property32(conn, window, ATOM_WM_TRANSIENT_FOR, ATOM_WINDOW, &[parent])?;
    let atoms = conn.owned;
    if atoms.window_type != 0 {
        set_property_atoms(conn, window, atoms.window_type, &[atoms.type_dialog])?;
    }
    Ok(())
}
