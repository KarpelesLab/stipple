//! The document a Text Services Framework client shows an input method,
//! without any COM: the platform-neutral half of the Windows TSF bridge
//! (`backend/windows/tsf`), kept apart so its logic is unit-tested on every
//! platform.
//!
//! TSF sees a text store (`ITextStoreACP`): a UTF-16 document, addressed by
//! "ACP" offsets (code units from the start), with a selection, read and
//! write locks the input method asks for, and compositions it starts over a
//! range of the document. The composition's text is part of that document.
//! Stipple's model is different: the focused field holds its committed text,
//! and the composition is a separate preedit shown at the caret (see
//! [`Ime`]). [`TextStore`] bridges the two. It holds the document TSF edits,
//! and after each lock ([`TextStore::flush`]) works out the [`Ime`] events
//! that take the App's field from what it last had to what the document now
//! says: the text outside the composition as `DeleteSurrounding` and
//! `Commit`, the composition as `Preedit`, highlighted where the input method
//! marks its target clause (display attributes) or else at its selection.
//!
//! The App reports its field with `Window::set_ime` after it has applied the
//! events, and [`TextStore::sync`] compares that with the text it predicted:
//! the same means in step, an older prediction means the report came before
//! the events were handled (ignored), and anything else is a change the App
//! made itself, which replaces the document (and ends any composition).

use crate::platform::event::Ime;

/// `TS_LF_SYNC`: the caller needs the lock now or not at all.
pub(crate) const LOCK_SYNC: u32 = 0x1;
/// `TS_LF_READ`.
pub(crate) const LOCK_READ: u32 = 0x2;
/// `TS_LF_READWRITE` (the read bit and the write bit).
pub(crate) const LOCK_READWRITE: u32 = 0x6;

/// `TF_DA_ATTR_INFO`: the clause being converted, as converted…
pub(crate) const ATTR_TARGET_CONVERTED: i32 = 1;
/// …and still unconverted.
pub(crate) const ATTR_TARGET_NOTCONVERTED: i32 = 3;

/// What became of a lock request (`ITextStoreACP::RequestLock`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LockGrant {
    /// Granted now with these flags: call `OnLockGranted`, then
    /// [`TextStore::release_lock`].
    Now(u32),
    /// Another lock is held; this one is granted when it is released
    /// (`TS_S_ASYNC`).
    Queued,
    /// A synchronous request while another lock is held
    /// (`TS_E_SYNCHRONOUS`).
    Refused,
}

/// Why a store call failed, as `TS_E_*`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StoreError {
    /// `TS_E_NOLOCK`: the caller doesn't hold the lock it needs.
    NoLock,
    /// `TS_E_INVALIDPOS`: an offset outside the document.
    InvalidPos,
}

/// Which end of the selection moves (`TsActiveSelEnd`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ActiveEnd {
    /// `TS_AE_NONE`.
    None,
    /// `TS_AE_START`.
    Start,
    /// `TS_AE_END`.
    #[default]
    End,
}

/// The document's selection, in UTF-16 offsets (`start <= end`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Selection {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) active: ActiveEnd,
}

impl Selection {
    fn caret(at: usize) -> Selection {
        Selection {
            start: at,
            end: at,
            active: ActiveEnd::End,
        }
    }

    /// Where the caret is: the moving end.
    fn active_pos(&self) -> usize {
        match self.active {
            ActiveEnd::Start => self.start,
            _ => self.end,
        }
    }
}

/// A change to the document, as `TS_TEXTCHANGE` reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TextChange {
    pub(crate) start: usize,
    pub(crate) old_end: usize,
    pub(crate) new_end: usize,
}

/// The App's field, as the store believes it is: its committed text (no
/// preedit), the caret and the selection's other end, in UTF-16 offsets.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AppText {
    pub(crate) text: Vec<u16>,
    pub(crate) caret: usize,
    pub(crate) anchor: usize,
}

impl AppText {
    /// From `ImeState`'s surrounding text and byte offsets.
    pub(crate) fn from_state(surrounding: &str, cursor: usize, anchor: usize) -> AppText {
        let units = |byte: usize| {
            let mut b = byte.min(surrounding.len());
            while !surrounding.is_char_boundary(b) {
                b -= 1;
            }
            surrounding[..b].encode_utf16().count()
        };
        AppText {
            text: surrounding.encode_utf16().collect(),
            caret: units(cursor),
            anchor: units(anchor),
        }
    }
}

