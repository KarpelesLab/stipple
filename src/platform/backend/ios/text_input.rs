//! Input methods on iOS: `UITextInput` on the view.
//!
//! The view is the text input the keyboard talks to while a text field has
//! focus (it is first responder then). Its document is the focused field's
//! text around the caret, from [`Window::set_ime`](crate::platform::Window::set_ime),
//! with the composition (marked text) spliced in at the caret. Positions are
//! UTF-16 offsets into that document, carried by `UITextPosition` and
//! `UITextRange` subclasses built at run time (`StippleTextPosition`,
//! `StippleTextRange`), each offset an instance variable.
//!
//! What the keyboard does arrives as [`Event::Ime`] and keys:
//!
//! - `setMarkedText:selectedRange:` is the composition: [`Ime::Preedit`] (an
//!   empty text ends it);
//! - `insertText:` commits it ([`Ime::Commit`]), or, with nothing marked,
//!   types (`Event::Text`, a newline as Enter), as `UIKeyInput` did;
//! - `unmarkText` commits the marked text as it stands;
//! - `replaceRange:withText:` (autocorrect, QuickType, dictation) deletes the
//!   range around the caret ([`Ime::DeleteSurrounding`]) and commits the new
//!   text;
//! - `setSelectedTextRange:` with a collapsed range (the space bar's trackpad)
//!   moves the caret with arrow keys;
//! - `deleteBackward` is Backspace.
//!
//! `firstRectForRange:` (the candidate bar) and `caretRectForPosition:` come
//! from the caret rectangle `set_ime` reports, in the view's coordinates, which
//! are the app's logical pixels. When the field's text or caret changes other
//! than by the keyboard's own edits, the keyboard's `inputDelegate` is told
//! (`textWillChange:` / `textDidChange:`, `selectionWillChange:` /
//! `selectionDidChange:`), so autocorrect reads the new text.

use std::cell::RefCell;
use std::ffi::{CString, c_void};
use std::sync::atomic::{AtomicIsize, Ordering};

use super::doc::{Doc, document, floor_boundary, spliced, utf16_len};
use super::{
    CgPoint, CgRect, CgSize, Class, Id, Sel, call_handler, class, class_addMethod,
    class_addProtocol, current_window, key_tap, msg_bool, msg_id, nsstring, objc_allocateClassPair,
    objc_getClass, objc_getProtocol, objc_msgSend, objc_registerClassPair, sel, string_of,
};
use crate::platform::ImeState;
use crate::platform::event::{Event, Ime, KeyCode, Modifiers};
use crate::platform::input;

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    fn class_addIvar(cls: Class, name: *const i8, size: usize, align: u8, types: *const i8)
    -> bool;
    fn class_getInstanceVariable(cls: Class, name: *const i8) -> *mut c_void;
    fn ivar_getOffset(ivar: *mut c_void) -> isize;
}

/// `NSRange`, two `NSUInteger`s.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub(super) struct NsRange {
    pub(super) location: usize,
    pub(super) length: usize,
}

/// `NSNotFound`.
const NOT_FOUND: isize = isize::MAX;

// `UITextLayoutDirection` and `UITextStorageDirection` share one parameter.
const DIRECTION_BACKWARD: isize = 1;
const DIRECTION_LEFT: isize = 3;
const DIRECTION_UP: isize = 4;
const DIRECTION_DOWN: isize = 5;

/// What the keyboard sees: the field's state from `set_ime` (updated in
/// place by the keyboard's own edits until the app reports back) and the
/// composition.
#[derive(Default)]
struct TextState {
    ime: Option<ImeState>,
    /// The marked text (composition), empty when not composing.
    marked: String,
    /// The selection within `marked`, in UTF-16 units.
    marked_sel: (usize, usize),
    /// The keyboard's `UITextInputDelegate` (not retained, as UIKit's
    /// `weak` property).
    delegate: Id,
    /// The view's `UITextInputStringTokenizer`, made on first use.
    tokenizer: Id,
}

thread_local! {
    static STATE: RefCell<TextState> = RefCell::new(TextState::default());
}

fn current_doc() -> Doc {
    STATE.with(|s| {
        let s = s.borrow();
        document(s.ime.as_ref(), &s.marked, s.marked_sel)
    })
}

