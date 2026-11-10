//! Dragging out of the app over XDND (version 5): we are the source.
//!
//! When the app hands a drag to the window ([`Window::start_drag`]), we take
//! the `XdndSelection`, grab the pointer and keyboard, and on each motion
//! look for the window under the pointer that sets `XdndAware` (walking down
//! from the root with `TranslateCoordinates`). Entering one sends it
//! `XdndEnter` with our types (`text/uri-list` for files, UTF-8 and plain
//! text for text, in `XdndTypeList` when there are more than three), each
//! move an `XdndPosition` (one at a time: the next waits for the target's
//! `XdndStatus`), and leaving it `XdndLeave`. Releasing the button over a
//! target that accepted sends `XdndDrop`; the target converts the selection,
//! which we serve from memory (`SelectionRequest`), and answers
//! `XdndFinished`. Escape, or a release over nothing or a refusal, cancels.
//!
//! Our own windows are XDND targets too, so a drag that comes back over the
//! window arrives through the receiving side like one from another app.
//!
//! [`Window::start_drag`]: crate::platform::Window::start_drag

use std::collections::VecDeque;
use std::io;

use super::super::{
    ATOM_ATOM, ATOM_STRING, Conn, SharedConn, X_BUTTON_PRESS, X_BUTTON_RELEASE, X_CLIENT_MESSAGE,
    X_KEY_PRESS, X_KEY_RELEASE, X_MOTION_NOTIFY, X_SELECTION_CLEAR, X_SELECTION_REQUEST, cursor_id,
    dbg, finish, rd_u16, rd_u32, set_property_atoms, set_property_str,
};
use super::{Atoms, VERSION, atoms_of, get_property, round_trip, send_client_message};
use crate::platform::event::{DropData, uri_list};
use crate::platform::window::CursorIcon;

const OP_SET_SELECTION_OWNER: u8 = 22;
const OP_SEND_EVENT: u8 = 25;
const OP_GRAB_POINTER: u8 = 26;
const OP_UNGRAB_POINTER: u8 = 27;
const OP_CHANGE_ACTIVE_POINTER_GRAB: u8 = 30;
const OP_GRAB_KEYBOARD: u8 = 31;
const OP_UNGRAB_KEYBOARD: u8 = 32;
const OP_TRANSLATE_COORDINATES: u8 = 40;
/// The pointer events the grab reports: presses, releases and motion.
const GRAB_EVENTS: u16 = 0x0004 | 0x0008 | 0x0040;
/// How deep the search for an `XdndAware` window goes below the root.
const MAX_DEPTH: usize = 16;

/// A window that takes drops, and the XDND version it speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Target {
    pub(super) window: u32,
    pub(super) version: u32,
}

/// What the drag says to the target, or does to itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Step {
    Enter(Target),
    /// The pointer at root coordinates, at server time `time`.
    Position {
        target: u32,
        x: i16,
        y: i16,
        time: u32,
    },
    Leave(u32),
    Drop {
        target: u32,
        time: u32,
    },
    /// Show whether the target takes the drag (the grab's cursor).
    Feedback(bool),
    /// Let go of the pointer and keyboard.
    Ungrab,
    /// The drag is over; forget it.
    End,
}

/// The protocol's state, apart from the wire: fed what happens, it answers
/// with the [`Step`]s to take.
#[derive(Debug, Default)]
pub(super) struct Machine {
    target: Option<Target>,
    /// An `XdndPosition` went out and its `XdndStatus` hasn't come back.
    awaiting: bool,
    /// The latest move made meanwhile, sent when the status arrives.
    pending: Option<(i16, i16, u32)>,
    /// The target's last answer.
    accepted: bool,
    /// The button came up (at this time) while a status was awaited.
    released: Option<u32>,
    /// Dropped on this window; waiting for its `XdndFinished`.
    dropped: Option<u32>,
}

impl Machine {
    /// The pointer moved to root `(x, y)` over `under` (the `XdndAware`
    /// window there, if any).
    pub(super) fn motion(&mut self, under: Option<Target>, x: i16, y: i16, time: u32) -> Vec<Step> {
        let mut steps = Vec::new();
        if self.released.is_some() || self.dropped.is_some() {
            return steps;
        }
        if under.map(|t| t.window) != self.target.map(|t| t.window) {
            if let Some(old) = self.target.take() {
                steps.push(Step::Leave(old.window));
            }
            self.awaiting = false;
            self.pending = None;
            if self.accepted || under.is_none() {
                steps.push(Step::Feedback(false));
            }
            self.accepted = false;
            if let Some(new) = under {
                steps.push(Step::Enter(new));
            }
            self.target = under;
        }
        let Some(target) = self.target else {
            return steps;
        };
        if self.awaiting {
            self.pending = Some((x, y, time));
        } else {
            self.awaiting = true;
            steps.push(Step::Position {
                target: target.window,
                x,
                y,
                time,
            });
        }
        steps
    }

