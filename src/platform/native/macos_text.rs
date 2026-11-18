//! AppKit: live text fields (see [`text`](super::super::text) for the
//! bookkeeping, and `docs/native-theme.md`, *Text fields*).
//!
//! A `text_field` or `number_input` is an `NSTextField` (an
//! `NSSecureTextField` for a password), a `text_area` an `NSTextView` in a
//! bordered `NSScrollView`, in the window's view like the other controls.
//! Unlike them they take first responder: while one does, the event loop
//! hands key events straight to AppKit ([`editing`]), so its field editor,
//! the input methods, the edit menu and undo are AppKit's own.
//!
//! What it holds is read back after every event AppKit handled ([`poll`],
//! and the delegate's change notifications): its text (the field editor's,
//! while it edits), selection and marked text become an
//! [`Event::NativeView`](crate::platform::Event::NativeView) edit for the
//! app; it becoming first responder (a click into it) the field's focus.
//! The delegate's `doCommandBySelector:` hands the app Return (submit),
//! Tab and Shift-Tab (focus moves on), Escape, and Up and Down in a
//! one-line field (a number steps).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;

use crate::platform::native::objc::{CgRect, Id, Sel, class, msg, nsstring, rust_string, sel};
use crate::platform::native::{
    Held, NativeKey, NativeText, NativeView, NativeViewEvent, TextMirror, TextPoke,
};

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    fn objc_allocateClassPair(superclass: Id, name: *const i8, extra: usize) -> Id;
    fn objc_registerClassPair(cls: Id);
    fn objc_getClass(name: *const i8) -> Id;
    fn class_addMethod(cls: Id, name: Sel, imp: *const c_void, types: *const i8) -> bool;
}

/// `NSRange`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct NsRange {
    location: u64,
    length: u64,
}

/// `NSNotFound`.
const NOT_FOUND: u64 = i64::MAX as u64;
/// `NSTextFieldRoundedBezel`, as the picture of a field draws.
const ROUNDED_BEZEL: u64 = 1;
/// `NSBezelBorder`.
const BEZEL_BORDER: u64 = 2;
/// `NSViewWidthSizable`.
const WIDTH_SIZABLE: u64 = 2;
/// `NSLineBreakByClipping`.
const CLIPPING: u64 = 2;

/// One live text control.
struct Field {
    /// The [`NativeView::id`] it shows.
    id: u64,
    /// The window's view it's in.
    view: Id,
    /// The `NSTextView` of a text area (in its scroll view); null for a
    /// one-line field, whose editor is the window's field editor while it
    /// edits.
    text_view: Id,
    multiline: bool,
    secure: bool,
    /// Return moves focus on (the field's action is Next).
    next: bool,
    mirror: TextMirror,
    /// It was first responder when last looked at.
    focused: bool,
    /// What [`configure`] last set (set again only when that changes).
    config: String,
}

thread_local! {
    /// The live text controls, by their control (the field, or the text
    /// area's scroll view).
    static FIELDS: RefCell<HashMap<usize, Field>> = RefCell::new(HashMap::new());
    static DELEGATE: Cell<Id> = const { Cell::new(std::ptr::null_mut()) };
    /// Set while the backend itself changes a control (its notifications
    /// aren't the user's).
    static SETTING: Cell<bool> = const { Cell::new(false) };
}

/// The delegate every live text control has.
fn delegate() -> Id {
    DELEGATE.with(|d| {
        if d.get().is_null() {
            let cls = unsafe {
                let name = c"StippleTextDelegate";
                let existing = objc_getClass(name.as_ptr());
                if existing.is_null() {
                    let cls = objc_allocateClassPair(class("NSObject"), name.as_ptr(), 0);
                    let methods: [(&str, *const c_void, &std::ffi::CStr); 6] = [
                        (
                            "control:textView:doCommandBySelector:",
                            field_command as *const c_void,
                            c"c@:@@:",
                        ),
                        (
                            "textView:doCommandBySelector:",
                            area_command as *const c_void,
                            c"c@:@:",
                        ),
                        ("controlTextDidChange:", changed as *const c_void, c"v@:@"),
                        ("textDidChange:", changed as *const c_void, c"v@:@"),
                        (
                            "textViewDidChangeSelection:",
                            changed as *const c_void,
                            c"v@:@",
                        ),
                        (
                            "controlTextDidBeginEditing:",
                            changed as *const c_void,
                            c"v@:@",
                        ),
                    ];
                    for (name, imp, types) in methods {
                        class_addMethod(cls, sel(name), imp, types.as_ptr());
                    }
                    objc_registerClassPair(cls);
                    cls
                } else {
                    existing
                }
            };
            d.set(unsafe { msg!(msg!(cls, "alloc", () -> Id), "init", () -> Id) });
        }
        d.get()
    })
}

