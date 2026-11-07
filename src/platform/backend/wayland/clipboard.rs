//! The system clipboard on Wayland: `wl_data_device_manager`, `wl_data_device`,
//! `wl_data_source` and `wl_data_offer`, for plain text.
//!
//! - **Copying** creates a `wl_data_source` offering the text types and makes it
//!   the selection, using the serial of the input event that triggered the copy.
//!   When another client pastes, the compositor sends `send(mime, fd)`; a short
//!   thread writes the text into that fd, so a slow reader can't stall the loop.
//! - **Pasting** reads the current selection's `wl_data_offer`: we create a pipe,
//!   pass its write end with `receive(mime, fd)`, and read the other end until the
//!   source closes it. While waiting, the compositor's messages are read and
//!   queued for the event loop, so the connection never backs up; the read gives
//!   up after a second. When the selection is our own, the text comes straight
//!   from memory: asking the compositor would have it ask us, mid-wait.
//!
//! Selection events only reach the client with keyboard focus, which is also the
//! only one that can paste.

use super::{WL_DISPLAY, WlConn, arg_str, arg_u32, close, read_str};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::time::{Duration, Instant};

// wl_data_device_manager requests.
const MANAGER_CREATE_DATA_SOURCE: u16 = 0;
const MANAGER_GET_DATA_DEVICE: u16 = 1;
// wl_data_source requests, then events.
const SOURCE_OFFER: u16 = 0;
const SOURCE_DESTROY: u16 = 1;
const SOURCE_SEND: u16 = 1;
const SOURCE_CANCELLED: u16 = 2;
// wl_data_device requests, then events.
const DEVICE_SET_SELECTION: u16 = 1;
const DEVICE_DATA_OFFER: u16 = 0;
const DEVICE_LEAVE: u16 = 2;
const DEVICE_DROP: u16 = 4;
const DEVICE_SELECTION: u16 = 5;
// wl_data_offer requests, then events.
const OFFER_RECEIVE: u16 = 1;
const OFFER_DESTROY: u16 = 2;
const OFFER_OFFER: u16 = 0;
// wl_display.delete_id: the server is done with a client object id.
const DISPLAY_DELETE_ID: u16 = 1;

/// The highest `wl_data_device_manager` version we bind. v3 only adds
/// drag-and-drop actions, which we ignore.
pub(super) const MANAGER_VERSION: u32 = 3;

/// The text types we offer, and accept in this order of preference.
const TEXT_MIMES: [&str; 5] = [
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
    "TEXT",
    "STRING",
];

/// How long a paste waits for the source to finish writing.
const READ_TIMEOUT: Duration = Duration::from_secs(1);

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}
const POLLIN: i16 = 1;
const O_CLOEXEC: i32 = 0o2_000_000;

unsafe extern "C" {
    fn pipe2(fds: *mut i32, flags: i32) -> i32;
    fn poll(fds: *mut PollFd, n: std::ffi::c_ulong, timeout_ms: i32) -> i32;
}

/// Clipboard state, kept on the connection.
#[derive(Debug, Default)]
pub(super) struct Clipboard {
    manager: u32,
    /// Our `wl_data_device`; 0 without a seat or a data-device manager.
    device: u32,
    /// The serial of the latest input event, which `set_selection` must cite.
    pub(super) serial: u32,
    /// Our `wl_data_source` while we own the selection.
    source: Option<u32>,
    /// The text `source` serves.
    text: String,
    /// Sources we destroyed whose ids the server hasn't released yet: a `send`
    /// for one may still be in flight, and its fd must be closed.
    dead_sources: Vec<u32>,
    /// Offers the compositor introduced, with the types each one announced.
    offers: Vec<(u32, Vec<String>)>,
    /// The offer holding the current selection.
    selection: Option<u32>,
}

/// Create the seat's data device.
pub(super) fn setup(conn: &mut WlConn, manager: u32, seat: u32) {
    let device = conn.new_id();
    let mut a = Vec::new();
    arg_u32(&mut a, device);
    arg_u32(&mut a, seat);
    if conn.send(manager, MANAGER_GET_DATA_DEVICE, &a).is_ok() {
        conn.clip.manager = manager;
        conn.clip.device = device;
    }
}

/// The best text type among `mimes`.
fn pick_mime(mimes: &[String]) -> Option<&'static str> {
    TEXT_MIMES
        .into_iter()
        .find(|want| mimes.iter().any(|m| m.eq_ignore_ascii_case(want)))
}

