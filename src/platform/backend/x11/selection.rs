//! The `CLIPBOARD` and `PRIMARY` selections, by hand on the wire (ICCCM §2).
//!
//! **Owning** (copying): `SetSelectionOwner`, then answer each
//! `SelectionRequest` by writing the requested target into the requestor's
//! property and sending it a `SelectionNotify`. The targets are `TARGETS`, the
//! text types (`UTF8_STRING`, `text/plain;charset=utf-8`, `STRING`, …),
//! `text/html` and `image/png`. Data larger than a request can carry goes
//! `INCR`: the property first holds the size, typed `INCR`, and each time the
//! requestor deletes it we write the next chunk, ending with an empty one.
//!
//! **Reading** (pasting) never blocks the loop: `ConvertSelection` to
//! `TARGETS`, then to the best type for each format asked for, each answer
//! arriving as a `SelectionNotify` the loop hands back here. An `INCR` answer
//! is read chunk by chunk as `PropertyNotify`s announce them. The result goes
//! to the app as [`Event::Clipboard`]; a request whose owner stays silent is
//! answered with what was read so far (or `None`) after [`TIMEOUT`]. When we
//! own the selection ourselves, the content comes straight from memory.
//!
//! The protocol logic ([`Reader`], [`Owner`]) is pure: it turns events into
//! [`Wire`] actions, which [`perform`] sends.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::{
    ATOM_ATOM, Conn, OP_CHANGE_PROPERTY, OP_SEND_EVENT, X11Window, finish, intern_atom, rd_u32,
};
use crate::platform::ControlFlow;
use crate::platform::clipboard::format::{self, Payload};
use crate::platform::clipboard::{
    ClipboardContent, ClipboardFormats, ClipboardKind, ClipboardRead,
};
use crate::platform::event::Event;
use crate::platform::window::Window;

const OP_CHANGE_WINDOW_ATTRIBUTES: u8 = 2;
const OP_SET_SELECTION_OWNER: u8 = 22;
const OP_CONVERT_SELECTION: u8 = 24;
const CW_EVENT_MASK: u32 = 0x0000_0800;
/// `PropertyChangeMask`.
pub(super) const EV_PROPERTY_CHANGE: u32 = 0x0040_0000;
/// `PRIMARY` is predefined.
const ATOM_PRIMARY: u32 = 1;
/// `PropertyNotify`'s states.
const NEW_VALUE: u8 = 0;
const DELETED: u8 = 1;

/// How long a read waits for the owner's next answer.
pub(super) const TIMEOUT: Duration = Duration::from_secs(2);
/// How long an outgoing `INCR` transfer waits for the requestor.
const SERVE_TIMEOUT: Duration = Duration::from_secs(10);

/// The atoms of the protocol and of every type we know, interned once.
#[derive(Clone, Debug, Default)]
pub(super) struct Atoms {
    pub(super) clipboard: u32,
    pub(super) primary: u32,
    targets: u32,
    incr: u32,
    /// The property on our window that answers are converted into.
    property: u32,
    /// The types we read and write, with their names.
    types: Vec<(u32, &'static str)>,
}

impl Atoms {
    pub(super) fn intern(conn: &mut Conn) -> std::io::Result<Atoms> {
        let mut types = Vec::new();
        let names = format::TEXT_MIMES
            .iter()
            .chain([format::HTML_MIME].iter())
            .chain(format::IMAGE_MIMES.iter());
        for name in names {
            types.push((intern_atom(conn, name.as_bytes())?, *name));
        }
        Ok(Atoms {
            clipboard: intern_atom(conn, b"CLIPBOARD")?,
            primary: ATOM_PRIMARY,
            targets: intern_atom(conn, b"TARGETS")?,
            incr: intern_atom(conn, b"INCR")?,
            property: intern_atom(conn, b"STIPPLE_SELECTION")?,
            types,
        })
    }

    fn selection(&self, kind: ClipboardKind) -> u32 {
        match kind {
            ClipboardKind::Clipboard => self.clipboard,
            ClipboardKind::Primary => self.primary,
        }
    }

    fn kind(&self, selection: u32) -> Option<ClipboardKind> {
        if selection == 0 {
            None
        } else if selection == self.clipboard {
            Some(ClipboardKind::Clipboard)
        } else if selection == self.primary {
            Some(ClipboardKind::Primary)
        } else {
            None
        }
    }

    fn name(&self, atom: u32) -> Option<&'static str> {
        self.types.iter().find(|(a, _)| *a == atom).map(|(_, n)| *n)
    }

    fn atom(&self, name: &str) -> u32 {
        self.types
            .iter()
            .find(|(_, n)| *n == name)
            .map_or(0, |(a, _)| *a)
    }
}