    /// `XdndStatus` from `from`: whether it takes the drag.
    pub(super) fn status(&mut self, from: u32, accept: bool) -> Vec<Step> {
        let Some(target) = self.target.filter(|t| t.window == from) else {
            return Vec::new();
        };
        if self.dropped.is_some() {
            return Vec::new();
        }
        self.awaiting = false;
        let mut steps = Vec::new();
        if accept != self.accepted {
            steps.push(Step::Feedback(accept));
        }
        self.accepted = accept;
        if let Some(time) = self.released.take() {
            steps.extend(self.finish_release(time));
        } else if let Some((x, y, time)) = self.pending.take() {
            self.awaiting = true;
            steps.push(Step::Position {
                target: target.window,
                x,
                y,
                time,
            });
        }
        steps
    }

    /// The button came up at server time `time`.
    pub(super) fn release(&mut self, time: u32) -> Vec<Step> {
        if self.released.is_some() || self.dropped.is_some() {
            return Vec::new();
        }
        let mut steps = vec![Step::Ungrab];
        if self.target.is_some() && self.awaiting {
            // Wait for the answer to the last position before deciding.
            self.released = Some(time);
        } else {
            steps.extend(self.finish_release(time));
        }
        steps
    }

    /// Drop if the target accepted, else leave it.
    fn finish_release(&mut self, time: u32) -> Vec<Step> {
        match self.target {
            Some(t) if self.accepted => {
                self.dropped = Some(t.window);
                vec![Step::Drop {
                    target: t.window,
                    time,
                }]
            }
            Some(t) => {
                self.target = None;
                vec![Step::Leave(t.window), Step::End]
            }
            None => vec![Step::End],
        }
    }

    /// Escape: leave the target and end, unless already dropped.
    pub(super) fn cancel(&mut self) -> Vec<Step> {
        if self.dropped.is_some() {
            return Vec::new();
        }
        let mut steps = vec![Step::Ungrab];
        if let Some(t) = self.target.take() {
            steps.push(Step::Leave(t.window));
        }
        steps.push(Step::End);
        steps
    }

    /// `XdndFinished` from `from`.
    pub(super) fn finished(&mut self, from: u32) -> Vec<Step> {
        if self.dropped == Some(from) {
            self.dropped = None;
            vec![Step::End]
        } else {
            Vec::new()
        }
    }
}

/// The data the drag serves, by the type a target asks for.
#[derive(Debug)]
struct Payload {
    data: DropData,
    /// The types offered, best first.
    types: Vec<u32>,
}

impl Payload {
    fn new(a: &Atoms, data: DropData) -> Option<Payload> {
        let types = match &data {
            DropData::Files(f) if !f.is_empty() => vec![a.uri_list],
            DropData::Text(_) => vec![a.utf8, a.text_plain_utf8, a.text_plain, ATOM_STRING],
            _ => return None,
        };
        Some(Payload { data, types })
    }

    /// The bytes for `target`, and their type. `None` for a type we don't
    /// offer.
    fn convert(&self, a: &Atoms, target: u32) -> Option<(u32, Vec<u8>)> {
        if !self.types.contains(&target) {
            return None;
        }
        match &self.data {
            DropData::Files(paths) => Some((target, uri_list(paths).into_bytes())),
            DropData::Text(text) if target == ATOM_STRING => {
                // Latin-1, with what doesn't fit as '?'.
                let bytes = text
                    .chars()
                    .map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?'))
                    .collect();
                Some((ATOM_STRING, bytes))
            }
            DropData::Text(text) => {
                let ty = if target == a.text_plain_utf8 || target == a.text_plain {
                    target
                } else {
                    a.utf8
                };
                Some((ty, text.as_bytes().to_vec()))
            }
            DropData::Internal(_) => None,
        }
    }
}

/// A drag out of one of our windows, from start to `XdndFinished`.
#[derive(Debug)]
pub(in super::super) struct Source {
    atoms: Atoms,
    /// Our window: the selection's owner and the grab's.
    window: u32,
    payload: Payload,
    machine: Machine,
    grabbed: bool,
    over: bool,
}

