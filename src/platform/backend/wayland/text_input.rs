//! Input methods on Wayland: `zwp_text_input_v3` (text-input-unstable-v3).
//!
//! The compositor relays between the focused client and an input method (such
//! as fcitx5 or IBus speaking `input-method-v2`). While a text field has focus
//! we `enable` the text input and keep it told the field's surrounding text and
//! caret rectangle, each change sealed with `commit`; the input method answers
//! with `preedit_string`, `commit_string` and `delete_surrounding_text`, which
//! take effect together on `done`. Without an input method running, nothing
//! comes back and keys arrive through `wl_keyboard` as usual.

use super::{WlConn, arg_str, arg_u32, dbg, read_str};
use crate::platform::ImeState;
use crate::platform::event::Ime;
use std::io::Write;

/// The global's interface name.
pub(super) const MANAGER: &str = "zwp_text_input_manager_v3";
// zwp_text_input_manager_v3 requests.
const MANAGER_GET_TEXT_INPUT: u16 = 1;
// zwp_text_input_v3 requests.
const ENABLE: u16 = 1;
const DISABLE: u16 = 2;
const SET_SURROUNDING_TEXT: u16 = 3;
const SET_TEXT_CHANGE_CAUSE: u16 = 4;
const SET_CONTENT_TYPE: u16 = 5;
const SET_CURSOR_RECTANGLE: u16 = 6;
const COMMIT: u16 = 7;
// zwp_text_input_v3 events.
const ENTER: u16 = 0;
const LEAVE: u16 = 1;
const PREEDIT_STRING: u16 = 2;
const COMMIT_STRING: u16 = 3;
const DELETE_SURROUNDING_TEXT: u16 = 4;
const DONE: u16 = 5;
/// The protocol caps surrounding text below the 4 KiB message size.
const MAX_SURROUNDING: usize = 4000;

/// The text input's state, kept on the connection.
#[derive(Debug, Default)]
pub(super) struct TextInput {
    object: Option<u32>,
    /// Whether the compositor has given our surface text-input focus.
    entered: bool,
    /// Whether we've enabled the text input since the last `enter`.
    enabled: bool,
    /// What the App asked for: the focused field, or `None`.
    wanted: Option<ImeState>,
    /// Changes received since the last `done`.
    preedit: Option<(String, i32, i32)>,
    commit: Option<String>,
    delete: Option<(u32, u32)>,
    /// Whether the field is showing a preedit we sent.
    showing: bool,
    /// Whether the last change came from the input method (the next state we
    /// send reports that cause).
    ime_edit: bool,
}

/// Create the seat's text input.
pub(super) fn setup(conn: &mut WlConn, manager: u32, seat: u32) {
    let id = conn.new_id();
    let mut a = Vec::new();
    arg_u32(&mut a, id);
    arg_u32(&mut a, seat);
    if conn.send(manager, MANAGER_GET_TEXT_INPUT, &a).is_ok() {
        conn.text_input.object = Some(id);
        dbg(format_args!("text-input: bound (object {id})"));
    }
}

/// Record what the App wants (`None`: no text field focused) and tell the
/// compositor.
pub(super) fn set_state(conn: &mut WlConn, state: Option<&ImeState>) {
    conn.text_input.wanted = state.cloned();
    apply(conn);
    let _ = conn.stream.flush();
}

/// The part of `text` to send as surrounding text, as `(start, end)` byte
/// offsets: all of it when short enough, else a window around the caret and
/// anchor on char boundaries.
fn surrounding_window(text: &str, cursor: usize, anchor: usize) -> (usize, usize) {
    if text.len() <= MAX_SURROUNDING {
        return (0, text.len());
    }
    let lo = cursor.min(anchor).min(text.len());
    let mut start = lo.saturating_sub(MAX_SURROUNDING / 2);
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (start + MAX_SURROUNDING).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (start, end)
}