/// Something to send, or to tell the app.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Wire {
    /// `ConvertSelection` into `property` on `requestor`.
    Convert {
        requestor: u32,
        selection: u32,
        target: u32,
        property: u32,
    },
    /// `ChangeProperty` (Replace).
    Property {
        window: u32,
        property: u32,
        ty: u32,
        format: u8,
        data: Vec<u8>,
    },
    /// Select (or, with 0, stop selecting) `PropertyChange` on another
    /// client's window.
    Listen { window: u32, mask: u32 },
    /// A `SelectionNotify` to a requestor (`property` 0: refused).
    Notify {
        requestor: u32,
        time: u32,
        selection: u32,
        target: u32,
        property: u32,
    },
    /// The content for a read, for the app (on `window`).
    Deliver {
        window: u32,
        kind: ClipboardKind,
        content: Option<ClipboardContent>,
    },
}

// ---- Reading ----------------------------------------------------------------

/// A read the app asked for.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Request {
    window: u32,
    kind: ClipboardKind,
    formats: ClipboardFormats,
}

#[derive(Debug)]
enum Phase {
    /// Waiting for the `TARGETS` list.
    Targets,
    /// Waiting for the conversion to `target`.
    Data { target: u32 },
    /// Reading `target` in `INCR` chunks.
    Incr { target: u32, buf: Vec<u8> },
}

#[derive(Debug)]
struct Active {
    req: Request,
    phase: Phase,
    /// The conversions still to ask for.
    wanted: VecDeque<u32>,
    content: ClipboardContent,
    deadline: Instant,
}

/// The reads in progress (one at a time; the rest wait their turn).
#[derive(Debug, Default)]
pub(super) struct Reader {
    queue: VecDeque<Request>,
    active: Option<Active>,
}

impl Reader {
    /// Queue a read of `kind` for `window`; starts it if nothing else runs.
    pub(super) fn request(
        &mut self,
        a: &Atoms,
        window: u32,
        kind: ClipboardKind,
        formats: ClipboardFormats,
        now: Instant,
    ) -> Vec<Wire> {
        self.queue.push_back(Request {
            window,
            kind,
            formats,
        });
        if self.active.is_none() {
            self.next(a, now)
        } else {
            Vec::new()
        }
    }

    /// When the read in progress gives up.
    pub(super) fn deadline(&self) -> Option<Instant> {
        self.active.as_ref().map(|r| r.deadline)
    }

    /// The window the read in progress converts into.
    pub(super) fn window(&self) -> Option<u32> {
        self.active.as_ref().map(|r| r.req.window)
    }

    /// Start the next queued read.
    fn next(&mut self, a: &Atoms, now: Instant) -> Vec<Wire> {
        let Some(req) = self.queue.pop_front() else {
            return Vec::new();
        };
        self.active = Some(Active {
            req,
            phase: Phase::Targets,
            wanted: VecDeque::new(),
            content: ClipboardContent::default(),
            deadline: now + TIMEOUT,
        });
        vec![convert(a, &req, a.targets)]
    }

    /// Whether a `SelectionNotify` answers the conversion in progress.
    pub(super) fn expects(&self, a: &Atoms, requestor: u32, selection: u32, target: u32) -> bool {
        let Some(r) = &self.active else { return false };
        let asked = match r.phase {
            Phase::Targets => a.targets,
            Phase::Data { target } => target,
            Phase::Incr { .. } => return false,
        };
        r.req.window == requestor && selection == a.selection(r.req.kind) && target == asked
    }

    /// The owner refused the conversion in progress (`property` None).
    pub(super) fn refused(&mut self, a: &Atoms, now: Instant) -> Vec<Wire> {
        let Some(r) = self.active.as_mut() else {
            return Vec::new();
        };
        if matches!(r.phase, Phase::Targets) && r.req.formats.text {
            // An owner without TARGETS: ask for text the old way.
            r.wanted = [a.atom("UTF8_STRING"), super::ATOM_STRING].into();
        }
        self.advance(a, now)
    }

    /// The property a `SelectionNotify` pointed at, read (and deleted): its
    /// type and bytes.
    pub(super) fn property(
        &mut self,
        a: &Atoms,
        ty: u32,
        data: Vec<u8>,
        now: Instant,
    ) -> Vec<Wire> {
        let Some(r) = self.active.as_mut() else {
            return Vec::new();
        };
        r.deadline = now + TIMEOUT;
        match r.phase {
            Phase::Targets => {
                let names: Vec<&str> = atoms_of(&data).filter_map(|t| a.name(t)).collect();
                r.wanted = format::pick(&names, r.req.formats)
                    .into_iter()
                    .map(|m| a.atom(m))
                    .collect();
            }
            Phase::Data { target } if ty == a.incr => {
                // Deleting the property (which the read did) starts the
                // transfer; the chunks follow as PropertyNotify.
                r.phase = Phase::Incr {
                    target,
                    buf: Vec::new(),
                };
                return Vec::new();
            }
            Phase::Data { target } => {
                if let Some(mime) = a.name(target) {
                    format::decode_into(&mut r.content, mime, &data);
                }
            }
            Phase::Incr { .. } => return Vec::new(),
        }
        self.advance(a, now)
    }

