//! Live OS text controls (see `docs/native-theme.md`, *Text fields*): what
//! a text field hands its `UITextField` / `NSTextField` / `EDIT`, what
//! comes back when the user edits it, and the bookkeeping every backend
//! keeps them in step with ([`TextMirror`]), free of the platforms so it is
//! tested on every host.
//!
//! The app's state is the truth. The control's edits come back as
//! [`NativeTextEdit`]s, each a change to the text the control held before
//! it (a range replaced, the new selection, the composition), which the App
//! applies to the field as keys would be ([`KeyInput::SetSelection`],
//! [`KeyInput::Text`], [`KeyInput::Preedit`]), so a field's length limit,
//! read-only state, filters and handlers apply. The next frame's
//! [`NativeText`] says what the field holds then, and the control is set to
//! it when it differs (a change the app refused or transformed), the caret
//! where the app put it. Edits are numbered: the control isn't set from a
//! frame that hasn't taken in every edit it sent (`seen`), which would
//! undo what was typed meanwhile.
//!
//! [`KeyInput::SetSelection`]: crate::core::KeyInput::SetSelection
//! [`KeyInput::Text`]: crate::core::KeyInput::Text
//! [`KeyInput::Preedit`]: crate::core::KeyInput::Preedit

use std::sync::atomic::{AtomicU64, Ordering};

/// A text field's state, for the live OS control that edits it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct NativeText {
    /// The text (without a composition in progress).
    pub text: String,
    /// The selection's fixed end and the caret, byte indices into
    /// [`text`](Self::text) (equal: no selection).
    pub anchor: usize,
    pub caret: usize,
    /// An input method's composition at the caret, not yet part of the
    /// text.
    pub composing: Option<Composition>,
    /// The hint shown while the field is empty.
    pub placeholder: String,
    /// Several lines (a text area): Return is a new line.
    pub multiline: bool,
    /// A password: shown masked, kept from input methods and the clipboard.
    pub secure: bool,
    /// The user can select and copy, but not change it.
    pub read_only: bool,
    /// The text isn't acceptable (a number out of range); the control
    /// shows it where the OS can (a red border).
    pub invalid: bool,
    /// The field has the app's keyboard focus: the control takes it.
    pub focused: bool,
    /// The last of the control's edits the app has taken in
    /// ([`NativeTextEdit::seq`]); 0 for none.
    pub seen: u64,
    /// Drawn in the dark appearance (for a field the OS shows live without
    /// drawing its frame otherwise: an invalid number being edited).
    pub(crate) dark: bool,
    /// The on-screen keyboard's hints (kind, capitals, corrections, action
    /// key, autofill).
    pub(crate) hints: crate::platform::InputHints,
}

/// An input method's composition (marked text): its text and the byte
/// range within it that is highlighted (its caret, when the ends are
/// equal).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Composition {
    pub text: String,
    pub cursor: Option<(usize, usize)>,
}

/// What the user did to a live text control's text: `replaced` (a byte
/// range of the text as the app last had it, composition left out)
/// became `inserted`; then the selection is `anchor..caret` and the
/// composition `composing`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct NativeTextEdit {
    /// Its number: higher than every edit any control sent before it.
    pub seq: u64,
    pub replaced: (usize, usize),
    pub inserted: String,
    pub anchor: usize,
    pub caret: usize,
    pub composing: Option<Composition>,
}

/// A key a live text control leaves to the app (it has no use for it, or
/// the app decides what it does): Return (or the on-screen keyboard's
/// action key), Tab, Shift-Tab, Escape, and Up and Down in a one-line
/// field (a number input steps).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum NativeKey {
    Enter,
    Tab,
    BackTab,
    Escape,
    Up,
    Down,
}

/// For self-tests (CI): what to do to a live text control, as the user
/// would. See [`poke_text`].
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TextPoke {
    /// Give it the keyboard focus, as a click or tap at the end of its
    /// text does (the caret goes there).
    Focus,
    /// Type this text at the caret (through the control's own text input:
    /// `insertText:`, `WM_CHAR`).
    Type(String),
    /// Compose this text at the caret, as an input method does
    /// (`setMarkedText:`); empty ends the composition without text.
    Compose(String),
    /// Press Return.
    Enter,
}