/// How the App's report compares with what the store predicted
/// ([`TextStore::sync`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sync {
    /// In step: nothing to tell TSF.
    InStep,
    /// In step, but the App's caret or selection moved: the store took it,
    /// and TSF must hear of it (`OnSelectionChange`).
    Selection,
    /// A report from before the App applied the last events: ignored.
    Stale,
    /// The App changed its text itself. Nothing is changed yet: end any
    /// composition (`composing`), then [`TextStore::reset`] and tell TSF
    /// (`OnTextChange` over the whole document, `OnSelectionChange`).
    Changed { composing: bool },
}

/// How many predictions not yet confirmed by the App are remembered.
const HISTORY: usize = 8;

/// The text store: the document, its selection and lock, the composition, and
/// what the App's field holds.
#[derive(Debug, Default)]
pub(crate) struct TextStore {
    /// The document as TSF sees it: the App's text with the composition in.
    text: Vec<u16>,
    sel: Selection,
    /// The lock held (0, [`LOCK_READ`] or [`LOCK_READWRITE`]).
    lock: u32,
    /// A lock asked for while another was held, granted on release.
    queued: u32,
    /// The composition's range in `text`, while one is in progress.
    composition: Option<(usize, usize)>,
    /// Display attributes over the composition: ranges in `text` with their
    /// `TF_DA_ATTR_INFO`.
    attrs: Vec<(usize, usize, i32)>,
    /// What the App's field holds once it has applied every event sent.
    app: AppText,
    /// What it held before the latest events, newest last: a report matching
    /// one of these is from before the App caught up.
    history: Vec<AppText>,
    /// The preedit last sent, and its highlighted range (bytes).
    preedit: Option<(String, Option<(usize, usize)>)>,
    /// The composition is being ended because the App moved on: what it
    /// leaves behind isn't committed.
    discard: bool,
}

impl TextStore {
    pub(crate) fn new() -> TextStore {
        TextStore::default()
    }

    // ---- Locks ------------------------------------------------------------

    /// Ask for a lock (`RequestLock`). Without one held it is granted at
    /// once, whether asked synchronously or not; with one held an
    /// asynchronous request waits for its release, and a synchronous one is
    /// refused. Nothing ever waits, so the message loop never blocks.
    pub(crate) fn request_lock(&mut self, flags: u32) -> LockGrant {
        let want = if flags & LOCK_READWRITE == LOCK_READWRITE {
            LOCK_READWRITE
        } else {
            LOCK_READ
        };
        if self.lock != 0 {
            if flags & LOCK_SYNC != 0 {
                return LockGrant::Refused;
            }
            // One queued request is enough: the strongest asked for.
            self.queued |= want;
            return LockGrant::Queued;
        }
        self.lock = want;
        LockGrant::Now(want)
    }

    /// The holder is done (`OnLockGranted` returned). Returns the queued
    /// lock, now held, for the caller to grant in turn.
    pub(crate) fn release_lock(&mut self) -> Option<u32> {
        self.lock = std::mem::take(&mut self.queued);
        (self.lock != 0).then_some(self.lock)
    }

    /// Whether a lock is held.
    pub(crate) fn locked(&self) -> bool {
        self.lock != 0
    }

    fn can_read(&self) -> Result<(), StoreError> {
        if self.lock & LOCK_READ != 0 {
            Ok(())
        } else {
            Err(StoreError::NoLock)
        }
    }

    fn can_write(&self) -> Result<(), StoreError> {
        if self.lock == LOCK_READWRITE {
            Ok(())
        } else {
            Err(StoreError::NoLock)
        }
    }

    // ---- Reading ----------------------------------------------------------

    /// The document's length (`GetEndACP`).
    pub(crate) fn len(&self) -> Result<usize, StoreError> {
        self.can_read()?;
        Ok(self.text.len())
    }

    /// The text from `start` to `end` (`None`: the end of the document), at
    /// most `max` code units (`GetText`).
    pub(crate) fn text(
        &self,
        start: usize,
        end: Option<usize>,
        max: usize,
    ) -> Result<&[u16], StoreError> {
        self.can_read()?;
        let end = end.unwrap_or(self.text.len());
        if start > end || end > self.text.len() {
            return Err(StoreError::InvalidPos);
        }
        Ok(&self.text[start..end.min(start.saturating_add(max))])
    }