    /// Whether a `PropertyNotify` announces the next `INCR` chunk (to read
    /// with delete, then hand to [`Reader::chunk`]).
    pub(super) fn chunk_ready(&self, a: &Atoms, window: u32, atom: u32, state: u8) -> bool {
        self.active.as_ref().is_some_and(|r| {
            matches!(r.phase, Phase::Incr { .. })
                && r.req.window == window
                && atom == a.property
                && state == NEW_VALUE
        })
    }

    /// An `INCR` chunk: more data, or (empty) the end.
    pub(super) fn chunk(&mut self, a: &Atoms, data: &[u8], now: Instant) -> Vec<Wire> {
        let Some(r) = self.active.as_mut() else {
            return Vec::new();
        };
        let Phase::Incr { target, buf } = &mut r.phase else {
            return Vec::new();
        };
        r.deadline = now + TIMEOUT;
        if !data.is_empty() {
            buf.extend_from_slice(data);
            return Vec::new();
        }
        if let Some(mime) = a.name(*target) {
            format::decode_into(&mut r.content, mime, buf);
        }
        self.advance(a, now)
    }

    /// Ask for the next wanted type, or deliver and start the next read.
    fn advance(&mut self, a: &Atoms, now: Instant) -> Vec<Wire> {
        let Some(r) = self.active.as_mut() else {
            return Vec::new();
        };
        if let Some(target) = r.wanted.pop_front() {
            r.phase = Phase::Data { target };
            return vec![convert(a, &r.req, target)];
        }
        self.finish(a, now)
    }

    /// Deliver what the read in progress got, and start the next one.
    fn finish(&mut self, a: &Atoms, now: Instant) -> Vec<Wire> {
        let Some(r) = self.active.take() else {
            return Vec::new();
        };
        let content = Some(r.content).filter(|c| !c.is_empty());
        let mut out = vec![Wire::Deliver {
            window: r.req.window,
            kind: r.req.kind,
            content,
        }];
        out.extend(self.next(a, now));
        out
    }

    /// Give up on a read whose owner went silent.
    pub(super) fn expire(&mut self, a: &Atoms, now: Instant) -> Vec<Wire> {
        match self.deadline() {
            Some(at) if at <= now => self.finish(a, now),
            _ => Vec::new(),
        }
    }

    /// Answer the reads for a closed window with nothing.
    fn forget_window(&mut self, window: u32) {
        self.queue.retain(|r| r.window != window);
        if self.window() == Some(window) {
            self.active = None;
        }
    }
}

fn convert(a: &Atoms, req: &Request, target: u32) -> Wire {
    Wire::Convert {
        requestor: req.window,
        selection: a.selection(req.kind),
        target,
        property: a.property,
    }
}

/// The 32-bit values in a format-32 property's bytes.
fn atoms_of(data: &[u8]) -> impl Iterator<Item = u32> + '_ {
    data.as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
}

// ---- Owning -----------------------------------------------------------------

/// A selection we own.
#[derive(Debug)]
struct Owned {
    window: u32,
    payload: Payload,
}

/// An `INCR` transfer to another client.
#[derive(Debug)]
struct Outgoing {
    requestor: u32,
    property: u32,
    ty: u32,
    data: Vec<u8>,
    sent: usize,
    /// The final, empty chunk was written.
    done: bool,
    deadline: Instant,
}

/// The selections we own, and the transfers in progress.
#[derive(Debug, Default)]
pub(super) struct Owner {
    clipboard: Option<Owned>,
    primary: Option<Owned>,
    outgoing: Vec<Outgoing>,
}

/// A `SelectionRequest`'s fields.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SelectionRequest {
    pub(super) time: u32,
    pub(super) owner: u32,
    pub(super) requestor: u32,
    pub(super) selection: u32,
    pub(super) target: u32,
    pub(super) property: u32,
}

impl SelectionRequest {
    pub(super) fn parse(ev: &[u8; 32]) -> SelectionRequest {
        let at = |o| rd_u32(ev, o).unwrap_or(0);
        SelectionRequest {
            time: at(4),
            owner: at(8),
            requestor: at(12),
            selection: at(16),
            target: at(20),
            property: at(24),
        }
    }
}

impl Owner {
    fn slot(&mut self, kind: ClipboardKind) -> &mut Option<Owned> {
        match kind {
            ClipboardKind::Clipboard => &mut self.clipboard,
            ClipboardKind::Primary => &mut self.primary,
        }
    }

    /// What we own as `kind`, if anything.
    pub(super) fn content(&self, kind: ClipboardKind) -> Option<&ClipboardContent> {
        let slot = match kind {
            ClipboardKind::Clipboard => &self.clipboard,
            ClipboardKind::Primary => &self.primary,
        };
        slot.as_ref().map(|o| &o.payload.content)
    }

