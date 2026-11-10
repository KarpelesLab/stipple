//! The input method's side of a text field: the host view's
//! `onCreateInputConnection`, and the `InputConnection` it returns, whose
//! methods (native, see [`classes`](super::classes)) run on the main thread.
//!
//! The editable they read and edit is an [`Editor`], shared with the UI
//! thread: its edits go to the field as [`Event::Ime`] (through the command
//! queue), and the field's state comes back through [`Window::set_ime`]
//! (`sync`), after which the input method hears of selection moves
//! (`updateSelection`) and of changes it didn't make (`restartInput`). Key
//! events an input method sends (`sendKeyEvent`, Backspace and Enter on most
//! keyboards) stay with `BaseInputConnection`, which posts them to the
//! window's input queue like any key.
//!
//! [`Window::set_ime`]: crate::platform::Window::set_ime

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use super::classes;
use super::jni::{self, Env, JObject, JValue, Jni};
use super::{Command, log, post_command};
use crate::platform::event::{ButtonState, Event, KeyCode, Modifiers};
use crate::platform::input_connection::{Editor, Sync};
use crate::platform::window::ImeState;

static EDITOR: Mutex<Editor> = Mutex::new(Editor::EMPTY);

/// Whether a text input has focus, so the keyboard should show and the view
/// is a text editor.
static TEXT_INPUT: AtomicBool = AtomicBool::new(false);

fn editor() -> std::sync::MutexGuard<'static, Editor> {
    EDITOR.lock().unwrap_or_else(|e| e.into_inner())
}

// EditorInfo constants.
const TYPE_CLASS_TEXT: i32 = 0x1;
const TYPE_TEXT_FLAG_CAP_SENTENCES: i32 = 0x4000;
const TYPE_TEXT_VARIATION_PASSWORD: i32 = 0x80;
const IME_ACTION_DONE: i32 = 6;
const IME_ACTION_NEXT: i32 = 5;
const IME_ACTION_PREVIOUS: i32 = 7;
const IME_FLAG_NO_FULLSCREEN: i32 = 0x0200_0000;
const IME_FLAG_NO_EXTRACT_UI: i32 = 0x1000_0000;

// ---- The UI thread's side ------------------------------------------------------

/// The field's state changed (see [`Editor::sync`]): what to tell the input
/// method.
pub(super) fn sync(state: Option<&ImeState>) -> Sync {
    editor().sync(state)
}

/// Whether a text input has focus. Returns whether that changed.
pub(super) fn set_text_input(active: bool) -> bool {
    TEXT_INPUT.swap(active, Ordering::AcqRel) != active
}

/// A key is about to reach the field (see [`Editor::key_typed`]): returns
/// the edit that ends a composition first, and has the input method hear of
/// it.
pub(super) fn key_typed() -> Vec<crate::platform::event::Ime> {
    let edits = editor().key_typed();
    if !edits.is_empty() {
        let current = super::CURRENT
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(instance) = current {
            instance.request(|r| {
                r.ime.get_or_insert(Sync::Selection);
            });
        }
    }
    edits
}

/// An edit is about to reach the field.
pub(super) fn handled() {
    editor().handled();
}

// ---- The main thread's side ------------------------------------------------------

fn input_method_manager(jni: &Jni, activity: JObject) -> Option<JObject> {
    let name = jni.string("input_method")?;
    jni.call_object(
        activity,
        c"getSystemService",
        c"(Ljava/lang/String;)Ljava/lang/Object;",
        &[JValue { l: name }],
    )
}

/// Tell the input method what changed in the field (`restartInput` or
/// `updateSelection`). Returns `false` without a host view.
pub(super) fn notify(jni: &Jni, what: Sync) -> bool {
    let host = classes::host();
    let view = host.view.get();
    if view.is_null() {
        return false;
    }
    (|| {
        let imm = input_method_manager(jni, host.activity.get())?;
        match what {
            Sync::Unchanged => Some(()),
            Sync::Restart => jni.call_void(
                imm,
                c"restartInput",
                c"(Landroid/view/View;)V",
                &[JValue { l: view }],
            ),
            Sync::Selection => {
                let (s, e, cs, ce) = {
                    let ed = editor();
                    let (s, e) = ed.selection16();
                    let (cs, ce) = ed.composing16();
                    (s, e, cs, ce)
                };
                jni.call_void(
                    imm,
                    c"updateSelection",
                    c"(Landroid/view/View;IIII)V",
                    &[
                        JValue { l: view },
                        JValue { i: s },
                        JValue { i: e },
                        JValue { i: cs },
                        JValue { i: ce },
                    ],
                )
            }
        }
    })();
    true
}