/// Replace the selection with `text` in the state the keyboard sees.
fn splice_selection(text: &str) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if let Some(ime) = &s.ime {
            let (lo, hi) = (ime.cursor.min(ime.anchor), ime.cursor.max(ime.anchor));
            s.ime = Some(spliced(ime, lo, hi, text));
        }
    });
}

fn send(event: Event) {
    call_handler(event, &current_window());
}

// ---- Positions and ranges ----------------------------------------------------

/// Instance-variable offsets of the position and range classes.
static POSITION_OFFSET: AtomicIsize = AtomicIsize::new(0);
static RANGE_START: AtomicIsize = AtomicIsize::new(0);
static RANGE_END: AtomicIsize = AtomicIsize::new(0);

unsafe fn ivar_at(obj: Id, offset: isize) -> *mut i64 {
    unsafe { obj.cast::<u8>().offset(offset).cast::<i64>() }
}

/// A subclass of `superclass` holding `ivars` (`i64`s), registered once.
unsafe fn ivar_class(name: &std::ffi::CStr, superclass: &str, ivars: &[&std::ffi::CStr]) -> Class {
    unsafe {
        let existing = objc_getClass(name.as_ptr());
        if !existing.is_null() {
            return existing;
        }
        let cls = objc_allocateClassPair(class(superclass), name.as_ptr(), 0);
        for ivar in ivars {
            class_addIvar(cls, ivar.as_ptr(), 8, 3, c"q".as_ptr());
        }
        cls
    }
}

fn position_class() -> Class {
    let name = c"StippleTextPosition";
    unsafe {
        let existing = objc_getClass(name.as_ptr());
        if !existing.is_null() {
            return existing;
        }
        let cls = ivar_class(name, "UITextPosition", &[c"offset"]);
        objc_registerClassPair(cls);
        let ivar = class_getInstanceVariable(cls, c"offset".as_ptr());
        POSITION_OFFSET.store(ivar_getOffset(ivar), Ordering::Relaxed);
        cls
    }
}

fn range_class() -> Class {
    let name = c"StippleTextRange";
    unsafe {
        let existing = objc_getClass(name.as_ptr());
        if !existing.is_null() {
            return existing;
        }
        let cls = ivar_class(name, "UITextRange", &[c"start", c"end"]);
        let methods: [(&str, *const c_void, &std::ffi::CStr); 3] = [
            ("start", range_start as *const c_void, c"@@:"),
            ("end", range_end as *const c_void, c"@@:"),
            ("isEmpty", range_is_empty as *const c_void, c"B@:"),
        ];
        for (name, imp, types) in methods {
            class_addMethod(cls, sel(name), imp, types.as_ptr());
        }
        objc_registerClassPair(cls);
        for (ivar, slot) in [(c"start", &RANGE_START), (c"end", &RANGE_END)] {
            let ivar = class_getInstanceVariable(cls, ivar.as_ptr());
            slot.store(ivar_getOffset(ivar), Ordering::Relaxed);
        }
        cls
    }
}

/// A new (autoreleased) `StippleTextPosition` at `offset`.
fn position(offset: usize) -> Id {
    unsafe {
        let p = msg_id(msg_id(position_class(), sel("alloc")), sel("init"));
        *ivar_at(p, POSITION_OFFSET.load(Ordering::Relaxed)) = offset as i64;
        msg_id(p, sel("autorelease"))
    }
}

/// A new (autoreleased) `StippleTextRange` from `a` to `b` (in either order).
fn range(a: usize, b: usize) -> Id {
    let (start, end) = (a.min(b), a.max(b));
    unsafe {
        let r = msg_id(msg_id(range_class(), sel("alloc")), sel("init"));
        *ivar_at(r, RANGE_START.load(Ordering::Relaxed)) = start as i64;
        *ivar_at(r, RANGE_END.load(Ordering::Relaxed)) = end as i64;
        msg_id(r, sel("autorelease"))
    }
}

unsafe fn is_kind(obj: Id, cls: Class) -> bool {
    if obj.is_null() {
        return false;
    }
    let f: unsafe extern "C" fn(Id, Sel, Class) -> i8 =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    unsafe { f(obj, sel("isKindOfClass:"), cls) != 0 }
}