/// Bring the compositor's view in line with `wanted`.
fn apply(conn: &mut WlConn) {
    let Some(object) = conn.text_input.object else {
        return;
    };
    let ti = &conn.text_input;
    match (&ti.wanted, ti.entered) {
        (Some(state), true) => {
            let state = state.clone();
            let first = !ti.enabled;
            if first {
                let _ = conn.send(object, ENABLE, &[]);
                // Plain text, no hints.
                let mut a = Vec::new();
                arg_u32(&mut a, 0);
                arg_u32(&mut a, 0);
                let _ = conn.send(object, SET_CONTENT_TYPE, &a);
            }
            let (start, end) = surrounding_window(&state.surrounding, state.cursor, state.anchor);
            let clamp = |i: usize| (i.clamp(start, end) - start) as u32;
            let mut a = Vec::new();
            arg_str(&mut a, &state.surrounding[start..end]);
            arg_u32(&mut a, clamp(state.cursor));
            arg_u32(&mut a, clamp(state.anchor));
            let _ = conn.send(object, SET_SURROUNDING_TEXT, &a);
            // Cause 0: the input method made the change; 1: anything else.
            let mut a = Vec::new();
            arg_u32(&mut a, if conn.text_input.ime_edit { 0 } else { 1 });
            conn.text_input.ime_edit = false;
            let _ = conn.send(object, SET_TEXT_CHANGE_CAUSE, &a);
            let r = state.cursor_area;
            let mut a = Vec::new();
            arg_u32(&mut a, r.min_x().round() as i32 as u32);
            arg_u32(&mut a, r.min_y().round() as i32 as u32);
            arg_u32(&mut a, r.width().round().max(1.0) as i32 as u32);
            arg_u32(&mut a, r.height().round().max(1.0) as i32 as u32);
            let _ = conn.send(object, SET_CURSOR_RECTANGLE, &a);
            let _ = conn.send(object, COMMIT, &[]);
            if !conn.text_input.enabled {
                dbg(format_args!("text-input: enabled"));
            }
            conn.text_input.enabled = true;
        }
        _ if ti.enabled => {
            let _ = conn.send(object, DISABLE, &[]);
            let _ = conn.send(object, COMMIT, &[]);
            conn.text_input.enabled = false;
            dbg(format_args!("text-input: disabled"));
        }
        _ => {}
    }
}