/// The window's view whose control, or text view, `obj` is.
fn view_of(obj: Id) -> Option<Id> {
    FIELDS.with(|f| {
        let f = f.try_borrow().ok()?;
        f.iter()
            .find(|(c, field)| **c == obj as usize || field.text_view == obj)
            .map(|(_, field)| field.view)
    })
}

/// A notification from a control or its editor: read the controls back.
extern "C" fn changed(_this: Id, _cmd: Sel, note: Id) {
    if SETTING.with(|s| s.get()) {
        return;
    }
    let obj: Id = unsafe { msg!(note, "object", () -> Id) };
    // A field editor's notifications come from the field (its delegate).
    if let Some(view) = view_of(obj) {
        poll(view);
    } else if !obj.is_null() {
        let d: Id = unsafe { msg!(obj, "delegate", () -> Id) };
        if let Some(view) = view_of(d) {
            poll(view);
        }
    }
}

extern "C" fn field_command(_this: Id, _cmd: Sel, control: Id, _tv: Id, command: Sel) -> bool {
    key_command(control, command)
}

extern "C" fn area_command(_this: Id, _cmd: Sel, text_view: Id, command: Sel) -> bool {
    let control = FIELDS.with(|f| {
        f.borrow()
            .iter()
            .find(|(_, field)| field.text_view == text_view)
            .map(|(c, _)| *c as Id)
    });
    control.is_some_and(|c| key_command(c, command))
}

/// A key the control would act on as a command: the app's, when it is one
/// the app decides (see the top of this file). Returns whether it was.
fn key_command(control: Id, command: Sel) -> bool {
    let Some((id, view, multiline, next)) = FIELDS.with(|f| {
        f.borrow()
            .get(&(control as usize))
            .map(|f| (f.id, f.view, f.multiline, f.next))
    }) else {
        return false;
    };
    let key = if command == sel("insertNewline:") {
        if multiline && !next {
            return false;
        }
        NativeKey::Enter
    } else if command == sel("insertTab:") {
        NativeKey::Tab
    } else if command == sel("insertBacktab:") {
        NativeKey::BackTab
    } else if command == sel("cancelOperation:") {
        NativeKey::Escape
    } else if command == sel("moveUp:") && !multiline {
        NativeKey::Up
    } else if command == sel("moveDown:") && !multiline {
        NativeKey::Down
    } else {
        return false;
    };
    // What was typed first, then the key.
    poll(view);
    crate::platform::backend::macos::queue_event(
        view,
        crate::platform::Event::NativeView {
            id,
            event: NativeViewEvent::Key(key),
        },
    );
    true
}

/// The window's first responder for the window `view` is in.
unsafe fn first_responder(view: Id) -> Id {
    unsafe {
        let window: Id = msg!(view, "window", () -> Id);
        if window.is_null() {
            return std::ptr::null_mut();
        }
        msg!(window, "firstResponder", () -> Id)
    }
}

/// The editor of `control` while it edits (a text area's text view
/// always), else null; and whether it is first responder.
unsafe fn editor(control: Id, f: &Field) -> (Id, bool) {
    unsafe {
        let fr = first_responder(f.view);
        if !f.text_view.is_null() {
            return (f.text_view, fr == f.text_view);
        }
        if fr.is_null() || fr == f.view {
            return (std::ptr::null_mut(), false);
        }
        let cls = class("NSTextView");
        let is_text: bool = msg!(fr, "isKindOfClass:", (cls: Id) -> bool);
        if !is_text {
            return (std::ptr::null_mut(), false);
        }
        let d: Id = msg!(fr, "delegate", () -> Id);
        if d == control {
            (fr, true)
        } else {
            (std::ptr::null_mut(), false)
        }
    }
}

