//! UIKit: live text fields (see [`text`](super::super::text) for the
//! bookkeeping, and `docs/native-theme.md`, *Text fields*).
//!
//! A `text_field`, `number_input` or `password_field` is a `UITextField`
//! (rounded, `secureTextEntry` for a password), a `text_area` a bordered
//! `UITextView`, in the app's view like the other controls, with the
//! field's keyboard (its `keyboardType`, capitals, corrections, return key
//! and `textContentType` from the field's hints). While one is first
//! responder the keyboard, its input methods, the edit menu, the
//! magnifier and the selection handles are UIKit's.
//!
//! What it holds is read back each time UIKit says it changed
//! (`UIControlEventEditingChanged`, the delegate's
//! `textFieldDidChangeSelection:`, `textViewDidChange:`,
//! `textViewDidChangeSelection:`): its text, selection and marked text
//! become an [`Event::NativeView`](crate::platform::Event::NativeView) edit
//! for the app. It beginning to edit (a tap into it) gives the field the
//! focus, it ending without the app asking (the keyboard's dismiss key)
//! takes it away. Return (`textFieldShouldReturn:`, or a new line typed in
//! a text area whose action is Next) goes to the app, which submits the
//! field or moves on.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;

use crate::platform::input_hints::ios::traits;
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
    location: usize,
    length: usize,
}

const EDITING_DID_BEGIN: u64 = 1 << 16;
const EDITING_CHANGED: u64 = 1 << 17;
const EDITING_DID_END: u64 = 1 << 18;
/// `UITextBorderStyleRoundedRect`, as the picture of a field draws.
const ROUNDED: i64 = 3;

/// One live text control.
struct Field {
    id: u64,
    multiline: bool,
    secure: bool,
    read_only: bool,
    /// Return moves focus on (the field's action is Next).
    next: bool,
    mirror: TextMirror,
    /// It was first responder when last looked at.
    focused: bool,
    /// What [`configure`] last set (it's set again only when that
    /// changes: a keyboard reloaded on every keystroke would lose its
    /// state).
    config: String,
}

thread_local! {
    static FIELDS: RefCell<HashMap<usize, Field>> = RefCell::new(HashMap::new());
    static DELEGATE: Cell<Id> = const { Cell::new(std::ptr::null_mut()) };
    /// Set while the backend itself changes a control.
    static SETTING: Cell<bool> = const { Cell::new(false) };
}

