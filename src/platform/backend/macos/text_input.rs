//! Input methods on macOS: `NSTextInputClient` on the view.
//!
//! While a text field has focus, key-downs go through
//! `-interpretKeyEvents:`, which hands them to the input context (the IME, or
//! the keyboard layout's dead keys). It answers by calling the view back:
//! `setMarkedText:selectedRange:replacementRange:` for the composition,
//! `insertText:replacementRange:` for final text, and `doCommandBySelector:`
//! for keys that aren't text (arrows, Return, Backspace), which then take the
//! usual key path. The candidate window asks
//! `firstRectForCharacterRange:actualRange:` where the caret is, which comes
//! from [`Window::set_ime`](crate::platform::Window::set_ime).

use super::{
    CTX, CgPoint, CgRect, Class, Id, Sel, class, class_addMethod, key_event, msg_id, msg_id_id,
    msg_void, msg_void_id, nsstring_to_string, objc_msgSend, responds_to, sel,
};
use crate::platform::ImeState;
use crate::platform::event::{ButtonState, Event, Ime};
use crate::platform::input;
use std::ffi::c_void;

/// `NSRange`, two `NSUInteger`s.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub(super) struct NsRange {
    location: u64,
    length: u64,
}

/// `NSNotFound` (`NSIntegerMax`).
const NOT_FOUND: u64 = i64::MAX as u64;

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    fn objc_getProtocol(name: *const i8) -> *mut c_void;
    fn class_addProtocol(cls: Class, protocol: *mut c_void) -> bool;
}

/// The number of UTF-16 code units in `s`, as AppKit counts.
fn utf16_len(s: &str) -> u64 {
    s.encode_utf16().count() as u64
}

/// The text of an `NSString` or `NSAttributedString` argument.
unsafe fn string_arg(s: Id) -> String {
    if s.is_null() {
        return String::new();
    }
    let s = if unsafe { responds_to(s, sel("string")) } {
        unsafe { msg_id(s, sel("string")) }
    } else {
        s
    };
    nsstring_to_string(s)
}

fn queue(event: Ime) {
    CTX.with(|c| c.borrow_mut().ime_events.push(Event::Ime(event)));
}

extern "C" fn insert_text(_this: Id, _cmd: Sel, string: Id, _replace: NsRange) {
    let text = unsafe { string_arg(string) };
    let had_marked = CTX.with(|c| !std::mem::take(&mut c.borrow_mut().marked).is_empty());
    if !text.is_empty() && !text.chars().all(char::is_control) {
        queue(Ime::Commit(text));
    } else if had_marked {
        queue(Ime::Preedit {
            text: String::new(),
            cursor: None,
        });
    }
}

extern "C" fn set_marked_text(
    _this: Id,
    _cmd: Sel,
    string: Id,
    selected: NsRange,
    _replace: NsRange,
) {
    let text = unsafe { string_arg(string) };
    let start = input::utf16_to_byte(&text, selected.location as usize);
    let end = input::utf16_to_byte(
        &text,
        selected.location.saturating_add(selected.length) as usize,
    );
    CTX.with(|c| c.borrow_mut().marked = text.clone());
    queue(Ime::Preedit {
        text,
        cursor: Some((start, end)),
    });
}

extern "C" fn unmark_text(_this: Id, _cmd: Sel) {
    let marked = CTX.with(|c| std::mem::take(&mut c.borrow_mut().marked));
    if !marked.is_empty() {
        queue(Ime::Commit(marked));
    }
}

extern "C" fn has_marked_text(_this: Id, _cmd: Sel) -> bool {
    CTX.with(|c| !c.borrow().marked.is_empty())
}

/// The caret's position in the focused field's text, in UTF-16 units.
fn caret_utf16() -> Option<u64> {
    CTX.with(|c| {
        let c = c.borrow();
        let state = c.ime.as_ref()?;
        let at = state.cursor.min(state.surrounding.len());
        state.surrounding.get(..at).map(utf16_len)
    })
}

extern "C" fn marked_range(_this: Id, _cmd: Sel) -> NsRange {
    let len = CTX.with(|c| utf16_len(&c.borrow().marked));
    if len == 0 {
        return NsRange {
            location: NOT_FOUND,
            length: 0,
        };
    }
    NsRange {
        location: caret_utf16().unwrap_or(0),
        length: len,
    }
}

extern "C" fn selected_range(_this: Id, _cmd: Sel) -> NsRange {
    NsRange {
        location: caret_utf16().unwrap_or(NOT_FOUND),
        length: 0,
    }
}