/// What `control` holds, and whether it is first responder.
unsafe fn read(control: Id, f: &Field) -> (Held, bool) {
    unsafe {
        let (ed, focused) = editor(control, f);
        if ed.is_null() {
            let text = rust_string(msg!(control, "stringValue", () -> Id)).unwrap_or_default();
            // Not editing: no selection of its own; the app's stays.
            let held = f.mirror.held();
            let sel = (held.sel.0.min(text.len()), held.sel.1.min(text.len()));
            let sel = if text.is_char_boundary(sel.0) && text.is_char_boundary(sel.1) {
                sel
            } else {
                (text.len(), text.len())
            };
            return (
                Held {
                    text,
                    sel,
                    marked: None,
                },
                false,
            );
        }
        let text = rust_string(msg!(ed, "string", () -> Id)).unwrap_or_default();
        let r: NsRange = msg!(ed, "selectedRange", () -> NsRange);
        let marked = if msg!(ed, "hasMarkedText", () -> bool) {
            let m: NsRange = msg!(ed, "markedRange", () -> NsRange);
            (m.location != NOT_FOUND)
                .then_some((m.location as usize, (m.location + m.length) as usize))
        } else {
            None
        };
        let (s, e) = (r.location as usize, (r.location + r.length) as usize);
        (Held::from_utf16(text, (s, e), marked), focused)
    }
}

/// Read window view `view`'s text controls back: queue what the user
/// changed in them for the app.
pub(crate) fn poll(view: Id) {
    if SETTING.with(|s| s.get()) {
        return;
    }
    let mut events = Vec::new();
    FIELDS.with(|fields| {
        let Ok(mut fields) = fields.try_borrow_mut() else {
            return;
        };
        for (control, f) in fields.iter_mut().filter(|(_, f)| f.view == view) {
            let (held, focused) = unsafe { read(*control as Id, f) };
            if focused && !f.focused {
                events.push((f.id, NativeViewEvent::Focus(true)));
            }
            f.focused = focused;
            if let Some(edit) = f.mirror.observe(held) {
                events.push((f.id, NativeViewEvent::Edited(edit)));
            }
        }
    });
    for (id, event) in events {
        crate::platform::backend::macos::queue_event(
            view,
            crate::platform::Event::NativeView { id, event },
        );
    }
}

/// Whether one of window view `view`'s text controls is first responder
/// (the event loop then leaves its keys to AppKit).
pub(crate) fn editing(view: Id) -> bool {
    let any = FIELDS.with(|f| f.borrow().values().any(|f| f.view == view));
    if !any {
        return false;
    }
    let fr = unsafe { first_responder(view) };
    if fr.is_null() || fr == view {
        return false;
    }
    FIELDS.with(|f| {
        f.borrow()
            .iter()
            .filter(|(_, f)| f.view == view)
            .any(|(c, f)| unsafe { editor(*c as Id, f).1 })
    })
}

/// Run `f` with the backend's own changes not reported back.
fn setting<R>(f: impl FnOnce() -> R) -> R {
    SETTING.with(|s| s.set(true));
    let r = f();
    SETTING.with(|s| s.set(false));
    r
}

/// Put `want` in `control`.
unsafe fn write(control: Id, f: &Field, want: &Held) {
    setting(|| unsafe {
        let (ed, _) = editor(control, f);
        let text = nsstring(&want.text);
        if ed.is_null() {
            msg!(control, "setStringValue:", (text: Id) -> ());
            return;
        }
        if msg!(ed, "hasMarkedText", () -> bool) {
            let ctx: Id = msg!(ed, "inputContext", () -> Id);
            if !ctx.is_null() {
                msg!(ctx, "discardMarkedText", () -> ());
            }
            msg!(ed, "unmarkText", () -> ());
        }
        msg!(ed, "setString:", (text: Id) -> ());
        let (s, e) = want.sel_utf16();
        let r = NsRange {
            location: s as u64,
            length: (e - s) as u64,
        };
        msg!(ed, "setSelectedRange:", (r: NsRange) -> ());
    })
}

/// Set what the spec says of a control besides its text.
unsafe fn configure(control: Id, f: &Field, t: &NativeText, v: &NativeView) {
    unsafe {
        let target = if f.text_view.is_null() {
            control
        } else {
            f.text_view
        };
        let editable = !t.read_only;
        msg!(target, "setEditable:", (editable: bool) -> ());
        let yes = true;
        msg!(target, "setSelectable:", (yes: bool) -> ());
        let size = if v.font_size > 0.0 { v.font_size } else { 13.0 };
        let font: Id = msg!(class("NSFont"), "systemFontOfSize:", (size: f64) -> Id);
        msg!(target, "setFont:", (font: Id) -> ());
        if f.text_view.is_null() {
            let hint = nsstring(&t.placeholder);
            msg!(control, "setPlaceholderString:", (hint: Id) -> ());
        }
        // An invalid number: a red ring.
        msg!(control, "setWantsLayer:", (yes: bool) -> ());
        let layer: Id = msg!(control, "layer", () -> Id);
        if !layer.is_null() {
            let width = if t.invalid { 2.0_f64 } else { 0.0 };
            msg!(layer, "setBorderWidth:", (width: f64) -> ());
            let red: Id = msg!(class("NSColor"), "systemRedColor", () -> Id);
            let cg: *const c_void = msg!(red, "CGColor", () -> *const c_void);
            msg!(layer, "setBorderColor:", (cg: *const c_void) -> ());
            let radius = 4.0_f64;
            msg!(layer, "setCornerRadius:", (radius: f64) -> ());
        }
    }
}