/// The delegate and target every live text control has.
fn delegate() -> Id {
    DELEGATE.with(|d| {
        if d.get().is_null() {
            let cls = unsafe {
                let name = c"StippleTextFieldDelegate";
                let existing = objc_getClass(name.as_ptr());
                if existing.is_null() {
                    let cls = objc_allocateClassPair(class("NSObject"), name.as_ptr(), 0);
                    let methods: [(&str, *const c_void, &std::ffi::CStr); 9] = [
                        ("stippleTextChanged:", changed as *const c_void, c"v@:@"),
                        ("stippleTextBegan:", changed as *const c_void, c"v@:@"),
                        ("stippleTextEnded:", ended as *const c_void, c"v@:@"),
                        (
                            "textFieldDidChangeSelection:",
                            changed as *const c_void,
                            c"v@:@",
                        ),
                        (
                            "textFieldShouldReturn:",
                            should_return as *const c_void,
                            c"c@:@",
                        ),
                        (
                            "textField:shouldChangeCharactersInRange:replacementString:",
                            field_may_change as *const c_void,
                            c"c@:@{_NSRange=QQ}@",
                        ),
                        ("textViewDidChange:", changed as *const c_void, c"v@:@"),
                        (
                            "textViewDidChangeSelection:",
                            changed as *const c_void,
                            c"v@:@",
                        ),
                        (
                            "textView:shouldChangeTextInRange:replacementText:",
                            area_may_change as *const c_void,
                            c"c@:@{_NSRange=QQ}@",
                        ),
                    ];
                    for (name, imp, types) in methods {
                        class_addMethod(cls, sel(name), imp, types.as_ptr());
                    }
                    let extra: [(&str, *const c_void); 2] = [
                        ("textViewDidBeginEditing:", changed as *const c_void),
                        ("textViewDidEndEditing:", ended as *const c_void),
                    ];
                    for (name, imp) in extra {
                        class_addMethod(cls, sel(name), imp, c"v@:@".as_ptr());
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

fn send(id: u64, event: NativeViewEvent) {
    crate::platform::native::text::trace(|| format!("send {event:?}"));
    crate::platform::backend::ios::send_event(crate::platform::Event::NativeView { id, event });
}

/// UIKit says `control` changed (its text, selection or editing): read it
/// back.
extern "C" fn changed(_this: Id, _cmd: Sel, control: Id) {
    if !SETTING.with(|s| s.get()) {
        poll(control);
    }
}

/// `control` stopped editing: unless the backend made it, the field loses
/// the focus (the keyboard was dismissed).
extern "C" fn ended(_this: Id, _cmd: Sel, control: Id) {
    if SETTING.with(|s| s.get()) {
        return;
    }
    poll(control);
    let id = FIELDS.with(|f| {
        let mut f = f.borrow_mut();
        let field = f.get_mut(&(control as usize))?;
        let was = std::mem::replace(&mut field.focused, false);
        was.then_some(field.id)
    });
    crate::platform::native::text::trace(|| format!("ended: blur sent {}", id.is_some()));
    if let Some(id) = id {
        send(id, NativeViewEvent::Focus(false));
    }
}

extern "C" fn should_return(_this: Id, _cmd: Sel, control: Id) -> bool {
    let id = FIELDS.with(|f| f.borrow().get(&(control as usize)).map(|f| f.id));
    crate::platform::native::text::trace(|| format!("return in {id:?}"));
    if let Some(id) = id {
        poll(control);
        send(id, NativeViewEvent::Key(NativeKey::Enter));
    }
    false
}

extern "C" fn field_may_change(
    _this: Id,
    _cmd: Sel,
    control: Id,
    _range: NsRange,
    _text: Id,
) -> bool {
    FIELDS.with(|f| {
        f.borrow()
            .get(&(control as usize))
            .is_none_or(|f| !f.read_only)
    })
}

/// A text area's new line, when its action is Next, moves on instead.
extern "C" fn area_may_change(
    _this: Id,
    _cmd: Sel,
    control: Id,
    _range: NsRange,
    text: Id,
) -> bool {
    let Some((id, read_only, next)) = FIELDS.with(|f| {
        f.borrow()
            .get(&(control as usize))
            .map(|f| (f.id, f.read_only, f.next))
    }) else {
        return true;
    };
    if read_only {
        return false;
    }
    if next && rust_string(text).as_deref() == Some("\n") {
        poll(control);
        send(id, NativeViewEvent::Key(NativeKey::Enter));
        return false;
    }
    true
}

/// The UTF-16 offset of `position` in `control`.
unsafe fn offset(control: Id, position: Id) -> usize {
    unsafe {
        let start: Id = msg!(control, "beginningOfDocument", () -> Id);
        let o: isize =
            msg!(control, "offsetFromPosition:toPosition:", (start: Id, position: Id) -> isize);
        o.max(0) as usize
    }
}

/// The UTF-16 range `range` (a `UITextRange`) covers in `control`.
unsafe fn range_of(control: Id, range: Id) -> Option<(usize, usize)> {
    if range.is_null() {
        return None;
    }
    unsafe {
        let s: Id = msg!(range, "start", () -> Id);
        let e: Id = msg!(range, "end", () -> Id);
        Some((offset(control, s), offset(control, e)))
    }
}

/// What `control` holds, and whether it is first responder.
unsafe fn read(control: Id) -> (Held, bool) {
    unsafe {
        let text = rust_string(msg!(control, "text", () -> Id)).unwrap_or_default();
        let sel_range: Id = msg!(control, "selectedTextRange", () -> Id);
        let len = text.encode_utf16().count();
        let sel = range_of(control, sel_range).unwrap_or((len, len));
        let marked_range: Id = msg!(control, "markedTextRange", () -> Id);
        let marked = range_of(control, marked_range);
        let focused: bool = msg!(control, "isFirstResponder", () -> bool);
        (Held::from_utf16(text, sel, marked), focused)
    }
}

/// Read `control` back and tell the app what the user changed.
fn poll(control: Id) {
    let found = FIELDS.with(|f| {
        let mut f = f.try_borrow_mut().ok()?;
        let field = f.get_mut(&(control as usize))?;
        let (held, focused) = unsafe { read(control) };
        let mut events = Vec::new();
        if focused && !field.focused {
            events.push(NativeViewEvent::Focus(true));
        }
        field.focused = focused;
        // Not editing, it has no selection of its own: the app's stays.
        let held = if focused {
            held
        } else {
            let sel = field.mirror.held().sel;
            let fits = |i: usize| i <= held.text.len() && held.text.is_char_boundary(i);
            Held {
                sel: if fits(sel.0) && fits(sel.1) {
                    sel
                } else {
                    (held.text.len(), held.text.len())
                },
                ..held
            }
        };
        if let Some(edit) = field.mirror.observe(held) {
            events.push(NativeViewEvent::Edited(edit));
        }
        Some((field.id, events))
    });
    if let Some((id, events)) = found {
        for e in events {
            send(id, e);
        }
    }
}

fn setting<R>(f: impl FnOnce() -> R) -> R {
    SETTING.with(|s| s.set(true));
    let r = f();
    SETTING.with(|s| s.set(false));
    r
}

/// Put `want` in `control`.
unsafe fn write(control: Id, want: &Held) {
    setting(|| unsafe {
        let marked: Id = msg!(control, "markedTextRange", () -> Id);
        if !marked.is_null() {
            msg!(control, "unmarkText", () -> ());
        }
        let text = nsstring(&want.text);
        msg!(control, "setText:", (text: Id) -> ());
        let (s, e) = want.sel_utf16();
        let start: Id = msg!(control, "beginningOfDocument", () -> Id);
        let (s, e) = (s as isize, e as isize);
        let from: Id = msg!(control, "positionFromPosition:offset:", (start: Id, s: isize) -> Id);
        let to: Id = msg!(control, "positionFromPosition:offset:", (start: Id, e: isize) -> Id);
        if !from.is_null() && !to.is_null() {
            let range: Id =
                msg!(control, "textRangeFromPosition:toPosition:", (from: Id, to: Id) -> Id);
            msg!(control, "setSelectedTextRange:", (range: Id) -> ());
        }
    })
}

/// Set what the spec says of a control besides its text.
unsafe fn configure(control: Id, t: &NativeText, v: &NativeView) {
    unsafe {
        let size = if v.font_size > 0.0 { v.font_size } else { 17.0 };
        let font: Id = msg!(class("UIFont"), "systemFontOfSize:", (size: f64) -> Id);
        msg!(control, "setFont:", (font: Id) -> ());
        let tr = traits(&t.hints);
        let set = |name: &str, v: isize| {
            msg!(control, name, (v: isize) -> ());
        };
        set("setKeyboardType:", tr.keyboard);
        set("setAutocapitalizationType:", tr.autocapitalization);
        set("setAutocorrectionType:", tr.autocorrection);
        set("setSpellCheckingType:", tr.autocorrection);
        set("setReturnKeyType:", tr.return_key);
        let secure = t.secure;
        msg!(control, "setSecureTextEntry:", (secure: bool) -> ());
        let content = crate::platform::backend::ios::text_input::content_type(tr.content);
        msg!(control, "setTextContentType:", (content: Id) -> ());
        let layer: Id = msg!(control, "layer", () -> Id);
        if t.multiline {
            let editable = !t.read_only;
            msg!(control, "setEditable:", (editable: bool) -> ());
            // A text view has no border of its own: the separator's color,
            // rounded, as a rounded field's.
            let color: Id = if t.invalid {
                msg!(class("UIColor"), "systemRedColor", () -> Id)
            } else {
                msg!(class("UIColor"), "separatorColor", () -> Id)
            };
            let cg: *const c_void = msg!(color, "CGColor", () -> *const c_void);
            msg!(layer, "setBorderColor:", (cg: *const c_void) -> ());
            let (width, radius) = (1.0_f64, 5.0_f64);
            msg!(layer, "setBorderWidth:", (width: f64) -> ());
            msg!(layer, "setCornerRadius:", (radius: f64) -> ());
        } else {
            let hint = nsstring(&t.placeholder);
            msg!(control, "setPlaceholder:", (hint: Id) -> ());
            // An invalid number: a red ring.
            let width = if t.invalid { 2.0_f64 } else { 0.0 };
            msg!(layer, "setBorderWidth:", (width: f64) -> ());
            let red: Id = msg!(class("UIColor"), "systemRedColor", () -> Id);
            let cg: *const c_void = msg!(red, "CGColor", () -> *const c_void);
            msg!(layer, "setBorderColor:", (cg: *const c_void) -> ());
            let radius = 5.0_f64;
            msg!(layer, "setCornerRadius:", (radius: f64) -> ());
        }
        // The keyboard takes new traits on its next showing.
        let first: bool = msg!(control, "isFirstResponder", () -> bool);
        if first {
            setting(|| msg!(control, "reloadInputViews", () -> ()));
        }
    }
}

/// Make `v`'s control (the caller adds it to the app's view).
pub(super) fn create(v: &NativeView) -> Option<Id> {
    let t = v.text.as_deref()?;
    let f = CgRect {
        x: v.frame.min_x(),
        y: v.frame.min_y(),
        width: v.frame.width(),
        height: v.frame.height(),
    };
    unsafe {
        let d = delegate();
        let control = if t.multiline {
            let alloc: Id = msg!(class("UITextView"), "alloc", () -> Id);
            let tv: Id = msg!(alloc, "initWithFrame:", (f: CgRect) -> Id);
            msg!(tv, "setDelegate:", (d: Id) -> ());
            tv
        } else {
            let alloc: Id = msg!(class("UITextField"), "alloc", () -> Id);
            let tf: Id = msg!(alloc, "initWithFrame:", (f: CgRect) -> Id);
            msg!(tf, "setBorderStyle:", (ROUNDED: i64) -> ());
            msg!(tf, "setDelegate:", (d: Id) -> ());
            let events = [
                ("stippleTextChanged:", EDITING_CHANGED),
                ("stippleTextBegan:", EDITING_DID_BEGIN),
                ("stippleTextEnded:", EDITING_DID_END),
            ];
            for (action, event) in events {
                let a = sel(action);
                msg!(tf, "addTarget:action:forControlEvents:", (d: Id, a: Sel, event: u64) -> ());
            }
            tf
        };
        if control.is_null() {
            return None;
        }
        FIELDS.with(|fs| {
            fs.borrow_mut().insert(
                control as usize,
                Field {
                    id: v.id,
                    multiline: t.multiline,
                    secure: t.secure,
                    read_only: t.read_only,
                    next: t.hints.enter() == Some(crate::platform::EnterAction::Next),
                    mirror: TextMirror::default(),
                    focused: false,
                    config: String::new(),
                },
            )
        });
        Some(control)
    }
}

/// Whether `control` is the kind of control `t` asks for (one line or
/// several): `false` means it must be made again.
pub(super) fn fits(control: Id, t: &NativeText) -> bool {
    FIELDS.with(|f| {
        f.borrow()
            .get(&(control as usize))
            .is_some_and(|f| f.multiline == t.multiline)
    })
}

/// Bring `control` to `new`: its settings, its focus, and the field's text
/// where the app's state differs from what it holds.
pub(super) fn update(_view: Id, control: &mut Id, new: &NativeView) {
    let Some(t) = new.text.as_deref() else {
        return;
    };
    let c = *control;
    let Some(mut f) = FIELDS.with(|fs| fs.borrow_mut().remove(&(c as usize))) else {
        return;
    };
    f.next = t.hints.enter() == Some(crate::platform::EnterAction::Next);
    f.read_only = t.read_only;
    f.secure = t.secure;
    unsafe {
        let config = format!(
            "{:?}",
            (
                new.font_size.to_bits(),
                traits(&t.hints),
                t.secure,
                &t.placeholder,
                t.read_only,
                t.invalid,
            )
        );
        if config != f.config {
            configure(c, t, new);
            f.config = config;
        }
        let first: bool = msg!(c, "isFirstResponder", () -> bool);
        crate::platform::native::text::trace(|| {
            format!("update focused={} first={first} seen={}", t.focused, t.seen)
        });
        if t.focused && !first {
            setting(|| msg!(c, "becomeFirstResponder", () -> bool));
            f.focused = msg!(c, "isFirstResponder", () -> bool);
            if t.seen >= f.mirror.sent() {
                f.mirror.forget();
            }
        } else if !t.focused && first {
            setting(|| msg!(c, "resignFirstResponder", () -> bool));
            f.focused = false;
        }
        if let Some(want) = f.mirror.wanted(t) {
            write(c, &want);
        }
    }
    FIELDS.with(|fs| fs.borrow_mut().insert(c as usize, f));
}

/// `control` goes: it lets the keyboard go first, quietly.
pub(super) fn destroy(control: Id) {
    let f = FIELDS.with(|fs| fs.borrow_mut().remove(&(control as usize)));
    if f.is_some() {
        unsafe {
            let first: bool = msg!(control, "isFirstResponder", () -> bool);
            if first {
                setting(|| msg!(control, "resignFirstResponder", () -> bool));
            }
            let nil: Id = std::ptr::null_mut();
            msg!(control, "setDelegate:", (nil: Id) -> ());
        }
    }
}

/// Whether `obj` is a live text control (touches on it are UIKit's).
pub(super) fn is_text(obj: Id) -> bool {
    FIELDS.with(|f| {
        f.try_borrow()
            .is_ok_and(|f| f.contains_key(&(obj as usize)))
    })
}

/// What `control` holds and whether it is first responder, if it is a
/// live text control (for `live_controls`).
pub(super) fn value_of(control: Id) -> Option<(String, bool)> {
    if !is_text(control) {
        return None;
    }
    let (held, focused) = unsafe { read(control) };
    Some((held.text, focused))
}

/// The self-test's `poke_text` on `control`.
pub(super) fn poke(control: Id, poke: TextPoke) -> bool {
    if !is_text(control) {
        return false;
    }
    unsafe {
        let first: bool = msg!(control, "isFirstResponder", () -> bool);
        if !first {
            msg!(control, "becomeFirstResponder", () -> bool);
        }
        let first: bool = msg!(control, "isFirstResponder", () -> bool);
        if !first {
            return false;
        }
        match poke {
            TextPoke::Focus => {}
            TextPoke::Type(text) => {
                let s = nsstring(&text);
                msg!(control, "insertText:", (s: Id) -> ());
            }
            TextPoke::Compose(text) => {
                let s = nsstring(&text);
                let r = NsRange {
                    location: text.encode_utf16().count(),
                    length: 0,
                };
                msg!(control, "setMarkedText:selectedRange:", (s: Id, r: NsRange) -> ());
            }
            TextPoke::Enter => {
                let multiline = FIELDS.with(|f| {
                    f.borrow()
                        .get(&(control as usize))
                        .is_some_and(|f| f.multiline)
                });
                if multiline {
                    let s = nsstring("\n");
                    msg!(control, "insertText:", (s: Id) -> ());
                } else {
                    let d: Id = msg!(control, "delegate", () -> Id);
                    msg!(d, "textFieldShouldReturn:", (control: Id) -> bool);
                }
            }
        }
    }
    // UIKit's own notifications may not have come yet.
    poll(control);
    true
}
