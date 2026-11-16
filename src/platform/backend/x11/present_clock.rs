//! The frame clock's display source on X11: the Present extension's
//! `PresentNotifyMSC`, which the server answers with a `CompleteNotify` at the
//! window's next vertical blank.
//!
//! It runs on a thread with a connection of its own, so the backend's
//! connection never sees Present's generic events (which are longer than the
//! 32 bytes its readers expect). The thread sleeps until a frame is asked for,
//! asks the server for the next MSC (media stream counter: the CRTC's vblank
//! count) on the app's first window, and ticks the frame clock when the
//! notify comes. An idle app has it parked, so it costs nothing.
//!
//! The notify carries the vblank's UST (microseconds) and MSC, and the change
//! in both between two notifies is the display's refresh as the server sees
//! it, which is reported to the frame clock. Under Xvfb, and for a window on
//! no CRTC, the server's fake vblank (60 Hz in Xorg) answers instead.
//!
//! Without Present, or once the window is gone, the source is taken away and
//! the frame clock's timer paces frames at RandR's refresh, as before.

use std::io::{self, Read};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use super::{Conn, dbg, finish, present_query_version, query_extension, rd_u32};

const PRESENT_NOTIFY_MSC: u8 = 2;
const PRESENT_SELECT_INPUT: u8 = 3;
/// `PresentCompleteNotifyMask`.
const COMPLETE_NOTIFY_MASK: u32 = 2;
/// `GenericEvent`'s event code.
const GENERIC_EVENT: u8 = 35;
/// `PresentCompleteNotify`'s event type.
const COMPLETE_NOTIFY: u16 = 1;
/// `PresentCompleteKindNotifyMSC`.
const KIND_NOTIFY_MSC: u8 = 1;

/// How many vblanks the reported refresh is measured over.
const REFRESH_SPAN: u64 = 60;

/// Whether a frame has been asked for and the thread hasn't picked it up yet.
static ARMED: Mutex<bool> = Mutex::new(false);
static WAKE: Condvar = Condvar::new();

/// `PresentSelectInput`: deliver `mask`'s events for `window` to this client,
/// under the event id `eid`.
pub(super) fn select_input_request(major: u8, eid: u32, window: u32, mask: u32) -> Vec<u8> {
    let mut req = vec![major, PRESENT_SELECT_INPUT, 0, 0];
    req.extend_from_slice(&eid.to_le_bytes());
    req.extend_from_slice(&window.to_le_bytes());
    req.extend_from_slice(&mask.to_le_bytes());
    finish(req)
}

/// `PresentNotifyMSC`: a `CompleteNotify` for `window` when its CRTC's MSC
/// reaches `target_msc` (at once if it has passed), with no divisor.
pub(super) fn notify_msc_request(major: u8, window: u32, serial: u32, target_msc: u64) -> Vec<u8> {
    let mut req = vec![major, PRESENT_NOTIFY_MSC, 0, 0];
    req.extend_from_slice(&window.to_le_bytes());
    req.extend_from_slice(&serial.to_le_bytes());
    req.extend_from_slice(&0u32.to_le_bytes()); // pad
    req.extend_from_slice(&target_msc.to_le_bytes());
    req.extend_from_slice(&0u64.to_le_bytes()); // divisor
    req.extend_from_slice(&0u64.to_le_bytes()); // remainder
    finish(req)
}

/// A `CompleteNotify` of kind `NotifyMSC`, from the event's bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Complete {
    pub serial: u32,
    /// When the vblank happened, in microseconds.
    pub ust: u64,
    pub msc: u64,
}

fn rd_u64(b: &[u8], off: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(off..off + 8)?.try_into().ok()?))
}

/// Parse one packet from the thread's connection: `Ok(Some)` for an MSC
/// notify from Present (`major`), `Ok(None)` for anything else, `Err` for
/// an X error (the window is gone).
pub(super) fn parse(packet: &[u8], major: u8) -> Result<Option<Complete>, u8> {
    match packet.first() {
        Some(0) => Err(packet.get(1).copied().unwrap_or(0)),
        Some(&t)
            if t & 0x7f == GENERIC_EVENT
                && packet.get(1) == Some(&major)
                && packet.get(8..10) == Some(&COMPLETE_NOTIFY.to_le_bytes())
                && packet.get(10) == Some(&KIND_NOTIFY_MSC) =>
        {
            Ok((|| {
                Some(Complete {
                    serial: rd_u32(packet, 20)?,
                    ust: rd_u64(packet, 24)?,
                    msc: rd_u64(packet, 32)?,
                })
            })())
        }
        _ => Ok(None),
    }
}

