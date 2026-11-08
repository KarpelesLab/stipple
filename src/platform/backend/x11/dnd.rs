//! Receiving drops over XDND (version 5), by hand on the wire.
//!
//! A drag source finds a window that sets `XdndAware`, then sends it
//! `ClientMessage`s: `XdndEnter` (the offered types), `XdndPosition` (root
//! coordinates; we answer each with `XdndStatus`, accepting or not), and
//! `XdndLeave` or `XdndDrop`. On a drop we `ConvertSelection` the
//! `XdndSelection` to the best type (`text/uri-list` for files, UTF-8 text
//! otherwise), read the property back when `SelectionNotify` arrives, deliver
//! [`Event::Drop`] and send `XdndFinished`. See freedesktop.org's XDND spec.

use std::collections::VecDeque;
use std::io::{self, Read};

use super::{
    ATOM_ATOM, ATOM_STRING, Conn, OP_GET_PROPERTY, OP_SEND_EVENT, SharedConn, X11Window, dbg,
    finish, intern_atom, rd_u16, rd_u32,
};
use crate::geometry::Point;
use crate::platform::ControlFlow;
use crate::platform::event::{DragOffer, DropData, Event, parse_uri_list};
use crate::platform::window::Window;

const OP_CONVERT_SELECTION: u8 = 24;
const OP_TRANSLATE_COORDINATES: u8 = 40;
/// The XDND protocol version we speak.
pub(super) const VERSION: u32 = 5;

/// The atoms of the protocol, interned once at startup.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Atoms {
    pub(super) aware: u32,
    enter: u32,
    position: u32,
    status: u32,
    leave: u32,
    drop: u32,
    finished: u32,
    selection: u32,
    type_list: u32,
    action_copy: u32,
    uri_list: u32,
    text_plain_utf8: u32,
    text_plain: u32,
    utf8: u32,
    /// The property on our window the dropped data is converted into.
    property: u32,
}

impl Atoms {
    pub(super) fn intern(conn: &mut Conn) -> io::Result<Atoms> {
        let mut atom = |name: &[u8]| intern_atom(conn, name);
        Ok(Atoms {
            aware: atom(b"XdndAware")?,
            enter: atom(b"XdndEnter")?,
            position: atom(b"XdndPosition")?,
            status: atom(b"XdndStatus")?,
            leave: atom(b"XdndLeave")?,
            drop: atom(b"XdndDrop")?,
            finished: atom(b"XdndFinished")?,
            selection: atom(b"XdndSelection")?,
            type_list: atom(b"XdndTypeList")?,
            action_copy: atom(b"XdndActionCopy")?,
            uri_list: atom(b"text/uri-list")?,
            text_plain_utf8: atom(b"text/plain;charset=utf-8")?,
            text_plain: atom(b"text/plain")?,
            utf8: atom(b"UTF8_STRING")?,
            property: atom(b"STIPPLE_XDND")?,
        })
    }

    /// Whether `ty` is one of the messages a drag source sends a target.
    pub(super) fn is_xdnd(&self, ty: u32) -> bool {
        ty != 0 && [self.enter, self.position, self.leave, self.drop].contains(&ty)
    }

    /// The type to convert the selection to for `types`, and what it holds:
    /// a URI list for files first, then text.
    fn pick(&self, types: &[u32]) -> Option<(u32, DragOffer)> {
        if types.contains(&self.uri_list) {
            return Some((self.uri_list, DragOffer::FILES));
        }
        [
            self.utf8,
            self.text_plain_utf8,
            self.text_plain,
            ATOM_STRING,
        ]
        .into_iter()
        .find(|t| types.contains(t))
        .map(|t| (t, DragOffer::TEXT))
    }

    /// What `types` carries.
    fn offer(&self, types: &[u32]) -> DragOffer {
        let text = [
            self.utf8,
            self.text_plain_utf8,
            self.text_plain,
            ATOM_STRING,
        ]
        .iter()
        .any(|t| types.contains(t));
        DragOffer {
            files: types.contains(&self.uri_list),
            text,
            internal: false,
        }
    }
}

/// The drag over one of our windows.
#[derive(Debug, Default)]
pub(super) struct Drag {
    /// The source's window and protocol version.
    source: u32,
    version: u32,
    /// Our window it is over.
    target: u32,
    types: Vec<u32>,
    /// Whether `DragEnter` went to the app (it waits for the first position).
    entered: bool,
    /// The last position, in logical pixels within `target`.
    position: Point,
    /// A drop waiting for `SelectionNotify`: the type asked for.
    converting: Option<u32>,
}