/// For self-tests (CI): act on live text control `id` as the user would.
/// Returns whether the control was there and took it.
#[doc(hidden)]
pub fn poke_text(id: u64, poke: TextPoke) -> bool {
    super::os_views::poke_text(id, poke)
}

/// What a text control holds: all its text (a composition in it
/// included), the selection (OS selections have no direction) and the
/// composition's range, in bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Held {
    pub(crate) text: String,
    pub(crate) sel: (usize, usize),
    pub(crate) marked: Option<(usize, usize)>,
}

// The backends read and write controls in UTF-16 offsets.
#[cfg_attr(
    not(any(target_os = "ios", target_os = "macos", target_os = "windows")),
    allow(dead_code)
)]
impl Held {
    /// What a control reports in UTF-16 offsets (AppKit's, UIKit's and
    /// Win32's), clamped to its text.
    pub(crate) fn from_utf16(
        text: String,
        sel: (usize, usize),
        marked: Option<(usize, usize)>,
    ) -> Held {
        let b = |u: usize| byte_of_utf16(&text, u);
        let (s, e) = (b(sel.0.min(sel.1)), b(sel.0.max(sel.1)));
        let marked = marked.map(|(s, e)| (b(s), b(e))).filter(|(s, e)| e > s);
        Held {
            sel: (s, e),
            marked,
            text,
        }
    }

    /// The selection in UTF-16 offsets.
    pub(crate) fn sel_utf16(&self) -> (usize, usize) {
        (
            utf16_of_byte(&self.text, self.sel.0),
            utf16_of_byte(&self.text, self.sel.1),
        )
    }
}

/// The byte index of UTF-16 offset `u` in `s` (clamped; inside a
/// character, its start).
pub(crate) fn byte_of_utf16(s: &str, u: usize) -> usize {
    let mut units = 0;
    for (i, c) in s.char_indices() {
        if units >= u {
            return i;
        }
        units += c.len_utf16();
        if units > u {
            return i;
        }
    }
    s.len()
}

/// The UTF-16 offset of byte index `b` in `s`.
pub(crate) fn utf16_of_byte(s: &str, b: usize) -> usize {
    let mut b = b.min(s.len());
    while !s.is_char_boundary(b) {
        b -= 1;
    }
    s[..b].encode_utf16().count()
}

/// A Win32 multi-line `EDIT`'s text (its lines end in `\r\n`) as the app
/// has it (`\n`), with UTF-16 offsets `sel` into it moved along.
#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
pub(crate) fn from_crlf(units: &[u16], sel: (usize, usize)) -> (String, (usize, usize)) {
    let (cr, lf) = (u16::from(b'\r'), u16::from(b'\n'));
    let mut out = Vec::with_capacity(units.len());
    let (mut a, mut b) = sel;
    for (i, &u) in units.iter().enumerate() {
        if u == cr && units.get(i + 1) == Some(&lf) {
            if sel.0 > i {
                a -= 1;
            }
            if sel.1 > i {
                b -= 1;
            }
            continue;
        }
        out.push(u);
    }
    (String::from_utf16_lossy(&out), (a, b))
}

/// The other way: `text`'s new lines as `\r\n`, and UTF-16 offsets `sel`
/// into `text` as offsets into that.
#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
pub(crate) fn to_crlf(text: &str, sel: (usize, usize)) -> (Vec<u16>, (usize, usize)) {
    let mut out = Vec::with_capacity(text.len() + 2);
    let (mut a, mut b) = sel;
    for (i, u) in text.encode_utf16().enumerate() {
        if u == u16::from(b'\n') {
            out.push(u16::from(b'\r'));
            if sel.0 > i {
                a += 1;
            }
            if sel.1 > i {
                b += 1;
            }
        }
        out.push(u);
    }
    (out, (a, b))
}