fn word(args: &[u8], i: usize) -> u32 {
    args.get(i..i + 4)
        .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Handle a clipboard event. Returns whether it was one.
pub(super) fn handle(conn: &mut WlConn, object: u32, opcode: u16, args: &[u8]) -> bool {
    let clip = &mut conn.clip;
    if clip.device == 0 {
        return false;
    }
    if object == clip.device {
        match opcode {
            DEVICE_DATA_OFFER => clip.offers.push((word(args, 0), Vec::new())),
            DEVICE_SELECTION => {
                let offer = Some(word(args, 0)).filter(|&id| id != 0);
                clip.selection = offer;
                // Drop every other offer: the previous selection, and any left
                // from a drag that crossed the window.
                destroy_offers_except(conn, offer);
            }
            DEVICE_LEAVE | DEVICE_DROP => {
                let keep = clip.selection;
                destroy_offers_except(conn, keep);
            }
            _ => {}
        }
        return true;
    }
    if let Some((_, mimes)) = clip.offers.iter_mut().find(|(id, _)| *id == object) {
        if opcode == OFFER_OFFER
            && let Some((mime, _)) = read_str(args, 0)
        {
            mimes.push(mime);
        }
        return true;
    }
    let ours = clip.source == Some(object);
    if ours || clip.dead_sources.contains(&object) {
        match opcode {
            SOURCE_SEND => {
                // send(mime, fd): the fd travels alongside the message.
                if let Some(fd) = conn.fds.pop_front() {
                    if ours {
                        serve(fd, conn.clip.text.clone());
                    } else {
                        unsafe { close(fd) };
                    }
                }
            }
            SOURCE_CANCELLED if ours => {
                // Another client took the selection.
                let _ = conn.send(object, SOURCE_DESTROY, &[]);
                conn.clip.dead_sources.push(object);
                conn.clip.source = None;
                conn.clip.text.clear();
            }
            _ => {}
        }
        return true;
    }
    if object == WL_DISPLAY && opcode == DISPLAY_DELETE_ID {
        let id = word(args, 0);
        conn.clip.dead_sources.retain(|&s| s != id);
        return true;
    }
    false
}

fn destroy_offers_except(conn: &mut WlConn, keep: Option<u32>) {
    let offers = std::mem::take(&mut conn.clip.offers);
    for (id, mimes) in offers {
        if Some(id) == keep {
            conn.clip.offers.push((id, mimes));
        } else {
            let _ = conn.send(id, OFFER_DESTROY, &[]);
        }
    }
}

/// Write `text` into a pasting client's fd, then close it, off the event loop.
fn serve(fd: i32, text: String) {
    // SAFETY: the fd was passed to us with `send` and nothing else owns it.
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    std::thread::spawn(move || {
        // A reader that went away gives EPIPE (Rust ignores SIGPIPE); nothing to do.
        let _ = file.write_all(text.as_bytes());
    });
}

/// Make `text` the selection.
pub(super) fn set_text(conn: &mut WlConn, text: &str) {
    let (manager, device) = (conn.clip.manager, conn.clip.device);
    if device == 0 {
        return;
    }
    if let Some(old) = conn.clip.source.take() {
        let _ = conn.send(old, SOURCE_DESTROY, &[]);
        conn.clip.dead_sources.push(old);
    }
    let source = conn.new_id();
    let mut a = Vec::new();
    arg_u32(&mut a, source);
    if conn.send(manager, MANAGER_CREATE_DATA_SOURCE, &a).is_err() {
        return;
    }
    for mime in TEXT_MIMES {
        let mut a = Vec::new();
        arg_str(&mut a, mime);
        let _ = conn.send(source, SOURCE_OFFER, &a);
    }
    let mut a = Vec::new();
    arg_u32(&mut a, source);
    arg_u32(&mut a, conn.clip.serial);
    let _ = conn.send(device, DEVICE_SET_SELECTION, &a);
    let _ = conn.stream.flush();
    conn.clip.source = Some(source);
    conn.clip.text = text.to_string();
}

/// Read the selection's text, if it has any.
pub(super) fn text(conn: &mut WlConn) -> Option<String> {
    if conn.clip.source.is_some() {
        return Some(conn.clip.text.clone());
    }
    let offer = conn.clip.selection?;
    let mime = conn
        .clip
        .offers
        .iter()
        .find(|(id, _)| *id == offer)
        .and_then(|(_, mimes)| pick_mime(mimes))?;
    let mut ends = [0i32; 2];
    if unsafe { pipe2(ends.as_mut_ptr(), O_CLOEXEC) } < 0 {
        return None;
    }
    // SAFETY: both ends are fresh fds that nothing else owns.
    let (reader, writer) = unsafe {
        (
            std::fs::File::from_raw_fd(ends[0]),
            std::fs::File::from_raw_fd(ends[1]),
        )
    };
    let mut a = Vec::new();
    arg_str(&mut a, mime);
    let sent = conn.send_with_fd(offer, OFFER_RECEIVE, &a, writer.as_raw_fd());
    let _ = conn.stream.flush();
    // Our copy of the write end must close, or the read never sees end-of-file.
    drop(writer);
    sent.ok()?;
    let bytes = read_pipe(conn, reader)?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Read `reader` to end-of-file, queueing the compositor's messages meanwhile.
/// `None` on a timeout or an error.
fn read_pipe(conn: &mut WlConn, mut reader: std::fs::File) -> Option<Vec<u8>> {
    let deadline = Instant::now() + READ_TIMEOUT;
    let sock = conn.stream.as_raw_fd();
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        let mut fds = [
            PollFd {
                fd: reader.as_raw_fd(),
                events: POLLIN,
                revents: 0,
            },
            PollFd {
                fd: sock,
                events: POLLIN,
                revents: 0,
            },
        ];
        let ms = left.as_millis().clamp(1, i32::MAX as u128) as i32;
        if unsafe { poll(fds.as_mut_ptr(), 2, ms) } < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return None;
        }
        if fds[1].revents != 0 {
            let message = conn.recv().ok()?;
            conn.queue.push_back(message);
        }
        if fds[0].revents != 0 {
            match reader.read(&mut buf) {
                Ok(0) => return Some(out),
                Ok(n) => out.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_utf8_text() {
        let offered = |m: &[&str]| m.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            pick_mime(&offered(&["text/plain", "UTF8_STRING", "text/html"])),
            Some("UTF8_STRING")
        );
        assert_eq!(
            pick_mime(&offered(&["text/plain;charset=UTF-8", "text/plain"])),
            Some("text/plain;charset=utf-8")
        );
        assert_eq!(pick_mime(&offered(&["image/png"])), None);
    }

    use std::os::unix::net::UnixStream;

    /// A connection whose other end plays the compositor.
    fn conn_pair() -> (WlConn, UnixStream) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let conn = WlConn {
            stream: ours,
            next_id: 2,
            fds: Default::default(),
            queue: Default::default(),
            clip: Clipboard::default(),
            buffer_scale: 1,
        };
        (conn, theirs)
    }

    /// Encode an event as the compositor would.
    fn message(object: u32, opcode: u16, args: &[u8]) -> Vec<u8> {
        let mut m = object.to_le_bytes().to_vec();
        m.extend_from_slice(&((((8 + args.len()) as u32) << 16) | opcode as u32).to_le_bytes());
        m.extend_from_slice(args);
        m
    }

    /// Read one request's header and arguments, plus any fd passed with it.
    fn read_request(sock: &UnixStream) -> (u32, u16, Vec<u8>, Vec<i32>) {
        let mut fds = Vec::new();
        let mut header = [0u8; 8];
        let n = crate::platform::scm::recv_with_fds(sock, &mut header, &mut fds, 4).unwrap();
        assert_eq!(n, 8);
        let object = u32::from_le_bytes(header[0..4].try_into().unwrap());
        let word = u32::from_le_bytes(header[4..8].try_into().unwrap());
        let mut args = vec![0u8; (word >> 16) as usize - 8];
        (&*sock).read_exact(&mut args).unwrap();
        (object, (word & 0xffff) as u16, args, fds)
    }

    #[test]
    fn paste_reads_the_offer_through_a_pipe_and_queues_events_meanwhile() {
        let (mut conn, compositor) = conn_pair();
        conn.clip.device = 10;
        conn.clip.manager = 9;
        // The compositor introduces an offer, its types, and makes it the selection.
        let offer: u32 = 0xff00_0001;
        let mut events = Vec::new();
        events.extend(message(10, DEVICE_DATA_OFFER, &offer.to_le_bytes()));
        let mut mime = Vec::new();
        arg_str(&mut mime, "text/plain");
        events.extend(message(offer, OFFER_OFFER, &mime));
        let mut mime = Vec::new();
        arg_str(&mut mime, "text/plain;charset=utf-8");
        events.extend(message(offer, OFFER_OFFER, &mime));
        events.extend(message(10, DEVICE_SELECTION, &offer.to_le_bytes()));
        (&compositor).write_all(&events).unwrap();
        for _ in 0..4 {
            let (object, opcode, args) = conn.recv().unwrap();
            assert!(handle(&mut conn, object, opcode, &args));
        }
        assert_eq!(conn.clip.selection, Some(offer));

        let source = std::thread::spawn(move || {
            let (object, opcode, args, fds) = read_request(&compositor);
            assert_eq!((object, opcode), (offer, OFFER_RECEIVE));
            assert_eq!(read_str(&args, 0).unwrap().0, "text/plain;charset=utf-8");
            assert_eq!(fds.len(), 1);
            // An unrelated event arrives while the paste waits.
            (&compositor).write_all(&message(42, 0, &[])).unwrap();
            std::thread::sleep(Duration::from_millis(50));
            let mut pipe = unsafe { std::fs::File::from_raw_fd(fds[0]) };
            pipe.write_all("pasted \u{2713}".as_bytes()).unwrap();
            compositor
        });
        assert_eq!(text(&mut conn).as_deref(), Some("pasted \u{2713}"));
        let _compositor = source.join().unwrap();
        assert_eq!(conn.queue.pop_front(), Some((42, 0, Vec::new())));
    }

    #[test]
    fn copy_offers_text_and_serves_a_send() {
        let (mut conn, compositor) = conn_pair();
        conn.clip.device = 10;
        conn.clip.manager = 9;
        conn.clip.serial = 77;
        set_text(&mut conn, "copied");
        let source = conn.clip.source.unwrap();
        // create_data_source, five offers, then set_selection(source, serial).
        let (object, opcode, args, _) = read_request(&compositor);
        assert_eq!(
            (object, opcode, word(&args, 0)),
            (9, MANAGER_CREATE_DATA_SOURCE, source)
        );
        for want in TEXT_MIMES {
            let (object, opcode, args, _) = read_request(&compositor);
            assert_eq!((object, opcode), (source, SOURCE_OFFER));
            assert_eq!(read_str(&args, 0).unwrap().0, want);
        }
        let (object, opcode, args, _) = read_request(&compositor);
        assert_eq!((object, opcode), (10, DEVICE_SET_SELECTION));
        assert_eq!((word(&args, 0), word(&args, 4)), (source, 77));
        // Our own selection reads back without a round trip.
        assert_eq!(text(&mut conn).as_deref(), Some("copied"));

        // Another client pastes: send(mime, fd).
        let mut ends = [0i32; 2];
        assert_eq!(unsafe { pipe2(ends.as_mut_ptr(), O_CLOEXEC) }, 0);
        let mut args = Vec::new();
        arg_str(&mut args, "UTF8_STRING");
        let sent = message(source, SOURCE_SEND, &args);
        crate::platform::scm::send_with_fds(&compositor, &sent, &[ends[1]]).unwrap();
        unsafe { close(ends[1]) };
        let (object, opcode, args) = conn.recv().unwrap();
        assert!(handle(&mut conn, object, opcode, &args));
        let mut got = String::new();
        let mut reader = unsafe { std::fs::File::from_raw_fd(ends[0]) };
        reader.read_to_string(&mut got).unwrap();
        assert_eq!(got, "copied");

        // Losing the selection forgets the text.
        (&compositor)
            .write_all(&message(source, SOURCE_CANCELLED, &[]))
            .unwrap();
        let (object, opcode, args) = conn.recv().unwrap();
        assert!(handle(&mut conn, object, opcode, &args));
        assert_eq!(conn.clip.source, None);
        assert!(conn.clip.dead_sources.contains(&source));
    }

    #[test]
    fn serves_text_into_the_fd_and_closes_it() {
        let mut ends = [0i32; 2];
        assert_eq!(unsafe { pipe2(ends.as_mut_ptr(), O_CLOEXEC) }, 0);
        serve(ends[1], "hello \u{2713}".to_string());
        let mut reader = unsafe { std::fs::File::from_raw_fd(ends[0]) };
        let mut got = String::new();
        reader.read_to_string(&mut got).unwrap();
        assert_eq!(got, "hello \u{2713}");
    }
}
