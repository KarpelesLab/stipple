//! Dialog windows on Wayland: `xdg-dialog-v1`.
//!
//! An owned window ([`WindowAttributes::parent`]) already names its parent's
//! toplevel with `xdg_toplevel.set_parent` (`owned.rs`). Where the compositor
//! advertises `xdg_wm_dialog_v1` (Mutter 46+, KWin 6.1+, among others), the
//! toplevel also gets an `xdg_dialog_v1`, which marks it as a dialog, and a
//! modal one ([`WindowAttributes::blocks_parent`]) says so with
//! `set_modal`: the compositor may then dim the parent, attach the dialog to
//! it, and refuse to focus the parent while it is up. The app keeps
//! dropping the parent's input either way, as it does without the protocol.
//!
//! The dialog object is destroyed before its toplevel (see
//! [`forget`]), which is the order the protocol asks for.
//!
//! [`WindowAttributes::parent`]: crate::platform::WindowAttributes::parent
//! [`WindowAttributes::blocks_parent`]: crate::platform::WindowAttributes

use std::io;

use super::{WlConn, arg_u32};

/// The global's interface name.
pub(super) const MANAGER: &str = "xdg_wm_dialog_v1";
// xdg_wm_dialog_v1 requests.
const WM_DIALOG_GET_XDG_DIALOG: u16 = 1;
// xdg_dialog_v1 requests.
const DIALOG_DESTROY: u16 = 0;
const DIALOG_SET_MODAL: u16 = 1;
const DIALOG_UNSET_MODAL: u16 = 2;

/// The bound `xdg_wm_dialog_v1`, and each dialog's toplevel and
/// `xdg_dialog_v1`, kept on the connection.
#[derive(Debug, Default)]
pub(super) struct Dialogs {
    manager: Option<u32>,
    dialogs: Vec<(u32, u32)>,
}

/// Keep the bound global (`None`: the compositor has no xdg-dialog-v1).
pub(super) fn setup(conn: &mut WlConn, manager: Option<u32>) {
    conn.dialogs.manager = manager;
}

/// `xdg_wm_dialog_v1.get_xdg_dialog(id: new_id<xdg_dialog_v1>, toplevel)`.
fn get_args(id: u32, toplevel: u32) -> Vec<u8> {
    let mut a = Vec::new();
    arg_u32(&mut a, id);
    arg_u32(&mut a, toplevel);
    a
}

/// The request that makes a dialog modal, or no longer modal.
fn modal_opcode(modal: bool) -> u16 {
    if modal {
        DIALOG_SET_MODAL
    } else {
        DIALOG_UNSET_MODAL
    }
}

/// Mark `toplevel`, an owned window, as a dialog, and as modal when `modal`.
/// Returns whether the compositor has xdg-dialog-v1.
pub(super) fn mark(conn: &mut WlConn, toplevel: u32, modal: bool) -> io::Result<bool> {
    let Some(manager) = conn.dialogs.manager else {
        return Ok(false);
    };
    let dialog = match conn.dialogs.dialogs.iter().find(|(t, _)| *t == toplevel) {
        Some(&(_, dialog)) => dialog,
        None => {
            let dialog = conn.new_id();
            conn.send(
                manager,
                WM_DIALOG_GET_XDG_DIALOG,
                &get_args(dialog, toplevel),
            )?;
            conn.dialogs.dialogs.push((toplevel, dialog));
            dialog
        }
    };
    // A new dialog isn't modal, so only modality is worth a request there.
    if modal {
        conn.send(dialog, modal_opcode(true), &[])?;
    }
    super::dbg(format_args!(
        "xdg_dialog_v1 {dialog} for toplevel {toplevel}: modal={modal}"
    ));
    Ok(true)
}

/// Destroy `toplevel`'s dialog, if it has one: before the toplevel itself.
pub(super) fn forget(conn: &mut WlConn, toplevel: u32) {
    if let Some(i) = conn
        .dialogs
        .dialogs
        .iter()
        .position(|(t, _)| *t == toplevel)
    {
        let (_, dialog) = conn.dialogs.dialogs.remove(i);
        let _ = conn.send(dialog, DIALOG_DESTROY, &[]);
    }
}

#[cfg(test)]
mod tests {
    use super::super::clipboard::tests::{conn_pair, read_request};
    use super::*;

    #[test]
    fn get_xdg_dialog_encodes_the_new_id_then_the_toplevel() {
        assert_eq!(get_args(9, 5), [9, 0, 0, 0, 5, 0, 0, 0]);
        assert_eq!(modal_opcode(true), 1);
        assert_eq!(modal_opcode(false), 2);
    }

    #[test]
    fn a_modal_window_gets_a_modal_dialog_destroyed_first() {
        let (mut conn, compositor) = conn_pair();
        assert!(!mark(&mut conn, 5, true).unwrap(), "no xdg-dialog-v1");
        setup(&mut conn, Some(3));
        assert!(mark(&mut conn, 5, true).unwrap());
        let (object, opcode, args, _) = read_request(&compositor);
        assert_eq!((object, opcode), (3, WM_DIALOG_GET_XDG_DIALOG));
        let dialog = u32::from_le_bytes(args[..4].try_into().unwrap());
        assert_eq!(args, get_args(dialog, 5));
        let (object, opcode, args, _) = read_request(&compositor);
        assert_eq!((object, opcode, args.len()), (dialog, DIALOG_SET_MODAL, 0));
        // Marked again: the same dialog, no second get_xdg_dialog.
        assert!(mark(&mut conn, 5, true).unwrap());
        let (object, opcode, _, _) = read_request(&compositor);
        assert_eq!((object, opcode), (dialog, DIALOG_SET_MODAL));
        forget(&mut conn, 5);
        let (object, opcode, _, _) = read_request(&compositor);
        assert_eq!((object, opcode), (dialog, DIALOG_DESTROY));
        // Gone: nothing more to destroy.
        forget(&mut conn, 5);
        compositor
            .set_nonblocking(true)
            .expect("a nonblocking socket");
        let mut rest = [0u8; 8];
        assert!(
            std::io::Read::read(&mut &compositor, &mut rest).is_err(),
            "nothing more was sent"
        );
    }

    #[test]
    fn an_owned_window_that_is_not_modal_is_only_a_dialog() {
        let (mut conn, compositor) = conn_pair();
        setup(&mut conn, Some(3));
        assert!(mark(&mut conn, 8, false).unwrap());
        let (object, opcode, args, _) = read_request(&compositor);
        assert_eq!((object, opcode), (3, WM_DIALOG_GET_XDG_DIALOG));
        let dialog = u32::from_le_bytes(args[..4].try_into().unwrap());
        forget(&mut conn, 8);
        // The next request is the destroy: no set_modal went out.
        let (object, opcode, _, _) = read_request(&compositor);
        assert_eq!((object, opcode), (dialog, DIALOG_DESTROY));
    }
}