/// The offset a position carries (0 for one that isn't ours).
fn offset_of(p: Id) -> usize {
    unsafe {
        if !is_kind(p, position_class()) {
            return 0;
        }
        (*ivar_at(p, POSITION_OFFSET.load(Ordering::Relaxed))).max(0) as usize
    }
}

/// The offsets a range carries (`None` for one that isn't ours).
fn range_of(r: Id) -> Option<(usize, usize)> {
    unsafe {
        if !is_kind(r, range_class()) {
            return None;
        }
        let start = (*ivar_at(r, RANGE_START.load(Ordering::Relaxed))).max(0) as usize;
        let end = (*ivar_at(r, RANGE_END.load(Ordering::Relaxed))).max(0) as usize;
        Some((start, end.max(start)))
    }
}

extern "C" fn range_start(this: Id, _cmd: Sel) -> Id {
    position(range_of(this).map_or(0, |r| r.0))
}

extern "C" fn range_end(this: Id, _cmd: Sel) -> Id {
    position(range_of(this).map_or(0, |r| r.1))
}

extern "C" fn range_is_empty(this: Id, _cmd: Sel) -> bool {
    range_of(this).is_none_or(|(a, b)| a == b)
}

// ---- UIKeyInput --------------------------------------------------------------

/// `hasText`: always, so Backspace on an empty-looking field still reaches the
/// app (UIKit only sends `deleteBackward` when there's text).
extern "C" fn has_text(_this: Id, _cmd: Sel) -> bool {
    true
}

/// Commit the marked text: it becomes part of the field's text.
fn commit(text: String) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.marked.clear();
        s.marked_sel = (0, 0);
    });
    splice_selection(&text);
    send(Event::Ime(Ime::Commit(text)));
}

/// `insertText:`, from the on-screen keyboard (and a hardware one): commits a
/// composition, or types. Return arrives as a newline and becomes Enter.
extern "C" fn insert_text(_this: Id, _cmd: Sel, text: Id) {
    let text = unsafe { string_of(text) };
    let composing = STATE.with(|s| !s.borrow().marked.is_empty());
    let mut lines = text.split('\n');
    let first = lines.next().unwrap_or_default().to_string();
    if composing {
        // The commit replaces the composition, even with nothing.
        commit(first);
    } else if !first.is_empty() {
        splice_selection(&first);
        send(Event::Text(first));
    }
    let win = current_window();
    for line in lines {
        key_tap(KeyCode::Enter, Modifiers::default(), &win);
        if !line.is_empty() {
            splice_selection(line);
            send(Event::Text(line.to_string()));
        }
    }
}

/// `deleteBackward`: the keyboard's delete key.
extern "C" fn delete_backward(_this: Id, _cmd: Sel) {
    let composing = STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.marked.is_empty() {
            return None;
        }
        s.marked.pop();
        let len = utf16_len(&s.marked);
        s.marked_sel = (len, len);
        Some(s.marked.clone())
    });
    if let Some(text) = composing {
        let at = text.len();
        let cursor = (!text.is_empty()).then_some((at, at));
        send(Event::Ime(Ime::Preedit { text, cursor }));
        return;
    }
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if let Some(ime) = &s.ime {
            let (lo, hi) = (ime.cursor.min(ime.anchor), ime.cursor.max(ime.anchor));
            let lo = if lo == hi {
                ime.surrounding[..floor_boundary(&ime.surrounding, lo)]
                    .char_indices()
                    .next_back()
                    .map_or(0, |(i, _)| i)
            } else {
                lo
            };
            s.ime = Some(spliced(ime, lo, hi, ""));
        }
    });
    key_tap(KeyCode::Backspace, Modifiers::default(), &current_window());
}

// ---- UITextInput: the composition ---------------------------------------------

/// The text of an `NSString` or `NSAttributedString` argument.
unsafe fn string_arg(s: Id) -> String {
    if s.is_null() {
        return String::new();
    }
    let responds: unsafe extern "C" fn(Id, Sel, Sel) -> i8 =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    let s = if unsafe { responds(s, sel("respondsToSelector:"), sel("string")) } != 0 {
        unsafe { msg_id(s, sel("string")) }
    } else {
        s
    };
    unsafe { string_of(s) }
}