extern "C" fn valid_attributes(_this: Id, _cmd: Sel) -> Id {
    unsafe { msg_id(class("NSArray"), sel("array")) }
}

extern "C" fn attributed_substring(
    _this: Id,
    _cmd: Sel,
    _range: NsRange,
    _actual: *mut NsRange,
) -> Id {
    std::ptr::null_mut()
}

extern "C" fn character_index(_this: Id, _cmd: Sel, _point: CgPoint) -> u64 {
    NOT_FOUND
}

extern "C" fn first_rect(this: Id, _cmd: Sel, _range: NsRange, _actual: *mut NsRange) -> CgRect {
    let area = CTX.with(|c| c.borrow().ime.as_ref().map(|s| s.cursor_area));
    let (window, r) = match (CTX.with(|c| c.borrow().window), area) {
        (w, Some(r)) if !w.is_null() => (w, r),
        _ => {
            return CgRect {
                origin: CgPoint { x: 0.0, y: 0.0 },
                size: super::CgSize {
                    width: 0.0,
                    height: 0.0,
                },
            };
        }
    };
    // The view is flipped, so its rect is top-left based like ours; AppKit
    // converts it to window and then screen coordinates.
    let in_view = CgRect {
        origin: CgPoint {
            x: r.min_x(),
            y: r.min_y(),
        },
        size: super::CgSize {
            width: r.width(),
            height: r.height(),
        },
    };
    unsafe {
        let to_view: unsafe extern "C" fn(Id, Sel, CgRect, Id) -> CgRect =
            std::mem::transmute(objc_msgSend as *const c_void);
        let in_window = to_view(
            this,
            sel("convertRect:toView:"),
            in_view,
            std::ptr::null_mut(),
        );
        let to_screen: unsafe extern "C" fn(Id, Sel, CgRect) -> CgRect =
            std::mem::transmute(objc_msgSend as *const c_void);
        to_screen(window, sel("convertRectToScreen:"), in_window)
    }
}

extern "C" fn do_command(_this: Id, _cmd: Sel, _selector: Sel) {
    CTX.with(|c| c.borrow_mut().ime_command = true);
}

/// Key-downs reach us through the event loop, so the view's own `keyDown:`
/// does nothing (`NSView`'s would beep).
extern "C" fn key_down_noop(_this: Id, _cmd: Sel, _event: Id) {}

extern "C" fn accepts_first_responder(_this: Id, _cmd: Sel) -> bool {
    true
}

/// Make the view class an `NSTextInputClient`.
///
/// # Safety
/// `cls` must be a class pair still being built (before
/// `objc_registerClassPair`).
pub(super) unsafe fn add_methods(cls: Class) {
    let range = "{_NSRange=QQ}";
    let rect = "{CGRect={CGPoint=dd}{CGSize=dd}}";
    let methods: [(&str, *const c_void, String); 13] = [
        (
            "insertText:replacementRange:",
            insert_text as *const c_void,
            format!("v@:@{range}"),
        ),
        (
            "setMarkedText:selectedRange:replacementRange:",
            set_marked_text as *const c_void,
            format!("v@:@{range}{range}"),
        ),
        ("unmarkText", unmark_text as *const c_void, "v@:".into()),
        (
            "hasMarkedText",
            has_marked_text as *const c_void,
            "c@:".into(),
        ),
        (
            "markedRange",
            marked_range as *const c_void,
            format!("{range}@:"),
        ),
        (
            "selectedRange",
            selected_range as *const c_void,
            format!("{range}@:"),
        ),
        (
            "validAttributesForMarkedText",
            valid_attributes as *const c_void,
            "@@:".into(),
        ),
        (
            "attributedSubstringForProposedRange:actualRange:",
            attributed_substring as *const c_void,
            format!("@@:{range}^{range}"),
        ),
        (
            "characterIndexForPoint:",
            character_index as *const c_void,
            "Q@:{CGPoint=dd}".into(),
        ),
        (
            "firstRectForCharacterRange:actualRange:",
            first_rect as *const c_void,
            format!("{rect}@:{range}^{range}"),
        ),
        (
            "doCommandBySelector:",
            do_command as *const c_void,
            "v@::".into(),
        ),
        ("keyDown:", key_down_noop as *const c_void, "v@:@".into()),
        (
            "acceptsFirstResponder",
            accepts_first_responder as *const c_void,
            "c@:".into(),
        ),
    ];
    for (name, imp, types) in methods {
        let types = std::ffi::CString::new(types).unwrap();
        unsafe { class_addMethod(cls, sel(name), imp, types.as_ptr()) };
    }
    let protocol = unsafe { objc_getProtocol(c"NSTextInputClient".as_ptr()) };
    if !protocol.is_null() {
        unsafe { class_addProtocol(cls, protocol) };
    }
}