/// Handle a message if it's for the text input, returning the IME events to
/// deliver (`None` if the message isn't ours). `surface` is our window's.
pub(super) fn handle(
    conn: &mut WlConn,
    object: u32,
    opcode: u16,
    args: &[u8],
    surface: u32,
) -> Option<Vec<Ime>> {
    if conn.text_input.object != Some(object) {
        return None;
    }
    let word = |i: usize| -> u32 {
        args.get(i..i + 4)
            .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let mut out = Vec::new();
    match opcode {
        ENTER if word(0) == surface => {
            dbg(format_args!("text-input: enter"));
            conn.text_input.entered = true;
            conn.text_input.enabled = false;
            apply(conn);
            let _ = conn.stream.flush();
        }
        LEAVE => {
            // Leaving clears the text input's state; the preedit goes too.
            let ti = &mut conn.text_input;
            ti.entered = false;
            ti.enabled = false;
            ti.preedit = None;
            ti.commit = None;
            ti.delete = None;
            if std::mem::take(&mut ti.showing) {
                out.push(Ime::Preedit {
                    text: String::new(),
                    cursor: None,
                });
            }
        }
        PREEDIT_STRING => {
            if let Some((text, next)) = read_str(args, 0) {
                let begin = word(next) as i32;
                let end = word(next + 4) as i32;
                conn.text_input.preedit = Some((text, begin, end));
            }
        }
        COMMIT_STRING => {
            if let Some((text, _)) = read_str(args, 0) {
                conn.text_input.commit = Some(text);
            }
        }
        DELETE_SURROUNDING_TEXT => {
            conn.text_input.delete = Some((word(0), word(4)));
        }
        DONE => out = take_done(&mut conn.text_input),
        _ => {}
    }
    Some(out)
}

/// The events for a `done`, in the protocol's order: delete around the
/// caret, insert the commit, then show the new preedit.
fn take_done(ti: &mut TextInput) -> Vec<Ime> {
    let mut out = Vec::new();
    if let Some((before, after)) = ti.delete.take() {
        out.push(Ime::DeleteSurrounding {
            before: before as usize,
            after: after as usize,
        });
    }
    let committed = ti.commit.take().filter(|t| !t.is_empty());
    let had_preedit = ti.showing;
    if let Some(text) = committed.clone() {
        out.push(Ime::Commit(text));
        ti.showing = false;
    }
    match ti.preedit.take() {
        Some((text, begin, end)) if !text.is_empty() => {
            // -1 for both ends hides the caret.
            let cursor = (begin >= 0 && end >= 0).then_some((begin as usize, end as usize));
            out.push(Ime::Preedit { text, cursor });
            ti.showing = true;
        }
        _ if had_preedit && committed.is_none() => {
            out.push(Ime::Preedit {
                text: String::new(),
                cursor: None,
            });
            ti.showing = false;
        }
        _ => {}
    }
    ti.ime_edit |= !out.is_empty();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn done_orders_delete_commit_preedit() {
        let mut ti = TextInput {
            preedit: Some(("かな".into(), 3, 3)),
            commit: Some("日本".into()),
            delete: Some((3, 0)),
            ..TextInput::default()
        };
        assert_eq!(
            take_done(&mut ti),
            vec![
                Ime::DeleteSurrounding {
                    before: 3,
                    after: 0
                },
                Ime::Commit("日本".into()),
                Ime::Preedit {
                    text: "かな".into(),
                    cursor: Some((3, 3))
                },
            ]
        );
        assert!(ti.showing);
        // A done with nothing pending clears the preedit that was shown.
        assert_eq!(
            take_done(&mut ti),
            vec![Ime::Preedit {
                text: String::new(),
                cursor: None
            }]
        );
        // ...once.
        assert_eq!(take_done(&mut ti), vec![]);
        // A hidden cursor is -1, -1.
        ti.preedit = Some(("x".into(), -1, -1));
        assert_eq!(
            take_done(&mut ti),
            vec![Ime::Preedit {
                text: "x".into(),
                cursor: None
            }]
        );
    }

    /// Read the requests the client wrote, as `(object, opcode, args)`.
    fn requests(theirs: &mut std::os::unix::net::UnixStream) -> Vec<(u32, u16, Vec<u8>)> {
        use std::io::Read;
        theirs.set_nonblocking(true).unwrap();
        let mut bytes = Vec::new();
        let _ = theirs.read_to_end(&mut bytes);
        let mut out = Vec::new();
        let mut at = 0;
        while at + 8 <= bytes.len() {
            let object = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
            let word = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap());
            let size = (word >> 16) as usize;
            out.push((
                object,
                (word & 0xffff) as u16,
                bytes[at + 8..at + size].to_vec(),
            ));
            at += size;
        }
        out
    }

    #[test]
    fn enables_on_enter_and_disables_when_focus_leaves_text() {
        let (ours, mut theirs) = std::os::unix::net::UnixStream::pair().unwrap();
        let mut conn = WlConn {
            stream: ours,
            next_id: 10,
            fds: Default::default(),
            queue: Default::default(),
            clip: Default::default(),
            buffer_scale: 1,
            cursor_device: None,
            enter_serial: None,
            cursor: Default::default(),
            text_input: Default::default(),
        };
        const MANAGER_ID: u32 = 5;
        const SEAT: u32 = 6;
        const SURFACE: u32 = 7;
        setup(&mut conn, MANAGER_ID, SEAT);
        let ti = conn.text_input.object.unwrap();
        let state = ImeState {
            cursor_area: crate::geometry::Rect::from_xywh(10.0, 20.0, 2.0, 18.0),
            surrounding: "héllo".into(),
            cursor: 3,
            anchor: 3,
        };
        // Wanted before the compositor gives us focus: nothing is sent yet.
        set_state(&mut conn, Some(&state));
        let sent = requests(&mut theirs);
        assert_eq!(sent.len(), 1); // just get_text_input
        assert_eq!(sent[0].0, MANAGER_ID);
        assert_eq!(sent[0].1, MANAGER_GET_TEXT_INPUT);
        // `enter` on our surface enables it and sends the state.
        let events = handle(&mut conn, ti, ENTER, &SURFACE.to_le_bytes(), SURFACE);
        assert_eq!(events, Some(vec![]));
        let sent = requests(&mut theirs);
        let ops: Vec<u16> = sent.iter().map(|r| r.1).collect();
        assert_eq!(
            ops,
            [
                ENABLE,
                SET_CONTENT_TYPE,
                SET_SURROUNDING_TEXT,
                SET_TEXT_CHANGE_CAUSE,
                SET_CURSOR_RECTANGLE,
                COMMIT
            ]
        );
        let (text, next) = read_str(&sent[2].2, 0).unwrap();
        assert_eq!(text, "héllo");
        assert_eq!(&sent[2].2[next..], &[3, 0, 0, 0, 3, 0, 0, 0]);
        let rect: Vec<i32> = sent[4]
            .2
            .chunks(4)
            .map(|c| i32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        assert_eq!(rect, [10, 20, 2, 18]);
        // Focus leaves text entry: disable and commit.
        set_state(&mut conn, None);
        let ops: Vec<u16> = requests(&mut theirs).iter().map(|r| r.1).collect();
        assert_eq!(ops, [DISABLE, COMMIT]);
        // Other objects' messages aren't ours.
        assert_eq!(handle(&mut conn, ti + 1, DONE, &[0; 4], SURFACE), None);
    }

    #[test]
    fn long_surrounding_text_is_windowed_around_the_caret() {
        assert_eq!(surrounding_window("short", 2, 2), (0, 5));
        let text = "é".repeat(5000); // 10000 bytes
        let (start, end) = surrounding_window(&text, 6000, 6000);
        assert!(end - start <= MAX_SURROUNDING);
        assert!(start <= 6000 && 6000 <= end);
        assert!(text.is_char_boundary(start) && text.is_char_boundary(end));
    }
}