extern "C" fn set_marked_text(_this: Id, _cmd: Sel, text: Id, selected: NsRange) {
    let text = unsafe { string_arg(text) };
    let len = utf16_len(&text);
    let sel_start = selected.location.min(len);
    let sel_end = selected.location.saturating_add(selected.length).min(len);
    let was = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.marked_sel = (sel_start, sel_end);
        std::mem::replace(&mut s.marked, text.clone())
    });
    if text.is_empty() {
        if !was.is_empty() {
            send(Event::Ime(Ime::Preedit { text, cursor: None }));
        }
        return;
    }
    let start = input::utf16_to_byte(&text, sel_start);
    let end = input::utf16_to_byte(&text, sel_end);
    send(Event::Ime(Ime::Preedit {
        text,
        cursor: Some((start, end)),
    }));
}

extern "C" fn unmark_text(_this: Id, _cmd: Sel) {
    let marked = STATE.with(|s| s.borrow().marked.clone());
    if !marked.is_empty() {
        commit(marked);
    }
}

extern "C" fn marked_text_range(_this: Id, _cmd: Sel) -> Id {
    match current_doc().marked {
        Some((a, b)) => range(a, b),
        None => std::ptr::null_mut(),
    }
}

extern "C" fn marked_text_style(_this: Id, _cmd: Sel) -> Id {
    std::ptr::null_mut()
}

extern "C" fn set_marked_text_style(_this: Id, _cmd: Sel, _style: Id) {}

// ---- UITextInput: the document -------------------------------------------------

/// The bytes of `doc.text` between UTF-16 offsets `a` and `b`.
fn byte_range(doc: &Doc, a: usize, b: usize) -> (usize, usize) {
    let (a, b) = (a.min(b), a.max(b));
    (
        input::utf16_to_byte(&doc.text, a),
        input::utf16_to_byte(&doc.text, b),
    )
}

extern "C" fn text_in_range(_this: Id, _cmd: Sel, r: Id) -> Id {
    let doc = current_doc();
    let Some((a, b)) = range_of(r) else {
        return std::ptr::null_mut();
    };
    let (a, b) = byte_range(&doc, a, b);
    nsstring(&doc.text[a..b])
}

/// `replaceRange:withText:`: autocorrect, QuickType and dictation replacing
/// a word. Expressed as deleting around the caret and committing, which only
/// works for a range that touches the caret; others are ignored.
extern "C" fn replace_range(_this: Id, _cmd: Sel, r: Id, text: Id) {
    let text = unsafe { string_arg(text) };
    let Some((a, b)) = range_of(r) else {
        return;
    };
    let doc = current_doc();
    if let Some(marked) = doc.marked {
        // Replacing the composition commits the new text in its place.
        if (a, b) == marked {
            commit(text);
        }
        return;
    }
    let (lo, hi) = doc.sel;
    let (a_byte, b_byte) = byte_range(&doc, a, b);
    let (lo_byte, hi_byte) = byte_range(&doc, lo, hi);
    if lo != hi {
        // A selection is replaced by typing over it; a range that covers it
        // also deletes around it.
        if a > lo || b < hi {
            return;
        }
        STATE.with(|s| {
            if let Some(ime) = &mut s.borrow_mut().ime {
                ime.cursor = lo_byte.min(ime.surrounding.len());
                ime.anchor = ime.cursor;
            }
        });
        if (a, b) != (lo, hi) {
            send(Event::Ime(Ime::DeleteSurrounding {
                before: lo_byte - a_byte,
                after: b_byte - hi_byte,
            }));
        }
    } else {
        if a > lo || b < lo {
            return;
        }
        send(Event::Ime(Ime::DeleteSurrounding {
            before: lo_byte - a_byte,
            after: b_byte - lo_byte,
        }));
    }
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if let Some(ime) = &s.ime {
            s.ime = Some(spliced(ime, a_byte, b_byte, &text));
        }
    });
    if !text.is_empty() {
        send(Event::Ime(Ime::Commit(text)));
    }
}

extern "C" fn selected_text_range(_this: Id, _cmd: Sel) -> Id {
    let (a, b) = current_doc().sel;
    range(a, b)
}