impl Source {
    /// Start dragging `data` out of `window`: own the selection, advertise
    /// the types and grab the pointer and keyboard. `None` when there is
    /// nothing to offer or the pointer can't be grabbed.
    pub(in super::super) fn start(
        shared: &SharedConn,
        atoms: Atoms,
        window: u32,
        data: DropData,
        backlog: &mut VecDeque<[u8; 32]>,
    ) -> Option<Source> {
        let payload = Payload::new(&atoms, data)?;
        let mut conn = shared.lock().unwrap();
        if payload.types.len() > 3 {
            set_property_atoms(&mut conn, window, atoms.type_list, &payload.types).ok()?;
        }
        let mut req = vec![OP_SET_SELECTION_OWNER, 0, 0, 0];
        req.extend_from_slice(&window.to_le_bytes());
        req.extend_from_slice(&atoms.selection.to_le_bytes());
        req.extend_from_slice(&0u32.to_le_bytes()); // CurrentTime
        conn.send(&finish(req)).ok()?;
        let cursor = cursor_id(&mut conn, CursorIcon::NotAllowed).unwrap_or(0);
        let grab = round_trip(&mut conn, grab_pointer_request(window, cursor), backlog);
        let status = grab.ok().flatten().map(|(reply, _)| reply[1]);
        if status != Some(0) {
            dbg(format_args!("xdnd source: pointer grab failed: {status:?}"));
            return None;
        }
        let mut req = vec![OP_GRAB_KEYBOARD, 0 /* owner events */, 0, 0];
        req.extend_from_slice(&window.to_le_bytes());
        req.extend_from_slice(&0u32.to_le_bytes()); // CurrentTime
        req.extend_from_slice(&[1, 1, 0, 0]); // async pointer and keyboard
        // Escape only works with the keyboard; the drag goes on without it.
        let _ = round_trip(&mut conn, req, backlog);
        dbg(format_args!(
            "xdnd source: dragging {:?} from {window:#x}",
            payload.data
        ));
        Some(Source {
            atoms,
            window,
            payload,
            machine: Machine::default(),
            grabbed: true,
            over: false,
        })
    }

    /// Whether the drag is over and can be forgotten.
    pub(in super::super) fn is_over(&self) -> bool {
        self.over
    }

    /// Handle event `buf` if it belongs to the drag (pointer and key events
    /// while grabbed, the target's messages, requests for the selection).
    /// `escape` says a key press is Escape. Returns whether it did.
    pub(in super::super) fn event(
        &mut self,
        shared: &SharedConn,
        backlog: &mut VecDeque<[u8; 32]>,
        buf: &[u8; 32],
        escape: bool,
    ) -> bool {
        let a = self.atoms;
        let code = buf[0] & 0x7f;
        let time = rd_u32(buf, 4).unwrap_or(0);
        let steps = match code {
            X_MOTION_NOTIFY if self.grabbed => {
                let x = rd_u16(buf, 20).unwrap_or(0) as i16;
                let y = rd_u16(buf, 22).unwrap_or(0) as i16;
                let under = {
                    let mut conn = shared.lock().unwrap();
                    find_target(&mut conn, &a, x, y, backlog).unwrap_or(None)
                };
                self.machine.motion(under, x, y, time)
            }
            X_BUTTON_RELEASE if self.grabbed && buf[1] == 1 => self.machine.release(time),
            X_KEY_PRESS if self.grabbed && escape => {
                dbg(format_args!("xdnd source: cancelled"));
                self.machine.cancel()
            }
            X_BUTTON_PRESS | X_BUTTON_RELEASE | X_KEY_PRESS | X_KEY_RELEASE if self.grabbed => {
                Vec::new()
            }
            X_CLIENT_MESSAGE => {
                let ty = rd_u32(buf, 8).unwrap_or(0);
                let l = |i: usize| rd_u32(buf, 12 + 4 * i).unwrap_or(0);
                if ty == a.status && ty != 0 {
                    self.machine.status(l(0), l(1) & 1 != 0)
                } else if ty == a.finished && ty != 0 {
                    dbg(format_args!(
                        "xdnd source: finished, {}",
                        if l(1) & 1 != 0 { "taken" } else { "refused" }
                    ));
                    self.machine.finished(l(0))
                } else {
                    return false;
                }
            }
            X_SELECTION_REQUEST if rd_u32(buf, 16) == Some(a.selection) => {
                let mut conn = shared.lock().unwrap();
                let _ = self.serve(&mut conn, buf);
                return true;
            }
            // Someone else took the selection: nothing left to serve.
            X_SELECTION_CLEAR if rd_u32(buf, 12) == Some(a.selection) => {
                if self.machine.dropped.is_some() {
                    self.over = true;
                }
                return true;
            }
            _ => return false,
        };
        let mut conn = shared.lock().unwrap();
        for step in steps {
            let _ = self.take(&mut conn, step);
        }
        true
    }