/// The XDND state of the event loop.
#[derive(Debug, Default)]
pub(super) struct Xdnd {
    pub(super) atoms: Atoms,
    pub(super) drag: Option<Drag>,
}

type Handler<'a> = &'a mut dyn FnMut(Event, &dyn Window) -> ControlFlow;

/// Send a format-32 `ClientMessage` of type `ty` to `window`.
fn send_client_message(conn: &mut Conn, window: u32, ty: u32, data: [u32; 5]) -> io::Result<()> {
    conn.send(&finish(client_message_request(window, ty, data)))
}

/// A `SendEvent` of a format-32 `ClientMessage`, before [`finish`].
fn client_message_request(window: u32, ty: u32, data: [u32; 5]) -> Vec<u8> {
    let mut ev = [0u8; 32];
    ev[0] = 33; // ClientMessage
    ev[1] = 32; // format
    ev[4..8].copy_from_slice(&window.to_le_bytes());
    ev[8..12].copy_from_slice(&ty.to_le_bytes());
    for (i, v) in data.iter().enumerate() {
        ev[12 + 4 * i..16 + 4 * i].copy_from_slice(&v.to_le_bytes());
    }
    let mut req = vec![OP_SEND_EVENT, 0 /* propagate */, 0, 0];
    req.extend_from_slice(&window.to_le_bytes());
    req.extend_from_slice(&0u32.to_le_bytes()); // event mask: the window's owner
    req.extend_from_slice(&ev);
    req
}

/// Send `req` and wait for its reply, putting events that arrive first in
/// `backlog`. Returns the 32-byte header and the extra data, or `None` if the
/// server answered with an error for this request's opcode.
fn round_trip(
    conn: &mut Conn,
    req: Vec<u8>,
    backlog: &mut VecDeque<[u8; 32]>,
) -> io::Result<Option<([u8; 32], Vec<u8>)>> {
    let opcode = req[0];
    conn.send(&finish(req))?;
    loop {
        let mut packet = [0u8; 32];
        conn.stream.read_exact(&mut packet)?;
        match packet[0] {
            1 => {
                let words = rd_u32(&packet, 4).unwrap_or(0) as usize;
                let mut extra = vec![0u8; words * 4];
                conn.stream.read_exact(&mut extra)?;
                return Ok(Some((packet, extra)));
            }
            0 if packet[10] == opcode => return Ok(None),
            _ => backlog.push_back(packet),
        }
    }
}

/// Read `prop` of `window` (any type), optionally deleting it. Returns its
/// type and bytes.
fn get_property(
    conn: &mut Conn,
    window: u32,
    prop: u32,
    delete: bool,
    backlog: &mut VecDeque<[u8; 32]>,
) -> io::Result<Option<(u32, Vec<u8>)>> {
    const MAX_WORDS: u32 = 4 * 1024 * 1024;
    let mut req = vec![OP_GET_PROPERTY, delete as u8, 0, 0];
    req.extend_from_slice(&window.to_le_bytes());
    req.extend_from_slice(&prop.to_le_bytes());
    req.extend_from_slice(&0u32.to_le_bytes()); // AnyPropertyType
    req.extend_from_slice(&0u32.to_le_bytes()); // offset
    req.extend_from_slice(&MAX_WORDS.to_le_bytes());
    Ok(round_trip(conn, req, backlog)?.map(|(reply, mut data)| {
        let ty = rd_u32(&reply, 8).unwrap_or(0);
        let format = reply[1] as usize;
        let items = rd_u32(&reply, 16).unwrap_or(0) as usize;
        data.truncate(items * format / 8);
        (ty, data)
    }))
}

/// Translate root coordinates to `window`'s.
fn translate(
    conn: &mut Conn,
    window: u32,
    x: i16,
    y: i16,
    backlog: &mut VecDeque<[u8; 32]>,
) -> io::Result<Option<(i16, i16)>> {
    let mut req = vec![OP_TRANSLATE_COORDINATES, 0, 0, 0];
    req.extend_from_slice(&conn.setup.root.to_le_bytes());
    req.extend_from_slice(&window.to_le_bytes());
    req.extend_from_slice(&x.to_le_bytes());
    req.extend_from_slice(&y.to_le_bytes());
    Ok(round_trip(conn, req, backlog)?.map(|(reply, _)| {
        (
            rd_u16(&reply, 12).unwrap_or(0) as i16,
            rd_u16(&reply, 14).unwrap_or(0) as i16,
        )
    }))
}

/// The 32-bit atoms in a format-32 property's bytes.
fn atoms_of(data: &[u8]) -> Vec<u32> {
    data.as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect()
}