/// The largest char boundary of `s` at or below `i`.
fn floor(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// For self-tests (CI): the last few things the live text controls'
/// bookkeeping did (edits sent, states set), once [`trace_text`] turned
/// it on.
#[doc(hidden)]
pub fn text_trace() -> Vec<String> {
    TRACE
        .lock()
        .map(|t| t.iter().cloned().collect())
        .unwrap_or_default()
}

/// For self-tests (CI): record what the live text controls' bookkeeping
/// does, for [`text_trace`].
#[doc(hidden)]
pub fn trace_text(on: bool) {
    TRACING.store(on, Ordering::Relaxed);
}

static TRACING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static TRACE: std::sync::Mutex<std::collections::VecDeque<String>> =
    std::sync::Mutex::new(std::collections::VecDeque::new());

/// Record `line` for [`text_trace`], when tracing.
pub(crate) fn trace(line: impl FnOnce() -> String) {
    if TRACING.load(Ordering::Relaxed)
        && let Ok(mut t) = TRACE.lock()
    {
        if t.len() >= 160 {
            t.pop_front();
        }
        t.push_back(line());
    }
}

/// Edits are numbered across every control (one recreated after it was a
/// picture a while starts above what the app saw of the last one).
static NEXT_SEQ: AtomicU64 = AtomicU64::new(1);

/// One live text control's bookkeeping: what it holds, as last told to
/// the app or set from it.
#[derive(Debug, Default)]
pub(crate) struct TextMirror {
    held: Held,
    /// The app's selection runs backwards (its caret before its anchor).
    backwards: bool,
    /// The last edit sent (0: none).
    sent: u64,
}

#[cfg_attr(
    not(any(target_os = "ios", target_os = "macos", target_os = "windows")),
    allow(dead_code)
)]
impl TextMirror {
    /// What to put in the control for the app's state `spec`, when it
    /// holds something else: `None` when it's in step, or while the app
    /// hasn't taken in every edit the control sent (its state is behind
    /// the control's then).
    pub(crate) fn wanted(&mut self, spec: &NativeText) -> Option<Held> {
        let r = self.wanted_inner(spec);
        trace(|| {
            format!(
                "wanted seen={} sent={} app={:?}@{}..{} held={:?} -> {:?}",
                spec.seen, self.sent, spec.text, spec.anchor, spec.caret, self.held, r
            )
        });
        r
    }

    fn wanted_inner(&mut self, spec: &NativeText) -> Option<Held> {
        if spec.seen < self.sent {
            return None;
        }
        let text = &spec.text;
        let (a, c) = (floor(text, spec.anchor), floor(text, spec.caret));
        let sel = (a.min(c), a.max(c));
        // Composing in the control, and the app has it too: in step.
        if let (Some((ms, me)), Some(comp)) = (self.held.marked, &spec.composing) {
            let mut shown = text[..sel.0].to_string();
            shown.push_str(&comp.text);
            shown.push_str(&text[sel.0..]);
            if shown == self.held.text
                && self.held.text.get(ms..me) == Some(comp.text.as_str())
                && ms == sel.0
            {
                return None;
            }
        }
        let want = Held {
            text: text.clone(),
            sel,
            marked: None,
        };
        self.backwards = a > c;
        if want == self.held {
            return None;
        }
        self.held = want.clone();
        Some(want)
    }

    /// The control now holds `now`: the edit to tell the app about, if
    /// anything changed.
    pub(crate) fn observe(&mut self, now: Held) -> Option<NativeTextEdit> {
        if now == self.held {
            return None;
        }
        trace(|| format!("observe {:?} (held {:?})", now, self.held));
        let old = std::mem::replace(&mut self.held, now);
        let new = &self.held;
        if old.sel != new.sel || old.marked.is_some() || new.marked.is_some() {
            self.backwards &= old.sel == new.sel;
        }
        let before = committed(&old);
        let after = committed(new);
        let (start, old_end, new_end) = changed(&before, &after);
        let (anchor, caret, composing) = match new.marked {
            Some((ms, me)) => {
                let inside = |i: usize| i.clamp(ms, me) - ms;
                let comp = Composition {
                    text: new.text[ms..me].to_string(),
                    cursor: Some((inside(new.sel.0), inside(new.sel.1))),
                };
                (ms, ms, Some(comp))
            }
            None if self.backwards => (new.sel.1, new.sel.0, None),
            None => (new.sel.0, new.sel.1, None),
        };
        self.sent = NEXT_SEQ.fetch_add(1, Ordering::Relaxed);
        Some(NativeTextEdit {
            seq: self.sent,
            replaced: (start, old_end),
            inserted: after[start..new_end].to_string(),
            anchor,
            caret,
            composing,
        })
    }

