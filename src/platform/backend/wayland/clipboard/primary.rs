//! The primary selection on Wayland: `zwp_primary_selection_device_manager_v1`
//! (from `primary-selection-unstable-v1`), whose device, source and offer
//! work like their `wl_data_*` counterparts minus drag and drop. A text
//! field's selection is offered as the primary selection; a middle click
//! reads it, through a pipe like a paste.

use super::{Payload, WlConn, arg_str, arg_u32, close, format, read_offer, read_str, serve, word};
use crate::platform::clipboard::{ClipboardContent, ClipboardFormats};

/// The global's interface name.
pub(in super::super) const MANAGER: &str = "zwp_primary_selection_device_manager_v1";

// zwp_primary_selection_device_manager_v1 requests.
const MANAGER_CREATE_SOURCE: u16 = 0;
const MANAGER_GET_DEVICE: u16 = 1;
// zwp_primary_selection_device_v1: requests, then events.
const DEVICE_SET_SELECTION: u16 = 0;
const DEVICE_DATA_OFFER: u16 = 0;
const DEVICE_SELECTION: u16 = 1;
// zwp_primary_selection_offer_v1: requests, then events.
const OFFER_RECEIVE: u16 = 0;
const OFFER_DESTROY: u16 = 1;
const OFFER_OFFER: u16 = 0;
// zwp_primary_selection_source_v1: requests, then events.
const SOURCE_OFFER: u16 = 0;
const SOURCE_DESTROY: u16 = 1;
const SOURCE_SEND: u16 = 0;
const SOURCE_CANCELLED: u16 = 1;

/// The primary selection's state, kept on the connection.
#[derive(Debug, Default)]
pub(in super::super) struct Primary {
    manager: u32,
    /// Our device; 0 without a seat or the protocol.
    device: u32,
    /// Our source while we own the selection, and what it serves.
    source: Option<u32>,
    payload: Payload,
    /// Sources destroyed whose ids the server hasn't released yet.
    dead_sources: Vec<u32>,
    /// Offers introduced, with their types.
    offers: Vec<(u32, Vec<String>)>,
    /// The offer holding the selection.
    selection: Option<u32>,
}

impl Primary {
    /// Whether the compositor has a primary selection for us.
    pub(in super::super) fn available(&self) -> bool {
        self.device != 0
    }

    /// The server released object `id`.
    pub(super) fn released(&mut self, id: u32) {
        self.dead_sources.retain(|&s| s != id);
    }
}

/// Create the seat's primary-selection device.
pub(in super::super) fn setup(conn: &mut WlConn, manager: u32, seat: u32) {
    let device = conn.new_id();
    let mut a = Vec::new();
    arg_u32(&mut a, device);
    arg_u32(&mut a, seat);
    if conn.send(manager, MANAGER_GET_DEVICE, &a).is_ok() {
        conn.clip.primary.manager = manager;
        conn.clip.primary.device = device;
    }
}

/// Handle a primary-selection event. Returns whether it was one.
pub(super) fn handle(conn: &mut WlConn, object: u32, opcode: u16, args: &[u8]) -> bool {
    let p = &mut conn.clip.primary;
    if p.device == 0 {
        return false;
    }
    if object == p.device {
        match opcode {
            DEVICE_DATA_OFFER => p.offers.push((word(args, 0), Vec::new())),
            DEVICE_SELECTION => {
                let offer = Some(word(args, 0)).filter(|&id| id != 0);
                p.selection = offer;
                // Drop the previous selection's offer.
                let offers = std::mem::take(&mut p.offers);
                for (id, mimes) in offers {
                    if Some(id) == offer {
                        conn.clip.primary.offers.push((id, mimes));
                    } else {
                        let _ = conn.send(id, OFFER_DESTROY, &[]);
                    }
                }
            }
            _ => {}
        }
        return true;
    }
    if let Some((_, mimes)) = p.offers.iter_mut().find(|(id, _)| *id == object) {
        if opcode == OFFER_OFFER
            && let Some((mime, _)) = read_str(args, 0)
        {
            mimes.push(mime);
        }
        return true;
    }
    let ours = p.source == Some(object);
    if !ours && !p.dead_sources.contains(&object) {
        return false;
    }
    match opcode {
        SOURCE_SEND => {
            if let Some(fd) = conn.fds.pop_front() {
                let mime = read_str(args, 0).map(|(m, _)| m).unwrap_or_default();
                let p = &conn.clip.primary;
                match p.payload.bytes(&mime).filter(|_| ours) {
                    Some(bytes) => serve(fd, bytes),
                    None => {
                        unsafe { close(fd) };
                    }
                }
            }
        }
        SOURCE_CANCELLED if ours => {
            // Another client took the selection.
            let _ = conn.send(object, SOURCE_DESTROY, &[]);
            let p = &mut conn.clip.primary;
            p.dead_sources.push(object);
            p.source = None;
            p.payload = Payload::default();
        }
        _ => {}
    }
    true
}