    /// The selection (`GetSelection`).
    pub(crate) fn selection(&self) -> Result<Selection, StoreError> {
        self.can_read()?;
        Ok(self.sel)
    }

    // ---- Writing ----------------------------------------------------------

    /// Select `start..end` (`SetSelection`).
    pub(crate) fn set_selection(
        &mut self,
        start: usize,
        end: usize,
        active: ActiveEnd,
    ) -> Result<(), StoreError> {
        self.can_write()?;
        if start > end || end > self.text.len() {
            return Err(StoreError::InvalidPos);
        }
        self.sel = Selection { start, end, active };
        Ok(())
    }

    /// Where text of any length can go in place of `start..end`
    /// (`QueryInsert`): anywhere in the document, as asked.
    pub(crate) fn query_insert(
        &self,
        start: usize,
        end: usize,
    ) -> Result<(usize, usize), StoreError> {
        if start > end || end > self.text.len() {
            return Err(StoreError::InvalidPos);
        }
        Ok((start, end))
    }

    /// Replace `start..end` with `with` (`SetText`); the caret goes after it.
    pub(crate) fn set_text(
        &mut self,
        start: usize,
        end: usize,
        with: &[u16],
    ) -> Result<TextChange, StoreError> {
        self.can_write()?;
        if start > end || end > self.text.len() {
            return Err(StoreError::InvalidPos);
        }
        Ok(self.replace(start, end, with))
    }

    /// Replace the selection with `with` (`InsertTextAtSelection`), or with
    /// `query_only` only say where it would go. Returns the inserted text's
    /// range, and the change made.
    pub(crate) fn insert_at_selection(
        &mut self,
        with: &[u16],
        query_only: bool,
    ) -> Result<(usize, usize, Option<TextChange>), StoreError> {
        let Selection { start, end, .. } = self.sel;
        if query_only {
            self.can_read()?;
            return Ok((start, end, None));
        }
        self.can_write()?;
        let change = self.replace(start, end, with);
        Ok((start, change.new_end, Some(change)))
    }

    fn replace(&mut self, start: usize, end: usize, with: &[u16]) -> TextChange {
        self.text.splice(start..end, with.iter().copied());
        let new_end = start + with.len();
        // Keep the composition over the same text. (TSF tracks the real range
        // and reports it before the flush; this keeps the store consistent in
        // between.)
        if let Some((cs, ce)) = self.composition {
            let shift = |at: usize| {
                if at >= end {
                    at - (end - start) + with.len()
                } else if at > start {
                    new_end
                } else {
                    at
                }
            };
            self.composition = Some(if end < cs || start > ce {
                (shift(cs), shift(ce))
            } else {
                // Touching or overlapping: the composition grows over it.
                (cs.min(start), shift(ce.max(end)).max(new_end))
            });
        }
        self.sel = Selection::caret(new_end);
        TextChange {
            start,
            old_end: end,
            new_end,
        }
    }

    // ---- Compositions -----------------------------------------------------

    /// An input method started a composition over `range`
    /// (`OnStartComposition`); `None` when its range is unknown (the
    /// selection is used).
    pub(crate) fn start_composition(&mut self, range: Option<(usize, usize)>) {
        let r = range.unwrap_or((self.sel.start, self.sel.end));
        self.composition = Some(self.clamp(r));
        self.attrs.clear();
        self.discard = false;
    }

    /// The composition's range is now `range` (`OnUpdateComposition`, or
    /// read back from TSF).
    pub(crate) fn update_composition(&mut self, range: (usize, usize)) {
        if self.composition.is_some() {
            self.composition = Some(self.clamp(range));
        }
    }

    /// The composition ended (`OnEndComposition`): its text stays in the
    /// document, committed by the next [`flush`](Self::flush).
    pub(crate) fn end_composition(&mut self) {
        self.composition = None;
        self.attrs.clear();
    }

    /// What the App's field holds, as far as the store knows.
    pub(crate) fn app(&self) -> &AppText {
        &self.app
    }

    /// Whether a composition is in progress.
    pub(crate) fn composing(&self) -> bool {
        self.composition.is_some()
    }

    /// The composition's display attributes, as ranges of the document with
    /// their `TF_DA_ATTR_INFO`.
    pub(crate) fn set_attributes(&mut self, attrs: Vec<(usize, usize, i32)>) {
        self.attrs = attrs;
    }