    /// Take `kind` for `window` with `content`: the `SetSelectionOwner` to send.
    pub(super) fn own(
        &mut self,
        a: &Atoms,
        kind: ClipboardKind,
        window: u32,
        content: ClipboardContent,
    ) -> Vec<u8> {
        let payload = Payload::new(content);
        let owner = if payload.is_empty() { 0 } else { window };
        *self.slot(kind) = (owner != 0).then_some(Owned { window, payload });
        let mut req = vec![OP_SET_SELECTION_OWNER, 0, 0, 0];
        req.extend_from_slice(&owner.to_le_bytes());
        req.extend_from_slice(&a.selection(kind).to_le_bytes());
        req.extend_from_slice(&0u32.to_le_bytes()); // CurrentTime
        finish(req)
    }

    /// Another client took the selection (a `SelectionClear`).
    pub(super) fn lost(&mut self, a: &Atoms, owner: u32, selection: u32) {
        if let Some(kind) = a.kind(selection) {
            let slot = self.slot(kind);
            if slot.as_ref().is_some_and(|o| o.window == owner) {
                *slot = None;
            }
        }
    }

    /// Forget what a closed window owned.
    fn forget_window(&mut self, window: u32) {
        for kind in [ClipboardKind::Clipboard, ClipboardKind::Primary] {
            let slot = self.slot(kind);
            if slot.as_ref().is_some_and(|o| o.window == window) {
                *slot = None;
            }
        }
    }

    /// Answer a `SelectionRequest`. Data over `max` bytes goes `INCR`.
    pub(super) fn serve(
        &mut self,
        a: &Atoms,
        req: SelectionRequest,
        max: usize,
        now: Instant,
    ) -> Vec<Wire> {
        self.outgoing.retain(|o| o.deadline > now);
        // An obsolete requestor may send property None: reply into the target.
        let property = if req.property == 0 {
            req.target
        } else {
            req.property
        };
        let mut out = Vec::new();
        let owned = a
            .kind(req.selection)
            .and_then(|kind| self.slot(kind).as_ref())
            .filter(|o| o.window == req.owner || req.owner == 0);
        let answered = match owned {
            None => false,
            Some(o) if req.target == a.targets => {
                let mut list = vec![a.targets];
                list.extend(o.payload.mimes().into_iter().map(|m| a.atom(m)));
                let data = list.iter().flat_map(|t| t.to_le_bytes()).collect();
                out.push(Wire::Property {
                    window: req.requestor,
                    property,
                    ty: ATOM_ATOM,
                    format: 32,
                    data,
                });
                true
            }
            Some(o) => match a.name(req.target).and_then(|m| o.payload.bytes(m)) {
                None => false,
                Some(data) if data.len() > max => {
                    out.push(Wire::Listen {
                        window: req.requestor,
                        mask: EV_PROPERTY_CHANGE,
                    });
                    out.push(Wire::Property {
                        window: req.requestor,
                        property,
                        ty: a.incr,
                        format: 32,
                        data: (data.len() as u32).to_le_bytes().to_vec(),
                    });
                    // A newer request for the same property replaces it.
                    self.outgoing
                        .retain(|o| (o.requestor, o.property) != (req.requestor, property));
                    self.outgoing.push(Outgoing {
                        requestor: req.requestor,
                        property,
                        ty: req.target,
                        data,
                        sent: 0,
                        done: false,
                        deadline: now + SERVE_TIMEOUT,
                    });
                    true
                }
                Some(data) => {
                    out.push(Wire::Property {
                        window: req.requestor,
                        property,
                        ty: req.target,
                        format: 8,
                        data,
                    });
                    true
                }
            },
        };
        out.push(Wire::Notify {
            requestor: req.requestor,
            time: req.time,
            selection: req.selection,
            target: req.target,
            property: if answered { property } else { 0 },
        });
        out
    }

    /// A `PropertyNotify` on another client's window: when it deleted the
    /// property of an `INCR` transfer, write the next chunk of up to `max`.
    pub(super) fn property_notify(
        &mut self,
        window: u32,
        atom: u32,
        state: u8,
        max: usize,
        now: Instant,
    ) -> Vec<Wire> {
        if state != DELETED {
            return Vec::new();
        }
        let Some(i) = self
            .outgoing
            .iter()
            .position(|o| o.requestor == window && o.property == atom)
        else {
            return Vec::new();
        };
        let o = &mut self.outgoing[i];
        if o.done {
            // It deleted the final empty chunk: the transfer is over.
            let o = self.outgoing.remove(i);
            return vec![Wire::Listen {
                window: o.requestor,
                mask: 0,
            }];
        }
        let end = (o.sent + max).min(o.data.len());
        let data = o.data[o.sent..end].to_vec();
        o.done = data.is_empty();
        o.sent = end;
        o.deadline = now + SERVE_TIMEOUT;
        vec![Wire::Property {
            window,
            property: atom,
            ty: o.ty,
            format: 8,
            data,
        }]
    }
}

// ---- On the connection --------------------------------------------------------

/// The selection state of the connection.
#[derive(Debug, Default)]
pub(super) struct Selections {
    pub(super) atoms: Atoms,
    pub(super) reader: Reader,
    pub(super) owner: Owner,
}

impl Selections {
    /// Forget a closed window's selections and reads.
    pub(super) fn forget_window(&mut self, window: u32) {
        self.reader.forget_window(window);
        self.owner.forget_window(window);
    }
}