    /// Put `step` on the wire.
    fn take(&mut self, conn: &mut Conn, step: Step) -> io::Result<()> {
        let a = self.atoms;
        let me = self.window;
        dbg(format_args!("xdnd source: {step:?}"));
        match step {
            Step::Enter(t) => {
                let [t0, t1, t2] = first_three(&self.payload.types);
                let more = u32::from(self.payload.types.len() > 3);
                let version = t.version.min(VERSION);
                send_client_message(
                    conn,
                    t.window,
                    a.enter,
                    [me, version << 24 | more, t0, t1, t2],
                )
            }
            Step::Position { target, x, y, time } => send_client_message(
                conn,
                target,
                a.position,
                [me, 0, position_word(x, y), time, a.action_copy],
            ),
            Step::Leave(target) => send_client_message(conn, target, a.leave, [me, 0, 0, 0, 0]),
            Step::Drop { target, time } => {
                send_client_message(conn, target, a.drop, [me, 0, time, 0, 0])
            }
            Step::Feedback(accepted) => {
                let icon = if accepted {
                    CursorIcon::Grabbing
                } else {
                    CursorIcon::NotAllowed
                };
                if !self.grabbed {
                    return Ok(());
                }
                let cursor = cursor_id(conn, icon)?;
                let mut req = vec![OP_CHANGE_ACTIVE_POINTER_GRAB, 0, 0, 0];
                req.extend_from_slice(&cursor.to_le_bytes());
                req.extend_from_slice(&0u32.to_le_bytes()); // CurrentTime
                req.extend_from_slice(&GRAB_EVENTS.to_le_bytes());
                req.extend_from_slice(&[0, 0]);
                conn.send(&finish(req))
            }
            Step::Ungrab => {
                self.grabbed = false;
                for op in [OP_UNGRAB_POINTER, OP_UNGRAB_KEYBOARD] {
                    let mut req = vec![op, 0, 0, 0];
                    req.extend_from_slice(&0u32.to_le_bytes()); // CurrentTime
                    conn.send(&finish(req))?;
                }
                Ok(())
            }
            Step::End => {
                self.over = true;
                Ok(())
            }
        }
    }

    /// Answer a `SelectionRequest` for the drag's selection: fill the
    /// requestor's property with the data in the asked type (or our type
    /// list for `TARGETS`), then tell it with `SelectionNotify`.
    fn serve(&self, conn: &mut Conn, ev: &[u8; 32]) -> io::Result<()> {
        let a = self.atoms;
        let time = rd_u32(ev, 4).unwrap_or(0);
        let requestor = rd_u32(ev, 12).unwrap_or(0);
        let selection = rd_u32(ev, 16).unwrap_or(0);
        let target = rd_u32(ev, 20).unwrap_or(0);
        let property = match rd_u32(ev, 24).unwrap_or(0) {
            0 => target,
            p => p,
        };
        // A property must fit in one request (no INCR transfers).
        let room = (conn.setup.max_request_len as usize)
            .saturating_mul(4)
            .saturating_sub(64);
        let served = if target == a.targets && target != 0 {
            let mut list = vec![a.targets];
            list.extend(&self.payload.types);
            set_property_atoms(conn, requestor, property, &list).is_ok()
        } else {
            match self.payload.convert(&a, target) {
                Some((ty, bytes)) if bytes.len() <= room => {
                    set_property_str(conn, requestor, property, ty, &bytes).is_ok()
                }
                _ => false,
            }
        };
        dbg(format_args!(
            "xdnd source: {requestor:#x} asks for atom {target}: {}",
            if served { "served" } else { "refused" }
        ));
        let reply = if served { property } else { 0 };
        conn.send(&selection_notify_request(
            requestor, time, selection, target, reply,
        ))
    }
}

/// The first three of `types`, padded with `None` (0).
fn first_three(types: &[u32]) -> [u32; 3] {
    let mut out = [0; 3];
    for (o, t) in out.iter_mut().zip(types) {
        *o = *t;
    }
    out
}