    /// The App moved on: the composition being ended now (by
    /// `TerminateComposition`) isn't committed, and nothing is sent until
    /// [`reset`](Self::reset).
    pub(crate) fn discard(&mut self) {
        self.discard = true;
    }

    fn clamp(&self, (s, e): (usize, usize)) -> (usize, usize) {
        let n = self.text.len();
        (s.min(e).min(n), s.max(e).min(n))
    }

    // ---- Events for the App -----------------------------------------------

    /// The events that bring the App's field to what the document says, after
    /// a lock (or a composition ending outside one).
    pub(crate) fn flush(&mut self) -> Vec<Ime> {
        if self.discard {
            return Vec::new();
        }
        // What the field should hold, and where its caret (and so the
        // preedit) goes.
        let (desired, want_caret) = match self.composition {
            Some((cs, ce)) => {
                let mut t = self.text[..cs].to_vec();
                t.extend_from_slice(&self.text[ce..]);
                (t, cs)
            }
            None => (
                self.text.clone(),
                self.sel.active_pos().min(self.text.len()),
            ),
        };
        let mut events = Vec::new();
        let mut committed = false;
        if desired != self.app.text {
            let (edit, caret) = diff(&self.app, &desired, want_caret);
            if edit.before > 0 || edit.after > 0 || self.app.caret != self.app.anchor {
                events.push(Ime::DeleteSurrounding {
                    before: edit.before,
                    after: edit.after,
                });
            }
            if !edit.insert.is_empty() {
                events.push(Ime::Commit(edit.insert));
                committed = true;
            }
            let old = std::mem::replace(
                &mut self.app,
                AppText {
                    text: desired,
                    caret,
                    anchor: caret,
                },
            );
            self.history.push(old);
            if self.history.len() > HISTORY {
                self.history.remove(0);
            }
        }
        let preedit = self.composition.and_then(|(cs, ce)| {
            let text = String::from_utf16_lossy(&self.text[cs..ce]);
            if text.is_empty() {
                return None;
            }
            let units = self.highlight(cs, ce);
            let cursor = units.map(|(s, e)| {
                (
                    crate::platform::input::utf16_to_byte(&text, s),
                    crate::platform::input::utf16_to_byte(&text, e),
                )
            });
            Some((text, cursor))
        });
        match &preedit {
            Some((text, cursor)) if committed || self.preedit.as_ref() != preedit.as_ref() => {
                events.push(Ime::Preedit {
                    text: text.clone(),
                    cursor: *cursor,
                });
            }
            None if self.preedit.is_some() && !committed => events.push(Ime::Preedit {
                text: String::new(),
                cursor: None,
            }),
            _ => {}
        }
        self.preedit = preedit;
        events
    }

    /// The range of the composition (`cs..ce` in the document) to highlight,
    /// in code units from its start: the target clause, else the selection
    /// when it is inside, else a caret at the end.
    fn highlight(&self, cs: usize, ce: usize) -> Option<(usize, usize)> {
        let target = self.attrs.iter().find(|(s, e, a)| {
            matches!(*a, ATTR_TARGET_CONVERTED | ATTR_TARGET_NOTCONVERTED) && *s < ce && *e > cs
        });
        if let Some(&(s, e, _)) = target {
            return Some((s.max(cs) - cs, e.min(ce) - cs));
        }
        let sel = self.sel;
        if sel.start >= cs && sel.end <= ce {
            return Some((sel.start - cs, sel.end - cs));
        }
        Some((ce - cs, ce - cs))
    }

    /// Compare the App's report of its field with what was predicted.
    pub(crate) fn sync(&mut self, app: &AppText) -> Sync {
        if app.text == self.app.text {
            self.history.clear();
            // While composing the App's caret sits at the composition, and
            // the selection is the input method's.
            if self.composition.is_some() {
                return Sync::InStep;
            }
            self.app.caret = app.caret;
            self.app.anchor = app.anchor;
            let sel = selection_of(app);
            if (self.sel.start, self.sel.end) != (sel.start, sel.end) {
                self.sel = sel;
                return Sync::Selection;
            }
            return Sync::InStep;
        }
        if self.history.contains(app) {
            return Sync::Stale;
        }
        // While composing, a field that shows the composition as its text
        // (one that doesn't mark it as preedit) is reporting what the input
        // method wrote: not a change of its own.
        if self.composition.is_some() && app.text == self.text {
            self.history.clear();
            return Sync::InStep;
        }
        Sync::Changed {
            composing: self.composition.is_some(),
        }
    }

