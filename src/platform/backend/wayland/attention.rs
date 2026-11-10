//! Attention requests and the bell on Wayland.
//!
//! Wayland has no "urgent" flag a client sets. What it has is
//! `xdg_activation_v1`: a client asks for an activation token and then asks
//! for its surface to be activated with it. Compositors don't let a window
//! that isn't focused take focus that way; they mark it urgent instead
//! (sway, KDE and GNOME do), which is what an attention request wants. The
//! bell is `xdg_system_bell_v1` where the compositor has it; elsewhere there
//! is no beep on Wayland.

use super::{WlConn, arg_str, arg_u32, read_str};

/// The globals' interface names.
pub(super) const ACTIVATION: &str = "xdg_activation_v1";
pub(super) const SYSTEM_BELL: &str = "xdg_system_bell_v1";
// xdg_activation_v1 requests.
const ACTIVATION_GET_TOKEN: u16 = 1;
const ACTIVATION_ACTIVATE: u16 = 2;
// xdg_activation_token_v1 requests, and its event.
const TOKEN_SET_SURFACE: u16 = 2;
const TOKEN_COMMIT: u16 = 3;
const TOKEN_DESTROY: u16 = 4;
const TOKEN_DONE: u16 = 0;
// xdg_system_bell_v1 requests.
const BELL_RING: u16 = 1;

/// The bound globals and the tokens in flight, kept on the connection.
#[derive(Debug, Default)]
pub(super) struct Attention {
    activation: Option<u32>,
    bell: Option<u32>,
    /// Tokens asked for, and the surface each will activate.
    pending: Vec<(u32, u32)>,
}

/// Keep the bound globals.
pub(super) fn setup(conn: &mut WlConn, activation: Option<u32>, bell: Option<u32>) {
    conn.attention.activation = activation;
    conn.attention.bell = bell;
}

/// Ask for `surface` to be activated, which a compositor shows as an
/// attention request while it isn't focused. Returns whether the compositor
/// supports it.
pub(super) fn request(conn: &mut WlConn, surface: u32) -> bool {
    let Some(activation) = conn.attention.activation else {
        return false;
    };
    let token = conn.new_id();
    if conn
        .send(activation, ACTIVATION_GET_TOKEN, &token.to_le_bytes())
        .is_err()
    {
        return false;
    }
    let _ = conn.send(token, TOKEN_SET_SURFACE, &surface.to_le_bytes());
    let _ = conn.send(token, TOKEN_COMMIT, &[]);
    conn.attention.pending.push((token, surface));
    true
}

/// Ring the system bell for `surface`. Returns whether the compositor has one.
pub(super) fn ring(conn: &mut WlConn, surface: u32) -> bool {
    let Some(bell) = conn.attention.bell else {
        return false;
    };
    conn.send(bell, BELL_RING, &surface.to_le_bytes()).is_ok()
}

/// Handle a token's `done`: activate with it, then destroy it. Returns
/// whether the message was a token's.
pub(super) fn handle(conn: &mut WlConn, object: u32, opcode: u16, args: &[u8]) -> bool {
    let Some(i) = conn
        .attention
        .pending
        .iter()
        .position(|(t, _)| *t == object)
    else {
        return false;
    };
    if opcode != TOKEN_DONE {
        return true;
    }
    let (token, surface) = conn.attention.pending.remove(i);
    if let (Some(activation), Some((name, _))) = (conn.attention.activation, read_str(args, 0)) {
        let _ = conn.send(
            activation,
            ACTIVATION_ACTIVATE,
            &activate_args(&name, surface),
        );
    }
    let _ = conn.send(token, TOKEN_DESTROY, &[]);
    true
}

/// `xdg_activation_v1.activate`'s arguments: the token string, the surface.
fn activate_args(token: &str, surface: u32) -> Vec<u8> {
    let mut a = Vec::new();
    arg_str(&mut a, token);
    arg_u32(&mut a, surface);
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activate_encodes_the_token_then_the_surface() {
        let a = activate_args("abc", 7);
        // Length 4 (with the NUL), "abc\0", then the surface id.
        assert_eq!(a, [4, 0, 0, 0, b'a', b'b', b'c', 0, 7, 0, 0, 0]);
        assert_eq!(read_str(&a, 0), Some(("abc".into(), 8)));
    }

    #[test]
    fn attention_activates_with_a_fresh_token() {
        use super::super::clipboard::tests::{conn_pair, message, read_request};
        let (mut conn, compositor) = conn_pair();
        assert!(!request(&mut conn, 7), "no xdg-activation");
        setup(&mut conn, Some(3), None);
        assert!(!ring(&mut conn, 7), "no system bell");
        assert!(request(&mut conn, 7));
        let (object, opcode, args, _) = read_request(&compositor);
        assert_eq!((object, opcode), (3, ACTIVATION_GET_TOKEN));
        let token = u32::from_le_bytes(args[..4].try_into().unwrap());
        let (object, opcode, args, _) = read_request(&compositor);
        assert_eq!(
            (object, opcode, args),
            (token, TOKEN_SET_SURFACE, 7u32.to_le_bytes().to_vec())
        );
        let (object, opcode, _, _) = read_request(&compositor);
        assert_eq!((object, opcode), (token, TOKEN_COMMIT));
        // The compositor hands the token back: activate with it.
        let mut done = Vec::new();
        arg_str(&mut done, "tok-1");
        let event = message(token, TOKEN_DONE, &done);
        assert!(handle(&mut conn, token, TOKEN_DONE, &event[8..]));
        let (object, opcode, args, _) = read_request(&compositor);
        assert_eq!((object, opcode), (3, ACTIVATION_ACTIVATE));
        assert_eq!(args, activate_args("tok-1", 7));
        let (object, opcode, _, _) = read_request(&compositor);
        assert_eq!((object, opcode), (token, TOKEN_DESTROY));
        assert!(!handle(&mut conn, token, TOKEN_DONE, &[]), "done with it");
    }

    #[test]
    fn the_bell_rings_for_the_surface() {
        use super::super::clipboard::tests::{conn_pair, read_request};
        let (mut conn, compositor) = conn_pair();
        setup(&mut conn, None, Some(4));
        assert!(ring(&mut conn, 7));
        let (object, opcode, args, _) = read_request(&compositor);
        assert_eq!(
            (object, opcode, args),
            (4, BELL_RING, 7u32.to_le_bytes().to_vec())
        );
    }
}