/// Root coordinates as `XdndPosition` packs them: x high, y low.
fn position_word(x: i16, y: i16) -> u32 {
    (u32::from(x as u16) << 16) | u32::from(y as u16)
}

/// `GrabPointer` on `window` (motion, presses and releases reported to it,
/// asynchronous) showing `cursor`, before [`finish`].
fn grab_pointer_request(window: u32, cursor: u32) -> Vec<u8> {
    let mut req = vec![OP_GRAB_POINTER, 0 /* owner events */, 0, 0];
    req.extend_from_slice(&window.to_le_bytes());
    req.extend_from_slice(&GRAB_EVENTS.to_le_bytes());
    req.extend_from_slice(&[1, 1]); // async pointer and keyboard
    req.extend_from_slice(&0u32.to_le_bytes()); // confine to: none
    req.extend_from_slice(&cursor.to_le_bytes());
    req.extend_from_slice(&0u32.to_le_bytes()); // CurrentTime
    req
}

/// A `SendEvent` of `SelectionNotify` to `requestor`, finished.
fn selection_notify_request(
    requestor: u32,
    time: u32,
    selection: u32,
    target: u32,
    property: u32,
) -> Vec<u8> {
    let mut notify = [0u8; 32];
    notify[0] = 31; // SelectionNotify
    notify[4..8].copy_from_slice(&time.to_le_bytes());
    notify[8..12].copy_from_slice(&requestor.to_le_bytes());
    notify[12..16].copy_from_slice(&selection.to_le_bytes());
    notify[16..20].copy_from_slice(&target.to_le_bytes());
    notify[20..24].copy_from_slice(&property.to_le_bytes());
    let mut req = vec![OP_SEND_EVENT, 0, 0, 0];
    req.extend_from_slice(&requestor.to_le_bytes());
    req.extend_from_slice(&0u32.to_le_bytes()); // event mask: the creator
    req.extend_from_slice(&notify);
    finish(req)
}