    /// Take the App's field as the document (after [`Sync::Changed`], or when
    /// a new field takes focus). Returns the document's old and new lengths
    /// for `OnTextChange`, and the event that clears a preedit still shown.
    pub(crate) fn reset(&mut self, app: AppText) -> ((usize, usize), Option<Ime>) {
        let old = self.text.len();
        self.text = app.text.clone();
        self.sel = selection_of(&app);
        self.app = app;
        self.history.clear();
        self.composition = None;
        self.attrs.clear();
        self.discard = false;
        let clear = self.preedit.take().map(|_| Ime::Preedit {
            text: String::new(),
            cursor: None,
        });
        ((old, self.text.len()), clear)
    }
}

/// The App's caret and anchor as a selection.
fn selection_of(app: &AppText) -> Selection {
    let (start, end) = (app.caret.min(app.anchor), app.caret.max(app.anchor));
    Selection {
        start,
        end,
        active: if app.caret < app.anchor {
            ActiveEnd::Start
        } else {
            ActiveEnd::End
        },
    }
}

/// One edit around the App's caret: delete `before` and `after` bytes, then
/// insert `insert`.
#[derive(Debug, PartialEq, Eq)]
struct Edit {
    before: usize,
    after: usize,
    insert: String,
}

/// The edit that turns `app` into `desired` with the caret ending as close to
/// `want` (an offset in `desired`) as can be: the App only edits around its
/// caret, and its caret ends after what it inserts. Returns the edit and where
/// the caret ends.
fn diff(app: &AppText, desired: &[u16], want: usize) -> (Edit, usize) {
    let a = &app.text;
    let c = app.caret.min(a.len());
    let prefix = a.iter().zip(desired).take_while(|(x, y)| x == y).count();
    let max_suffix = a.len().min(desired.len()) - prefix;
    let suffix = a
        .iter()
        .rev()
        .zip(desired.iter().rev())
        .take(max_suffix)
        .take_while(|(x, y)| x == y)
        .count();
    // Keep `a[k..]`, which is `desired[p..]`: k at or after the caret, and
    // inside the common suffix. Pick it to put p near `want`.
    let lo = c.max(a.len() - suffix);
    let k = (a.len() + want)
        .saturating_sub(desired.len())
        .clamp(lo, a.len());
    let p = desired.len() + k - a.len();
    // Keep `a[..j]`, which is `desired[..j]`.
    let j = prefix.min(c).min(p);
    let bytes = |s: &[u16]| String::from_utf16_lossy(s).len();
    let edit = Edit {
        before: bytes(&a[j..c]),
        after: bytes(&a[c..k]),
        insert: String::from_utf16_lossy(&desired[j..p]),
    };
    (edit, p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    /// A store holding `text` with the caret at `caret` (UTF-16), in step
    /// with the App.
    fn store(text: &str, caret: usize) -> TextStore {
        let mut s = TextStore::new();
        s.reset(AppText {
            text: u(text),
            caret,
            anchor: caret,
        });
        s
    }

    /// Apply events as `EditBuffer` does, to check they reach the document.
    fn apply(app: &mut (String, usize, Option<String>), events: &[Ime]) {
        for e in events {
            match e {
                Ime::DeleteSurrounding { before, after } => {
                    let (t, c, _) = app;
                    let start = *c - before;
                    t.replace_range(start..*c + after, "");
                    *c = start;
                }
                Ime::Commit(s) => {
                    app.0.insert_str(app.1, s);
                    app.1 += s.len();
                    app.2 = None;
                }
                Ime::Preedit { text, .. } => {
                    app.2 = (!text.is_empty()).then(|| text.clone());
                }
                Ime::SetSelection { .. } => {}
            }
        }
    }

    fn write<R>(s: &mut TextStore, f: impl FnOnce(&mut TextStore) -> R) -> R {
        assert_eq!(
            s.request_lock(LOCK_READWRITE),
            LockGrant::Now(LOCK_READWRITE)
        );
        let r = f(s);
        assert_eq!(s.release_lock(), None);
        r
    }

    #[test]
    fn locks_are_granted_queued_or_refused_without_waiting() {
        let mut s = TextStore::new();
        assert_eq!(s.len(), Err(StoreError::NoLock));
        assert_eq!(
            s.request_lock(LOCK_READ | LOCK_SYNC),
            LockGrant::Now(LOCK_READ)
        );
        assert_eq!(s.len(), Ok(0));
        // Reading doesn't allow writing.
        assert_eq!(s.set_text(0, 0, &u("x")), Err(StoreError::NoLock));
        // Held: synchronous requests are refused, asynchronous ones queue.
        assert_eq!(
            s.request_lock(LOCK_READWRITE | LOCK_SYNC),
            LockGrant::Refused
        );
        assert_eq!(s.request_lock(LOCK_READ), LockGrant::Queued);
        assert_eq!(s.request_lock(LOCK_READWRITE), LockGrant::Queued);
        // On release the strongest queued lock is granted, once.
        assert_eq!(s.release_lock(), Some(LOCK_READWRITE));
        assert!(s.locked());
        assert!(s.set_text(0, 0, &u("x")).is_ok());
        assert_eq!(s.release_lock(), None);
        assert!(!s.locked());
        // An asynchronous request with nothing held is granted at once.
        assert_eq!(s.request_lock(LOCK_READ), LockGrant::Now(LOCK_READ));
    }

    #[test]
    fn acp_offsets_text_and_selection() {
        let mut s = store("a😀b", 3);
        write(&mut s, |s| {
            assert_eq!(s.len(), Ok(4));
            assert_eq!(s.text(0, None, 100).unwrap(), &u("a😀b")[..]);
            assert_eq!(s.text(1, Some(3), 100).unwrap(), &u("😀")[..]);
            assert_eq!(s.text(1, None, 1).unwrap().len(), 1);
            assert_eq!(s.text(5, None, 1), Err(StoreError::InvalidPos));
            assert_eq!(s.text(2, Some(1), 1), Err(StoreError::InvalidPos));
            let sel = s.selection().unwrap();
            assert_eq!((sel.start, sel.end), (3, 3));
            assert_eq!(
                s.set_selection(0, 9, ActiveEnd::End),
                Err(StoreError::InvalidPos)
            );
            s.set_selection(0, 1, ActiveEnd::Start).unwrap();
            assert_eq!(s.selection().unwrap().active, ActiveEnd::Start);
            assert_eq!(s.query_insert(1, 3), Ok((1, 3)));
            assert_eq!(s.query_insert(1, 7), Err(StoreError::InvalidPos));
        });
        // The App's byte offsets become code units.
        let app = AppText::from_state("a😀b", 5, 1);
        assert_eq!((app.caret, app.anchor), (3, 1));
    }

    #[test]
    fn insert_and_replace_outside_a_composition_commit() {
        // Voice typing or the touch keyboard: text inserted at the caret.
        let mut s = store("hello ", 6);
        let mut app = ("hello ".to_string(), 6, None);
        let (start, end, change) =
            write(&mut s, |s| s.insert_at_selection(&u("world"), false)).unwrap();
        assert_eq!((start, end), (6, 11));
        assert_eq!(
            change,
            Some(TextChange {
                start: 6,
                old_end: 6,
                new_end: 11
            })
        );
        let ev = s.flush();
        assert_eq!(ev, vec![Ime::Commit("world".into())]);
        apply(&mut app, &ev);
        assert_eq!(app.0, "hello world");
        // A correction replacing the word before the caret.
        write(&mut s, |s| s.set_text(6, 11, &u("there")).unwrap());
        let ev = s.flush();
        assert_eq!(
            ev,
            vec![
                Ime::DeleteSurrounding {
                    before: 5,
                    after: 0
                },
                Ime::Commit("there".into())
            ]
        );
        apply(&mut app, &ev);
        assert_eq!(app.0, "hello there");
        // Nothing changed: nothing to send.
        assert!(s.flush().is_empty());
        // A query doesn't change anything.
        let q = write(&mut s, |s| s.insert_at_selection(&u("x"), true)).unwrap();
        assert_eq!(q, (11, 11, None));
    }

    #[test]
    fn a_composition_is_a_preedit_until_it_ends() {
        let mut s = store("ab", 2);
        let mut app = ("ab".to_string(), 2, None);
        // The IME starts composing at the caret and types a reading.
        write(&mut s, |s| {
            s.start_composition(Some((2, 2)));
            s.set_text(2, 2, &u("に")).unwrap();
        });
        let ev = s.flush();
        assert_eq!(
            ev,
            vec![Ime::Preedit {
                text: "に".into(),
                cursor: Some((3, 3))
            }]
        );
        apply(&mut app, &ev);
        write(&mut s, |s| {
            s.set_text(2, 3, &u("にほん")).unwrap();
        });
        let ev = s.flush();
        assert_eq!(
            ev,
            vec![Ime::Preedit {
                text: "にほん".into(),
                cursor: Some((9, 9))
            }]
        );
        // The App's report (preedit not included, caret at its start) is in step.
        assert_eq!(s.sync(&AppText::from_state("ab", 2, 2)), Sync::InStep);
        // Converted: the target clause is highlighted.
        write(&mut s, |s| {
            s.set_text(2, 5, &u("日本")).unwrap();
            s.set_attributes(vec![(2, 4, ATTR_TARGET_CONVERTED)]);
        });
        let ev = s.flush();
        assert_eq!(
            ev,
            vec![Ime::Preedit {
                text: "日本".into(),
                cursor: Some((0, 6))
            }]
        );
        // Committed: the composition ends and its text stays.
        write(&mut s, |s| s.end_composition());
        let ev = s.flush();
        assert_eq!(ev, vec![Ime::Commit("日本".into())]);
        apply(&mut app, &ev);
        assert_eq!(app, ("ab日本".to_string(), 8, None));
        assert_eq!(s.sync(&AppText::from_state("ab日本", 8, 8)), Sync::InStep);
    }

    #[test]
    fn a_cancelled_composition_clears_the_preedit() {
        let mut s = store("", 0);
        write(&mut s, |s| {
            s.start_composition(None);
            s.set_text(0, 0, &u("か")).unwrap();
        });
        assert_eq!(s.flush().len(), 1);
        // Escape: the IME deletes its text and ends the composition.
        write(&mut s, |s| {
            s.set_text(0, 1, &[]).unwrap();
            s.end_composition();
        });
        assert_eq!(
            s.flush(),
            vec![Ime::Preedit {
                text: String::new(),
                cursor: None
            }]
        );
    }

    #[test]
    fn a_partial_commit_and_the_rest_composing() {
        let mut s = store("", 0);
        let mut app = (String::new(), 0, None);
        write(&mut s, |s| {
            s.start_composition(None);
            s.set_text(0, 0, &u("今日は")).unwrap();
        });
        apply(&mut app, &s.flush());
        // The first clause is committed: the composition now starts after it.
        write(&mut s, |s| s.update_composition((2, 3)));
        let ev = s.flush();
        assert_eq!(
            ev,
            vec![
                Ime::Commit("今日".into()),
                Ime::Preedit {
                    text: "は".into(),
                    cursor: Some((3, 3))
                }
            ]
        );
        apply(&mut app, &ev);
        assert_eq!(app, ("今日".to_string(), 6, Some("は".into())));
    }

    #[test]
    fn reconversion_turns_committed_text_back_into_a_preedit() {
        // The IME takes the word before the caret back into a composition.
        let mut s = store("x日本", 3);
        let mut app = ("x日本".to_string(), 7, None);
        write(&mut s, |s| s.start_composition(Some((1, 3))));
        let ev = s.flush();
        assert_eq!(
            ev,
            vec![
                Ime::DeleteSurrounding {
                    before: 6,
                    after: 0
                },
                Ime::Preedit {
                    text: "日本".into(),
                    cursor: Some((6, 6))
                }
            ]
        );
        apply(&mut app, &ev);
        assert_eq!(app, ("x".to_string(), 1, Some("日本".into())));
    }

    #[test]
    fn a_selection_is_replaced() {
        // "hello" with "ell" selected, caret after it: the IME types over it.
        let mut s = TextStore::new();
        s.reset(AppText {
            text: u("hello"),
            caret: 4,
            anchor: 1,
        });
        write(&mut s, |s| {
            assert_eq!(
                s.selection().unwrap(),
                Selection {
                    start: 1,
                    end: 4,
                    active: ActiveEnd::End
                }
            );
            s.insert_at_selection(&u("ipp"), false).unwrap();
        });
        let ev = s.flush();
        assert_eq!(
            ev,
            vec![
                Ime::DeleteSurrounding {
                    before: 3,
                    after: 0
                },
                Ime::Commit("ipp".into())
            ]
        );
    }

    #[test]
    fn reports_in_step_stale_and_changed() {
        let mut s = store("a", 1);
        write(&mut s, |s| s.insert_at_selection(&u("b"), false).unwrap());
        s.flush();
        // A frame drawn before the commit was handled reports the old text.
        assert_eq!(s.sync(&AppText::from_state("a", 1, 1)), Sync::Stale);
        assert_eq!(s.sync(&AppText::from_state("ab", 2, 2)), Sync::InStep);
        // The old text now is a change the App made (an undo, say).
        assert_eq!(
            s.sync(&AppText::from_state("a", 1, 1)),
            Sync::Changed { composing: false }
        );
        let (lens, clear) = s.reset(AppText::from_state("a", 1, 1));
        assert_eq!((lens, clear), ((2, 1), None));
        // The caret moving is a selection change.
        assert_eq!(s.sync(&AppText::from_state("a", 0, 0)), Sync::Selection);
        write(&mut s, |s| {
            assert_eq!(s.selection().unwrap().start, 0);
        });
    }

    #[test]
    fn the_apps_echo_of_a_composition_keeps_it() {
        let mut s = store("", 0);
        write(&mut s, |s| {
            s.start_composition(Some((0, 0)));
            s.set_text(0, 0, &u("にほん")).unwrap();
        });
        assert_eq!(s.flush().len(), 1);
        // The field's report during the composition: its text without the
        // preedit, the caret at the preedit's start.
        assert_eq!(s.sync(&AppText::from_state("", 0, 0)), Sync::InStep);
        // Or with the composition shown as text, by a field that doesn't
        // mark its preedit.
        assert_eq!(s.sync(&AppText::from_state("にほん", 9, 9)), Sync::InStep);
        assert!(s.composing());
        // The composition carries on.
        write(&mut s, |s| s.set_text(0, 3, &u("日本")).unwrap());
        assert_eq!(
            s.flush(),
            vec![Ime::Preedit {
                text: "日本".into(),
                cursor: Some((6, 6))
            }]
        );
    }

    #[test]
    fn focus_moving_mid_composition_drops_it() {
        let mut s = store("", 0);
        write(&mut s, |s| {
            s.start_composition(None);
            s.set_text(0, 0, &u("あ")).unwrap();
        });
        s.flush();
        assert_eq!(
            s.sync(&AppText::from_state("other", 5, 5)),
            Sync::Changed { composing: true }
        );
        // Terminating it: the IME's last edits aren't committed.
        s.discard();
        write(&mut s, |s| s.end_composition());
        assert!(s.flush().is_empty());
        let (lens, clear) = s.reset(AppText::from_state("other", 5, 5));
        assert_eq!(lens, (1, 5));
        assert_eq!(
            clear,
            Some(Ime::Preedit {
                text: String::new(),
                cursor: None
            })
        );
        assert!(!s.composing());
        // The new field's text is the document.
        write(&mut s, |s| {
            assert_eq!(s.text(0, None, 9).unwrap(), &u("other")[..])
        });
    }

    #[test]
    fn edits_keep_the_composition_over_its_text() {
        let mut s = store("abcd", 2);
        write(&mut s, |s| {
            s.start_composition(Some((1, 3)));
            // Inside: it grows.
            s.set_text(2, 2, &u("xy")).unwrap();
            assert_eq!(s.composition, Some((1, 5)));
            // Before: it moves.
            s.set_text(0, 1, &[]).unwrap();
            assert_eq!(s.composition, Some((0, 4)));
            // After: it stays.
            s.set_text(5, 5, &u("z")).unwrap();
            assert_eq!(s.composition, Some((0, 4)));
        });
    }

    #[test]
    fn diff_edits_around_the_caret() {
        let app = AppText {
            text: u("abc"),
            caret: 3,
            anchor: 3,
        };
        // An edit away from the caret is widened to reach it.
        let (edit, caret) = diff(&app, &u("aXc"), 2);
        assert_eq!(
            edit,
            Edit {
                before: 2,
                after: 0,
                insert: "Xc".into()
            }
        );
        assert_eq!(caret, 3);
        // An edit after the caret deletes forward.
        let app = AppText {
            text: u("abc"),
            caret: 0,
            anchor: 0,
        };
        let (edit, caret) = diff(&app, &u("Xbc"), 1);
        assert_eq!(
            edit,
            Edit {
                before: 0,
                after: 1,
                insert: "X".into()
            }
        );
        assert_eq!(caret, 1);
    }
}