impl Xdnd {
    /// Handle an XDND `ClientMessage` (`buf`) for our window `win`.
    pub(super) fn client_message(
        &mut self,
        shared: &SharedConn,
        backlog: &mut VecDeque<[u8; 32]>,
        buf: &[u8; 32],
        win: &X11Window,
        handler: Handler<'_>,
    ) -> ControlFlow {
        let a = self.atoms;
        let ty = rd_u32(buf, 8).unwrap_or(0);
        let l = |i: usize| rd_u32(buf, 12 + 4 * i).unwrap_or(0);
        let source = l(0);
        if ty == a.enter {
            let version = l(1) >> 24;
            let mut types: Vec<u32> = (2..5).map(l).filter(|&t| t != 0).collect();
            if l(1) & 1 != 0 {
                let mut conn = shared.lock().unwrap();
                if let Ok(Some((_, data))) =
                    get_property(&mut conn, source, a.type_list, false, backlog)
                {
                    types = atoms_of(&data);
                }
            }
            dbg(format_args!(
                "xdnd: enter from {source:#x} v{version}, {} types",
                types.len()
            ));
            // A new drag replaces one that never left (a crashed source).
            let flow = self.leave(win, handler);
            self.drag = Some(Drag {
                source,
                version: version.min(VERSION),
                target: win.window,
                types,
                ..Drag::default()
            });
            return flow;
        }
        let Some(drag) = self.drag.as_mut().filter(|d| d.source == source) else {
            dbg(format_args!(
                "xdnd: message from unknown source {source:#x}"
            ));
            return ControlFlow::Wait;
        };
        if ty == a.position {
            let (rx, ry) = ((l(2) >> 16) as i16, (l(2) & 0xffff) as i16);
            let local = {
                let mut conn = shared.lock().unwrap();
                translate(&mut conn, win.window, rx, ry, backlog)
                    .ok()
                    .flatten()
            };
            let (x, y) = local.unwrap_or((rx, ry));
            let scale = win.scale_factor().get();
            let position = Point::new(x as f64 / scale, y as f64 / scale);
            drag.position = position;
            drag.target = win.window;
            win.drop_accepted.set(false);
            let event = if drag.entered {
                Event::DragOver { position }
            } else {
                drag.entered = true;
                Event::DragEnter {
                    position,
                    offer: a.offer(&drag.types),
                }
            };
            let flow = handler(event, win);
            let accepted = win.drop_accepted.get() && a.pick(&drag.types).is_some();
            // Status: accept or not, and send every move (no "quiet" rect).
            let data = [
                win.window,
                accepted as u32 | 2,
                0,
                0,
                if accepted { a.action_copy } else { 0 },
            ];
            let mut conn = shared.lock().unwrap();
            let _ = send_client_message(&mut conn, source, a.status, data);
            flow
        } else if ty == a.leave {
            dbg(format_args!("xdnd: leave"));
            self.leave(win, handler)
        } else if ty == a.drop {
            let time = l(2);
            let pick = a.pick(&drag.types).filter(|_| win.drop_accepted.get());
            let Some((target_type, _)) = pick else {
                dbg(format_args!("xdnd: drop refused"));
                let mut conn = shared.lock().unwrap();
                let _ =
                    send_client_message(&mut conn, source, a.finished, [win.window, 0, 0, 0, 0]);
                drop(conn);
                return self.leave(win, handler);
            };
            drag.converting = Some(target_type);
            let mut req = vec![OP_CONVERT_SELECTION, 0, 0, 0];
            req.extend_from_slice(&win.window.to_le_bytes()); // requestor
            req.extend_from_slice(&a.selection.to_le_bytes());
            req.extend_from_slice(&target_type.to_le_bytes());
            req.extend_from_slice(&a.property.to_le_bytes());
            req.extend_from_slice(&time.to_le_bytes());
            dbg(format_args!("xdnd: drop, converting to atom {target_type}"));
            let mut conn = shared.lock().unwrap();
            let _ = conn.send(&finish(req));
            ControlFlow::Wait
        } else {
            ControlFlow::Wait
        }
    }

    /// The drag left (or is replaced): tell the app if it had entered.
    fn leave(&mut self, win: &X11Window, handler: Handler<'_>) -> ControlFlow {
        match self.drag.take() {
            Some(d) if d.entered => handler(Event::DragLeave, win),
            _ => ControlFlow::Wait,
        }
    }

    /// The window a pending drop's `SelectionNotify` belongs to.
    pub(super) fn converting_window(&self) -> Option<u32> {
        self.drag
            .as_ref()
            .filter(|d| d.converting.is_some())
            .map(|d| d.target)
    }