/// The largest property chunk one request carries.
pub(super) fn max_chunk(conn: &Conn) -> usize {
    // Leave room for ChangeProperty's 24-byte header.
    ((conn.setup.max_request_len as usize) * 4)
        .saturating_sub(64)
        .clamp(4096, 256 * 1024)
}

/// A request's bytes, or `None` for what goes to the app.
pub(super) fn encode(w: &Wire) -> Option<Vec<u8>> {
    let u32s = |req: &mut Vec<u8>, vals: &[u32]| {
        for v in vals {
            req.extend_from_slice(&v.to_le_bytes());
        }
    };
    let mut req;
    match w {
        Wire::Convert {
            requestor,
            selection,
            target,
            property,
        } => {
            req = vec![OP_CONVERT_SELECTION, 0, 0, 0];
            u32s(&mut req, &[*requestor, *selection, *target, *property, 0]);
        }
        Wire::Property {
            window,
            property,
            ty,
            format,
            data,
        } => {
            req = vec![OP_CHANGE_PROPERTY, 0 /* Replace */, 0, 0];
            u32s(&mut req, &[*window, *property, *ty]);
            req.extend_from_slice(&[*format, 0, 0, 0]);
            let units = data.len() / (*format as usize / 8).max(1);
            u32s(&mut req, &[units as u32]);
            req.extend_from_slice(data);
            while !req.len().is_multiple_of(4) {
                req.push(0);
            }
        }
        Wire::Listen { window, mask } => {
            req = vec![OP_CHANGE_WINDOW_ATTRIBUTES, 0, 0, 0];
            u32s(&mut req, &[*window, CW_EVENT_MASK, *mask]);
        }
        Wire::Notify {
            requestor,
            time,
            selection,
            target,
            property,
        } => {
            let mut ev = [0u8; 32];
            ev[0] = 31; // SelectionNotify
            for (i, v) in [*time, *requestor, *selection, *target, *property]
                .iter()
                .enumerate()
            {
                ev[4 + 4 * i..8 + 4 * i].copy_from_slice(&v.to_le_bytes());
            }
            req = vec![OP_SEND_EVENT, 0 /* propagate */, 0, 0];
            u32s(&mut req, &[*requestor, 0 /* event mask: the creator */]);
            req.extend_from_slice(&ev);
        }
        Wire::Deliver { .. } => return None,
    }
    Some(finish(req))
}

/// Send the requests among `wires`; return the events for the app, with the
/// window each goes to.
pub(super) fn perform(conn: &mut Conn, wires: Vec<Wire>) -> Vec<(u32, Event)> {
    let mut events = Vec::new();
    for w in wires {
        match encode(&w) {
            Some(req) => {
                let _ = conn.send(&req);
            }
            None => {
                if let Wire::Deliver {
                    window,
                    kind,
                    content,
                } = w
                {
                    super::dbg(format_args!(
                        "selection {kind:?} read: {}",
                        if content.is_some() {
                            "content"
                        } else {
                            "nothing"
                        }
                    ));
                    events.push((window, Event::Clipboard { kind, content }));
                }
            }
        }
    }
    events
}

// ---- The event loop's and the window's entry points ----------------------------

type Handler<'a> = &'a mut dyn FnMut(Event, &dyn Window) -> ControlFlow;

/// Hand `events` to the app, each on its window (or the first one).
pub(super) fn deliver(
    events: Vec<(u32, Event)>,
    windows: &[X11Window],
    handler: Handler<'_>,
) -> ControlFlow {
    for (xid, event) in events {
        let Some(win) = windows.iter().find(|w| w.window == xid).or(windows.first()) else {
            break;
        };
        if handler(event, win) == ControlFlow::Exit {
            return ControlFlow::Exit;
        }
    }
    ControlFlow::Wait
}

/// [`Window::read_clipboard`] for `window`: from memory when we own the
/// selection, else start a conversion and answer later.
pub(super) fn read(
    conn: &mut Conn,
    window: u32,
    kind: ClipboardKind,
    formats: ClipboardFormats,
) -> ClipboardRead {
    if let Some(content) = conn.sel.owner.content(kind) {
        let content = content.clone().only(formats);
        return ClipboardRead::Ready((!content.is_empty()).then_some(content));
    }
    if conn.sel.atoms.clipboard == 0 {
        return ClipboardRead::Ready(None);
    }
    let sel = &mut conn.sel;
    let wires = sel
        .reader
        .request(&sel.atoms, window, kind, formats, Instant::now());
    // Nothing is delivered yet: a read starts with a conversion.
    let _ = perform(conn, wires);
    ClipboardRead::Pending
}

/// [`Window::write_clipboard`] for `window`: own the selection.
pub(super) fn write(conn: &mut Conn, window: u32, kind: ClipboardKind, content: &ClipboardContent) {
    if conn.sel.atoms.clipboard == 0 {
        return;
    }
    let sel = &mut conn.sel;
    let req = sel.owner.own(&sel.atoms, kind, window, content.clone());
    let _ = conn.send(&req);
}

