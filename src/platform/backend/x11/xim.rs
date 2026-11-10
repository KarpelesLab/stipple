//! An XIM client: input methods (IBus, Fcitx, uim, kinput2…) on X11, spoken
//! over the wire like the rest of the backend, without libX11.
//!
//! XIM is a protocol carried in `ClientMessage`s and window properties
//! between the application and an input method server ("The Input Method
//! Protocol", X11R6). This module has three layers:
//!
//! - **Discovery and transport** ([`Xim`]): find the server through the root
//!   window's `XIM_SERVERS` property and the `@im=` name in `$XMODIFIERS`
//!   (`@server=NAME` is a selection the server owns), make a small
//!   communication window, and do the `_XIM_XCONNECT` handshake. After that
//!   each XIM message travels in one `_XIM_PROTOCOL` client message when it
//!   fits in 20 bytes, and otherwise in a property on the other side's
//!   window, announced by a format-32 client message ("property with
//!   ClientMessage", transport version 0.0, what libX11 uses).
//! - **The protocol** ([`Protocol`]): a state machine that never blocks.
//!   `XIM_CONNECT`, `XIM_OPEN` and `XIM_ENCODING_NEGOTIATION` open the input
//!   method; an input context (`XIM_CREATE_IC`, on-the-spot:
//!   `XIMPreeditCallbacks | XIMStatusNothing`) is created per window the first
//!   time a text field there takes focus. While one does, key events go to
//!   the server (`XIM_FORWARD_EVENT`); it answers with `XIM_COMMIT` (text),
//!   the `XIM_PREEDIT_*` callbacks (the composition, drawn inline by the
//!   field) or the key itself, forwarded back when the input method didn't
//!   use it. Focus and the caret position (`spotLocation`, for the candidate
//!   window) follow [`Window::set_ime`](crate::platform::Window::set_ime).
//! - **Text**: servers send COMPOUND_TEXT unless they accept UTF-8, which is
//!   offered first; see [`super::ctext`].
//!
//! Every wait has a deadline (the handshake, creating an input context, a
//! synchronous key). When one passes, or the server's window is destroyed,
//! or it answers with an error during setup, the client switches itself off:
//! keys it held go back to the direct path (keysyms, dead keys and compose),
//! and any preedit is cleared. With no server (or `STIPPLE_XIM=0`) it never
//! starts. A server started after the app isn't picked up.

use std::collections::VecDeque;
use std::io;
use std::io::Read;
use std::time::{Duration, Instant};

use super::{
    ATOM_STRING, CW_EVENT_MASK, Conn, ControlFlow, EV_STRUCTURE_NOTIFY, Event, OP_CHANGE_PROPERTY,
    OP_CHANGE_WINDOW_ATTRIBUTES, OP_CREATE_WINDOW, OP_GET_PROPERTY, OP_SEND_EVENT, SharedConn,
    Window, X_CLIENT_MESSAGE, X11Window, dbg, finish, intern_atom, pad4, rd_u16, rd_u32,
};
use crate::platform::event::Ime;

// ---- Protocol constants ------------------------------------------------------

const XIM_CONNECT: u8 = 1;
const XIM_CONNECT_REPLY: u8 = 2;
const XIM_AUTH_REQUIRED: u8 = 10;
const XIM_AUTH_NG: u8 = 14;
const XIM_ERROR: u8 = 20;
const XIM_OPEN: u8 = 30;
const XIM_OPEN_REPLY: u8 = 31;
const XIM_SET_EVENT_MASK: u8 = 37;
const XIM_ENCODING_NEGOTIATION: u8 = 38;
const XIM_ENCODING_NEGOTIATION_REPLY: u8 = 39;
const XIM_CREATE_IC: u8 = 50;
const XIM_CREATE_IC_REPLY: u8 = 51;
const XIM_SET_IC_VALUES: u8 = 54;
const XIM_SET_IC_FOCUS: u8 = 58;
const XIM_UNSET_IC_FOCUS: u8 = 59;
const XIM_FORWARD_EVENT: u8 = 60;
const XIM_SYNC: u8 = 61;
const XIM_SYNC_REPLY: u8 = 62;
const XIM_COMMIT: u8 = 63;
const XIM_RESET_IC: u8 = 64;
const XIM_PREEDIT_START: u8 = 73;
const XIM_PREEDIT_START_REPLY: u8 = 74;
const XIM_PREEDIT_DRAW: u8 = 75;
const XIM_PREEDIT_CARET: u8 = 76;
const XIM_PREEDIT_CARET_REPLY: u8 = 77;
const XIM_PREEDIT_DONE: u8 = 78;

/// `XIMPreeditCallbacks | XIMStatusNothing`: the server tells us the
/// composition and we draw it.
const INPUT_STYLE: u32 = 0x0002 | 0x0400;

/// `XIM_FORWARD_EVENT` / `XIM_COMMIT` flag bits.
const FLAG_SYNC: u16 = 1;
const COMMIT_CHARS: u16 = 2;
const COMMIT_KEYSYM: u16 = 4;

/// `XIM_PREEDIT_DRAW` status bits.
const DRAW_NO_STRING: u32 = 1;
const DRAW_NO_FEEDBACK: u32 = 2;
/// `XIMReverse`: the segment being converted.
const FEEDBACK_REVERSE: u32 = 1;

/// Core event masks, as `XIM_SET_EVENT_MASK` uses them.
const KEY_PRESS_MASK: u32 = 1;
const KEY_RELEASE_MASK: u32 = 2;

/// Encodings we offer, in order of preference.
const ENCODINGS: [&str; 2] = ["UTF-8", "COMPOUND_TEXT"];

/// How long the server may take to answer during setup. Keys take the
/// direct path meanwhile, so this can be generous: a busy server (IBus
/// switching engines) can take seconds.
const SETUP_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a synchronous key may wait for the server.
const SYNC_TIMEOUT: Duration = Duration::from_millis(1500);

/// The largest message sent in one client message.
const CM_DATA: usize = 20;
/// Properties used, in turn, for messages too big for a client message.
const DATA_ATOMS: usize = 4;

// ---- Encoding ----------------------------------------------------------------

/// An XIM message: the 4-byte header (major opcode, minor opcode 0, length
/// in 4-byte units) and `body` padded to 4 bytes.
fn message(major: u8, body: &[u8]) -> Vec<u8> {
    let mut m = vec![major, 0, 0, 0];
    m.extend_from_slice(body);
    pad4(&mut m);
    let words = ((m.len() - 4) / 4) as u16;
    m[2..4].copy_from_slice(&words.to_le_bytes());
    m
}

/// A message whose body is the IM and IC ids and then `rest`.
fn ic_message(major: u8, im: u16, ic: u16, rest: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(4 + rest.len());
    body.extend_from_slice(&im.to_le_bytes());
    body.extend_from_slice(&ic.to_le_bytes());
    body.extend_from_slice(rest);
    message(major, &body)
}

fn pad_to(buf: &mut Vec<u8>, len: usize) {
    buf.resize(buf.len() + (4 - len % 4) % 4, 0);
}

/// One XICATTRIBUTE: id, value length, value, padded.
fn attribute(id: u16, value: &[u8]) -> Vec<u8> {
    let mut a = Vec::with_capacity(4 + value.len() + 3);
    a.extend_from_slice(&id.to_le_bytes());
    a.extend_from_slice(&(value.len() as u16).to_le_bytes());
    a.extend_from_slice(value);
    pad_to(&mut a, value.len());
    a
}

fn connect_message() -> Vec<u8> {
    // Byte order 'l' (little-endian), protocol 1.0, no authentication.
    message(XIM_CONNECT, &[b'l', 0, 1, 0, 0, 0, 0, 0])
}

fn open_message(locale: &str) -> Vec<u8> {
    let name = &locale.as_bytes()[..locale.len().min(255)];
    let mut body = vec![name.len() as u8];
    body.extend_from_slice(name);
    message(XIM_OPEN, &body)
}

fn encoding_message(im: u16) -> Vec<u8> {
    let mut names = Vec::new();
    for e in ENCODINGS {
        names.push(e.len() as u8);
        names.extend_from_slice(e.as_bytes());
    }
    let mut body = im.to_le_bytes().to_vec();
    body.extend_from_slice(&(names.len() as u16).to_le_bytes());
    let n = names.len();
    body.extend(names);
    pad_to(&mut body, n);
    body.extend_from_slice(&[0, 0, 0, 0]); // no detailed encoding info
    message(XIM_ENCODING_NEGOTIATION, &body)
}

fn spot_value(spot: (i16, i16)) -> [u8; 4] {
    let (x, y) = (spot.0.to_le_bytes(), spot.1.to_le_bytes());
    [x[0], x[1], y[0], y[1]]
}

fn forward_message(im: u16, ic: u16, flag: u16, event: &[u8; 32]) -> Vec<u8> {
    let mut rest = Vec::with_capacity(36);
    rest.extend_from_slice(&flag.to_le_bytes());
    rest.extend_from_slice(&0u16.to_le_bytes()); // high bits of the serial
    rest.extend_from_slice(event);
    ic_message(XIM_FORWARD_EVENT, im, ic, &rest)
}