/// The refresh between two notifies, from their UST and MSC.
pub(super) fn refresh(prev: Complete, next: Complete) -> Option<Duration> {
    let frames = next.msc.checked_sub(prev.msc).filter(|&n| n > 0)?;
    let micros = next.ust.checked_sub(prev.ust).filter(|&n| n > 0)?;
    Some(Duration::from_nanos(micros.saturating_mul(1000) / frames))
}

/// How many bytes the packet at the start of `buf` takes, once its header is
/// in: 32, and a generic event's extra words.
fn packet_len(buf: &[u8]) -> Option<usize> {
    if buf.len() < 32 {
        return None;
    }
    let extra = if buf[0] & 0x7f == GENERIC_EVENT || buf[0] == 1 {
        rd_u32(buf, 4).unwrap_or(0) as usize * 4
    } else {
        0
    };
    Some(32 + extra)
}

/// Start the Present clock for `window`, if the server has Present. The
/// connection is made on the thread, so a slow server doesn't hold up the
/// first frame; until it's up, the timer paces frames.
pub(super) fn install(window: u32) {
    if std::env::var_os("STIPPLE_X11_PRESENT_CLOCK").is_some_and(|v| v == "0") {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("stipple-present".into())
        .spawn(move || {
            if let Err(e) = run(window) {
                dbg(format_args!("present clock stopped: {e}"));
            }
            crate::platform::frame::uninstall();
        });
}

fn run(window: u32) -> io::Result<()> {
    let mut conn = Conn::connect().map_err(|e| io::Error::other(format!("{e:?}")))?;
    let Some((smaj, smin)) = present_query_version(&mut conn)? else {
        return Err(io::Error::other("no Present extension"));
    };
    let Some(major) = query_extension(&mut conn, b"Present")? else {
        return Err(io::Error::other("no Present extension"));
    };
    let eid = conn.new_id();
    conn.send(&select_input_request(
        major,
        eid,
        window,
        COMPLETE_NOTIFY_MASK,
    ))?;
    dbg(format_args!(
        "present clock: Present {smaj}.{smin} on window {window:#x}"
    ));
    crate::platform::frame::name_source("Present");
    crate::platform::frame::install(
        Arc::new(|| {
            *ARMED.lock().unwrap_or_else(|e| e.into_inner()) = true;
            WAKE.notify_one();
        }),
        None,
    );
    // A notify that never comes (a window the server doesn't refresh) is
    // given up on after this, and asked for again on the next frame.
    conn.stream
        .set_read_timeout(Some(Duration::from_millis(250)))?;
    let mut buf: Vec<u8> = Vec::new();
    let mut serial = 0u32;
    let mut last: Option<Complete> = None;
    let mut anchor: Option<Complete> = None;
    loop {
        {
            let mut armed = ARMED.lock().unwrap_or_else(|e| e.into_inner());
            while !*armed {
                armed = WAKE.wait(armed).unwrap_or_else(|e| e.into_inner());
            }
            *armed = false;
        }
        serial = serial.wrapping_add(1);
        // The vblank after the last one seen: at once if that has passed (the
        // first frame after a pause comes without waiting, as the timer's).
        let target = last.map_or(0, |l| l.msc + 1);
        conn.send(&notify_msc_request(major, window, serial, target))?;
        let complete = loop {
            match read_packet(&mut conn, &mut buf)? {
                None => break None, // timed out
                Some(packet) => match parse(&packet, major) {
                    Ok(Some(c)) if c.serial == serial => break Some(c),
                    Ok(_) => {}
                    Err(code) => return Err(io::Error::other(format!("X error {code}"))),
                },
            }
        };
        let Some(complete) = complete else {
            continue;
        };
        // The refresh over a second or so of vblanks, which evens out the
        // jitter of a fake vblank (a timer, under Xvfb).
        match anchor {
            Some(a) if complete.msc < a.msc => anchor = Some(complete),
            Some(a) if complete.msc - a.msc >= REFRESH_SPAN => {
                if let Some(interval) = refresh(a, complete) {
                    crate::platform::frame::report_refresh("Present", interval);
                }
                anchor = Some(complete);
            }
            Some(_) => {}
            None => anchor = Some(complete),
        }
        last = Some(complete);
        crate::platform::frame::tick();
    }
}

/// The next whole packet from the connection, or `None` if none came within
/// the read timeout (a partial one stays in `buf`).
fn read_packet(conn: &mut Conn, buf: &mut Vec<u8>) -> io::Result<Option<Vec<u8>>> {
    loop {
        if let Some(len) = packet_len(buf)
            && buf.len() >= len
        {
            return Ok(Some(buf.drain(..len).collect()));
        }
        let mut chunk = [0u8; 256];
        match conn.stream.read(&mut chunk) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return Ok(None);
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_encoded_as_presentproto_lays_them_out() {
        let r = select_input_request(140, 0x0420_0001, 0x0420_0002, COMPLETE_NOTIFY_MASK);
        assert_eq!(r.len(), 16);
        assert_eq!(&r[..4], &[140, 3, 4, 0]);
        assert_eq!(rd_u32(&r, 4), Some(0x0420_0001));
        assert_eq!(rd_u32(&r, 8), Some(0x0420_0002));
        assert_eq!(rd_u32(&r, 12), Some(2));

        let r = notify_msc_request(140, 0x0420_0002, 7, 0x1_0000_0005);
        assert_eq!(r.len(), 40, "sz_xPresentNotifyMSCReq");
        assert_eq!(&r[..4], &[140, 2, 10, 0]);
        assert_eq!(rd_u32(&r, 4), Some(0x0420_0002));
        assert_eq!(rd_u32(&r, 8), Some(7));
        assert_eq!(rd_u64(&r, 16), Some(0x1_0000_0005), "target MSC");
        assert_eq!(rd_u64(&r, 24), Some(0), "divisor");
        assert_eq!(rd_u64(&r, 32), Some(0), "remainder");
    }

    fn complete_notify(major: u8, kind: u8, serial: u32, ust: u64, msc: u64) -> Vec<u8> {
        let mut e = vec![GENERIC_EVENT, major, 9, 0];
        e.extend_from_slice(&2u32.to_le_bytes()); // 8 bytes past 32
        e.extend_from_slice(&COMPLETE_NOTIFY.to_le_bytes());
        e.push(kind);
        e.push(0); // mode
        e.extend_from_slice(&0x0420_0001u32.to_le_bytes()); // eid
        e.extend_from_slice(&0x0420_0002u32.to_le_bytes()); // window
        e.extend_from_slice(&serial.to_le_bytes());
        e.extend_from_slice(&ust.to_le_bytes());
        e.extend_from_slice(&msc.to_le_bytes());
        e
    }

    #[test]
    fn complete_notifies_are_parsed_and_framed() {
        let e = complete_notify(140, KIND_NOTIFY_MSC, 3, 1_000_000, 60);
        assert_eq!(e.len(), 40);
        assert_eq!(packet_len(&e), Some(40));
        assert_eq!(
            parse(&e, 140),
            Ok(Some(Complete {
                serial: 3,
                ust: 1_000_000,
                msc: 60
            }))
        );
        // Another extension's generic event, or a pixmap's completion.
        assert_eq!(parse(&e, 141), Ok(None));
        let p = complete_notify(140, 0, 3, 1_000_000, 60);
        assert_eq!(parse(&p, 140), Ok(None));
        // A core event is 32 bytes; an error ends the clock.
        let mut core = vec![0u8; 32];
        core[0] = 12; // Expose
        assert_eq!(packet_len(&core), Some(32));
        assert_eq!(parse(&core, 140), Ok(None));
        core[0] = 0;
        core[1] = 3; // BadWindow
        assert_eq!(parse(&core, 140), Err(3));
        assert_eq!(packet_len(&e[..20]), None, "not all in yet");
    }

    #[test]
    fn the_refresh_comes_from_ust_and_msc() {
        let a = Complete {
            serial: 1,
            ust: 1_000_000,
            msc: 100,
        };
        let b = Complete {
            serial: 2,
            ust: 1_016_667,
            msc: 101,
        };
        assert_eq!(refresh(a, b), Some(Duration::from_micros(16_667)));
        let c = Complete {
            serial: 3,
            ust: 1_050_000,
            msc: 103,
        };
        assert_eq!(refresh(a, c), Some(Duration::from_nanos(16_666_666)));
        assert_eq!(refresh(b, a), None, "backwards");
        assert_eq!(refresh(a, a), None, "no vblank between");
    }
}