/// `setSelectedTextRange:`: the keyboard moving the caret (dragging on the
/// space bar). A collapsed range moves the caret by arrow keys; selections
/// and moves while composing are left to the app.
extern "C" fn set_selected_text_range(_this: Id, _cmd: Sel, r: Id) {
    let Some((a, b)) = range_of(r) else {
        return;
    };
    let doc = current_doc();
    if a != b || doc.marked.is_some() || doc.sel.0 != doc.sel.1 || a == doc.sel.0 {
        return;
    }
    let (from, to) = byte_range(&doc, doc.sel.0, a);
    let steps = doc.text[from..to].chars().count();
    let key = if a < doc.sel.0 {
        KeyCode::ArrowLeft
    } else {
        KeyCode::ArrowRight
    };
    let at = input::utf16_to_byte(&doc.text, a);
    STATE.with(|s| {
        if let Some(ime) = &mut s.borrow_mut().ime {
            ime.cursor = at.min(ime.surrounding.len());
            ime.anchor = ime.cursor;
        }
    });
    let win = current_window();
    for _ in 0..steps {
        key_tap(key, Modifiers::default(), &win);
    }
}

extern "C" fn beginning_of_document(_this: Id, _cmd: Sel) -> Id {
    position(0)
}

extern "C" fn end_of_document(_this: Id, _cmd: Sel) -> Id {
    position(utf16_len(&current_doc().text))
}

extern "C" fn text_range_from_positions(_this: Id, _cmd: Sel, from: Id, to: Id) -> Id {
    if from.is_null() || to.is_null() {
        return std::ptr::null_mut();
    }
    range(offset_of(from), offset_of(to))
}

/// The position `by` UTF-16 units from `p`, or nil outside the document.
fn moved(p: Id, by: isize) -> Id {
    if p.is_null() {
        return std::ptr::null_mut();
    }
    let len = utf16_len(&current_doc().text) as isize;
    let to = offset_of(p) as isize + by;
    if (0..=len).contains(&to) {
        position(to as usize)
    } else {
        std::ptr::null_mut()
    }
}

extern "C" fn position_from_offset(_this: Id, _cmd: Sel, p: Id, by: isize) -> Id {
    moved(p, by)
}

extern "C" fn position_in_direction(
    _this: Id,
    _cmd: Sel,
    p: Id,
    direction: isize,
    by: isize,
) -> Id {
    match direction {
        // A single line: up is the start, down the end.
        DIRECTION_UP => position(0),
        DIRECTION_DOWN => position(utf16_len(&current_doc().text)),
        DIRECTION_LEFT | DIRECTION_BACKWARD => moved(p, -by),
        _ => moved(p, by),
    }
}