/// Show or hide the soft keyboard for the host view. Returns `false` without
/// one (the caller falls back to the decor view).
pub(super) fn soft_input(jni: &Jni, show: bool) -> bool {
    let host = classes::host();
    let view = host.view.get();
    if view.is_null() {
        return false;
    }
    (|| {
        let imm = input_method_manager(jni, host.activity.get())?;
        if show {
            jni.call_bool(view, c"requestFocus", c"()Z", &[])?;
            jni.call_bool(
                imm,
                c"showSoftInput",
                c"(Landroid/view/View;I)Z",
                &[JValue { l: view }, JValue { i: 0 }],
            )
        } else {
            let token = jni.call_object(view, c"getWindowToken", c"()Landroid/os/IBinder;", &[])?;
            jni.call_bool(
                imm,
                c"hideSoftInputFromWindow",
                c"(Landroid/os/IBinder;I)Z",
                &[JValue { l: token }, JValue { i: 0 }],
            )
        }
    })()
    .is_some()
}

/// Drive the host view's `InputConnection` from the app, the way an input
/// method would (through the Java objects, so through the natives' binding),
/// and log what the editable reads back. For CI, which has no composing
/// keyboard to type with; needs a focused text field.
pub(super) fn self_test(jni: &Jni) {
    let view = classes::host().view.get();
    let result = (|| {
        let info = jni.new_named(c"android/view/inputmethod/EditorInfo", c"()V", &[])?;
        let ic = jni.call_object(
            view,
            c"onCreateInputConnection",
            c"(Landroid/view/inputmethod/EditorInfo;)Landroid/view/inputmethod/InputConnection;",
            &[JValue { l: info }],
        );
        let Some(ic) = ic else {
            return Some("no connection (no text field has focus)".to_string());
        };
        let input_type = jni.int_field(info, c"inputType")?;
        let text = |s: &str| jni.string16(s);
        let seq = c"(Ljava/lang/CharSequence;I)Z";
        jni.call_bool(
            ic,
            c"setComposingText",
            seq,
            &[JValue { l: text("にほ")? }, JValue { i: 1 }],
        )?;
        let composing = before(jni, ic)?;
        jni.call_bool(
            ic,
            c"commitText",
            seq,
            &[
                JValue {
                    l: text("日本🎉")?
                },
                JValue { i: 1 },
            ],
        )?;
        let committed = before(jni, ic)?;
        // The emoji is two UTF-16 units.
        jni.call_bool(
            ic,
            c"deleteSurroundingText",
            c"(II)Z",
            &[JValue { i: 2 }, JValue { i: 0 }],
        )?;
        let deleted = before(jni, ic)?;
        jni.call_bool(
            ic,
            c"setComposingText",
            seq,
            &[JValue { l: text("!")? }, JValue { i: 1 }],
        )?;
        jni.call_bool(ic, c"finishComposingText", c"()Z", &[])?;
        let finished = before(jni, ic)?;
        Some(format!(
            "inputType 0x{input_type:x}, composing {composing:?}, committed {committed:?}, \
             deleted {deleted:?}, finished {finished:?}"
        ))
    })();
    log(&format!(
        "stipple: input connection self-test: {}",
        result.as_deref().unwrap_or("failed")
    ));
}

/// `getTextBeforeCursor(64, 0)`, read back.
fn before(jni: &Jni, ic: JObject) -> Option<String> {
    let s = jni.call_object(
        ic,
        c"getTextBeforeCursor",
        c"(II)Ljava/lang/CharSequence;",
        &[JValue { i: 64 }, JValue { i: 0 }],
    )?;
    jni.char_sequence(s)
}

// ---- The natives ---------------------------------------------------------------------

/// Send the editor's edits to the field.
fn send(events: Vec<crate::platform::event::Ime>) {
    for ime in events {
        post_command(Command::Ime(ime));
    }
}

/// Press and release `code` with `modifiers`, as the field's key handling
/// takes it.
fn key(code: KeyCode, modifiers: Modifiers) {
    for state in [ButtonState::Pressed, ButtonState::Released] {
        post_command(Command::Deliver(Event::Key {
            code,
            state,
            modifiers,
        }));
    }
}

fn boolean(b: bool) -> u8 {
    u8::from(b)
}

pub(super) unsafe extern "C" fn on_check_is_text_editor(_env: Env, _this: JObject) -> u8 {
    boolean(TEXT_INPUT.load(Ordering::Acquire))
}

pub(super) unsafe extern "C" fn on_create_input_connection(
    env: Env,
    this: JObject,
    info: JObject,
) -> JObject {
    if !TEXT_INPUT.load(Ordering::Acquire) {
        return std::ptr::null_mut();
    }
    unsafe {
        jni::in_native_object(env, |jni| {
            // A password field keeps its text from the input method: no
            // editor state, and a password keyboard that learns nothing.
            let (password, (start, end)) = {
                let ed = editor();
                (!ed.is_focused(), ed.selection16())
            };
            let input_type = if password {
                TYPE_CLASS_TEXT | TYPE_TEXT_VARIATION_PASSWORD
            } else {
                TYPE_CLASS_TEXT | TYPE_TEXT_FLAG_CAP_SENTENCES
            };
            jni.set_int_field(info, c"inputType", input_type)?;
            jni.set_int_field(
                info,
                c"imeOptions",
                IME_ACTION_DONE | IME_FLAG_NO_FULLSCREEN | IME_FLAG_NO_EXTRACT_UI,
            )?;
            jni.set_int_field(info, c"initialSelStart", start)?;
            jni.set_int_field(info, c"initialSelEnd", end)?;
            let ic = jni.new_object(
                classes::input_class()?,
                c"(Landroid/view/View;Z)V",
                &[JValue { l: this }, JValue { z: 0 }],
            )?;
            log(&format!(
                "stipple: input connection created (inputType 0x{input_type:x}, selection {start}..{end})"
            ));
            Some(ic)
        })
    }
}