/// Make `content` the primary selection, citing the latest input serial.
pub(in super::super) fn set(conn: &mut WlConn, content: &ClipboardContent) {
    let (manager, device) = (conn.clip.primary.manager, conn.clip.primary.device);
    if device == 0 {
        return;
    }
    if let Some(old) = conn.clip.primary.source.take() {
        let _ = conn.send(old, SOURCE_DESTROY, &[]);
        conn.clip.primary.dead_sources.push(old);
    }
    let payload = Payload::new(content.clone());
    let source = conn.new_id();
    let mut a = Vec::new();
    arg_u32(&mut a, source);
    if conn.send(manager, MANAGER_CREATE_SOURCE, &a).is_err() {
        return;
    }
    for mime in payload.mimes() {
        let mut a = Vec::new();
        arg_str(&mut a, mime);
        let _ = conn.send(source, SOURCE_OFFER, &a);
    }
    let mut a = Vec::new();
    arg_u32(&mut a, source);
    arg_u32(&mut a, conn.clip.serial);
    let _ = conn.send(device, DEVICE_SET_SELECTION, &a);
    let _ = std::io::Write::flush(&mut conn.stream);
    conn.clip.primary.source = Some(source);
    conn.clip.primary.payload = payload;
}

/// Read the primary selection in `formats`.
pub(in super::super) fn read(
    conn: &mut WlConn,
    formats: ClipboardFormats,
) -> Option<ClipboardContent> {
    let p = &conn.clip.primary;
    if p.source.is_some() {
        return Some(p.payload.content.clone().only(formats)).filter(|c| !c.is_empty());
    }
    let offer = p.selection?;
    let mimes = p
        .offers
        .iter()
        .find(|(id, _)| *id == offer)
        .map(|(_, mimes)| format::pick(mimes, formats))?;
    read_offer(conn, offer, OFFER_RECEIVE, &mimes)
}

#[cfg(test)]
mod tests {
    use super::super::tests::{conn_pair, message, read_request};
    use super::*;
    use std::io::{Read, Write};
    use std::os::fd::FromRawFd;

    #[test]
    fn owns_serves_and_reads_the_primary_selection() {
        let (mut conn, compositor) = conn_pair();
        conn.clip.primary.manager = 20;
        conn.clip.primary.device = 21;
        conn.clip.serial = 5;
        set(&mut conn, &ClipboardContent::text("sel"));
        let source = conn.clip.primary.source.unwrap();
        let (object, opcode, args, _) = read_request(&compositor);
        assert_eq!(
            (object, opcode, word(&args, 0)),
            (20, MANAGER_CREATE_SOURCE, source)
        );
        for want in format::TEXT_MIMES {
            let (object, opcode, args, _) = read_request(&compositor);
            assert_eq!((object, opcode), (source, SOURCE_OFFER));
            assert_eq!(read_str(&args, 0).unwrap().0, want);
        }
        let (object, opcode, args, _) = read_request(&compositor);
        assert_eq!((object, opcode), (21, DEVICE_SET_SELECTION));
        assert_eq!((word(&args, 0), word(&args, 4)), (source, 5));
        assert_eq!(
            read(&mut conn, ClipboardFormats::TEXT),
            Some(ClipboardContent::text("sel"))
        );

        // Another client reads it: send(mime, fd).
        let mut ends = [0i32; 2];
        assert_eq!(
            unsafe { super::super::pipe2(ends.as_mut_ptr(), super::super::O_CLOEXEC) },
            0
        );
        let mut args = Vec::new();
        arg_str(&mut args, "text/plain;charset=utf-8");
        let sent = message(source, SOURCE_SEND, &args);
        crate::platform::scm::send_with_fds(&compositor, &sent, &[ends[1]]).unwrap();
        unsafe { close(ends[1]) };
        let (object, opcode, args) = conn.recv().unwrap();
        assert!(super::super::handle(&mut conn, object, opcode, &args));
        let mut got = String::new();
        let mut reader = unsafe { std::fs::File::from_raw_fd(ends[0]) };
        reader.read_to_string(&mut got).unwrap();
        assert_eq!(got, "sel");

        // Another client takes it, and offers its own.
        let offer: u32 = 0xff00_0009;
        let mut events = message(source, SOURCE_CANCELLED, &[]);
        events.extend(message(21, DEVICE_DATA_OFFER, &offer.to_le_bytes()));
        let mut mime = Vec::new();
        arg_str(&mut mime, "UTF8_STRING");
        events.extend(message(offer, OFFER_OFFER, &mime));
        events.extend(message(21, DEVICE_SELECTION, &offer.to_le_bytes()));
        (&compositor).write_all(&events).unwrap();
        for _ in 0..4 {
            let (object, opcode, args) = conn.recv().unwrap();
            assert!(super::super::handle(&mut conn, object, opcode, &args));
        }
        assert_eq!(conn.clip.primary.source, None);
        let (object, opcode, _, _) = read_request(&compositor);
        assert_eq!((object, opcode), (source, SOURCE_DESTROY));
        let owner = std::thread::spawn(move || {
            let (object, opcode, args, fds) = read_request(&compositor);
            assert_eq!((object, opcode), (offer, OFFER_RECEIVE));
            assert_eq!(read_str(&args, 0).unwrap().0, "UTF8_STRING");
            let mut pipe = unsafe { std::fs::File::from_raw_fd(fds[0]) };
            pipe.write_all(b"theirs").unwrap();
            // Keep the connection open until the read is done.
            compositor
        });
        assert_eq!(
            read(&mut conn, ClipboardFormats::TEXT),
            Some(ClipboardContent::text("theirs"))
        );
        let _compositor = owner.join().unwrap();
    }
}
