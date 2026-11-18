//! Following the display's scale while the app runs.
//!
//! On X11 the scale is `Xft.dpi` in the root window's `RESOURCE_MANAGER`
//! property (96 DPI is 1x), which desktops rewrite when the user changes the
//! scale in their settings (GNOME's and Xfce's settings daemons, KDE's, or a
//! plain `xrdb -merge`). The backend listens for property changes on the root
//! window, and when `RESOURCE_MANAGER` changes it reads `Xft.dpi` again; a
//! new scale reaches every window as `ScaleFactorChanged`, then `Resized` at
//! its (unchanged) physical size so the app lays out again at the new scale.

use std::collections::VecDeque;
use std::io;
use std::io::Read as _;

use super::{Conn, OP_CHANGE_WINDOW_ATTRIBUTES, OP_GET_PROPERTY, finish, rd_u32};

/// The predefined `RESOURCE_MANAGER` atom.
pub(super) const RESOURCE_MANAGER: u32 = 23;
/// `PropertyChangeMask`.
const EV_PROPERTY_CHANGE: u32 = 0x0040_0000;
/// `CWEventMask`.
const CW_EVENT_MASK: u32 = 0x0000_0800;

/// Listen for property changes on the root window (ours alone: each client
/// has its own event mask on a window).
pub(super) fn watch(conn: &mut Conn) -> io::Result<()> {
    let mut req = vec![OP_CHANGE_WINDOW_ATTRIBUTES, 0, 0, 0];
    req.extend_from_slice(&conn.setup.root.to_le_bytes());
    req.extend_from_slice(&CW_EVENT_MASK.to_le_bytes());
    req.extend_from_slice(&EV_PROPERTY_CHANGE.to_le_bytes());
    conn.send(&finish(req))
}

/// Whether `buf` is a `PropertyNotify` of the root window, which [`changed`]
/// takes (and nothing else needs).
pub(super) fn is_root_property(root: u32, buf: &[u8; 32]) -> bool {
    buf[0] & 0x7f == super::X_PROPERTY_NOTIFY && rd_u32(buf, 4) == Some(root)
}

/// For a root `PropertyNotify`: the new scale, when it is `RESOURCE_MANAGER`
/// that changed and its `Xft.dpi` gives another scale than `conn.scale`
/// (which it then becomes). Events read while waiting for the property go to
/// `backlog`.
pub(super) fn changed(
    conn: &mut Conn,
    backlog: &mut VecDeque<[u8; 32]>,
    buf: &[u8; 32],
) -> io::Result<Option<f64>> {
    if rd_u32(buf, 8) != Some(RESOURCE_MANAGER) {
        return Ok(None);
    }
    let resources = resources(conn, backlog)?;
    // No Xft.dpi (the resources were cleared): 1x, as at startup.
    let scale = super::parse_xft_scale(&resources).unwrap_or(1.0);
    if (scale - conn.scale).abs() < 1e-6 {
        return Ok(None);
    }
    super::dbg(format_args!("Xft.dpi scale {} -> {scale}", conn.scale));
    conn.scale = scale;
    Ok(Some(scale))
}

/// The root window's `RESOURCE_MANAGER` text, read while events arrive.
fn resources(conn: &mut Conn, backlog: &mut VecDeque<[u8; 32]>) -> io::Result<String> {
    const MAX_WORDS: u32 = 64 * 1024;
    let mut req = vec![OP_GET_PROPERTY, 0 /* don't delete */, 0, 0];
    req.extend_from_slice(&conn.setup.root.to_le_bytes());
    req.extend_from_slice(&RESOURCE_MANAGER.to_le_bytes());
    req.extend_from_slice(&0u32.to_le_bytes()); // any type
    req.extend_from_slice(&0u32.to_le_bytes()); // offset
    req.extend_from_slice(&MAX_WORDS.to_le_bytes());
    conn.send(&finish(req))?;
    loop {
        let mut packet = [0u8; 32];
        conn.stream.read_exact(&mut packet)?;
        match packet[0] {
            1 => {
                let extra = rd_u32(&packet, 4).unwrap_or(0) as usize * 4;
                let mut body = vec![0u8; extra];
                conn.stream.read_exact(&mut body)?;
                let len = (rd_u32(&packet, 16).unwrap_or(0) as usize).min(extra);
                return Ok(String::from_utf8_lossy(&body[..len]).into_owned());
            }
            0 if packet[10] == OP_GET_PROPERTY => return Ok(String::new()),
            _ => backlog.push_back(packet),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_property_events_are_told_apart() {
        let mut ev = [0u8; 32];
        ev[0] = super::super::X_PROPERTY_NOTIFY | 0x80; // sent
        ev[4..8].copy_from_slice(&0x123u32.to_le_bytes());
        ev[8..12].copy_from_slice(&RESOURCE_MANAGER.to_le_bytes());
        assert!(is_root_property(0x123, &ev));
        assert!(!is_root_property(0x456, &ev), "another window's");
        ev[0] = 12; // Expose
        assert!(!is_root_property(0x123, &ev));
    }
}