    /// What the control holds, as far as the mirror knows.
    #[cfg_attr(target_os = "windows", allow(dead_code))]
    pub(crate) fn held(&self) -> &Held {
        &self.held
    }

    /// The last edit it sent (0: none).
    pub(crate) fn sent(&self) -> u64 {
        self.sent
    }

    /// Forget what the control holds (it took the focus and selected
    /// everything, say): the next [`wanted`](Self::wanted) sets it again.
    pub(crate) fn forget(&mut self) {
        self.held = Held {
            text: "\u{0}".into(),
            ..Held::default()
        };
    }
}

/// `held`'s text without its composition.
fn committed(held: &Held) -> String {
    match held.marked {
        Some((s, e)) => {
            let mut t = held.text[..s].to_string();
            t.push_str(&held.text[e..]);
            t
        }
        None => held.text.clone(),
    }
}

/// Where `a` and `b` differ: the start, and the end in each (char
/// boundaries).
fn changed(a: &str, b: &str) -> (usize, usize, usize) {
    let mut start = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
    start = floor(a, start).min(floor(b, start));
    let max_suffix = (a.len() - start).min(b.len() - start);
    let mut suffix = a
        .bytes()
        .rev()
        .zip(b.bytes().rev())
        .take(max_suffix)
        .take_while(|(x, y)| x == y)
        .count();
    while !a.is_char_boundary(a.len() - suffix) || !b.is_char_boundary(b.len() - suffix) {
        suffix -= 1;
    }
    (start, a.len() - suffix, b.len() - suffix)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(text: &str, anchor: usize, caret: usize, seen: u64) -> NativeText {
        NativeText {
            text: text.into(),
            anchor,
            caret,
            seen,
            ..NativeText::default()
        }
    }

    fn held(text: &str, sel: (usize, usize), marked: Option<(usize, usize)>) -> Held {
        Held {
            text: text.into(),
            sel,
            marked,
        }
    }

    #[test]
    fn utf16_offsets_map_to_bytes_and_back() {
        let s = "aé😀b";
        assert_eq!(byte_of_utf16(s, 0), 0);
        assert_eq!(byte_of_utf16(s, 2), 3);
        // Inside the surrogate pair: the emoji's start.
        assert_eq!(byte_of_utf16(s, 3), 3);
        assert_eq!(byte_of_utf16(s, 4), 7);
        assert_eq!(byte_of_utf16(s, 99), s.len());
        assert_eq!(utf16_of_byte(s, 7), 4);
        assert_eq!(utf16_of_byte(s, 4), 2, "inside a char: its start");
        let h = Held::from_utf16(s.into(), (5, 4), Some((2, 4)));
        assert_eq!(h.sel, (7, 8));
        assert_eq!(h.marked, Some((3, 7)));
        assert_eq!(h.sel_utf16(), (4, 5));
    }

    #[test]
    fn typing_becomes_an_edit_and_the_app_catching_up_leaves_the_control_alone() {
        let mut m = TextMirror::default();
        assert_eq!(
            m.wanted(&spec("ab", 2, 2, 0)),
            Some(held("ab", (2, 2), None))
        );
        // In step: nothing to set.
        assert_eq!(m.wanted(&spec("ab", 2, 2, 0)), None);
        let e = m.observe(held("abc", (3, 3), None)).unwrap();
        assert_eq!((e.replaced, e.inserted.as_str()), ((2, 2), "c"));
        assert_eq!((e.anchor, e.caret), (3, 3));
        assert!(e.composing.is_none());
        // A frame from before the app took the edit in: left alone.
        assert_eq!(m.wanted(&spec("ab", 2, 2, 0)), None);
        // Taken in: in step.
        assert_eq!(m.wanted(&spec("abc", 3, 3, e.seq)), None);
        // Nothing new: no edit.
        assert!(m.observe(held("abc", (3, 3), None)).is_none());
        // Edits count up.
        let f = m.observe(held("abc", (0, 3), None)).unwrap();
        assert!(f.seq > e.seq);
        assert_eq!((f.replaced, f.inserted.as_str()), ((3, 3), ""));
        assert_eq!((f.anchor, f.caret), (0, 3));
    }

    #[test]
    fn a_refused_edit_is_set_back_with_the_apps_caret() {
        let mut m = TextMirror::default();
        m.wanted(&spec("1234", 4, 4, 0));
        let e = m.observe(held("1234x", (5, 5), None)).unwrap();
        // The app's length limit kept "1234", caret at its end.
        assert_eq!(
            m.wanted(&spec("1234", 4, 4, e.seq)),
            Some(held("1234", (4, 4), None))
        );
        // Transformed (upper-cased), the caret where the app has it.
        let e = m.observe(held("1234a", (5, 5), None)).unwrap();
        assert_eq!(
            m.wanted(&spec("1234A", 5, 5, e.seq)),
            Some(held("1234A", (5, 5), None))
        );
    }

    #[test]
    fn a_composition_is_the_apps_preedit_and_its_commit_a_replacement() {
        let mut m = TextMirror::default();
        m.wanted(&spec("Hi", 2, 2, 0));
        // Marked "にほ" at the end, the caret after it.
        let comp = "Hiにほ";
        let e = m.observe(held(comp, (8, 8), Some((2, 8)))).unwrap();
        assert_eq!((e.replaced, e.inserted.as_str()), ((2, 2), ""));
        assert_eq!((e.anchor, e.caret), (2, 2));
        let c = e.composing.clone().unwrap();
        assert_eq!(c.text, "にほ");
        assert_eq!(c.cursor, Some((6, 6)));
        // The app has it as its preedit: the control is left composing.
        let mut s = spec("Hi", 2, 2, e.seq);
        s.composing = Some(c);
        assert_eq!(m.wanted(&s), None);
        // Committed as 日本: the text gains it, the composition ends.
        let e = m.observe(held("Hi日本", (8, 8), None)).unwrap();
        assert_eq!((e.replaced, e.inserted.as_str()), ((2, 2), "日本"));
        assert_eq!((e.anchor, e.caret), (8, 8));
        assert!(e.composing.is_none());
    }

    #[test]
    fn a_backwards_selection_keeps_its_direction_until_the_user_moves_it() {
        let mut m = TextMirror::default();
        assert_eq!(
            m.wanted(&spec("hello", 4, 1, 0)),
            Some(held("hello", (1, 4), None))
        );
        // The OS reports it ordered: in step.
        assert_eq!(m.wanted(&spec("hello", 4, 1, 0)), None);
        // Typed over: a plain caret.
        let e = m.observe(held("hXo", (2, 2), None)).unwrap();
        assert_eq!((e.replaced, e.inserted.as_str()), ((1, 4), "X"));
        assert_eq!((e.anchor, e.caret), (2, 2));
    }

    #[test]
    fn win32_line_ends_map_both_ways() {
        let units: Vec<u16> = "ab\r\ncd\r\n".encode_utf16().collect();
        let (text, sel) = from_crlf(&units, (5, 8));
        assert_eq!(text, "ab\ncd\n");
        assert_eq!(sel, (4, 6));
        let (back, sel) = to_crlf(&text, (4, 6));
        assert_eq!(back, units);
        assert_eq!(sel, (5, 8));
    }

    #[test]
    fn the_change_is_found_on_char_boundaries() {
        assert_eq!(changed("aéb", "aèb"), (1, 3, 3));
        assert_eq!(changed("aa", "aaa"), (2, 2, 3));
        assert_eq!(changed("abc", ""), (0, 3, 0));
        assert_eq!(changed("", ""), (0, 0, 0));
    }
}
