//! The X root window's `AT_SPI_BUS` property: the accessibility bus's
//! address, which `at-spi-bus-launcher` sets on the display it serves (as
//! `libatspi` reads it). Read when the backend connects, and again when the
//! property changes (the launcher restarted); handed to the AT-SPI bridge,
//! which tries it after `$AT_SPI_BUS_ADDRESS` and before the session bus
//! (see `platform::atspi::address`).

use std::collections::VecDeque;
use std::io;
use std::io::Read as _;
use std::sync::atomic::{AtomicU32, Ordering};

use super::{Conn, OP_GET_PROPERTY, finish, intern_atom, rd_u32};

/// The `AT_SPI_BUS` atom, once interned.
static ATOM: AtomicU32 = AtomicU32::new(0);
/// The longest address read, in 4-byte units.
const MAX_WORDS: u32 = 1024;

/// Intern the atom and read the property, before any event is selected
/// (the reply is read straight off the connection).
pub(super) fn read_at_startup(conn: &mut Conn) -> io::Result<()> {
    let atom = intern_atom(conn, b"AT_SPI_BUS")?;
    ATOM.store(atom, Ordering::Relaxed);
    let mut backlog = VecDeque::new();
    let value = read(conn, atom, &mut backlog)?;
    debug_assert!(backlog.is_empty(), "no events are selected yet");
    crate::platform::atspi::set_x11_address(value);
    Ok(())
}

/// For a root `PropertyNotify`: when it is `AT_SPI_BUS` that changed, read
/// it again and hand it over. Events read meanwhile go to `backlog`.
pub(super) fn changed(
    conn: &mut Conn,
    backlog: &mut VecDeque<[u8; 32]>,
    buf: &[u8; 32],
) -> io::Result<()> {
    let atom = ATOM.load(Ordering::Relaxed);
    if atom == 0 || rd_u32(buf, 8) != Some(atom) {
        return Ok(());
    }
    // State 1 is `PropertyDelete`: the launcher went away.
    let value = if buf[16] == 1 {
        None
    } else {
        read(conn, atom, backlog)?
    };
    crate::platform::atspi::set_x11_address(value);
    Ok(())
}

/// The root window's `atom` property as text (`None` when unset).
fn read(
    conn: &mut Conn,
    atom: u32,
    backlog: &mut VecDeque<[u8; 32]>,
) -> io::Result<Option<String>> {
    let mut req = vec![OP_GET_PROPERTY, 0 /* don't delete */, 0, 0];
    req.extend_from_slice(&conn.setup.root.to_le_bytes());
    req.extend_from_slice(&atom.to_le_bytes());
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
                return Ok(property_text(&packet, &body));
            }
            0 if packet[10] == OP_GET_PROPERTY => return Ok(None),
            _ => backlog.push_back(packet),
        }
    }
}

/// A `GetProperty` reply's value as text: `None` when the property is
/// unset (type 0) or isn't 8-bit.
fn property_text(reply: &[u8; 32], body: &[u8]) -> Option<String> {
    let (format, ty) = (reply[1], rd_u32(reply, 8).unwrap_or(0));
    if ty == 0 || format != 8 {
        return None;
    }
    let len = (rd_u32(reply, 16).unwrap_or(0) as usize).min(body.len());
    Some(String::from_utf8_lossy(&body[..len]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_address_from_a_reply() {
        let mut reply = [0u8; 32];
        reply[0] = 1;
        reply[1] = 8;
        reply[8..12].copy_from_slice(&31u32.to_le_bytes()); // STRING
        let addr = b"unix:path=/run/user/1/at-spi/bus,guid=ab";
        reply[16..20].copy_from_slice(&(addr.len() as u32).to_le_bytes());
        let mut body = addr.to_vec();
        body.resize(addr.len().div_ceil(4) * 4, 0);
        assert_eq!(
            property_text(&reply, &body).as_deref(),
            Some("unix:path=/run/user/1/at-spi/bus,guid=ab")
        );
        // Unset: type None.
        reply[8..12].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(property_text(&reply, &body), None);
    }
}