/// The `XdndAware` window under root `(x, y)`, walking down from the root.
fn find_target(
    conn: &mut Conn,
    a: &Atoms,
    x: i16,
    y: i16,
    backlog: &mut VecDeque<[u8; 32]>,
) -> io::Result<Option<Target>> {
    let root = conn.setup.root;
    let mut window = root;
    for _ in 0..MAX_DEPTH {
        let mut req = vec![OP_TRANSLATE_COORDINATES, 0, 0, 0];
        req.extend_from_slice(&root.to_le_bytes());
        req.extend_from_slice(&window.to_le_bytes());
        req.extend_from_slice(&x.to_le_bytes());
        req.extend_from_slice(&y.to_le_bytes());
        let Some((reply, _)) = round_trip(conn, req, backlog)? else {
            return Ok(None);
        };
        let child = rd_u32(&reply, 8).unwrap_or(0);
        if child == 0 {
            return Ok(None);
        }
        if let Some((ty, data)) = get_property(conn, child, a.aware, false, backlog)?
            && ty == ATOM_ATOM
            && let Some(&version) = atoms_of(&data).first()
            && version >= 3
        {
            return Ok(Some(Target {
                window: child,
                version,
            }));
        }
        window = child;
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Target = Target {
        window: 0x50,
        version: 5,
    };
    const U: Target = Target {
        window: 0x60,
        version: 4,
    };

    fn pos(target: u32, x: i16, time: u32) -> Step {
        Step::Position {
            target,
            x,
            y: 10,
            time,
        }
    }

    #[test]
    fn enters_positions_one_at_a_time_and_drops_when_accepted() {
        let mut m = Machine::default();
        assert!(m.motion(None, 1, 10, 1).is_empty());
        assert_eq!(
            m.motion(Some(T), 2, 10, 2),
            vec![Step::Enter(T), pos(0x50, 2, 2)]
        );
        // Moves while the status is awaited wait too; only the latest goes.
        assert!(m.motion(Some(T), 3, 10, 3).is_empty());
        assert!(m.motion(Some(T), 4, 10, 4).is_empty());
        assert_eq!(
            m.status(0x50, true),
            vec![Step::Feedback(true), pos(0x50, 4, 4)]
        );
        // A status from another window is ignored.
        assert!(m.status(0x99, false).is_empty());
        assert_eq!(m.status(0x50, true), vec![]);
        assert_eq!(
            m.release(9),
            vec![
                Step::Ungrab,
                Step::Drop {
                    target: 0x50,
                    time: 9
                }
            ]
        );
        // Nothing more until the target finishes.
        assert!(m.motion(Some(U), 5, 10, 10).is_empty());
        assert!(m.finished(0x60).is_empty());
        assert_eq!(m.finished(0x50), vec![Step::End]);
    }

    #[test]
    fn moving_between_targets_leaves_the_old_one() {
        let mut m = Machine::default();
        m.motion(Some(T), 1, 10, 1);
        m.status(0x50, true);
        assert_eq!(
            m.motion(Some(U), 2, 10, 2),
            vec![
                Step::Leave(0x50),
                Step::Feedback(false),
                Step::Enter(U),
                pos(0x60, 2, 2)
            ]
        );
        // Off every target: leave, and nothing to position.
        m.status(0x60, false);
        assert_eq!(
            m.motion(None, 3, 10, 3),
            vec![Step::Leave(0x60), Step::Feedback(false)]
        );
        assert_eq!(m.release(4), vec![Step::Ungrab, Step::End]);
    }

    #[test]
    fn a_refusal_or_escape_leaves_without_dropping() {
        let mut m = Machine::default();
        m.motion(Some(T), 1, 10, 1);
        assert_eq!(m.status(0x50, false), vec![]);
        assert_eq!(
            m.release(2),
            vec![Step::Ungrab, Step::Leave(0x50), Step::End]
        );

        let mut m = Machine::default();
        m.motion(Some(T), 1, 10, 1);
        assert_eq!(m.cancel(), vec![Step::Ungrab, Step::Leave(0x50), Step::End]);
    }

    #[test]
    fn a_release_before_the_status_waits_for_it() {
        let mut m = Machine::default();
        m.motion(Some(T), 1, 10, 1);
        assert_eq!(m.release(5), vec![Step::Ungrab]);
        assert_eq!(
            m.status(0x50, true),
            vec![
                Step::Feedback(true),
                Step::Drop {
                    target: 0x50,
                    time: 5
                }
            ]
        );

        let mut m = Machine::default();
        m.motion(Some(T), 1, 10, 1);
        m.release(5);
        assert_eq!(m.status(0x50, false), vec![Step::Leave(0x50), Step::End]);
    }

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
    fn payload_serves_uri_lists_and_text() {
        let a = atoms();
        let files = Payload::new(&a, DropData::Files(vec!["/tmp/a b".into()])).unwrap();
        assert_eq!(files.types, vec![100]);
        assert_eq!(
            files.convert(&a, 100),
            Some((100, b"file:///tmp/a%20b\r\n".to_vec()))
        );
        assert_eq!(files.convert(&a, 101), None);
        let text = Payload::new(&a, DropData::Text("h\u{e9}\u{2713}".into())).unwrap();
        assert_eq!(text.types, vec![101, 103, 102, ATOM_STRING]);
        assert_eq!(
            text.convert(&a, 101),
            Some((101, "h\u{e9}\u{2713}".as_bytes().to_vec()))
        );
        assert_eq!(
            text.convert(&a, ATOM_STRING),
            Some((ATOM_STRING, vec![b'h', 0xe9, b'?']))
        );
        assert!(Payload::new(&a, DropData::Internal("x".into())).is_none());
        assert!(Payload::new(&a, DropData::Files(vec![])).is_none());
    }

    #[test]
    fn requests_are_well_formed() {
        assert_eq!(position_word(300, 20), 300 << 16 | 20);
        assert_eq!(position_word(-1, 0), 0xffff_0000);
        let grab = finish(grab_pointer_request(0x1234, 0x99));
        assert_eq!(grab.len(), 24);
        assert_eq!(grab[0], OP_GRAB_POINTER);
        assert_eq!(u16::from_le_bytes([grab[2], grab[3]]), 6);
        assert_eq!(u32::from_le_bytes(grab[4..8].try_into().unwrap()), 0x1234);
        assert_eq!(u16::from_le_bytes([grab[8], grab[9]]), GRAB_EVENTS);
        assert_eq!(u32::from_le_bytes(grab[16..20].try_into().unwrap()), 0x99);
        let notify = selection_notify_request(0x77, 5, 6, 7, 8);
        assert_eq!(notify.len(), 44);
        assert_eq!(notify[12], 31);
        assert_eq!(u32::from_le_bytes(notify[4..8].try_into().unwrap()), 0x77);
        assert_eq!(u32::from_le_bytes(notify[32..36].try_into().unwrap()), 8);
        assert_eq!(first_three(&[1, 2]), [1, 2, 0]);
        assert_eq!(first_three(&[1, 2, 3, 4]), [1, 2, 3]);
    }
}