/// A `SelectionNotify` for one of our reads: fetch the answer and go on.
pub(super) fn selection_notify(
    conn: &mut Conn,
    backlog: &mut VecDeque<[u8; 32]>,
    ev: &[u8; 32],
) -> Vec<(u32, Event)> {
    let at = |o| rd_u32(ev, o).unwrap_or(0);
    let (requestor, selection, target, property) = (at(8), at(12), at(16), at(20));
    if !conn
        .sel
        .reader
        .expects(&conn.sel.atoms, requestor, selection, target)
    {
        return Vec::new();
    }
    let now = Instant::now();
    let wires = if property == 0 {
        let sel = &mut conn.sel;
        sel.reader.refused(&sel.atoms, now)
    } else {
        let got = super::dnd::get_property(conn, requestor, property, true, backlog);
        let (ty, data) = got.ok().flatten().unwrap_or_default();
        let sel = &mut conn.sel;
        sel.reader.property(&sel.atoms, ty, data, now)
    };
    perform(conn, wires)
}

/// A `PropertyNotify`: the next chunk of an `INCR` read into our window, or
/// a requestor taking a chunk of one we serve.
pub(super) fn property_notify(
    conn: &mut Conn,
    backlog: &mut VecDeque<[u8; 32]>,
    ev: &[u8; 32],
) -> Vec<(u32, Event)> {
    let (window, atom, state) = (
        rd_u32(ev, 4).unwrap_or(0),
        rd_u32(ev, 8).unwrap_or(0),
        ev[16],
    );
    let now = Instant::now();
    let wires = if conn
        .sel
        .reader
        .chunk_ready(&conn.sel.atoms, window, atom, state)
    {
        let got = super::dnd::get_property(conn, window, atom, true, backlog);
        let (_, data) = got.ok().flatten().unwrap_or_default();
        let sel = &mut conn.sel;
        sel.reader.chunk(&sel.atoms, &data, now)
    } else {
        let max = max_chunk(conn);
        conn.sel
            .owner
            .property_notify(window, atom, state, max, now)
    };
    perform(conn, wires)
}

/// A `SelectionRequest` for a selection we own.
pub(super) fn selection_request(conn: &mut Conn, ev: &[u8; 32]) {
    let req = SelectionRequest::parse(ev);
    let max = max_chunk(conn);
    let sel = &mut conn.sel;
    let wires = sel.owner.serve(&sel.atoms, req, max, Instant::now());
    perform(conn, wires);
}

/// A `SelectionClear`: another client owns the selection now.
pub(super) fn selection_clear(conn: &mut Conn, ev: &[u8; 32]) {
    let (owner, selection) = (rd_u32(ev, 8).unwrap_or(0), rd_u32(ev, 12).unwrap_or(0));
    let sel = &mut conn.sel;
    sel.owner.lost(&sel.atoms, owner, selection);
}