/// The text of a `CharSequence` argument.
fn text_of(env: Env, cs: JObject) -> Option<String> {
    unsafe { jni::in_native(env, |jni| jni.char_sequence(cs)) }
}

/// A Java string to return as a `CharSequence`.
fn java_string(env: Env, s: Option<String>) -> JObject {
    unsafe { jni::in_native_object(env, |jni| jni.string16(&s?)) }
}

pub(super) unsafe extern "C" fn commit_text(
    env: Env,
    _this: JObject,
    text: JObject,
    _cursor: i32,
) -> u8 {
    let text = text_of(env, text).unwrap_or_default();
    send(editor().commit(&text));
    1
}

pub(super) unsafe extern "C" fn set_composing_text(
    env: Env,
    _this: JObject,
    text: JObject,
    _cursor: i32,
) -> u8 {
    let text = text_of(env, text).unwrap_or_default();
    send(editor().set_composing(&text));
    1
}

pub(super) unsafe extern "C" fn finish_composing_text(_env: Env, _this: JObject) -> u8 {
    send(editor().finish_composing());
    1
}

pub(super) unsafe extern "C" fn delete_surrounding_text(
    _env: Env,
    _this: JObject,
    before: i32,
    after: i32,
) -> u8 {
    send(editor().delete_surrounding(before.max(0) as usize, after.max(0) as usize));
    1
}

pub(super) unsafe extern "C" fn delete_surrounding_text_in_code_points(
    _env: Env,
    _this: JObject,
    before: i32,
    after: i32,
) -> u8 {
    send(editor().delete_surrounding_chars(before.max(0) as usize, after.max(0) as usize));
    1
}

pub(super) unsafe extern "C" fn set_composing_region(
    _env: Env,
    _this: JObject,
    start: i32,
    end: i32,
) -> u8 {
    let (edits, applied) =
        editor().set_composing_region(start.max(0) as usize, end.max(0) as usize);
    send(edits);
    u8::from(applied)
}

pub(super) unsafe extern "C" fn set_selection(
    _env: Env,
    _this: JObject,
    start: i32,
    end: i32,
) -> u8 {
    let Some((moved, select)) = editor().set_selection(start.max(0) as usize, end.max(0) as usize)
    else {
        return 0;
    };
    let code = if moved < 0 {
        KeyCode::ArrowLeft
    } else {
        KeyCode::ArrowRight
    };
    for _ in 0..moved.unsigned_abs() {
        key(code, Modifiers::default());
    }
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    for _ in 0..select {
        key(KeyCode::ArrowRight, shift);
    }
    1
}

pub(super) unsafe extern "C" fn get_text_before_cursor(
    env: Env,
    _this: JObject,
    n: i32,
    _flags: i32,
) -> JObject {
    let text = editor().text_before(n.max(0) as usize);
    java_string(env, Some(text))
}

pub(super) unsafe extern "C" fn get_text_after_cursor(
    env: Env,
    _this: JObject,
    n: i32,
    _flags: i32,
) -> JObject {
    let text = editor().text_after(n.max(0) as usize);
    java_string(env, Some(text))
}

pub(super) unsafe extern "C" fn get_selected_text(
    env: Env,
    _this: JObject,
    _flags: i32,
) -> JObject {
    let text = editor().selected();
    java_string(env, text)
}

pub(super) unsafe extern "C" fn get_cursor_caps_mode(_env: Env, _this: JObject, req: i32) -> i32 {
    editor().caps_mode(req)
}

/// The keyboard's action key (Done, Next, ...) as the key it stands for:
/// Next and Previous move focus like Tab and Shift+Tab, the rest press Enter.
pub(super) unsafe extern "C" fn perform_editor_action(
    _env: Env,
    _this: JObject,
    action: i32,
) -> u8 {
    send(editor().finish_composing());
    match action {
        IME_ACTION_NEXT => key(KeyCode::Tab, Modifiers::default()),
        IME_ACTION_PREVIOUS => key(
            KeyCode::Tab,
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        ),
        _ => key(KeyCode::Enter, Modifiers::default()),
    }
    1
}

/// The input method's Select all, Cut, Copy and Paste (`android.R.id.*`),
/// as their shortcuts.
pub(super) unsafe extern "C" fn perform_context_menu_action(
    _env: Env,
    _this: JObject,
    id: i32,
) -> u8 {
    let c = match id {
        0x0102_001f => 'a',
        0x0102_0020 => 'x',
        0x0102_0021 => 'c',
        0x0102_0022 => 'v',
        _ => return 0,
    };
    send(editor().finish_composing());
    key(
        KeyCode::Char(c),
        Modifiers {
            ctrl: true,
            ..Modifiers::default()
        },
    );
    1
}