/// Make `v`'s control in window view `view`.
pub(super) fn create(view: Id, v: &NativeView) -> Option<Id> {
    let t = v.text.as_deref()?;
    let f = CgRect {
        x: v.frame.min_x(),
        y: v.frame.min_y(),
        width: v.frame.width(),
        height: v.frame.height(),
    };
    unsafe {
        let (control, text_view) = if t.multiline {
            let alloc: Id = msg!(class("NSScrollView"), "alloc", () -> Id);
            let scroll: Id = msg!(alloc, "initWithFrame:", (f: CgRect) -> Id);
            let yes = true;
            msg!(scroll, "setHasVerticalScroller:", (yes: bool) -> ());
            msg!(scroll, "setBorderType:", (BEZEL_BORDER: u64) -> ());
            let content: crate::platform::native::objc::CgSize =
                msg!(scroll, "contentSize", () -> crate::platform::native::objc::CgSize);
            let inner = CgRect {
                x: 0.0,
                y: 0.0,
                width: content.width,
                height: content.height,
            };
            let alloc: Id = msg!(class("NSTextView"), "alloc", () -> Id);
            let tv: Id = msg!(alloc, "initWithFrame:", (inner: CgRect) -> Id);
            msg!(tv, "setVerticallyResizable:", (yes: bool) -> ());
            let no = false;
            msg!(tv, "setHorizontallyResizable:", (no: bool) -> ());
            msg!(tv, "setRichText:", (no: bool) -> ());
            msg!(tv, "setAllowsUndo:", (yes: bool) -> ());
            msg!(tv, "setAutoresizingMask:", (WIDTH_SIZABLE: u64) -> ());
            let container: Id = msg!(tv, "textContainer", () -> Id);
            msg!(container, "setWidthTracksTextView:", (yes: bool) -> ());
            let d = delegate();
            msg!(tv, "setDelegate:", (d: Id) -> ());
            msg!(scroll, "setDocumentView:", (tv: Id) -> ());
            msg!(tv, "release", () -> ());
            (scroll, tv)
        } else {
            let cls = if t.secure {
                "NSSecureTextField"
            } else {
                "NSTextField"
            };
            let alloc: Id = msg!(class(cls), "alloc", () -> Id);
            let field: Id = msg!(alloc, "initWithFrame:", (f: CgRect) -> Id);
            let yes = true;
            msg!(field, "setBezeled:", (yes: bool) -> ());
            msg!(field, "setBezelStyle:", (ROUNDED_BEZEL: u64) -> ());
            let cell: Id = msg!(field, "cell", () -> Id);
            if !cell.is_null() {
                msg!(cell, "setScrollable:", (yes: bool) -> ());
                msg!(cell, "setUsesSingleLineMode:", (yes: bool) -> ());
                msg!(cell, "setLineBreakMode:", (CLIPPING: u64) -> ());
            }
            let d = delegate();
            msg!(field, "setDelegate:", (d: Id) -> ());
            (field, std::ptr::null_mut())
        };
        if control.is_null() {
            return None;
        }
        let field = Field {
            id: v.id,
            view,
            text_view,
            multiline: t.multiline,
            secure: t.secure,
            next: t.hints.enter() == Some(crate::platform::EnterAction::Next),
            mirror: TextMirror::default(),
            focused: false,
            config: String::new(),
        };
        FIELDS.with(|fs| fs.borrow_mut().insert(control as usize, field));
        Some(control)
    }
}

/// Whether `control` is the kind of control `t` asks for (one line or
/// several, a password or not): `false` means it must be made again.
pub(super) fn fits(control: Id, t: &NativeText) -> bool {
    FIELDS.with(|f| {
        f.borrow()
            .get(&(control as usize))
            .is_some_and(|f| f.multiline == t.multiline && f.secure == t.secure)
    })
}