    /// Handle a `SelectionNotify` (`buf`): if it answers our drop's
    /// conversion, read the data, deliver the drop and finish.
    pub(super) fn selection_notify(
        &mut self,
        shared: &SharedConn,
        backlog: &mut VecDeque<[u8; 32]>,
        buf: &[u8; 32],
        win: &X11Window,
        handler: Handler<'_>,
    ) -> ControlFlow {
        let a = self.atoms;
        let selection = rd_u32(buf, 12).unwrap_or(0);
        let property = rd_u32(buf, 20).unwrap_or(0);
        if selection != a.selection || self.converting_window() != Some(win.window) {
            return ControlFlow::Wait;
        }
        let Some(drag) = self.drag.take() else {
            return ControlFlow::Wait;
        };
        let data = if property == 0 {
            dbg(format_args!("xdnd: conversion refused by the source"));
            None
        } else {
            let mut conn = shared.lock().unwrap();
            match get_property(&mut conn, win.window, property, true, backlog) {
                Ok(Some((ty, bytes))) => drop_data(&a, drag.converting, ty, &bytes),
                _ => None,
            }
        };
        let flow = match data {
            Some(data) => {
                dbg(format_args!("xdnd: dropped {data:?}"));
                handler(
                    Event::Drop {
                        position: drag.position,
                        data,
                    },
                    win,
                )
            }
            None => handler(Event::DragLeave, win),
        };
        let action = if drag.version >= 2 { a.action_copy } else { 0 };
        let mut conn = shared.lock().unwrap();
        let _ = send_client_message(
            &mut conn,
            drag.source,
            a.finished,
            [win.window, 1, action, 0, 0],
        );
        flow
    }
}

/// Turn the converted selection (`bytes` of type `ty`, asked as `asked`) into
/// what was dropped. `None` for an empty or unsupported (`INCR`) answer.
fn drop_data(a: &Atoms, asked: Option<u32>, ty: u32, bytes: &[u8]) -> Option<DropData> {
    if ty == 0 || bytes.is_empty() {
        return None;
    }
    let text = String::from_utf8_lossy(bytes);
    if asked == Some(a.uri_list) {
        let files = parse_uri_list(&text);
        return (!files.is_empty()).then_some(DropData::Files(files));
    }
    if ty == ATOM_STRING {
        // Latin-1.
        return Some(DropData::Text(bytes.iter().map(|&b| b as char).collect()));
    }
    if ty == ATOM_ATOM {
        return None;
    }
    Some(DropData::Text(text.trim_end_matches('\0').to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atoms() -> Atoms {
        Atoms {
            uri_list: 100,
            utf8: 101,
            text_plain: 102,
            text_plain_utf8: 103,
            ..Atoms::default()
        }
    }

    #[test]
    fn picks_files_over_text() {
        let a = atoms();
        assert_eq!(a.pick(&[102, 100]), Some((100, DragOffer::FILES)));
        assert_eq!(a.pick(&[102, 101]), Some((101, DragOffer::TEXT)));
        assert_eq!(a.pick(&[7]), None);
        let offer = a.offer(&[100, 102]);
        assert!(offer.files && offer.text);
    }

    #[test]
    fn converts_uri_lists_and_text() {
        let a = atoms();
        assert_eq!(
            drop_data(&a, Some(100), 100, b"file:///tmp/x%20y\r\n"),
            Some(DropData::Files(vec!["/tmp/x y".into()]))
        );
        assert_eq!(
            drop_data(&a, Some(101), 101, "héllo".as_bytes()),
            Some(DropData::Text("héllo".into()))
        );
        assert_eq!(
            drop_data(&a, Some(31), ATOM_STRING, &[0x68, 0xe9]),
            Some(DropData::Text("hé".into()))
        );
        assert_eq!(drop_data(&a, Some(100), 100, b""), None);
    }

    #[test]
    fn client_message_is_a_44_byte_send_event() {
        let req = finish(client_message_request(0x1234, 77, [1, 2, 3, 4, 5]));
        assert_eq!(req.len(), 44);
        assert_eq!(req[0], OP_SEND_EVENT);
        assert_eq!(u16::from_le_bytes([req[2], req[3]]), 11);
        assert_eq!(req[12], 33);
        assert_eq!(req[13], 32);
        assert_eq!(u32::from_le_bytes(req[20..24].try_into().unwrap()), 77);
        assert_eq!(u32::from_le_bytes(req[40..44].try_into().unwrap()), 5);
    }
}