/// Answer reads whose owner went silent.
pub(super) fn expire(conn: &mut Conn) -> Vec<(u32, Event)> {
    let sel = &mut conn.sel;
    let wires = sel.reader.expire(&sel.atoms, Instant::now());
    perform(conn, wires)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::PhysicalSize;
    use crate::render::Pixmap;

    fn atoms() -> Atoms {
        let mut types = Vec::new();
        let names = format::TEXT_MIMES
            .iter()
            .chain([format::HTML_MIME].iter())
            .chain(format::IMAGE_MIMES.iter());
        for (i, name) in names.enumerate() {
            // STRING is predefined.
            let atom = if *name == "STRING" {
                31
            } else {
                100 + i as u32
            };
            types.push((atom, *name));
        }
        Atoms {
            clipboard: 90,
            primary: 1,
            targets: 91,
            incr: 92,
            property: 93,
            types,
        }
    }

    /// Bytes that don't compress (a linear congruential sequence).
    fn noise(n: usize) -> Vec<u8> {
        let mut x = 12345u32;
        (0..n)
            .map(|_| {
                x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
                (x >> 16) as u8
            })
            .collect()
    }

    fn atom_list(a: &[u32]) -> Vec<u8> {
        a.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    #[test]
    fn reads_targets_then_each_format() {
        let a = atoms();
        let now = Instant::now();
        let mut r = Reader::default();
        let utf8 = a.atom("UTF8_STRING");
        let html = a.atom("text/html");
        let png = a.atom("image/png");
        // TARGETS first.
        let w = r.request(&a, 7, ClipboardKind::Clipboard, ClipboardFormats::ALL, now);
        assert_eq!(
            w,
            vec![Wire::Convert {
                requestor: 7,
                selection: 90,
                target: 91,
                property: 93
            }]
        );
        assert!(r.expects(&a, 7, 90, 91));
        assert!(!r.expects(&a, 8, 90, 91));
        // The owner offers STRING, UTF8_STRING and HTML: UTF8 is asked first.
        let w = r.property(&a, ATOM_ATOM, atom_list(&[91, 31, utf8, html, 4242]), now);
        assert!(matches!(w[..], [Wire::Convert { target, .. }] if target == utf8));
        let w = r.property(&a, utf8, "h\u{e9}llo".into(), now);
        assert!(matches!(w[..], [Wire::Convert { target, .. }] if target == html));
        assert!(!r.expects(&a, 7, 90, png));
        let w = r.property(&a, html, b"<b>h</b>".to_vec(), now);
        let Wire::Deliver {
            window, content, ..
        } = &w[0]
        else {
            panic!("{w:?}")
        };
        assert_eq!(*window, 7);
        let content = content.as_ref().unwrap();
        assert_eq!(content.text.as_deref(), Some("h\u{e9}llo"));
        assert_eq!(content.html.as_deref(), Some("<b>h</b>"));
        assert!(r.deadline().is_none());
    }

    #[test]
    fn reads_incr_chunks() {
        let a = atoms();
        let now = Instant::now();
        let mut r = Reader::default();
        let utf8 = a.atom("UTF8_STRING");
        r.request(&a, 7, ClipboardKind::Primary, ClipboardFormats::TEXT, now);
        r.property(&a, ATOM_ATOM, atom_list(&[utf8]), now);
        // The answer is INCR: wait for chunks.
        assert!(
            r.property(&a, a.incr, 10u32.to_le_bytes().to_vec(), now)
                .is_empty()
        );
        assert!(!r.expects(&a, 7, 1, utf8));
        assert!(!r.chunk_ready(&a, 7, a.property, DELETED));
        assert!(r.chunk_ready(&a, 7, a.property, NEW_VALUE));
        assert!(!r.chunk_ready(&a, 8, a.property, NEW_VALUE));
        assert!(r.chunk(&a, b"hello ", now).is_empty());
        assert!(r.chunk(&a, b"world", now).is_empty());
        let w = r.chunk(&a, b"", now);
        assert_eq!(
            w,
            vec![Wire::Deliver {
                window: 7,
                kind: ClipboardKind::Primary,
                content: Some(ClipboardContent::text("hello world")),
            }]
        );
    }

    #[test]
    fn falls_back_without_targets_and_times_out() {
        let a = atoms();
        let now = Instant::now();
        let mut r = Reader::default();
        r.request(&a, 7, ClipboardKind::Clipboard, ClipboardFormats::TEXT, now);
        // A second read waits its turn.
        assert!(
            r.request(&a, 7, ClipboardKind::Primary, ClipboardFormats::TEXT, now)
                .is_empty()
        );
        // No TARGETS: UTF8_STRING, then STRING.
        let w = r.refused(&a, now);
        assert!(matches!(w[..], [Wire::Convert { target, .. }] if target == a.atom("UTF8_STRING")));
        let w = r.refused(&a, now);
        assert!(matches!(w[..], [Wire::Convert { target: 31, .. }]));
        let w = r.property(&a, 31, b"caf\xe9".to_vec(), now);
        // Delivered, and the queued PRIMARY read starts.
        assert!(
            matches!(&w[0], Wire::Deliver { content: Some(c), .. } if c.text.as_deref() == Some("caf\u{e9}"))
        );
        assert!(matches!(
            w[1],
            Wire::Convert {
                selection: 1,
                target: 91,
                ..
            }
        ));
        // Its owner never answers.
        assert!(r.expire(&a, now + TIMEOUT / 2).is_empty());
        let w = r.expire(&a, now + TIMEOUT);
        assert_eq!(
            w,
            vec![Wire::Deliver {
                window: 7,
                kind: ClipboardKind::Primary,
                content: None
            }]
        );
        assert!(r.deadline().is_none());
    }

    #[test]
    fn serves_targets_text_and_refuses_the_rest() {
        let a = atoms();
        let now = Instant::now();
        let mut o = Owner::default();
        let req = o.own(
            &a,
            ClipboardKind::Clipboard,
            5,
            ClipboardContent::text("hi"),
        );
        assert_eq!(req[0], OP_SET_SELECTION_OWNER);
        assert_eq!(rd_u32(&req, 4), Some(5));
        assert_eq!(rd_u32(&req, 8), Some(90));
        let ask = |target, property| SelectionRequest {
            time: 3,
            owner: 5,
            requestor: 9,
            selection: 90,
            target,
            property,
        };
        let w = o.serve(&a, ask(91, 50), 1000, now);
        let Wire::Property {
            ty, format, data, ..
        } = &w[0]
        else {
            panic!()
        };
        assert_eq!((*ty, *format), (ATOM_ATOM, 32));
        let offered: Vec<u32> = atoms_of(data).collect();
        assert!(offered.contains(&91) && offered.contains(&31));
        assert!(!offered.contains(&a.atom("text/html")));
        assert!(matches!(w[1], Wire::Notify { property: 50, .. }));
        // Text, into the target atom for an obsolete requestor.
        let utf8 = a.atom("UTF8_STRING");
        let w = o.serve(&a, ask(utf8, 0), 1000, now);
        assert!(
            matches!(&w[0], Wire::Property { property, data, .. } if *property == utf8 && data == b"hi")
        );
        // No HTML here; PRIMARY isn't ours.
        let w = o.serve(&a, ask(a.atom("text/html"), 50), 1000, now);
        assert!(matches!(w[..], [Wire::Notify { property: 0, .. }]));
        let w = o.serve(
            &a,
            SelectionRequest {
                selection: 1,
                ..ask(utf8, 50)
            },
            1000,
            now,
        );
        assert!(matches!(w[..], [Wire::Notify { property: 0, .. }]));
        // Lost to another client.
        o.lost(&a, 5, 90);
        assert!(o.content(ClipboardKind::Clipboard).is_none());
    }

    #[test]
    fn serves_large_data_incrementally() {
        let a = atoms();
        let now = Instant::now();
        let mut o = Owner::default();
        let image = Pixmap::from_rgba8(PhysicalSize::new(64, 64), noise(64 * 64 * 4));
        o.own(
            &a,
            ClipboardKind::Clipboard,
            5,
            ClipboardContent::image(image.clone()),
        );
        let png = a.atom("image/png");
        let ask = SelectionRequest {
            time: 0,
            owner: 5,
            requestor: 9,
            selection: 90,
            target: png,
            property: 50,
        };
        let max = 1000;
        let w = o.serve(&a, ask, max, now);
        assert_eq!(
            w[0],
            Wire::Listen {
                window: 9,
                mask: EV_PROPERTY_CHANGE
            }
        );
        let Wire::Property { ty, data, .. } = &w[1] else {
            panic!()
        };
        assert_eq!(*ty, a.incr);
        let total = u32::from_le_bytes(data[..4].try_into().unwrap()) as usize;
        assert!(total > max);
        assert!(matches!(w[2], Wire::Notify { property: 50, .. }));
        // Each deletion brings the next chunk; a new value doesn't.
        assert!(o.property_notify(9, 50, NEW_VALUE, max, now).is_empty());
        assert!(o.property_notify(9, 51, DELETED, max, now).is_empty());
        let mut got = Vec::new();
        loop {
            let w = o.property_notify(9, 50, DELETED, max, now);
            let [Wire::Property { ty, data, .. }] = &w[..] else {
                panic!("{w:?}")
            };
            assert_eq!(*ty, png);
            assert!(data.len() <= max);
            if data.is_empty() {
                break;
            }
            got.extend_from_slice(data);
        }
        assert_eq!(got.len(), total);
        assert_eq!(Pixmap::decode(&got).unwrap(), image);
        // Deleting the empty chunk ends it.
        assert_eq!(
            o.property_notify(9, 50, DELETED, max, now),
            vec![Wire::Listen { window: 9, mask: 0 }]
        );
        assert!(o.property_notify(9, 50, DELETED, max, now).is_empty());
    }

    #[test]
    fn requests_are_well_formed() {
        let convert = encode(&Wire::Convert {
            requestor: 1,
            selection: 2,
            target: 3,
            property: 4,
        })
        .unwrap();
        assert_eq!(convert.len(), 24);
        assert_eq!((convert[0], convert[2]), (OP_CONVERT_SELECTION, 6));
        assert_eq!(rd_u32(&convert, 12), Some(3));
        assert_eq!(rd_u32(&convert, 20), Some(0)); // CurrentTime
        let prop = encode(&Wire::Property {
            window: 1,
            property: 2,
            ty: 3,
            format: 8,
            data: b"hello".to_vec(),
        })
        .unwrap();
        assert_eq!(prop.len(), 24 + 8);
        assert_eq!(rd_u32(&prop, 20), Some(5));
        let atoms = encode(&Wire::Property {
            window: 1,
            property: 2,
            ty: ATOM_ATOM,
            format: 32,
            data: atom_list(&[7, 8]),
        })
        .unwrap();
        assert_eq!(rd_u32(&atoms, 20), Some(2)); // in 32-bit units
        let notify = encode(&Wire::Notify {
            requestor: 0xcafe,
            time: 1,
            selection: 2,
            target: 3,
            property: 4,
        })
        .unwrap();
        // SendEvent: 12-byte header + the 32-byte event = 11 words.
        assert_eq!(
            (notify[0], notify.len(), notify[2]),
            (OP_SEND_EVENT, 44, 11)
        );
        assert_eq!(rd_u32(&notify, 4), Some(0xcafe));
        assert_eq!(notify[12], 31);
        assert_eq!(rd_u32(&notify, 12 + 8), Some(0xcafe)); // requestor
        assert_eq!(rd_u32(&notify, 12 + 20), Some(4)); // property
        let listen = encode(&Wire::Listen {
            window: 9,
            mask: EV_PROPERTY_CHANGE,
        })
        .unwrap();
        assert_eq!((listen[0], listen.len()), (OP_CHANGE_WINDOW_ATTRIBUTES, 16));
        assert!(
            encode(&Wire::Deliver {
                window: 1,
                kind: ClipboardKind::Clipboard,
                content: None
            })
            .is_none()
        );
    }
}