/// Bring `control` (made by [`create`], for window view `view`) to `new`:
/// its settings, its focus, and the field's text where the app's state
/// differs from what it holds.
pub(super) fn update(view: Id, control: &mut Id, new: &NativeView) {
    let Some(t) = new.text.as_deref() else {
        return;
    };
    let c = *control;
    let Some(mut f) = FIELDS.with(|fs| fs.borrow_mut().remove(&(c as usize))) else {
        return;
    };
    f.next = t.hints.enter() == Some(crate::platform::EnterAction::Next);
    unsafe {
        let config = format!(
            "{:?}",
            (
                new.font_size.to_bits(),
                &t.placeholder,
                t.read_only,
                t.invalid
            )
        );
        if config != f.config {
            configure(c, &f, t, new);
            f.config = config;
        }
        let (_, focused) = editor(c, &f);
        if t.focused && !focused {
            // Becoming first responder selects it all: the app's selection
            // goes back in below.
            let window: Id = msg!(view, "window", () -> Id);
            let target = if f.text_view.is_null() {
                c
            } else {
                f.text_view
            };
            if !window.is_null() {
                setting(|| msg!(window, "makeFirstResponder:", (target: Id) -> bool));
            }
            f.focused = editor(c, &f).1;
            if t.seen >= f.mirror.sent() {
                f.mirror.forget();
            }
        } else if !t.focused && focused {
            // The app's focus moved elsewhere: the keyboard goes back to its
            // view.
            let window: Id = msg!(view, "window", () -> Id);
            if !window.is_null() {
                setting(|| msg!(window, "makeFirstResponder:", (view: Id) -> bool));
            }
            f.focused = false;
        }
        if let Some(want) = f.mirror.wanted(t) {
            write(c, &f, &want);
        }
    }
    FIELDS.with(|fs| fs.borrow_mut().insert(c as usize, f));
}

/// `control` goes: the keyboard goes back to the app's view if it was
/// first responder. (The caller removes and releases it.)
pub(super) fn destroy(control: Id) {
    let f = FIELDS.with(|fs| fs.borrow_mut().remove(&(control as usize)));
    if let Some(f) = f
        && unsafe { editor(control, &f).1 }
    {
        unsafe {
            let window: Id = msg!(f.view, "window", () -> Id);
            let view = f.view;
            if !window.is_null() {
                setting(|| msg!(window, "makeFirstResponder:", (view: Id) -> bool));
            }
        }
    }
}

/// What `control` holds and whether it is first responder, if it is a
/// live text control (for `live_controls`).
pub(super) fn value_of(control: Id) -> Option<(String, bool)> {
    FIELDS.with(|fs| {
        let fs = fs.borrow();
        let f = fs.get(&(control as usize))?;
        let (held, focused) = unsafe { read(control, f) };
        Some((held.text, focused))
    })
}

/// The self-test's `poke_text` on `control`.
pub(super) fn poke(control: Id, poke: TextPoke) -> bool {
    let Some((view, text_view)) = FIELDS.with(|fs| {
        fs.borrow()
            .get(&(control as usize))
            .map(|f| (f.view, f.text_view))
    }) else {
        return false;
    };
    unsafe {
        let window: Id = msg!(view, "window", () -> Id);
        let target = if text_view.is_null() {
            control
        } else {
            text_view
        };
        let editor = || {
            FIELDS.with(|fs| {
                fs.borrow()
                    .get(&(control as usize))
                    .map(|f| editor(control, f))
                    .unwrap_or((std::ptr::null_mut(), false))
            })
        };
        if !editor().1 && !window.is_null() {
            msg!(window, "makeFirstResponder:", (target: Id) -> bool);
        }
        let (ed, focused) = editor();
        if ed.is_null() || !focused {
            return false;
        }
        match poke {
            // AppKit selects a field's text when it takes the focus; a click
            // at its end puts the caret there.
            TextPoke::Focus => {
                let s: Id = msg!(ed, "string", () -> Id);
                let len: u64 = msg!(s, "length", () -> u64);
                let end = NsRange {
                    location: len,
                    length: 0,
                };
                msg!(ed, "setSelectedRange:", (end: NsRange) -> ());
            }
            TextPoke::Type(text) => {
                let s = nsstring(&text);
                msg!(ed, "insertText:", (s: Id) -> ());
            }
            TextPoke::Compose(text) => {
                let s = nsstring(&text);
                let len = text.encode_utf16().count() as u64;
                let sel = NsRange {
                    location: len,
                    length: 0,
                };
                let none = NsRange {
                    location: NOT_FOUND,
                    length: 0,
                };
                msg!(ed, "setMarkedText:selectedRange:replacementRange:", (s: Id, sel: NsRange, none: NsRange) -> ());
            }
            TextPoke::Enter => {
                let c = sel("insertNewline:");
                msg!(ed, "doCommandBySelector:", (c: Sel) -> ());
            }
        }
    }
    poll(view);
    true
}
