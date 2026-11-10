//! Dragging out of the app on Wayland: a `wl_data_source` handed to
//! `wl_data_device.start_drag`.
//!
//! When the app hands a drag to the window ([`Window::start_drag`]), we
//! create a source offering `text/uri-list` (files) or the text types, ask
//! for the copy action (from version 3), and start the drag from our surface
//! citing the serial of the button press that began it, with no drag icon.
//! The compositor then owns the pointer: it shows the drag to whichever
//! client is under it, and when one drops and reads the data we get
//! `send(mime, fd)`, written from memory by a short thread. `cancelled` (no
//! drop, or the target refused) and `dnd_finished` end it; the source is
//! destroyed then.
//!
//! A drag that comes back over our own surface arrives through the data
//! device like any other; its drop is read from memory (asking the
//! compositor would have it ask us, mid-wait).
//!
//! [`Window::start_drag`]: crate::platform::Window::start_drag

use super::{
    ACTION_COPY, MANAGER_CREATE_DATA_SOURCE, SOURCE_CANCELLED, SOURCE_DESTROY, SOURCE_OFFER,
    SOURCE_SEND, TEXT_MIMES, URI_LIST, WlConn, arg_str, arg_u32, close, read_str, serve,
};
use crate::platform::DropData;
use std::io::Write;

// wl_data_source: the request added in version 3, and its later events.
const SOURCE_SET_ACTIONS: u16 = 2;
const SOURCE_DND_DROP_PERFORMED: u16 = 3;
const SOURCE_DND_FINISHED: u16 = 4;
// wl_data_device.start_drag.
const DEVICE_START_DRAG: u16 = 0;

/// The drag out of the app in progress.
#[derive(Debug)]
pub(super) struct DragOut {
    source: u32,
    pub(super) data: DropData,
}

/// The types `data` is offered as, best first.
fn mimes(data: &DropData) -> &'static [&'static str] {
    match data {
        DropData::Files(f) if !f.is_empty() => &[URI_LIST],
        DropData::Text(_) => &TEXT_MIMES,
        _ => &[],
    }
}

/// `data` as `mime`: a URI list for files, UTF-8 text (Latin-1 for `STRING`)
/// for text. `None` for a type it isn't offered as.
fn bytes(data: &DropData, mime: &str) -> Option<Vec<u8>> {
    if !mimes(data).iter().any(|m| m.eq_ignore_ascii_case(mime)) {
        return None;
    }
    match data {
        DropData::Files(paths) => Some(crate::platform::uri_list(paths).into_bytes()),
        DropData::Text(text) if mime == "STRING" => Some(
            text.chars()
                .map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?'))
                .collect(),
        ),
        DropData::Text(text) => Some(text.as_bytes().to_vec()),
        DropData::Internal(_) => None,
    }
}

/// Start dragging `data` out of `surface`. Returns whether the drag started
/// (there is a data device, a button press to cite, and something to offer).
pub(in super::super) fn start(conn: &mut WlConn, surface: u32, data: &DropData) -> bool {
    let (manager, device) = (conn.clip.manager, conn.clip.device);
    let offered = mimes(data);
    let Some(serial) = conn.clip.press_serial else {
        return false;
    };
    if device == 0 || offered.is_empty() {
        return false;
    }
    // One drag at a time: a source still waiting for its end goes.
    end(conn);
    let source = conn.new_id();
    let mut a = Vec::new();
    arg_u32(&mut a, source);
    if conn.send(manager, MANAGER_CREATE_DATA_SOURCE, &a).is_err() {
        return false;
    }
    for mime in offered {
        let mut a = Vec::new();
        arg_str(&mut a, mime);
        let _ = conn.send(source, SOURCE_OFFER, &a);
    }
    if conn.clip.version >= 3 {
        let mut a = Vec::new();
        arg_u32(&mut a, ACTION_COPY);
        let _ = conn.send(source, SOURCE_SET_ACTIONS, &a);
    }
    let mut a = Vec::new();
    arg_u32(&mut a, source);
    arg_u32(&mut a, surface); // origin
    arg_u32(&mut a, 0); // no icon
    arg_u32(&mut a, serial);
    let started = conn.send(device, DEVICE_START_DRAG, &a).is_ok();
    let _ = conn.stream.flush();
    conn.clip.drag_out = Some(DragOut {
        source,
        data: data.clone(),
    });
    started
}

/// Forget the drag: destroy its source. Also called on each button press:
/// the compositor gives the pointer back only once a drag is over, and
/// before version 3 nothing else says a drop that worked is done.
pub(in super::super) fn end(conn: &mut WlConn) {
    if let Some(old) = conn.clip.drag_out.take() {
        let _ = conn.send(old.source, SOURCE_DESTROY, &[]);
        let _ = conn.stream.flush();
        conn.clip.dead_sources.push(old.source);
    }
}

