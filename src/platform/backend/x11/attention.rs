//! Focus, attention, the bell and decorations on X11: `FocusIn`/`FocusOut`,
//! `_NET_WM_STATE_DEMANDS_ATTENTION` with the ICCCM urgency hint, the core
//! `Bell` request, and `_MOTIF_WM_HINTS` for undecorated windows.

use std::io;

use super::{Conn, finish, intern_atom, property32_request, wm_state_message};
use crate::platform::UserAttention;

/// `FocusChange` in an event mask.
pub(super) const EV_FOCUS_CHANGE: u32 = 0x0020_0000;
/// The `FocusIn` and `FocusOut` event codes.
pub(super) const X_FOCUS_IN: u8 = 9;
pub(super) const X_FOCUS_OUT: u8 = 10;

/// The core `Bell` request.
const OP_BELL: u8 = 104;
/// The predefined `WM_HINTS` atom, also its type.
const ATOM_WM_HINTS: u32 = 35;
/// `WM_HINTS` flags (ICCCM 4.1.2.4): the input field is set; the window is
/// urgent (`XUrgencyHint`).
const INPUT_HINT: u32 = 1;
const URGENCY_HINT: u32 = 1 << 8;
/// `_MOTIF_WM_HINTS` flags: the decorations field is set.
const MWM_HINTS_DECORATIONS: u32 = 2;

/// The atoms this module uses, interned at startup (0 until then).
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Atoms {
    demands_attention: u32,
    motif_wm_hints: u32,
}

impl Atoms {
    pub(super) fn intern(conn: &mut Conn) -> io::Result<Self> {
        Ok(Self {
            demands_attention: intern_atom(conn, b"_NET_WM_STATE_DEMANDS_ATTENTION")?,
            motif_wm_hints: intern_atom(conn, b"_MOTIF_WM_HINTS")?,
        })
    }
}

/// Whether a `FocusIn`/`FocusOut` event means the window gained (`Some(true)`)
/// or lost (`Some(false)`) keyboard focus. Events from keyboard grabs (a
/// window manager's Alt+Tab, a menu) and the pointer-root focus model's
/// pointer-following details are not focus changes, nor is focus moving
/// between the window and its own children.
pub(super) fn focus_change(buf: &[u8; 32]) -> Option<bool> {
    let focused = match buf[0] & 0x7f {
        X_FOCUS_IN => true,
        X_FOCUS_OUT => false,
        _ => return None,
    };
    // detail: Ancestor 0, Virtual 1, Inferior 2, Nonlinear 3,
    // NonlinearVirtual 4, Pointer 5, PointerRoot 6, None 7.
    // mode: Normal 0, Grab 1, Ungrab 2, WhileGrabbed 3.
    let (detail, mode) = (buf[1], buf[8]);
    if matches!(detail, 2 | 5 | 6 | 7) || matches!(mode, 1 | 2) {
        return None;
    }
    Some(focused)
}

/// The nine `WM_HINTS` words: flags, input, initial state, icon pixmap, icon
/// window, icon x/y, icon mask, window group. The window takes input (as
/// without the property) and is urgent or not.
fn wm_hints(urgent: bool) -> [u32; 9] {
    let mut hints = [0u32; 9];
    hints[0] = INPUT_HINT | if urgent { URGENCY_HINT } else { 0 };
    hints[1] = 1; // input: true
    hints
}

/// Mark `window` urgent (or not): the urgency hint, which every window manager
/// reads, and `_NET_WM_STATE_DEMANDS_ATTENTION`, which EWMH ones show (a
/// flashing taskbar entry). Both are dropped when the window is focused.
pub(super) fn set_urgent(
    conn: &mut Conn,
    window: u32,
    kind: Option<UserAttention>,
) -> io::Result<()> {
    let urgent = kind.is_some();
    let req = property32_request(window, ATOM_WM_HINTS, ATOM_WM_HINTS, &wm_hints(urgent));
    conn.send(&finish(req))?;
    let (root, state, attention) = (
        conn.setup.root,
        conn.atom_net_wm_state,
        conn.attention.demands_attention,
    );
    if state != 0 && attention != 0 {
        conn.send(&wm_state_message(root, window, state, urgent, attention, 0))?;
    }
    Ok(())
}

/// The `Bell` request at the base volume (percent 0).
pub(super) fn bell_request() -> Vec<u8> {
    finish(vec![OP_BELL, 0, 0, 0])
}

/// Ask the window manager not to decorate `window`, through the Motif hints
/// most of them read (there is no EWMH property for it).
pub(super) fn set_undecorated(conn: &mut Conn, window: u32) -> io::Result<()> {
    let prop = conn.attention.motif_wm_hints;
    if prop == 0 {
        return Ok(());
    }
    // flags, functions, decorations, input mode, status.
    let hints = [MWM_HINTS_DECORATIONS, 0, 0, 0, 0];
    conn.send(&finish(property32_request(window, prop, prop, &hints)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn focus_event(code: u8, detail: u8, mode: u8) -> [u8; 32] {
        let mut buf = [0u8; 32];
        buf[0] = code;
        buf[1] = detail;
        buf[4..8].copy_from_slice(&0x0040_0001u32.to_le_bytes());
        buf[8] = mode;
        buf
    }

    #[test]
    fn focus_events_become_focus_changes() {
        // Nonlinear: focus moved from another window.
        assert_eq!(focus_change(&focus_event(X_FOCUS_IN, 3, 0)), Some(true));
        assert_eq!(focus_change(&focus_event(X_FOCUS_OUT, 3, 0)), Some(false));
        // Ancestor (to or from the root), as SetInputFocus with no WM does.
        assert_eq!(focus_change(&focus_event(X_FOCUS_IN, 0, 0)), Some(true));
        // A keyboard grab (Alt+Tab held) or the pointer following focus:
        // not a change.
        assert_eq!(focus_change(&focus_event(X_FOCUS_OUT, 3, 1)), None);
        assert_eq!(focus_change(&focus_event(X_FOCUS_IN, 3, 2)), None);
        assert_eq!(focus_change(&focus_event(X_FOCUS_IN, 5, 0)), None);
        assert_eq!(focus_change(&focus_event(X_FOCUS_OUT, 2, 0)), None);
        // Other events.
        assert_eq!(focus_change(&focus_event(2, 3, 0)), None);
    }

    #[test]
    fn urgency_hint_words() {
        assert_eq!(wm_hints(true)[0], INPUT_HINT | URGENCY_HINT);
        assert_eq!(wm_hints(false)[0], INPUT_HINT);
        assert_eq!(wm_hints(true)[1], 1, "the window still takes input");
        let req = finish(property32_request(
            7,
            ATOM_WM_HINTS,
            ATOM_WM_HINTS,
            &wm_hints(true),
        ));
        // ChangeProperty header (6 words) + 9 values.
        assert_eq!(u16::from_le_bytes([req[2], req[3]]), 6 + 9);
        assert_eq!(&req[4..8], &7u32.to_le_bytes());
        assert_eq!(&req[24..28], &(INPUT_HINT | URGENCY_HINT).to_le_bytes());
    }

    #[test]
    fn bell_is_a_one_word_request() {
        assert_eq!(bell_request(), [OP_BELL, 0, 1, 0]);
    }
}