// ---- Decoding helpers ----------------------------------------------------------

fn u16_at(b: &[u8], off: usize) -> Option<u16> {
    rd_u16(b, off)
}

fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    rd_u32(b, off)
}

/// The attribute ids the server gave names in `XIM_OPEN_REPLY`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct AttrIds {
    input_style: Option<u16>,
    client_window: Option<u16>,
    focus_window: Option<u16>,
    preedit_attributes: Option<u16>,
    spot_location: Option<u16>,
}

/// Parse `XIM_OPEN_REPLY`'s body: the IM id and the IC attributes we use.
fn parse_open_reply(body: &[u8]) -> Option<(u16, AttrIds)> {
    let im = u16_at(body, 0)?;
    let im_len = u16_at(body, 2)? as usize;
    // Skip the IM attributes, then read the IC attribute list.
    let ic_at = 4 + im_len;
    let ic_len = u16_at(body, ic_at)? as usize;
    let list = body.get(ic_at + 4..ic_at + 4 + ic_len)?;
    let mut ids = AttrIds::default();
    let mut at = 0;
    while at + 6 <= list.len() {
        let id = u16_at(list, at)?;
        let n = u16_at(list, at + 4)? as usize;
        let name = list.get(at + 6..at + 6 + n)?;
        match name {
            b"inputStyle" => ids.input_style = Some(id),
            b"clientWindow" => ids.client_window = Some(id),
            b"focusWindow" => ids.focus_window = Some(id),
            b"preeditAttributes" => ids.preedit_attributes = Some(id),
            b"spotLocation" => ids.spot_location = Some(id),
            _ => {}
        }
        at += 6 + n + (4 - (2 + n) % 4) % 4;
    }
    Some((im, ids))
}

/// A string and its length prefix (CARD16) at `off`.
fn string16(body: &[u8], off: usize) -> Option<&[u8]> {
    let n = u16_at(body, off)? as usize;
    body.get(off + 2..off + 2 + n)
}

// ---- The protocol state machine -------------------------------------------------