/// Handle an event for the drag's source. Returns whether it was one.
pub(super) fn handle(conn: &mut WlConn, object: u32, opcode: u16, args: &[u8]) -> bool {
    let Some(drag) = conn.clip.drag_out.as_ref().filter(|d| d.source == object) else {
        return false;
    };
    match opcode {
        SOURCE_SEND => {
            // send(mime, fd): the fd travels alongside the message.
            let mime = read_str(args, 0).map(|(m, _)| m).unwrap_or_default();
            let data = bytes(&drag.data, &mime);
            if let Some(fd) = conn.fds.pop_front() {
                match data {
                    Some(data) => serve(fd, data),
                    None => unsafe {
                        close(fd);
                    },
                }
            }
        }
        SOURCE_CANCELLED | SOURCE_DND_FINISHED => {
            super::super::dbg(format_args!(
                "dnd source: {}",
                if opcode == SOURCE_CANCELLED {
                    "cancelled"
                } else {
                    "finished"
                }
            ));
            end(conn);
        }
        // target(mime), dnd_drop_performed, action(a): nothing to do.
        SOURCE_DND_DROP_PERFORMED => {}
        _ => {}
    }
    true
}

#[cfg(test)]
mod tests {
    use super::super::tests::{conn_pair, message, read_request};
    use super::super::{O_CLOEXEC, pipe2, word};
    use super::*;
    use std::io::Read;
    use std::os::fd::FromRawFd;

    #[test]
    fn offers_by_kind_and_converts() {
        let files = DropData::Files(vec!["/tmp/a b".into()]);
        assert_eq!(mimes(&files), &[URI_LIST]);
        assert_eq!(
            bytes(&files, URI_LIST).unwrap(),
            b"file:///tmp/a%20b\r\n".to_vec()
        );
        assert_eq!(bytes(&files, "text/plain"), None);
        let text = DropData::Text("h\u{e9}".into());
        assert_eq!(bytes(&text, "UTF8_STRING").unwrap(), "h\u{e9}".as_bytes());
        assert_eq!(bytes(&text, "STRING").unwrap(), vec![b'h', 0xe9]);
        assert!(mimes(&DropData::Internal("x".into())).is_empty());
    }

    #[test]
    fn starts_a_drag_serves_a_send_and_ends_on_finish() {
        let (mut conn, compositor) = conn_pair();
        conn.clip.device = 10;
        conn.clip.manager = 9;
        conn.clip.version = 3;
        let files = DropData::Files(vec!["/tmp/out.txt".into()]);
        // No press to cite: no drag.
        assert!(!start(&mut conn, 3, &files));
        conn.clip.press_serial = Some(42);
        assert!(start(&mut conn, 3, &files));
        let source = conn.clip.drag_out.as_ref().unwrap().source;
        // create_data_source, offer(uri-list), set_actions(copy), then
        // start_drag(source, origin, no icon, serial).
        let (object, opcode, args, _) = read_request(&compositor);
        assert_eq!(
            (object, opcode, word(&args, 0)),
            (9, MANAGER_CREATE_DATA_SOURCE, source)
        );
        let (object, opcode, args, _) = read_request(&compositor);
        assert_eq!((object, opcode), (source, SOURCE_OFFER));
        assert_eq!(read_str(&args, 0).unwrap().0, URI_LIST);
        let (object, opcode, args, _) = read_request(&compositor);
        assert_eq!(
            (object, opcode, word(&args, 0)),
            (source, SOURCE_SET_ACTIONS, 1)
        );
        let (object, opcode, args, _) = read_request(&compositor);
        assert_eq!((object, opcode), (10, DEVICE_START_DRAG));
        assert_eq!([0, 4, 8, 12].map(|i| word(&args, i)), [source, 3, 0, 42]);

        // The target reads it: send(uri-list, fd).
        let mut ends = [0i32; 2];
        assert_eq!(unsafe { pipe2(ends.as_mut_ptr(), O_CLOEXEC) }, 0);
        let mut args = Vec::new();
        arg_str(&mut args, URI_LIST);
        let sent = message(source, SOURCE_SEND, &args);
        crate::platform::scm::send_with_fds(&compositor, &sent, &[ends[1]]).unwrap();
        unsafe { close(ends[1]) };
        let (object, opcode, args) = conn.recv().unwrap();
        assert!(super::super::handle(&mut conn, object, opcode, &args));
        let mut got = String::new();
        let mut reader = unsafe { std::fs::File::from_raw_fd(ends[0]) };
        reader.read_to_string(&mut got).unwrap();
        assert_eq!(got, "file:///tmp/out.txt\r\n");

        // dnd_finished: the source is destroyed.
        (&compositor)
            .write_all(&message(source, SOURCE_DND_FINISHED, &[]))
            .unwrap();
        let (object, opcode, args) = conn.recv().unwrap();
        assert!(super::super::handle(&mut conn, object, opcode, &args));
        assert!(conn.clip.drag_out.is_none());
        assert!(conn.clip.dead_sources.contains(&source));
        let (object, opcode, _, _) = read_request(&compositor);
        assert_eq!((object, opcode), (source, SOURCE_DESTROY));
    }
}