extern "C" fn compare_positions(_this: Id, _cmd: Sel, a: Id, b: Id) -> isize {
    match offset_of(a).cmp(&offset_of(b)) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

extern "C" fn offset_between(_this: Id, _cmd: Sel, a: Id, b: Id) -> isize {
    offset_of(b) as isize - offset_of(a) as isize
}

extern "C" fn input_delegate(_this: Id, _cmd: Sel) -> Id {
    STATE.with(|s| s.borrow().delegate)
}

extern "C" fn set_input_delegate(_this: Id, _cmd: Sel, delegate: Id) {
    STATE.with(|s| s.borrow_mut().delegate = delegate);
}

extern "C" fn tokenizer(this: Id, _cmd: Sel) -> Id {
    let existing = STATE.with(|s| s.borrow().tokenizer);
    if !existing.is_null() {
        return existing;
    }
    let made = unsafe {
        let alloc = msg_id(class("UITextInputStringTokenizer"), sel("alloc"));
        let init: unsafe extern "C" fn(Id, Sel, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        init(alloc, sel("initWithTextInput:"), this)
    };
    STATE.with(|s| s.borrow_mut().tokenizer = made);
    made
}

extern "C" fn position_within_range(_this: Id, _cmd: Sel, r: Id, direction: isize) -> Id {
    let Some((a, b)) = range_of(r) else {
        return std::ptr::null_mut();
    };
    match direction {
        DIRECTION_LEFT | DIRECTION_UP | DIRECTION_BACKWARD => position(a),
        _ => position(b),
    }
}

extern "C" fn character_range_by_extending(_this: Id, _cmd: Sel, p: Id, direction: isize) -> Id {
    if p.is_null() {
        return std::ptr::null_mut();
    }
    let at = offset_of(p);
    let len = utf16_len(&current_doc().text);
    match direction {
        DIRECTION_LEFT | DIRECTION_UP | DIRECTION_BACKWARD => range(at.saturating_sub(1), at),
        _ => range(at, (at + 1).min(len)),
    }
}

/// `NSWritingDirectionNatural`.
extern "C" fn base_writing_direction(_this: Id, _cmd: Sel, _p: Id, _direction: isize) -> isize {
    -1
}

extern "C" fn set_base_writing_direction(_this: Id, _cmd: Sel, _direction: isize, _r: Id) {}

// ---- UITextInput: geometry ---------------------------------------------------------

/// The caret rectangle `set_ime` reported, in the view's coordinates (the
/// app's logical pixels).
fn caret_area() -> CgRect {
    let area = STATE.with(|s| s.borrow().ime.as_ref().map(|i| i.cursor_area));
    match area {
        Some(r) => CgRect {
            origin: CgPoint {
                x: r.min_x(),
                y: r.min_y(),
            },
            size: CgSize {
                width: r.width(),
                height: r.height(),
            },
        },
        None => CgRect {
            origin: CgPoint { x: 0.0, y: 0.0 },
            size: CgSize {
                width: 0.0,
                height: 0.0,
            },
        },
    }
}

/// `firstRectForRange:`: where the candidate bar goes. The field reports
/// only the caret (the start of the composition), so that is the answer.
extern "C" fn first_rect_for_range(_this: Id, _cmd: Sel, _r: Id) -> CgRect {
    caret_area()
}

extern "C" fn caret_rect_for_position(_this: Id, _cmd: Sel, _p: Id) -> CgRect {
    let mut r = caret_area();
    r.size.width = r.size.width.clamp(1.0, 2.0);
    r
}

extern "C" fn selection_rects_for_range(_this: Id, _cmd: Sel, _r: Id) -> Id {
    unsafe { msg_id(class("NSArray"), sel("array")) }
}

extern "C" fn closest_position_to_point(_this: Id, _cmd: Sel, _point: CgPoint) -> Id {
    position(current_doc().sel.1)
}

extern "C" fn closest_position_within_range(_this: Id, _cmd: Sel, _point: CgPoint, r: Id) -> Id {
    let caret = current_doc().sel.1;
    match range_of(r) {
        Some((a, b)) => position(caret.clamp(a, b)),
        None => position(caret),
    }
}

extern "C" fn character_range_at_point(_this: Id, _cmd: Sel, _point: CgPoint) -> Id {
    let caret = current_doc().sel.1;
    range(caret, caret)
}

// ---- Wiring ------------------------------------------------------------------------

/// Make the view class a `UITextInput` (which includes `UIKeyInput`).
///
/// # Safety
/// `cls` must be a class pair still being built (before
/// `objc_registerClassPair`).
pub(super) unsafe fn add_methods(cls: Class) {
    let rect = "{CGRect={CGPoint=dd}{CGSize=dd}}";
    let point = "{CGPoint=dd}";
    let methods: Vec<(&str, *const c_void, String)> = vec![
        ("hasText", has_text as *const c_void, "B@:".into()),
        ("insertText:", insert_text as *const c_void, "v@:@".into()),
        (
            "deleteBackward",
            delete_backward as *const c_void,
            "v@:".into(),
        ),
        (
            "setMarkedText:selectedRange:",
            set_marked_text as *const c_void,
            "v@:@{_NSRange=QQ}".into(),
        ),
        ("unmarkText", unmark_text as *const c_void, "v@:".into()),
        (
            "markedTextRange",
            marked_text_range as *const c_void,
            "@@:".into(),
        ),
        (
            "markedTextStyle",
            marked_text_style as *const c_void,
            "@@:".into(),
        ),
        (
            "setMarkedTextStyle:",
            set_marked_text_style as *const c_void,
            "v@:@".into(),
        ),
        (
            "textInRange:",
            text_in_range as *const c_void,
            "@@:@".into(),
        ),
        (
            "replaceRange:withText:",
            replace_range as *const c_void,
            "v@:@@".into(),
        ),
        (
            "selectedTextRange",
            selected_text_range as *const c_void,
            "@@:".into(),
        ),
        (
            "setSelectedTextRange:",
            set_selected_text_range as *const c_void,
            "v@:@".into(),
        ),
        (
            "beginningOfDocument",
            beginning_of_document as *const c_void,
            "@@:".into(),
        ),
        (
            "endOfDocument",
            end_of_document as *const c_void,
            "@@:".into(),
        ),
        (
            "textRangeFromPosition:toPosition:",
            text_range_from_positions as *const c_void,
            "@@:@@".into(),
        ),
        (
            "positionFromPosition:offset:",
            position_from_offset as *const c_void,
            "@@:@q".into(),
        ),
        (
            "positionFromPosition:inDirection:offset:",
            position_in_direction as *const c_void,
            "@@:@qq".into(),
        ),
        (
            "comparePosition:toPosition:",
            compare_positions as *const c_void,
            "q@:@@".into(),
        ),
        (
            "offsetFromPosition:toPosition:",
            offset_between as *const c_void,
            "q@:@@".into(),
        ),
        (
            "inputDelegate",
            input_delegate as *const c_void,
            "@@:".into(),
        ),
        (
            "setInputDelegate:",
            set_input_delegate as *const c_void,
            "v@:@".into(),
        ),
        ("tokenizer", tokenizer as *const c_void, "@@:".into()),
        (
            "positionWithinRange:farthestInDirection:",
            position_within_range as *const c_void,
            "@@:@q".into(),
        ),
        (
            "characterRangeByExtendingPosition:inDirection:",
            character_range_by_extending as *const c_void,
            "@@:@q".into(),
        ),
        (
            "baseWritingDirectionForPosition:inDirection:",
            base_writing_direction as *const c_void,
            "q@:@q".into(),
        ),
        (
            "setBaseWritingDirection:forRange:",
            set_base_writing_direction as *const c_void,
            "v@:q@".into(),
        ),
        (
            "firstRectForRange:",
            first_rect_for_range as *const c_void,
            format!("{rect}@:@"),
        ),
        (
            "caretRectForPosition:",
            caret_rect_for_position as *const c_void,
            format!("{rect}@:@"),
        ),
        (
            "selectionRectsForRange:",
            selection_rects_for_range as *const c_void,
            "@@:@".into(),
        ),
        (
            "closestPositionToPoint:",
            closest_position_to_point as *const c_void,
            format!("@@:{point}"),
        ),
        (
            "closestPositionToPoint:withinRange:",
            closest_position_within_range as *const c_void,
            format!("@@:{point}@"),
        ),
        (
            "characterRangeAtPoint:",
            character_range_at_point as *const c_void,
            format!("@@:{point}"),
        ),
    ];
    for (name, imp, types) in methods {
        let types = CString::new(types).unwrap();
        unsafe { class_addMethod(cls, sel(name), imp, types.as_ptr()) };
    }
    // The position and range classes, before the keyboard asks for one.
    position_class();
    range_class();
    for protocol in [c"UIKeyInput", c"UITextInput"] {
        let p = unsafe { objc_getProtocol(protocol.as_ptr()) };
        if !p.is_null() {
            unsafe { class_addProtocol(cls, p) };
        }
    }
}

/// Tell the keyboard's input delegate about a change it didn't make.
fn notify_delegate(text: bool) {
    let (delegate, view) = (
        STATE.with(|s| s.borrow().delegate),
        super::CTX.with(|c| c.borrow().view),
    );
    if delegate.is_null() || view.is_null() {
        return;
    }
    let (will, did) = if text {
        ("textWillChange:", "textDidChange:")
    } else {
        ("selectionWillChange:", "selectionDidChange:")
    };
    unsafe {
        super::msg_void_id(delegate, sel(will), view);
        super::msg_void_id(delegate, sel(did), view);
    }
}

/// Record the focused field (or none), from `Window::set_ime`. Losing it
/// mid-composition drops the marked text (the app drops its preedit); a
/// change the keyboard didn't make itself is passed on to its delegate.
pub(super) fn set_state(state: Option<&ImeState>) {
    let (text_changed, selection_changed, dropped) = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let old = s.ime.take();
        s.ime = state.cloned();
        let dropped = state.is_none() && !s.marked.is_empty();
        if dropped {
            s.marked.clear();
            s.marked_sel = (0, 0);
        }
        let composing = !s.marked.is_empty();
        let text = old.as_ref().map(|o| &o.surrounding) != state.map(|n| &n.surrounding);
        let selection =
            old.as_ref().map(|o| (o.cursor, o.anchor)) != state.map(|n| (n.cursor, n.anchor));
        (text && !composing, selection && !composing, dropped)
    });
    if text_changed || dropped {
        notify_delegate(true);
    } else if selection_changed {
        notify_delegate(false);
    }
}