/// What the event loop does with the server's answers.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Output {
    /// Deliver an IME event to this window.
    Ime(u32, Ime),
    /// Handle this key event directly (the input method didn't use it, or
    /// XIM switched off while holding it).
    Replay([u8; 32]),
    /// The input method committed a keysym rather than text.
    Keysym(u32, u32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// Waiting for the `_XIM_XCONNECT` answer.
    XConnect,
    Connect,
    Open,
    Encoding,
    Ready,
    Off,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum IcState {
    None,
    Creating,
    Ready(u16),
}

/// An input context: one per window that has had a text field focused.
#[derive(Debug)]
struct Ic {
    window: u32,
    state: IcState,
    /// A text field in the window has focus (from `set_ime`).
    want: bool,
    /// Where the caret is, in window pixels (the bottom of its rectangle).
    spot: (i16, i16),
    /// What the server was last told.
    focused: bool,
    spot_sent: Option<(i16, i16)>,
    /// `XIM_SET_EVENT_MASK` for this context: forward and synchronous masks.
    masks: Option<(u32, u32)>,
    /// The composition, with each character's feedback.
    preedit: Vec<(char, u32)>,
    caret: usize,
}

/// The XIM protocol, without the transport: messages in, messages and
/// [`Output`]s out.
#[derive(Debug)]
pub(super) struct Protocol {
    phase: Phase,
    /// When the current wait gives up.
    deadline: Option<Instant>,
    im: u16,
    ids: AttrIds,
    utf8: bool,
    /// The IM-wide event masks, until a context has its own.
    masks: (u32, u32),
    ics: Vec<Ic>,
    /// A synchronous key the server hasn't answered yet.
    in_flight: Option<[u8; 32]>,
    /// Keys waiting for it, with their window.
    queue: VecDeque<(u32, [u8; 32])>,
    locale: String,
    /// Messages to send, in order.
    out: Vec<Vec<u8>>,
    outputs: Vec<Output>,
}

impl Protocol {
    pub(super) fn new(locale: String, now: Instant) -> Self {
        Self {
            phase: Phase::XConnect,
            deadline: Some(now + SETUP_TIMEOUT),
            im: 0,
            ids: AttrIds::default(),
            utf8: false,
            masks: (KEY_PRESS_MASK | KEY_RELEASE_MASK, 0),
            ics: Vec::new(),
            in_flight: None,
            queue: VecDeque::new(),
            locale,
            out: Vec::new(),
            outputs: Vec::new(),
        }
    }

    pub(super) fn is_off(&self) -> bool {
        self.phase == Phase::Off
    }

    pub(super) fn deadline(&self) -> Option<Instant> {
        self.deadline.filter(|_| !self.is_off())
    }

    /// The messages to send and the outputs to deliver, since the last call.
    pub(super) fn take(&mut self) -> (Vec<Vec<u8>>, Vec<Output>) {
        (
            std::mem::take(&mut self.out),
            std::mem::take(&mut self.outputs),
        )
    }

    /// Give up: hand back held keys, clear any composition, and stop.
    pub(super) fn fail(&mut self, why: &str) {
        if self.is_off() {
            return;
        }
        dbg(format_args!("xim: off ({why})"));
        self.phase = Phase::Off;
        self.deadline = None;
        if let Some(ev) = self.in_flight.take() {
            self.outputs.push(Output::Replay(ev));
        }
        for (_, ev) in self.queue.drain(..) {
            self.outputs.push(Output::Replay(ev));
        }
        for ic in self.ics.drain(..) {
            if !ic.preedit.is_empty() {
                self.outputs.push(Output::Ime(
                    ic.window,
                    Ime::Preedit {
                        text: String::new(),
                        cursor: None,
                    },
                ));
            }
        }
        self.out.clear();
    }

    /// Give up if the current wait has run out.
    pub(super) fn check_timeout(&mut self, now: Instant) {
        if self.deadline().is_some_and(|d| now >= d) {
            self.fail("the server didn't answer in time");
        }
    }

    /// The transport is connected (`_XIM_XCONNECT` answered).
    pub(super) fn connected(&mut self, now: Instant) {
        if self.phase == Phase::XConnect {
            self.phase = Phase::Connect;
            self.deadline = Some(now + SETUP_TIMEOUT);
            self.out.push(connect_message());
        }
    }

    /// Whether we're waiting for the server, and so need a deadline.
    fn waiting(&self) -> bool {
        self.phase != Phase::Ready
            || self.in_flight.is_some()
            || self.ics.iter().any(|ic| ic.state == IcState::Creating)
    }

    fn settle_deadline(&mut self, now: Instant, timeout: Duration) {
        if !self.waiting() {
            self.deadline = None;
        } else if self.deadline.is_none() {
            self.deadline = Some(now + timeout);
        }
    }

    /// A text field in `window` took focus (`Some(caret spot)`), or none has
    /// it any more (`None`).
    pub(super) fn set_ime(&mut self, window: u32, spot: Option<(i16, i16)>, now: Instant) {
        if self.is_off() {
            return;
        }
        let i = match self.ics.iter().position(|ic| ic.window == window) {
            Some(i) => i,
            None if spot.is_some() => {
                self.ics.push(Ic {
                    window,
                    state: IcState::None,
                    want: false,
                    spot: (0, 0),
                    focused: false,
                    spot_sent: None,
                    masks: None,
                    preedit: Vec::new(),
                    caret: 0,
                });
                self.ics.len() - 1
            }
            None => return,
        };
        let ic = &mut self.ics[i];
        ic.want = spot.is_some();
        if let Some(s) = spot {
            ic.spot = s;
        }
        self.sync_ic(i, now);
    }

    /// Bring the server's view of context `i` up to date.
    fn sync_ic(&mut self, i: usize, now: Instant) {
        if self.phase != Phase::Ready {
            return;
        }
        let (im, ids) = (self.im, self.ids);
        let ic = &mut self.ics[i];
        match ic.state {
            IcState::None if ic.want => {
                let mut attrs = Vec::new();
                if let Some(id) = ids.input_style {
                    attrs.extend(attribute(id, &INPUT_STYLE.to_le_bytes()));
                }
                if let Some(id) = ids.client_window {
                    attrs.extend(attribute(id, &ic.window.to_le_bytes()));
                }
                if let Some(id) = ids.focus_window {
                    attrs.extend(attribute(id, &ic.window.to_le_bytes()));
                }
                if let (Some(pa), Some(sl)) = (ids.preedit_attributes, ids.spot_location) {
                    attrs.extend(attribute(pa, &attribute(sl, &spot_value(ic.spot))));
                    ic.spot_sent = Some(ic.spot);
                }
                let mut body = im.to_le_bytes().to_vec();
                body.extend_from_slice(&(attrs.len() as u16).to_le_bytes());
                body.extend(attrs);
                self.out.push(message(XIM_CREATE_IC, &body));
                ic.state = IcState::Creating;
                self.settle_deadline(now, SETUP_TIMEOUT);
            }
            IcState::Ready(id) => {
                if ic.want && !ic.focused {
                    self.out.push(ic_message(XIM_SET_IC_FOCUS, im, id, &[]));
                    ic.focused = true;
                } else if !ic.want && ic.focused {
                    if !ic.preedit.is_empty() {
                        // Drop the composition the field just gave up.
                        self.out.push(ic_message(XIM_RESET_IC, im, id, &[]));
                        ic.preedit.clear();
                        ic.caret = 0;
                    }
                    self.out.push(ic_message(XIM_UNSET_IC_FOCUS, im, id, &[]));
                    ic.focused = false;
                }
                if ic.want
                    && ic.spot_sent != Some(ic.spot)
                    && let (Some(pa), Some(sl)) = (ids.preedit_attributes, ids.spot_location)
                {
                    let attrs = attribute(pa, &attribute(sl, &spot_value(ic.spot)));
                    let mut rest = (attrs.len() as u16).to_le_bytes().to_vec();
                    rest.extend_from_slice(&[0, 0]);
                    rest.extend(attrs);
                    self.out.push(ic_message(XIM_SET_IC_VALUES, im, id, &rest));
                    ic.spot_sent = Some(ic.spot);
                }
            }
            _ => {}
        }
    }

    /// A key event for `window`. Returns whether XIM took it (it went to the
    /// server, or waits to); if not, the caller handles it directly.
    pub(super) fn key(&mut self, window: u32, event: &[u8; 32], now: Instant) -> bool {
        if self.phase != Phase::Ready || event[0] & 0x80 != 0 {
            return false; // not ready, or a key we replayed ourselves
        }
        let Some(ic) = self.ics.iter().find(|ic| ic.window == window) else {
            return false;
        };
        let IcState::Ready(_) = ic.state else {
            return false;
        };
        if !ic.want {
            return false;
        }
        let (forward, _) = ic.masks.unwrap_or(self.masks);
        let bit = if event[0] & 0x7f == super::X_KEY_PRESS {
            KEY_PRESS_MASK
        } else {
            KEY_RELEASE_MASK
        };
        if forward & bit == 0 {
            return false;
        }
        if self.in_flight.is_some() || !self.queue.is_empty() {
            self.queue.push_back((window, *event));
        } else {
            self.forward(window, *event, now);
        }
        true
    }

    /// Send a key to the server.
    fn forward(&mut self, window: u32, event: [u8; 32], now: Instant) {
        let Some(ic) = self.ics.iter().find(|ic| ic.window == window) else {
            self.outputs.push(Output::Replay(event));
            return;
        };
        let IcState::Ready(id) = ic.state else {
            self.outputs.push(Output::Replay(event));
            return;
        };
        let (forward, sync) = ic.masks.unwrap_or(self.masks);
        let bit = if event[0] & 0x7f == super::X_KEY_PRESS {
            KEY_PRESS_MASK
        } else {
            KEY_RELEASE_MASK
        };
        if !ic.want || forward & bit == 0 {
            self.outputs.push(Output::Replay(event));
            return;
        }
        let flag = if sync & bit != 0 { FLAG_SYNC } else { 0 };
        self.out.push(forward_message(self.im, id, flag, &event));
        if flag != 0 {
            self.in_flight = Some(event);
            self.deadline = Some(now + SYNC_TIMEOUT);
        }
    }

    /// The synchronous key was answered: send the ones that waited.
    fn sync_done(&mut self, now: Instant) {
        if self.in_flight.take().is_none() {
            return;
        }
        self.deadline = None;
        while self.in_flight.is_none()
            && let Some((window, ev)) = self.queue.pop_front()
        {
            self.forward(window, ev, now);
        }
        self.settle_deadline(now, SYNC_TIMEOUT);
    }

    fn ic_index(&self, id: u16) -> Option<usize> {
        self.ics
            .iter()
            .position(|ic| ic.state == IcState::Ready(id))
    }

    fn decode_text(&self, bytes: &[u8]) -> String {
        if self.utf8 {
            String::from_utf8_lossy(bytes).into_owned()
        } else {
            super::ctext::decode(bytes)
        }
    }

    /// Tell the window the composition changed.
    fn emit_preedit(&mut self, i: usize) {
        let ic = &self.ics[i];
        let text: String = ic.preedit.iter().map(|&(c, _)| c).collect();
        let byte = |n: usize| ic.preedit[..n].iter().map(|(c, _)| c.len_utf8()).sum();
        // The segment being converted shows as a highlighted range; otherwise
        // the caret.
        let reverse = ic
            .preedit
            .iter()
            .position(|&(_, f)| f & FEEDBACK_REVERSE != 0);
        let cursor = match reverse {
            Some(start) => {
                let end = ic.preedit[start..]
                    .iter()
                    .position(|&(_, f)| f & FEEDBACK_REVERSE == 0)
                    .map_or(ic.preedit.len(), |n| start + n);
                (byte(start), byte(end))
            }
            None => {
                let b = byte(ic.caret.min(ic.preedit.len()));
                (b, b)
            }
        };
        let window = ic.window;
        self.outputs.push(Output::Ime(
            window,
            Ime::Preedit {
                cursor: (!text.is_empty()).then_some(cursor),
                text,
            },
        ));
    }

    /// One message from the server.
    pub(super) fn message(&mut self, msg: &[u8], now: Instant) {
        if self.is_off() || msg.len() < 4 {
            return;
        }
        let major = msg[0];
        let len = 4 + 4 * u16::from_le_bytes([msg[2], msg[3]]) as usize;
        let body = &msg[4..len.min(msg.len())];
        dbg(format_args!("xim: <- {major} ({} bytes)", body.len()));
        match major {
            XIM_CONNECT_REPLY if self.phase == Phase::Connect => {
                self.phase = Phase::Open;
                self.deadline = Some(now + SETUP_TIMEOUT);
                self.out.push(open_message(&self.locale));
            }
            XIM_AUTH_REQUIRED..=XIM_AUTH_NG => self.fail("the server wants authentication"),
            XIM_OPEN_REPLY if self.phase == Phase::Open => match parse_open_reply(body) {
                Some((im, ids)) if ids.input_style.is_some() && ids.client_window.is_some() => {
                    self.im = im;
                    self.ids = ids;
                    self.phase = Phase::Encoding;
                    self.deadline = Some(now + SETUP_TIMEOUT);
                    self.out.push(encoding_message(im));
                }
                _ => self.fail("unusable XIM_OPEN_REPLY"),
            },
            XIM_ENCODING_NEGOTIATION_REPLY if self.phase == Phase::Encoding => {
                // The index into our list; -1 (none) leaves COMPOUND_TEXT, the
                // protocol's default.
                let index = u16_at(body, 4).unwrap_or(u16::MAX) as i16;
                self.utf8 = index >= 0 && ENCODINGS.get(index as usize) == Some(&"UTF-8");
                self.phase = Phase::Ready;
                self.deadline = None;
                dbg(format_args!(
                    "xim: ready (im {}, {})",
                    self.im,
                    if self.utf8 { "UTF-8" } else { "COMPOUND_TEXT" }
                ));
                for i in 0..self.ics.len() {
                    self.sync_ic(i, now);
                }
            }
            XIM_SET_EVENT_MASK => {
                let (ic, fwd, sync) = (
                    u16_at(body, 2).unwrap_or(0),
                    u32_at(body, 4).unwrap_or(0),
                    u32_at(body, 8).unwrap_or(0),
                );
                match self.ic_index(ic).filter(|_| ic != 0) {
                    Some(i) => self.ics[i].masks = Some((fwd, sync)),
                    None => self.masks = (fwd, sync),
                }
            }
            XIM_CREATE_IC_REPLY => {
                let id = u16_at(body, 2).unwrap_or(0);
                if let Some(i) = self.ics.iter().position(|ic| ic.state == IcState::Creating) {
                    self.ics[i].state = IcState::Ready(id);
                    self.settle_deadline(now, SETUP_TIMEOUT);
                    self.sync_ic(i, now);
                }
            }
            XIM_FORWARD_EVENT => {
                let (ic, flag) = (u16_at(body, 2).unwrap_or(0), u16_at(body, 4).unwrap_or(0));
                if let Some(ev) = body.get(8..40) {
                    let mut ev: [u8; 32] = ev.try_into().unwrap_or([0; 32]);
                    // Mark it as ours, so it isn't sent back to the server.
                    ev[0] |= 0x80;
                    self.outputs.push(Output::Replay(ev));
                }
                if flag & FLAG_SYNC != 0 {
                    self.out.push(ic_message(XIM_SYNC_REPLY, self.im, ic, &[]));
                }
                self.sync_done(now);
            }
            XIM_COMMIT => {
                let (ic, flag) = (u16_at(body, 2).unwrap_or(0), u16_at(body, 4).unwrap_or(0));
                let window = self.ic_index(ic).map(|i| self.ics[i].window).unwrap_or(0);
                if flag & COMMIT_KEYSYM != 0 {
                    let ks = u32_at(body, 8).unwrap_or(0);
                    if flag & COMMIT_CHARS != 0 {
                        if let Some(s) = string16(body, 12) {
                            let text = self.decode_text(s);
                            self.outputs.push(Output::Ime(window, Ime::Commit(text)));
                        }
                    } else {
                        self.outputs.push(Output::Keysym(window, ks));
                    }
                } else if flag & COMMIT_CHARS != 0
                    && let Some(s) = string16(body, 6)
                {
                    let text = self.decode_text(s);
                    dbg(format_args!("xim: commit {text:?}"));
                    self.outputs.push(Output::Ime(window, Ime::Commit(text)));
                }
                if flag & FLAG_SYNC != 0 {
                    self.out.push(ic_message(XIM_SYNC_REPLY, self.im, ic, &[]));
                }
                self.sync_done(now);
            }
            XIM_SYNC => {
                let ic = u16_at(body, 2).unwrap_or(0);
                self.out.push(ic_message(XIM_SYNC_REPLY, self.im, ic, &[]));
            }
            XIM_SYNC_REPLY => self.sync_done(now),
            XIM_PREEDIT_START => {
                let ic = u16_at(body, 2).unwrap_or(0);
                // No length limit.
                self.out.push(ic_message(
                    XIM_PREEDIT_START_REPLY,
                    self.im,
                    ic,
                    &(-1i32).to_le_bytes(),
                ));
                if let Some(i) = self.ic_index(ic) {
                    self.ics[i].preedit.clear();
                    self.ics[i].caret = 0;
                }
            }
            XIM_PREEDIT_DRAW => self.preedit_draw(body),
            XIM_PREEDIT_CARET => {
                let ic = u16_at(body, 2).unwrap_or(0);
                let position = u32_at(body, 4).unwrap_or(0) as usize;
                let direction = u32_at(body, 8).unwrap_or(11);
                let mut reply = 0u32;
                if let Some(i) = self.ic_index(ic) {
                    let p = &mut self.ics[i];
                    let len = p.preedit.len();
                    p.caret = match direction {
                        0 => (p.caret + 1).min(len),    // XIMForwardChar
                        1 => p.caret.saturating_sub(1), // XIMBackwardChar
                        8 => 0,                         // XIMLineStart
                        9 => len,                       // XIMLineEnd
                        10 => position.min(len),        // XIMAbsolutePosition
                        _ => p.caret,
                    };
                    reply = p.caret as u32;
                    self.emit_preedit(i);
                }
                self.out.push(ic_message(
                    XIM_PREEDIT_CARET_REPLY,
                    self.im,
                    ic,
                    &reply.to_le_bytes(),
                ));
            }
            XIM_PREEDIT_DONE => {
                let ic = u16_at(body, 2).unwrap_or(0);
                if let Some(i) = self.ic_index(ic)
                    && !self.ics[i].preedit.is_empty()
                {
                    self.ics[i].preedit.clear();
                    self.ics[i].caret = 0;
                    self.emit_preedit(i);
                }
            }
            XIM_ERROR => {
                let code = u16_at(body, 6).unwrap_or(0);
                dbg(format_args!("xim: error {code}"));
                if self.phase != Phase::Ready
                    || self.ics.iter().any(|ic| ic.state == IcState::Creating)
                {
                    self.fail("the server refused the setup");
                } else {
                    self.sync_done(now);
                }
            }
            // Trigger keys (we forward every key anyway), status callbacks
            // (we asked for none), geometry, replies we don't wait for.
            _ => {}
        }
    }

    fn preedit_draw(&mut self, body: &[u8]) {
        let ic = u16_at(body, 2).unwrap_or(0);
        let Some(i) = self.ic_index(ic) else {
            return;
        };
        let caret = u32_at(body, 4).unwrap_or(0) as i32;
        let first = u32_at(body, 8).unwrap_or(0) as i32;
        let length = u32_at(body, 12).unwrap_or(0) as i32;
        let status = u32_at(body, 16).unwrap_or(0);
        let (text, feedback): (Vec<char>, Vec<u32>) = if status & DRAW_NO_STRING != 0 {
            (Vec::new(), Vec::new())
        } else {
            let raw = string16(body, 20).unwrap_or(&[]);
            let text: Vec<char> = self.decode_text(raw).chars().collect();
            let fb_at = 22 + raw.len() + (4 - (2 + raw.len()) % 4) % 4;
            let feedback = if status & DRAW_NO_FEEDBACK != 0 {
                Vec::new()
            } else {
                let n = u16_at(body, fb_at).unwrap_or(0) as usize / 4;
                (0..n)
                    .map_while(|k| u32_at(body, fb_at + 4 + 4 * k))
                    .collect()
            };
            (text, feedback)
        };
        let p = &mut self.ics[i];
        let len = p.preedit.len();
        let first = (first.max(0) as usize).min(len);
        let end = (first + length.max(0) as usize).min(len);
        let new = text
            .into_iter()
            .enumerate()
            .map(|(k, c)| (c, feedback.get(k).copied().unwrap_or(0)));
        p.preedit.splice(first..end, new);
        p.caret = (caret.max(0) as usize).min(p.preedit.len());
        self.emit_preedit(i);
    }
}

// ---- Transport ---------------------------------------------------------------------

/// The atoms the transport uses.
#[derive(Clone, Copy, Debug, Default)]
struct Atoms {
    protocol: u32,
    moredata: u32,
    xconnect: u32,
    data: [u32; DATA_ATOMS],
}

/// A `SendEvent` of a `ClientMessage` to `window` (delivered to whoever
/// created it), before [`finish`].
fn client_message(window: u32, ty: u32, format: u8, data: &[u8; 20]) -> Vec<u8> {
    let mut req = vec![OP_SEND_EVENT, 0, 0, 0];
    req.extend_from_slice(&window.to_le_bytes());
    req.extend_from_slice(&0u32.to_le_bytes()); // event mask
    req.extend_from_slice(&[X_CLIENT_MESSAGE, format, 0, 0]);
    req.extend_from_slice(&window.to_le_bytes());
    req.extend_from_slice(&ty.to_le_bytes());
    req.extend_from_slice(data);
    req
}

fn words(values: [u32; 5]) -> [u8; 20] {
    let mut data = [0u8; 20];
    for (i, v) in values.iter().enumerate() {
        data[4 * i..4 * i + 4].copy_from_slice(&v.to_le_bytes());
    }
    data
}

/// The X requests that carry one XIM message to `server` (its
/// communication window): a format-8 client message when it fits, else a
/// property (appended to, as the server may not have read the last one) and
/// a format-32 client message naming it.
fn transport_requests(server: u32, atoms: &Atoms, seq: usize, msg: &[u8]) -> Vec<Vec<u8>> {
    if msg.len() <= CM_DATA {
        let mut data = [0u8; 20];
        data[..msg.len()].copy_from_slice(msg);
        return vec![finish(client_message(server, atoms.protocol, 8, &data))];
    }
    let atom = atoms.data[seq % DATA_ATOMS];
    let mut prop = vec![OP_CHANGE_PROPERTY, 2 /* Append */, 0, 0];
    prop.extend_from_slice(&server.to_le_bytes());
    prop.extend_from_slice(&atom.to_le_bytes());
    prop.extend_from_slice(&ATOM_STRING.to_le_bytes());
    prop.extend_from_slice(&[8, 0, 0, 0]);
    prop.extend_from_slice(&(msg.len() as u32).to_le_bytes());
    prop.extend_from_slice(msg);
    pad4(&mut prop);
    vec![
        finish(prop),
        finish(client_message(
            server,
            atoms.protocol,
            32,
            &words([msg.len() as u32, atom, 0, 0, 0]),
        )),
    ]
}

/// Split a buffer of server messages (a property can hold several) into
/// messages. Zero padding after the last one ends it.
fn split_messages(mut buf: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    while buf.len() >= 4 && buf[0] != 0 {
        let len = 4 + 4 * u16::from_le_bytes([buf[2], buf[3]]) as usize;
        let take = len.min(buf.len());
        out.push(&buf[..take]);
        buf = &buf[take..];
    }
    out
}

/// The live XIM connection: the protocol plus its X transport.
#[derive(Debug)]
pub(super) struct Xim {
    proto: Protocol,
    atoms: Atoms,
    /// Our communication window.
    comm: u32,
    /// The window that owns the server's `@server=` selection.
    server: u32,
    /// The server's communication window, once connected.
    server_comm: u32,
    /// Format-8 pieces of a message (`_XIM_MOREDATA`) so far.
    pending: Vec<u8>,
    /// How far into each property we've read (servers append to them).
    offsets: Vec<(u32, usize)>,
    seq: usize,
    /// Outputs produced while taking a key, delivered with the next update.
    stash: Vec<Output>,
    /// Whether `_XIM_XCONNECT` went out. It waits for the event loop: an
    /// answer arriving while startup code reads replies directly would be
    /// lost.
    started: bool,
}

/// `@im=NAME` from `$XMODIFIERS`, if set.
fn im_name(modifiers: &str) -> Option<&str> {
    let at = modifiers.find("@im=")?;
    let rest = &modifiers[at + 4..];
    let end = rest
        .find(|c: char| c == '@' || c.is_whitespace())
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

/// The locale name for `XIM_OPEN`, such as `en_US` (without the codeset).
fn locale_name() -> String {
    let raw = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .find(|v| !v.is_empty())
        .unwrap_or_default();
    let base = raw.split(['.', '@']).next().unwrap_or("");
    if base.is_empty() || base == "POSIX" {
        "C".into()
    } else {
        base.into()
    }
}

/// Send `req` and wait for its reply, putting events that arrive first in
/// `backlog`. `None` if the server answered with an error.
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

/// `GetProperty` of `len` 4-byte units from `offset` (in units). Returns the
/// bytes and how many remain after them.
fn get_property(
    conn: &mut Conn,
    window: u32,
    prop: u32,
    offset: u32,
    len: u32,
    delete: bool,
    backlog: &mut VecDeque<[u8; 32]>,
) -> io::Result<Option<(Vec<u8>, u32)>> {
    let mut req = vec![OP_GET_PROPERTY, delete as u8, 0, 0];
    req.extend_from_slice(&window.to_le_bytes());
    req.extend_from_slice(&prop.to_le_bytes());
    req.extend_from_slice(&0u32.to_le_bytes()); // AnyPropertyType
    req.extend_from_slice(&offset.to_le_bytes());
    req.extend_from_slice(&len.to_le_bytes());
    Ok(round_trip(conn, req, backlog)?.map(|(reply, mut data)| {
        let format = reply[1] as usize;
        let after = rd_u32(&reply, 12).unwrap_or(0);
        let items = rd_u32(&reply, 16).unwrap_or(0) as usize;
        data.truncate(items * format / 8);
        (data, after)
    }))
}

impl Xim {
    /// Find the XIM server and start connecting. Call before any window
    /// exists (the replies are read directly). `None` when there is no
    /// server, or XIM is switched off.
    pub(super) fn connect(conn: &mut Conn) -> Option<Self> {
        if std::env::var_os("STIPPLE_XIM").is_some_and(|v| v == "0") {
            return None;
        }
        let modifiers = std::env::var("XMODIFIERS").unwrap_or_default();
        let name = im_name(&modifiers).unwrap_or("");
        if name == "none" {
            return None;
        }
        match Self::try_connect(conn, name) {
            Ok(xim) => xim,
            Err(e) => {
                dbg(format_args!("xim: setup failed: {e}"));
                None
            }
        }
    }

    fn try_connect(conn: &mut Conn, name: &str) -> io::Result<Option<Self>> {
        let mut scratch = VecDeque::new();
        let root = conn.setup.root;
        let servers_atom = intern_atom(conn, b"XIM_SERVERS")?;
        let servers = match get_property(conn, root, servers_atom, 0, 1024, false, &mut scratch)? {
            Some((data, _)) => data
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32::from_le_bytes(*c))
                .collect(),
            None => Vec::new(),
        };
        let server_atom = if name.is_empty() {
            servers.first().copied()
        } else {
            let a = intern_atom(conn, format!("@server={name}").as_bytes())?;
            servers.contains(&a).then_some(a)
        };
        let Some(server_atom) = server_atom else {
            dbg(format_args!("xim: no server for {name:?} in XIM_SERVERS"));
            return Ok(None);
        };
        // GetSelectionOwner.
        let mut req = vec![23, 0, 0, 0];
        req.extend_from_slice(&server_atom.to_le_bytes());
        let owner = round_trip(conn, req, &mut scratch)?
            .and_then(|(reply, _)| rd_u32(&reply, 8))
            .unwrap_or(0);
        if owner == 0 {
            dbg(format_args!("xim: the server's selection has no owner"));
            return Ok(None);
        }
        let mut atoms = Atoms {
            protocol: intern_atom(conn, b"_XIM_PROTOCOL")?,
            moredata: intern_atom(conn, b"_XIM_MOREDATA")?,
            xconnect: intern_atom(conn, b"_XIM_XCONNECT")?,
            data: [0; DATA_ATOMS],
        };
        for (i, a) in atoms.data.iter_mut().enumerate() {
            *a = intern_atom(conn, format!("_STIPPLE_XIM_{i}").as_bytes())?;
        }

        // Our communication window: unmapped and input-only.
        let comm = conn.new_id();
        let mut req = vec![OP_CREATE_WINDOW, 0, 0, 0];
        req.extend_from_slice(&comm.to_le_bytes());
        req.extend_from_slice(&root.to_le_bytes());
        req.extend_from_slice(&[0, 0, 0, 0, 1, 0, 1, 0]); // x, y, width, height
        req.extend_from_slice(&[0, 0, 2, 0]); // border width, class InputOnly
        req.extend_from_slice(&0u32.to_le_bytes()); // visual: CopyFromParent
        req.extend_from_slice(&0u32.to_le_bytes()); // no attributes
        conn.send(&finish(req))?;

        let xim = Self {
            proto: Protocol::new(locale_name(), Instant::now()),
            atoms,
            comm,
            server: owner,
            server_comm: 0,
            pending: Vec::new(),
            offsets: Vec::new(),
            seq: 0,
            stash: Vec::new(),
            started: false,
        };
        // Hear about the server going away.
        xim.watch(conn, owner)?;
        dbg(format_args!(
            "xim: found {name:?} (window {owner:#x}), comm {comm:#x}"
        ));
        Ok(Some(xim))
    }

    /// Send `_XIM_XCONNECT`: our window and transport version 0.0.
    fn start(&mut self, conn: &mut Conn, now: Instant) {
        self.started = true;
        self.proto.deadline = Some(now + SETUP_TIMEOUT);
        let req = client_message(
            self.server,
            self.atoms.xconnect,
            32,
            &words([self.comm, 0, 0, 0, 0]),
        );
        if conn.send(&finish(req)).is_err() {
            self.proto.fail("write failed");
        }
        dbg(format_args!("xim: connecting"));
    }

    /// Select `StructureNotify` on another client's window, for its
    /// `DestroyNotify`.
    fn watch(&self, conn: &mut Conn, window: u32) -> io::Result<()> {
        let mut req = vec![OP_CHANGE_WINDOW_ATTRIBUTES, 0, 0, 0];
        req.extend_from_slice(&window.to_le_bytes());
        req.extend_from_slice(&CW_EVENT_MASK.to_le_bytes());
        req.extend_from_slice(&EV_STRUCTURE_NOTIFY.to_le_bytes());
        conn.send(&finish(req))
    }

    /// When the loop must wake up for a timeout.
    pub(super) fn deadline(&self) -> Option<Instant> {
        self.proto.deadline()
    }

    /// Whether `event` is for XIM: a client message to our window, or the
    /// server's window being destroyed.
    pub(super) fn claims(&self, event: &[u8; 32]) -> bool {
        match event[0] & 0x7f {
            X_CLIENT_MESSAGE => rd_u32(event, 4) == Some(self.comm),
            17 /* DestroyNotify */ => {
                let w = rd_u32(event, 8).unwrap_or(0);
                w != 0 && (w == self.server || w == self.server_comm)
            }
            _ => false,
        }
    }

    /// Handle an event [`claims`](Self::claims) accepted.
    pub(super) fn event(
        &mut self,
        shared: &SharedConn,
        backlog: &mut VecDeque<[u8; 32]>,
        event: &[u8; 32],
    ) -> Vec<Output> {
        let now = Instant::now();
        let mut conn = shared.lock().unwrap();
        if event[0] & 0x7f == 17 {
            self.proto.fail("the server went away");
            return self.flush(&mut conn);
        }
        let ty = rd_u32(event, 8).unwrap_or(0);
        let format = event[1];
        let data: [u8; 20] = event[12..32].try_into().unwrap_or([0; 20]);
        if ty == self.atoms.xconnect {
            if self.server_comm == 0 {
                self.server_comm = rd_u32(&data, 0).unwrap_or(0);
                dbg(format_args!(
                    "xim: transport {}.{}, server comm {:#x}",
                    rd_u32(&data, 4).unwrap_or(0),
                    rd_u32(&data, 8).unwrap_or(0),
                    self.server_comm
                ));
                if self.server_comm == 0 {
                    self.proto.fail("refused connection");
                } else {
                    let _ = self.watch(&mut conn, self.server_comm);
                    self.proto.connected(now);
                }
            }
        } else if ty == self.atoms.protocol || ty == self.atoms.moredata {
            if format == 8 {
                self.pending.extend_from_slice(&data);
                if ty == self.atoms.protocol {
                    let buf = std::mem::take(&mut self.pending);
                    for msg in split_messages(&buf) {
                        self.proto.message(msg, now);
                    }
                }
            } else if format == 32 {
                let len = rd_u32(&data, 0).unwrap_or(0) as usize;
                let atom = rd_u32(&data, 4).unwrap_or(0);
                match self.read_property(&mut conn, atom, len, backlog) {
                    Ok(Some(buf)) => {
                        for msg in split_messages(&buf) {
                            self.proto.message(msg, now);
                        }
                    }
                    Ok(None) => dbg(format_args!("xim: property {atom} missing")),
                    Err(e) => dbg(format_args!("xim: reading property: {e}")),
                }
            }
        }
        self.flush(&mut conn)
    }

    /// Read `len` bytes of a message from property `atom` on our window,
    /// following on from what was read of it before.
    fn read_property(
        &mut self,
        conn: &mut Conn,
        atom: u32,
        len: usize,
        backlog: &mut VecDeque<[u8; 32]>,
    ) -> io::Result<Option<Vec<u8>>> {
        let slot = match self.offsets.iter().position(|&(a, _)| a == atom) {
            Some(i) => i,
            None => {
                self.offsets.push((atom, 0));
                self.offsets.len() - 1
            }
        };
        let offset = self.offsets[slot].1;
        let (begin, end) = (offset / 4, (offset + len).div_ceil(4));
        // Deleted once read to the end.
        let got = get_property(
            conn,
            self.comm,
            atom,
            begin as u32,
            (end - begin) as u32,
            true,
            backlog,
        )?;
        let Some((data, after)) = got else {
            self.offsets[slot].1 = 0;
            return Ok(None);
        };
        self.offsets[slot].1 = if after > 0 { offset + len } else { 0 };
        let skip = offset % 4;
        Ok(data
            .get(skip..(skip + len).min(data.len()))
            .map(<[u8]>::to_vec))
    }

    /// Send what the protocol queued, and return its outputs.
    fn flush(&mut self, conn: &mut Conn) -> Vec<Output> {
        let (msgs, outputs) = self.proto.take();
        if self.server_comm != 0 {
            for msg in msgs {
                dbg(format_args!("xim: -> {} ({} bytes)", msg[0], msg.len()));
                for req in transport_requests(self.server_comm, &self.atoms, self.seq, &msg) {
                    if conn.send(&req).is_err() {
                        self.proto.fail("write failed");
                    }
                }
                if msg.len() > CM_DATA {
                    self.seq += 1;
                }
            }
        }
        outputs
    }

    /// Apply the `set_ime` calls since the last time, and any timeout.
    pub(super) fn update(
        &mut self,
        shared: &SharedConn,
        requests: impl Iterator<Item = (u32, Option<(i16, i16)>)>,
    ) -> Vec<Output> {
        let now = Instant::now();
        for (window, spot) in requests {
            self.proto.set_ime(window, spot, now);
        }
        let mut conn = shared.lock().unwrap();
        if !self.started {
            self.start(&mut conn, now);
        }
        self.proto.check_timeout(now);
        let mut outputs = std::mem::take(&mut self.stash);
        outputs.extend(self.flush(&mut conn));
        outputs
    }

    /// Offer a key event to the input method. `true` if it took it.
    pub(super) fn key(&mut self, shared: &SharedConn, window: u32, event: &[u8; 32]) -> bool {
        if !self.proto.key(window, event, Instant::now()) {
            return false;
        }
        let mut conn = shared.lock().unwrap();
        // Only a failed write gives outputs here (the keys handed back).
        let outputs = self.flush(&mut conn);
        self.stash.extend(outputs);
        true
    }
}

/// Deliver XIM outputs: IME events to their window's handler, keys back
/// into the event stream (via `backlog`).
pub(super) fn deliver(
    outputs: Vec<Output>,
    windows: &[X11Window],
    backlog: &mut VecDeque<[u8; 32]>,
    handler: &mut dyn FnMut(Event, &dyn Window) -> ControlFlow,
) -> ControlFlow {
    let mut flow = ControlFlow::Wait;
    for out in outputs {
        let (window, event) = match out {
            Output::Replay(mut ev) => {
                ev[0] |= 0x80;
                backlog.push_back(ev);
                continue;
            }
            Output::Ime(w, ime) => (w, Event::Ime(ime)),
            Output::Keysym(w, ks) => {
                let mods = crate::platform::event::Modifiers::default();
                match super::keysym_to_event(ks, crate::platform::event::ButtonState::Pressed, mods)
                {
                    Some(ev) => (w, ev),
                    None => continue,
                }
            }
        };
        let Some(win) = windows
            .iter()
            .find(|w| w.window == window)
            .or(windows.first())
        else {
            continue;
        };
        if handler(event, win) == ControlFlow::Exit {
            flow = ControlFlow::Exit;
        }
    }
    flow
}

#[cfg(test)]
mod tests {
    use super::*;

    fn le16(v: u16) -> [u8; 2] {
        v.to_le_bytes()
    }

    /// A server message from its opcode and body.
    fn server(major: u8, body: &[u8]) -> Vec<u8> {
        message(major, body)
    }

    /// `XIM_OPEN_REPLY` naming the IC attributes we look for.
    fn open_reply(im: u16) -> Vec<u8> {
        let mut ic_attrs = Vec::new();
        for (id, name) in [
            (1u16, &b"inputStyle"[..]),
            (2, b"clientWindow"),
            (3, b"focusWindow"),
            (4, b"preeditAttributes"),
            (5, b"spotLocation"),
            (6, b"filterEvents"),
        ] {
            ic_attrs.extend_from_slice(&le16(id));
            ic_attrs.extend_from_slice(&le16(3)); // type: CARD32 (unused)
            ic_attrs.extend_from_slice(&le16(name.len() as u16));
            ic_attrs.extend_from_slice(name);
            let n = name.len();
            pad_to(&mut ic_attrs, 2 + n);
        }
        // One IM attribute: queryInputStyle.
        let mut im_attrs = Vec::new();
        im_attrs.extend_from_slice(&le16(0));
        im_attrs.extend_from_slice(&le16(10));
        im_attrs.extend_from_slice(&le16(15));
        im_attrs.extend_from_slice(b"queryInputStyle");
        pad_to(&mut im_attrs, 2 + 15);
        let mut body = le16(im).to_vec();
        body.extend_from_slice(&le16(im_attrs.len() as u16));
        body.extend(im_attrs);
        body.extend_from_slice(&le16(ic_attrs.len() as u16));
        body.extend_from_slice(&[0, 0]);
        body.extend(ic_attrs);
        server(XIM_OPEN_REPLY, &body)
    }

    fn key_event(window: u32, press: bool, keycode: u8) -> [u8; 32] {
        let mut ev = [0u8; 32];
        ev[0] = if press { 2 } else { 3 };
        ev[1] = keycode;
        ev[12..16].copy_from_slice(&window.to_le_bytes());
        ev
    }

    const WIN: u32 = 0x40_0001;

    /// A protocol run through the handshake and an input context for `WIN`
    /// (id 7), with what it sent along the way.
    fn ready(encoding_index: i16) -> (Protocol, Instant, Vec<Vec<u8>>) {
        let now = Instant::now();
        let mut p = Protocol::new("ja_JP".into(), now);
        let mut sent = Vec::new();
        p.set_ime(WIN, Some((10, 20)), now);
        p.connected(now);
        p.message(&server(XIM_CONNECT_REPLY, &[1, 0, 0, 0]), now);
        p.message(&open_reply(3), now);
        let mut body = le16(3).to_vec();
        body.extend_from_slice(&le16(0));
        body.extend_from_slice(&encoding_index.to_le_bytes());
        body.extend_from_slice(&[0, 0]);
        p.message(&server(XIM_ENCODING_NEGOTIATION_REPLY, &body), now);
        sent.extend(p.take().0);
        p.message(&server(XIM_CREATE_IC_REPLY, &[3, 0, 7, 0]), now);
        // IMdkit's static event flow: forward presses and releases, async.
        let mut mask = vec![3, 0, 7, 0];
        mask.extend_from_slice(&3u32.to_le_bytes());
        mask.extend_from_slice(&(!3u32).to_le_bytes());
        p.message(&server(XIM_SET_EVENT_MASK, &mask), now);
        sent.extend(p.take().0);
        (p, now, sent)
    }

    #[test]
    fn message_header_counts_words_after_it() {
        let m = message(XIM_SYNC_REPLY, &[3, 0, 7, 0]);
        assert_eq!(m, [62, 0, 1, 0, 3, 0, 7, 0]);
        // Bodies are padded to 4 bytes.
        assert_eq!(
            message(XIM_OPEN, &[2, b'j', b'a']),
            [30, 0, 1, 0, 2, b'j', b'a', 0]
        );
    }

    #[test]
    fn encodes_setup_messages() {
        assert_eq!(connect_message(), [1, 0, 2, 0, b'l', 0, 1, 0, 0, 0, 0, 0]);
        let open = open_message("en_US");
        assert_eq!(open, [30, 0, 2, 0, 5, b'e', b'n', b'_', b'U', b'S', 0, 0]);
        let enc = encoding_message(3);
        // im, byte length 20 ("UTF-8" and "COMPOUND_TEXT" with their length
        // bytes), the names, padding to 4, then an empty info list.
        let mut want = vec![38, 0, 7, 0, 3, 0, 20, 0, 5];
        want.extend_from_slice(b"UTF-8");
        want.push(13);
        want.extend_from_slice(b"COMPOUND_TEXT");
        want.extend_from_slice(&[0, 0, 0, 0]);
        assert_eq!(enc, want);
    }

    #[test]
    fn parses_open_reply_attribute_ids() {
        let msg = open_reply(3);
        let (im, ids) = parse_open_reply(&msg[4..]).unwrap();
        assert_eq!(im, 3);
        assert_eq!(
            ids,
            AttrIds {
                input_style: Some(1),
                client_window: Some(2),
                focus_window: Some(3),
                preedit_attributes: Some(4),
                spot_location: Some(5),
            }
        );
        assert!(parse_open_reply(&[3, 0, 200, 0]).is_none());
    }

    #[test]
    fn handshake_then_create_ic_with_on_the_spot_style() {
        let (p, _, sent) = ready(1);
        let ops: Vec<u8> = sent.iter().map(|m| m[0]).collect();
        assert_eq!(
            ops,
            [
                XIM_CONNECT,
                XIM_OPEN,
                XIM_ENCODING_NEGOTIATION,
                XIM_CREATE_IC,
                XIM_SET_IC_FOCUS
            ]
        );
        assert!(!p.utf8, "index 1 is COMPOUND_TEXT");
        // XIM_CREATE_IC: im 3, then inputStyle, clientWindow, focusWindow and
        // preeditAttributes { spotLocation (10, 20) }.
        let create = &sent[3];
        let mut want = vec![50, 0, 0, 0, 3, 0, 36, 0];
        want.extend_from_slice(&[1, 0, 4, 0, 0x02, 0x04, 0, 0]);
        want.extend_from_slice(&[2, 0, 4, 0]);
        want.extend_from_slice(&WIN.to_le_bytes());
        want.extend_from_slice(&[3, 0, 4, 0]);
        want.extend_from_slice(&WIN.to_le_bytes());
        want.extend_from_slice(&[4, 0, 8, 0, 5, 0, 4, 0, 10, 0, 20, 0]);
        let words = ((want.len() - 4) / 4) as u16;
        want[2..4].copy_from_slice(&words.to_le_bytes());
        assert_eq!(create, &want);
        assert_eq!(sent[4], [58, 0, 1, 0, 3, 0, 7, 0]);
        assert_eq!(p.deadline(), None, "nothing is pending");
    }

    #[test]
    fn utf8_is_used_when_the_server_picks_it() {
        let (mut p, now, _) = ready(0);
        assert!(p.utf8);
        let mut body = vec![3, 0, 7, 0, COMMIT_CHARS as u8, 0];
        let s = "日本".as_bytes();
        body.extend_from_slice(&le16(s.len() as u16));
        body.extend_from_slice(s);
        p.message(&server(XIM_COMMIT, &body), now);
        assert_eq!(p.take().1, [Output::Ime(WIN, Ime::Commit("日本".into()))]);
    }

    #[test]
    fn keys_go_to_the_server_and_unused_ones_come_back() {
        let (mut p, now, _) = ready(1);
        let ev = key_event(WIN, true, 38);
        assert!(p.key(WIN, &ev, now));
        let (msgs, outs) = p.take();
        assert!(outs.is_empty());
        let mut want = vec![60, 0, 10, 0, 3, 0, 7, 0, 0, 0, 0, 0];
        want.extend_from_slice(&ev);
        assert_eq!(msgs, [want.clone()]);
        // Asynchronous: the next key goes straight away too.
        assert!(p.key(WIN, &key_event(WIN, false, 38), now));
        assert_eq!(p.take().0.len(), 1);
        // The server forwards the key back: it's replayed, marked as ours.
        p.message(&server(XIM_FORWARD_EVENT, &want[4..]), now);
        let mut back = ev;
        back[0] |= 0x80;
        assert_eq!(p.take().1, [Output::Replay(back)]);
        // ...and isn't offered to the server again.
        assert!(!p.key(WIN, &back, now));
        // Other windows, and windows without a focused field, are direct.
        assert!(!p.key(WIN + 1, &key_event(WIN + 1, true, 38), now));
        p.set_ime(WIN, None, now);
        assert_eq!(p.take().0, [ic_message(XIM_UNSET_IC_FOCUS, 3, 7, &[])]);
        assert!(!p.key(WIN, &ev, now));
    }

    #[test]
    fn synchronous_keys_wait_for_the_reply_and_time_out() {
        let (mut p, now, _) = ready(1);
        let mut mask = vec![3, 0, 7, 0];
        mask.extend_from_slice(&3u32.to_le_bytes());
        mask.extend_from_slice(&3u32.to_le_bytes());
        p.message(&server(XIM_SET_EVENT_MASK, &mask), now);
        let (a, b) = (key_event(WIN, true, 38), key_event(WIN, true, 39));
        assert!(p.key(WIN, &a, now));
        assert!(p.key(WIN, &b, now));
        let msgs = p.take().0;
        assert_eq!(msgs.len(), 1, "the second key waits");
        assert_eq!(&msgs[0][8..10], &[1, 0], "synchronous flag");
        assert!(p.deadline().is_some());
        p.message(&server(XIM_SYNC_REPLY, &[3, 0, 7, 0]), now);
        assert_eq!(p.take().0.len(), 1, "then it goes");
        // No answer for the second: XIM switches off and hands it back.
        p.check_timeout(now + SYNC_TIMEOUT + Duration::from_millis(1));
        assert!(p.is_off());
        assert_eq!(p.take().1, [Output::Replay(b)]);
        assert!(!p.key(WIN, &a, now), "direct from now on");
    }

    #[test]
    fn setup_times_out() {
        let now = Instant::now();
        let mut p = Protocol::new("C".into(), now);
        p.check_timeout(now + Duration::from_secs(1));
        assert!(!p.is_off());
        p.check_timeout(now + SETUP_TIMEOUT);
        assert!(p.is_off());
    }

    #[test]
    fn setup_error_switches_off() {
        let now = Instant::now();
        let mut p = Protocol::new("C".into(), now);
        p.connected(now);
        p.message(
            &server(XIM_ERROR, &[0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0]),
            now,
        );
        assert!(p.is_off());
    }

    fn draw(caret: u32, first: u32, len: u32, text: &[u8], feedback: &[u32]) -> Vec<u8> {
        let mut body = vec![3, 0, 7, 0];
        for v in [caret, first, len, if text.is_empty() { 1 } else { 0 }] {
            body.extend_from_slice(&v.to_le_bytes());
        }
        body.extend_from_slice(&le16(text.len() as u16));
        body.extend_from_slice(text);
        pad_to(&mut body, 2 + text.len());
        body.extend_from_slice(&le16(4 * feedback.len() as u16));
        body.extend_from_slice(&[0, 0]);
        for f in feedback {
            body.extend_from_slice(&f.to_le_bytes());
        }
        server(XIM_PREEDIT_DRAW, &body)
    }

    #[test]
    fn preedit_callbacks_track_the_composition() {
        let (mut p, now, _) = ready(1);
        p.message(&server(XIM_PREEDIT_START, &[3, 0, 7, 0]), now);
        let mut reply = vec![74, 0, 2, 0, 3, 0, 7, 0];
        reply.extend_from_slice(&(-1i32).to_le_bytes());
        assert_eq!(p.take().0, [reply]);
        // "にほ" in JIS X 0208 (0x244B 0x245B), underlined, caret at the end.
        p.message(&draw(2, 0, 0, b"\x1b$(B$K$[", &[2, 2]), now);
        let pre = |text: &str, c: (usize, usize)| {
            Output::Ime(
                WIN,
                Ime::Preedit {
                    text: text.into(),
                    cursor: Some(c),
                },
            )
        };
        assert_eq!(p.take().1, [pre("にほ", (6, 6))]);
        // Replace both with "日本", the converted segment reversed.
        p.message(&draw(2, 0, 2, "\x1b%G日本\x1b%@".as_bytes(), &[1, 1]), now);
        assert_eq!(p.take().1, [pre("日本", (0, 6))]);
        // Append, then move the caret.
        p.message(&draw(3, 2, 0, b"a", &[2]), now);
        assert_eq!(p.take().1, [pre("日本a", (0, 6))]);
        let mut caret = vec![3, 0, 7, 0];
        caret.extend_from_slice(&1u32.to_le_bytes());
        caret.extend_from_slice(&10u32.to_le_bytes()); // absolute
        caret.extend_from_slice(&0u32.to_le_bytes());
        p.message(&server(XIM_PREEDIT_CARET, &caret), now);
        let (msgs, _) = p.take();
        assert_eq!(
            msgs,
            [ic_message(
                XIM_PREEDIT_CARET_REPLY,
                3,
                7,
                &1u32.to_le_bytes()
            )]
        );
        // Delete (no string), then done.
        p.message(&draw(0, 0, 3, b"", &[]), now);
        assert_eq!(
            p.take().1,
            [Output::Ime(
                WIN,
                Ime::Preedit {
                    text: String::new(),
                    cursor: None
                }
            )]
        );
        p.message(&server(XIM_PREEDIT_DONE, &[3, 0, 7, 0]), now);
        assert!(p.take().1.is_empty(), "already empty");
    }

    #[test]
    fn commit_decodes_text_and_answers_sync() {
        let (mut p, now, _) = ready(1);
        // IMdkit always sets the synchronous flag on commits.
        let mut body = vec![3, 0, 7, 0, (COMMIT_CHARS | FLAG_SYNC) as u8, 0];
        let s = b"\x1b$(BCf";
        body.extend_from_slice(&le16(s.len() as u16));
        body.extend_from_slice(s);
        p.message(&server(XIM_COMMIT, &body), now);
        let (msgs, outs) = p.take();
        assert_eq!(outs, [Output::Ime(WIN, Ime::Commit("中".into()))]);
        assert_eq!(msgs, [ic_message(XIM_SYNC_REPLY, 3, 7, &[])]);
        // A keysym-only commit.
        let mut body = vec![3, 0, 7, 0, COMMIT_KEYSYM as u8, 0, 0, 0];
        body.extend_from_slice(&0xff0du32.to_le_bytes());
        p.message(&server(XIM_COMMIT, &body), now);
        assert_eq!(p.take().1, [Output::Keysym(WIN, 0xff0d)]);
        // XIM_SYNC is answered.
        p.message(&server(XIM_SYNC, &[3, 0, 7, 0]), now);
        assert_eq!(p.take().0, [ic_message(XIM_SYNC_REPLY, 3, 7, &[])]);
    }

    #[test]
    fn spot_moves_and_focus_loss_reset_the_context() {
        let (mut p, now, _) = ready(1);
        p.set_ime(WIN, Some((30, 40)), now);
        let mut want = vec![54, 0, 5, 0, 3, 0, 7, 0, 12, 0, 0, 0];
        want.extend_from_slice(&[4, 0, 8, 0, 5, 0, 4, 0, 30, 0, 40, 0]);
        assert_eq!(p.take().0, [want]);
        p.set_ime(WIN, Some((30, 40)), now);
        assert!(p.take().0.is_empty(), "unchanged");
        // Mid-composition, the field loses focus.
        p.message(&draw(1, 0, 0, b"k", &[2]), now);
        p.take();
        p.set_ime(WIN, None, now);
        let ops: Vec<u8> = p.take().0.iter().map(|m| m[0]).collect();
        assert_eq!(ops, [XIM_RESET_IC, XIM_UNSET_IC_FOCUS]);
    }

    #[test]
    fn failure_clears_preedit_and_returns_keys() {
        let (mut p, now, _) = ready(1);
        p.message(&draw(1, 0, 0, b"k", &[2]), now);
        p.take();
        p.fail("test");
        assert_eq!(
            p.take().1,
            [Output::Ime(
                WIN,
                Ime::Preedit {
                    text: String::new(),
                    cursor: None
                }
            )]
        );
    }

    #[test]
    fn small_messages_ride_in_one_client_message() {
        let atoms = Atoms {
            protocol: 0x100,
            moredata: 0x101,
            xconnect: 0x102,
            data: [0x110, 0x111, 0x112, 0x113],
        };
        let reqs = transport_requests(0x77, &atoms, 0, &[58, 0, 1, 0, 3, 0, 7, 0]);
        assert_eq!(reqs.len(), 1);
        let r = &reqs[0];
        // SendEvent, 11 words, to 0x77 with no event mask.
        assert_eq!(&r[..12], &[25, 0, 11, 0, 0x77, 0, 0, 0, 0, 0, 0, 0]);
        // ClientMessage, format 8, window, _XIM_PROTOCOL, the message.
        assert_eq!(&r[12..24], &[33, 8, 0, 0, 0x77, 0, 0, 0, 0, 1, 0, 0]);
        assert_eq!(&r[24..32], &[58, 0, 1, 0, 3, 0, 7, 0]);
        assert!(r[32..].iter().all(|&b| b == 0));
    }

    #[test]
    fn large_messages_go_through_a_property() {
        let atoms = Atoms {
            protocol: 0x100,
            moredata: 0x101,
            xconnect: 0x102,
            data: [0x110, 0x111, 0x112, 0x113],
        };
        let msg = forward_message(3, 7, 0, &key_event(WIN, true, 38));
        assert_eq!(msg.len(), 44);
        let reqs = transport_requests(0x77, &atoms, 5, &msg);
        assert_eq!(reqs.len(), 2);
        // ChangeProperty, Append, the second data atom, STRING, format 8.
        let p = &reqs[0];
        assert_eq!(&p[..4], &[18, 2, 17, 0]);
        assert_eq!(
            &p[4..24],
            &[
                0x77, 0, 0, 0, 0x11, 1, 0, 0, 31, 0, 0, 0, 8, 0, 0, 0, 44, 0, 0, 0
            ]
        );
        assert_eq!(&p[24..], &msg[..]);
        // ClientMessage, format 32: length and atom.
        let c = &reqs[1];
        assert_eq!(&c[12..16], &[33, 32, 0, 0]);
        assert_eq!(&c[24..32], &[44, 0, 0, 0, 0x11, 1, 0, 0]);
    }

    #[test]
    fn splits_buffers_into_messages() {
        let a = message(XIM_SYNC, &[3, 0, 7, 0]);
        let b = message(XIM_PREEDIT_DONE, &[3, 0, 7, 0]);
        let mut buf = [a.clone(), b.clone()].concat();
        buf.extend_from_slice(&[0; 12]); // a client message's padding
        assert_eq!(split_messages(&buf), [&a[..], &b[..]]);
    }

    #[test]
    fn reads_im_name_and_locale() {
        assert_eq!(im_name("@im=ibus"), Some("ibus"));
        assert_eq!(im_name("@im=fcitx @other=x"), Some("fcitx"));
        assert_eq!(im_name(""), None);
    }

    /// A scripted fake server: drives the real transport encoding and the
    /// protocol together, decoding what the client sends from its X requests
    /// the way a server would read them.
    #[test]
    fn scripted_server_conversation() {
        let atoms = Atoms {
            protocol: 0x100,
            moredata: 0x101,
            xconnect: 0x102,
            data: [0x110, 0x111, 0x112, 0x113],
        };
        // What the "server" receives: decode each request back to a message.
        let decode = |reqs: Vec<Vec<u8>>| -> Vec<u8> {
            let last = reqs.last().unwrap();
            if last[13] == 8 {
                let data = &last[24..44];
                let len = 4 + 4 * u16::from_le_bytes([data[2], data[3]]) as usize;
                data[..len].to_vec()
            } else {
                let n = u32::from_le_bytes(reqs[0][20..24].try_into().unwrap()) as usize;
                reqs[0][24..24 + n].to_vec()
            }
        };
        let now = Instant::now();
        let mut p = Protocol::new("en_US".into(), now);
        let mut seq = 0;
        let mut exchange = |p: &mut Protocol, reply: Option<Vec<u8>>| -> Vec<Vec<u8>> {
            if let Some(r) = reply {
                for m in split_messages(&r) {
                    p.message(m, now);
                }
            }
            let (msgs, _) = p.take();
            msgs.into_iter()
                .map(|m| {
                    let reqs = transport_requests(0x77, &atoms, seq, &m);
                    seq += 1;
                    decode(reqs)
                })
                .collect()
        };
        p.set_ime(WIN, Some((5, 6)), now);
        p.connected(now);
        let script: Vec<(Option<Vec<u8>>, &[u8])> = vec![
            (None, &[XIM_CONNECT]),
            (Some(server(XIM_CONNECT_REPLY, &[1, 0, 0, 0])), &[XIM_OPEN]),
            (Some(open_reply(1)), &[XIM_ENCODING_NEGOTIATION]),
            (
                Some(server(
                    XIM_ENCODING_NEGOTIATION_REPLY,
                    &[1, 0, 0, 0, 1, 0, 0, 0],
                )),
                &[XIM_CREATE_IC],
            ),
            (
                Some(server(XIM_CREATE_IC_REPLY, &[1, 0, 2, 0])),
                &[XIM_SET_IC_FOCUS],
            ),
        ];
        for (reply, want) in script {
            let got: Vec<u8> = exchange(&mut p, reply).iter().map(|m| m[0]).collect();
            assert_eq!(got, want);
        }
        assert!(p.key(WIN, &key_event(WIN, true, 38), now));
        let sent = exchange(&mut p, None);
        assert_eq!(sent[0][0], XIM_FORWARD_EVENT);
        assert_eq!(&sent[0][4..8], &[1, 0, 2, 0]);
        assert_eq!(&sent[0][12..], &key_event(WIN, true, 38));
    }
}