/// Run a key-down through the input context when a text field has focus,
/// returning the events it produced, or `None` to take the plain key path
/// (no text field, or a Command/Control shortcut).
///
/// # Safety
/// `view` must be the live `StippleView` and `ev` a key-down `NSEvent`.
pub(super) unsafe fn key_down(view: Id, ev: Id) -> Option<Vec<Event>> {
    let (active, composing) = CTX.with(|c| {
        let c = c.borrow();
        (c.ime.is_some(), !c.marked.is_empty())
    });
    if !active {
        return None;
    }
    let flags = unsafe { super::msg_u64(ev, sel("modifierFlags")) };
    let modifiers = input::mac_modifiers(flags);
    if (modifiers.meta || modifiers.ctrl) && !composing {
        return None;
    }
    CTX.with(|c| {
        let mut c = c.borrow_mut();
        c.ime_events.clear();
        c.ime_command = false;
    });
    unsafe {
        let events = msg_id_id(class("NSArray"), sel("arrayWithObject:"), ev);
        msg_void_id(view, sel("interpretKeyEvents:"), events);
    }
    let (mut events, command) = CTX.with(|c| {
        let mut c = c.borrow_mut();
        (std::mem::take(&mut c.ime_events), c.ime_command)
    });
    if command && let Some(key) = unsafe { key_event(ev, ButtonState::Pressed) } {
        events.push(key);
    }
    Some(events)
}

/// Record the focused field (or none). Losing it mid-composition drops the
/// marked text; a moved caret makes the input context ask again where to put
/// its candidate window.
pub(super) fn set_state(state: Option<&ImeState>) {
    let (view, composing) = CTX.with(|c| {
        let mut c = c.borrow_mut();
        c.ime = state.cloned();
        (c.view, !c.marked.is_empty())
    });
    if view.is_null() {
        return;
    }
    unsafe {
        let context = msg_id(view, sel("inputContext"));
        if context.is_null() {
            return;
        }
        if state.is_none() && composing {
            CTX.with(|c| c.borrow_mut().marked.clear());
            msg_void(context, sel("discardMarkedText"));
        } else if state.is_some() {
            msg_void(context, sel("invalidateCharacterCoordinates"));
        }
    }
}

/// For CI: drive the `NSTextInputClient` methods through real Objective-C
/// dispatch, as an input method would, and report what came out.
///
/// # Safety
/// `view` must be the live `StippleView`.
pub(super) unsafe fn self_check(view: Id) -> String {
    unsafe {
        let protocol = objc_getProtocol(c"NSTextInputClient".as_ptr());
        let conforms = {
            let f: unsafe extern "C" fn(Id, Sel, *mut c_void) -> i8 =
                std::mem::transmute(objc_msgSend as *const c_void);
            !protocol.is_null() && f(view, sel("conformsToProtocol:"), protocol) != 0
        };
        let context = !msg_id(view, sel("inputContext")).is_null();
        CTX.with(|c| c.borrow_mut().ime_events.clear());
        let set: unsafe extern "C" fn(Id, Sel, Id, NsRange, NsRange) =
            std::mem::transmute(objc_msgSend as *const c_void);
        let none = NsRange {
            location: NOT_FOUND,
            length: 0,
        };
        set(
            view,
            sel("setMarkedText:selectedRange:replacementRange:"),
            super::nsstring("にほん"),
            NsRange {
                location: 3,
                length: 0,
            },
            none,
        );
        let has: unsafe extern "C" fn(Id, Sel) -> i8 =
            std::mem::transmute(objc_msgSend as *const c_void);
        let marked = has(view, sel("hasMarkedText")) != 0;
        let insert: unsafe extern "C" fn(Id, Sel, Id, NsRange) =
            std::mem::transmute(objc_msgSend as *const c_void);
        insert(
            view,
            sel("insertText:replacementRange:"),
            super::nsstring("日本"),
            none,
        );
        let events = CTX.with(|c| std::mem::take(&mut c.borrow_mut().ime_events));
        format!("conforms={conforms} context={context} marked={marked} events={events:?}")
    }
}