/// For self-tests: the text input as the keyboard sees it, read through objc
/// dispatch on `view`.
///
/// # Safety
/// `view` must be the live `StippleUIView`.
pub(super) unsafe fn status(view: Id) -> String {
    unsafe {
        let protocol = objc_getProtocol(c"UITextInput".as_ptr());
        let conforms = {
            let f: unsafe extern "C" fn(Id, Sel, *mut c_void) -> i8 =
                std::mem::transmute(objc_msgSend as *const c_void);
            !protocol.is_null() && f(view, sel("conformsToProtocol:"), protocol) != 0
        };
        let two: unsafe extern "C" fn(Id, Sel, Id, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let offset: unsafe extern "C" fn(Id, Sel, Id, Id) -> isize =
            std::mem::transmute(objc_msgSend as *const c_void);
        let one: unsafe extern "C" fn(Id, Sel, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let rect_of: unsafe extern "C" fn(Id, Sel, Id) -> CgRect =
            std::mem::transmute(objc_msgSend as *const c_void);
        let begin = msg_id(view, sel("beginningOfDocument"));
        let end = msg_id(view, sel("endOfDocument"));
        let all = two(view, sel("textRangeFromPosition:toPosition:"), begin, end);
        let doc = string_of(one(view, sel("textInRange:"), all));
        let selected = msg_id(view, sel("selectedTextRange"));
        let (s0, s1) = if selected.is_null() {
            (NOT_FOUND, NOT_FOUND)
        } else {
            (
                offset(
                    view,
                    sel("offsetFromPosition:toPosition:"),
                    begin,
                    msg_id(selected, sel("start")),
                ),
                offset(
                    view,
                    sel("offsetFromPosition:toPosition:"),
                    begin,
                    msg_id(selected, sel("end")),
                ),
            )
        };
        let marked_range = msg_id(view, sel("markedTextRange"));
        let marked = if marked_range.is_null() {
            "none".to_string()
        } else {
            format!(
                "{:?}",
                string_of(one(view, sel("textInRange:"), marked_range))
            )
        };
        let r = rect_of(
            view,
            sel("firstRectForRange:"),
            if marked_range.is_null() {
                selected
            } else {
                marked_range
            },
        );
        let caret = rect_of(
            view,
            sel("caretRectForPosition:"),
            if selected.is_null() {
                end
            } else {
                msg_id(selected, sel("end"))
            },
        );
        format!(
            "text_input={conforms} doc={doc:?} selected={s0}..{s1} marked={marked} has_text={} \
             first_rect=({:.0},{:.0},{:.0},{:.0}) caret=({:.0},{:.0},{:.0},{:.0})",
            msg_bool(view, sel("hasText")),
            r.origin.x,
            r.origin.y,
            r.size.width,
            r.size.height,
            caret.origin.x,
            caret.origin.y,
            caret.size.width,
            caret.size.height,
        )
    }
}

/// For self-tests: `[view replaceRange:(start..end) withText:text]`, the
/// range built through the view's own position methods.
///
/// # Safety
/// `view` must be the live `StippleUIView`.
pub(super) unsafe fn replace(view: Id, start: usize, end: usize, text: &str) {
    unsafe {
        let from: unsafe extern "C" fn(Id, Sel, Id, isize) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let two: unsafe extern "C" fn(Id, Sel, Id, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let begin = msg_id(view, sel("beginningOfDocument"));
        let a = from(
            view,
            sel("positionFromPosition:offset:"),
            begin,
            start as isize,
        );
        let b = from(
            view,
            sel("positionFromPosition:offset:"),
            begin,
            end as isize,
        );
        let r = two(view, sel("textRangeFromPosition:toPosition:"), a, b);
        if !r.is_null() {
            two(view, sel("replaceRange:withText:"), r, nsstring(text));
        }
    }
}
