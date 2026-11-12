//! Just enough hand-written JNI to reach the few Java APIs the NDK doesn't
//! cover: the clipboard (`ClipboardManager`), the character a key types
//! (`KeyCharacterMap`), the system accent color (`Resources.getColor`), the
//! animator duration scale (`Settings.Global`) and opening URLs
//! (`Intent.ACTION_VIEW`).
//!
//! `JavaVM` and `JNIEnv` are pointers to function tables whose layout is fixed
//! by the JNI specification, so each function is read from its index in the
//! table. Every call that can throw is followed by an exception check, which
//! clears the exception and gives up: a missing API on an old release then
//! reads as "not available" instead of crashing.

use std::cell::Cell;
use std::ffi::{CStr, c_char, c_void};

pub(super) type JObject = *mut c_void;
type JMethod = *mut c_void;
/// A `JNIEnv*`.
pub(super) type Env = *mut c_void;

/// A JNI `jvalue`: one argument of a `Call*MethodA` call.
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) union JValue {
    pub(super) i: i32,
    pub(super) f: f32,
    pub(super) l: JObject,
    pub(super) z: u8,
}

const JNI_OK: i32 = 0;
const JNI_VERSION_1_6: i32 = 0x0001_0006;

// `JNIInvokeInterface` (the `JavaVM` table) indices.
const VM_ATTACH_CURRENT_THREAD: usize = 4;
const VM_DETACH_CURRENT_THREAD: usize = 5;
const VM_GET_ENV: usize = 6;

// `JNINativeInterface` (the `JNIEnv` table) indices.
const FIND_CLASS: usize = 6;
const EXCEPTION_DESCRIBE: usize = 16;
const EXCEPTION_CLEAR: usize = 17;
const PUSH_LOCAL_FRAME: usize = 19;
const POP_LOCAL_FRAME: usize = 20;
const NEW_GLOBAL_REF: usize = 21;
const DELETE_GLOBAL_REF: usize = 22;
const NEW_OBJECT_A: usize = 30;
const GET_OBJECT_CLASS: usize = 31;
const GET_METHOD_ID: usize = 33;
const CALL_OBJECT_METHOD_A: usize = 36;
const CALL_BOOLEAN_METHOD_A: usize = 39;
const CALL_INT_METHOD_A: usize = 51;
const CALL_FLOAT_METHOD_A: usize = 57;
const CALL_VOID_METHOD_A: usize = 63;
const GET_FIELD_ID: usize = 94;
const GET_INT_FIELD: usize = 100;
const GET_STATIC_METHOD_ID: usize = 113;
const CALL_STATIC_OBJECT_METHOD_A: usize = 116;
const SET_INT_FIELD: usize = 109;
const CALL_STATIC_FLOAT_METHOD_A: usize = 137;
const NEW_STRING: usize = 163;
const GET_STRING_LENGTH: usize = 164;
const GET_STRING_CHARS: usize = 165;
const RELEASE_STRING_CHARS: usize = 166;
const NEW_STRING_UTF: usize = 167;
const GET_STRING_UTF_CHARS: usize = 169;
const RELEASE_STRING_UTF_CHARS: usize = 170;
const NEW_INT_ARRAY: usize = 179;
const IS_SAME_OBJECT: usize = 24;
const GET_STATIC_FIELD_ID: usize = 144;
const GET_STATIC_INT_FIELD: usize = 150;
const GET_ARRAY_LENGTH: usize = 171;
const NEW_OBJECT_ARRAY: usize = 172;
const SET_OBJECT_ARRAY_ELEMENT: usize = 174;
const GET_INT_ARRAY_REGION: usize = 203;
const REGISTER_NATIVES: usize = 215;
const EXCEPTION_CHECK: usize = 228;
const NEW_DIRECT_BYTE_BUFFER: usize = 229;

/// Read entry `index` of the function table behind `ptr` (a `JavaVM*` or
/// `JNIEnv*`, each a pointer to a pointer to the table).
unsafe fn slot(ptr: *mut c_void, index: usize) -> *const c_void {
    unsafe {
        let table = *(ptr as *const *const *const c_void);
        *table.add(index)
    }
}

thread_local! {
    /// The env of a thread attached for the whole event loop, if any.
    static ENV: Cell<Env> = const { Cell::new(std::ptr::null_mut()) };
}

/// Attach the calling thread (the UI thread) to `vm` for as long as the event
/// loop runs. Undone by [`detach`].
pub(super) unsafe fn attach(vm: *mut c_void) {
    if vm.is_null() {
        return;
    }
    let mut env: Env = std::ptr::null_mut();
    let ok = unsafe {
        let f: unsafe extern "C" fn(*mut c_void, *mut Env, *mut c_void) -> i32 =
            std::mem::transmute(slot(vm, VM_ATTACH_CURRENT_THREAD));
        f(vm, &mut env, std::ptr::null_mut())
    };
    if ok == JNI_OK {
        ENV.with(|e| e.set(env));
    }
}

/// Detach the UI thread again. ART aborts the process if a native thread
/// exits while still attached.
pub(super) unsafe fn detach(vm: *mut c_void) {
    if vm.is_null() || ENV.with(|e| e.replace(std::ptr::null_mut())).is_null() {
        return;
    }
    unsafe {
        let f: unsafe extern "C" fn(*mut c_void) -> i32 =
            std::mem::transmute(slot(vm, VM_DETACH_CURRENT_THREAD));
        f(vm);
    }
}

/// Run `f` with an env for the calling thread, attaching it for the call if
/// it isn't attached already, inside a local reference frame.
pub(super) unsafe fn with_env<R>(vm: *mut c_void, f: impl FnOnce(&Jni) -> Option<R>) -> Option<R> {
    if vm.is_null() {
        return None;
    }
    unsafe {
        let mut env = ENV.with(Cell::get);
        let mut attached_here = false;
        if env.is_null() {
            let get_env: unsafe extern "C" fn(*mut c_void, *mut Env, i32) -> i32 =
                std::mem::transmute(slot(vm, VM_GET_ENV));
            if get_env(vm, &mut env, JNI_VERSION_1_6) != JNI_OK {
                let attach: unsafe extern "C" fn(*mut c_void, *mut Env, *mut c_void) -> i32 =
                    std::mem::transmute(slot(vm, VM_ATTACH_CURRENT_THREAD));
                if attach(vm, &mut env, std::ptr::null_mut()) != JNI_OK {
                    return None;
                }
                attached_here = true;
            }
        }
        let jni = Jni(env);
        let result = if jni.push_frame() {
            let r = f(&jni);
            jni.clear();
            jni.pop_frame();
            r
        } else {
            None
        };
        if attached_here {
            let detach: unsafe extern "C" fn(*mut c_void) -> i32 =
                std::mem::transmute(slot(vm, VM_DETACH_CURRENT_THREAD));
            detach(vm);
        }
        result
    }
}

/// A `JNIEnv*` with typed wrappers for the calls used here. Each returns
/// `None` (with the exception cleared) when the call threw or found nothing.
pub(super) struct Jni(pub(super) Env);

impl Jni {
    pub(super) unsafe fn f(&self, index: usize) -> *const c_void {
        unsafe { slot(self.0, index) }
    }

    /// Whether an exception is pending; clears it.
    pub(super) fn clear(&self) -> bool {
        unsafe {
            let check: unsafe extern "C" fn(Env) -> u8 =
                std::mem::transmute(self.f(EXCEPTION_CHECK));
            if check(self.0) == 0 {
                return false;
            }
            let clear: unsafe extern "C" fn(Env) = std::mem::transmute(self.f(EXCEPTION_CLEAR));
            clear(self.0);
            true
        }
    }

    pub(super) fn ok<T>(&self, value: *mut T) -> Option<*mut T> {
        (!self.clear() && !value.is_null()).then_some(value)
    }

    pub(super) fn push_frame(&self) -> bool {
        unsafe {
            let f: unsafe extern "C" fn(Env, i32) -> i32 =
                std::mem::transmute(self.f(PUSH_LOCAL_FRAME));
            f(self.0, 32) == JNI_OK
        }
    }

    pub(super) fn pop_frame(&self) {
        unsafe {
            let f: unsafe extern "C" fn(Env, JObject) -> JObject =
                std::mem::transmute(self.f(POP_LOCAL_FRAME));
            f(self.0, std::ptr::null_mut());
        }
    }

    pub(super) fn find_class(&self, name: &CStr) -> Option<JObject> {
        unsafe {
            let f: unsafe extern "C" fn(Env, *const c_char) -> JObject =
                std::mem::transmute(self.f(FIND_CLASS));
            self.ok(f(self.0, name.as_ptr()))
        }
    }

    pub(super) fn class_of(&self, obj: JObject) -> Option<JObject> {
        unsafe {
            let f: unsafe extern "C" fn(Env, JObject) -> JObject =
                std::mem::transmute(self.f(GET_OBJECT_CLASS));
            self.ok(f(self.0, obj))
        }
    }

    pub(super) fn method(&self, class: JObject, name: &CStr, sig: &CStr) -> Option<JMethod> {
        unsafe {
            let f: unsafe extern "C" fn(Env, JObject, *const c_char, *const c_char) -> JMethod =
                std::mem::transmute(self.f(GET_METHOD_ID));
            self.ok(f(self.0, class, name.as_ptr(), sig.as_ptr()))
        }
    }

    pub(super) fn static_method(&self, class: JObject, name: &CStr, sig: &CStr) -> Option<JMethod> {
        unsafe {
            let f: unsafe extern "C" fn(Env, JObject, *const c_char, *const c_char) -> JMethod =
                std::mem::transmute(self.f(GET_STATIC_METHOD_ID));
            self.ok(f(self.0, class, name.as_ptr(), sig.as_ptr()))
        }
    }

    /// Call an object-returning instance method, looked up on `obj`'s class.
    pub(super) fn call_object(
        &self,
        obj: JObject,
        name: &CStr,
        sig: &CStr,
        args: &[JValue],
    ) -> Option<JObject> {
        let m = self.method(self.class_of(obj)?, name, sig)?;
        unsafe {
            let f: unsafe extern "C" fn(Env, JObject, JMethod, *const JValue) -> JObject =
                std::mem::transmute(self.f(CALL_OBJECT_METHOD_A));
            self.ok(f(self.0, obj, m, args.as_ptr()))
        }
    }

    pub(super) fn call_int(
        &self,
        obj: JObject,
        name: &CStr,
        sig: &CStr,
        args: &[JValue],
    ) -> Option<i32> {
        let m = self.method(self.class_of(obj)?, name, sig)?;
        unsafe {
            let f: unsafe extern "C" fn(Env, JObject, JMethod, *const JValue) -> i32 =
                std::mem::transmute(self.f(CALL_INT_METHOD_A));
            let r = f(self.0, obj, m, args.as_ptr());
            (!self.clear()).then_some(r)
        }
    }

    pub(super) fn call_bool(
        &self,
        obj: JObject,
        name: &CStr,
        sig: &CStr,
        args: &[JValue],
    ) -> Option<bool> {
        let m = self.method(self.class_of(obj)?, name, sig)?;
        unsafe {
            let f: unsafe extern "C" fn(Env, JObject, JMethod, *const JValue) -> u8 =
                std::mem::transmute(self.f(CALL_BOOLEAN_METHOD_A));
            let r = f(self.0, obj, m, args.as_ptr());
            (!self.clear()).then_some(r != 0)
        }
    }

    /// Read the `int` field `name` of `obj`.
    pub(super) fn int_field(&self, obj: JObject, name: &CStr) -> Option<i32> {
        let class = self.class_of(obj)?;
        unsafe {
            let get_id: unsafe extern "C" fn(
                Env,
                JObject,
                *const c_char,
                *const c_char,
            ) -> JMethod = std::mem::transmute(self.f(GET_FIELD_ID));
            let id = self.ok(get_id(self.0, class, name.as_ptr(), c"I".as_ptr()))?;
            let get: unsafe extern "C" fn(Env, JObject, JMethod) -> i32 =
                std::mem::transmute(self.f(GET_INT_FIELD));
            let v = get(self.0, obj, id);
            (!self.clear()).then_some(v)
        }
    }

    pub(super) fn call_void(
        &self,
        obj: JObject,
        name: &CStr,
        sig: &CStr,
        args: &[JValue],
    ) -> Option<()> {
        let m = self.method(self.class_of(obj)?, name, sig)?;
        unsafe {
            let f: unsafe extern "C" fn(Env, JObject, JMethod, *const JValue) =
                std::mem::transmute(self.f(CALL_VOID_METHOD_A));
            f(self.0, obj, m, args.as_ptr());
        }
        (!self.clear()).then_some(())
    }

    pub(super) fn call_static_object(
        &self,
        class: &CStr,
        name: &CStr,
        sig: &CStr,
        args: &[JValue],
    ) -> Option<JObject> {
        let class = self.find_class(class)?;
        let m = self.static_method(class, name, sig)?;
        unsafe {
            let f: unsafe extern "C" fn(Env, JObject, JMethod, *const JValue) -> JObject =
                std::mem::transmute(self.f(CALL_STATIC_OBJECT_METHOD_A));
            self.ok(f(self.0, class, m, args.as_ptr()))
        }
    }

    pub(super) fn call_static_float(
        &self,
        class: &CStr,
        name: &CStr,
        sig: &CStr,
        args: &[JValue],
    ) -> Option<f32> {
        let class = self.find_class(class)?;
        let m = self.static_method(class, name, sig)?;
        unsafe {
            let f: unsafe extern "C" fn(Env, JObject, JMethod, *const JValue) -> f32 =
                std::mem::transmute(self.f(CALL_STATIC_FLOAT_METHOD_A));
            let r = f(self.0, class, m, args.as_ptr());
            (!self.clear()).then_some(r)
        }
    }

    /// A Java string from `s`. JNI's "modified UTF-8" differs from UTF-8 only
    /// for NUL and characters beyond the BMP; NULs end the string here.
    pub(super) fn string(&self, s: &str) -> Option<JObject> {
        let s: String = s.chars().take_while(|&c| c != '\0').collect();
        let c = std::ffi::CString::new(s).ok()?;
        unsafe {
            let f: unsafe extern "C" fn(Env, *const c_char) -> JObject =
                std::mem::transmute(self.f(NEW_STRING_UTF));
            self.ok(f(self.0, c.as_ptr()))
        }
    }

    pub(super) fn to_rust(&self, s: JObject) -> Option<String> {
        unsafe {
            let get: unsafe extern "C" fn(Env, JObject, *mut u8) -> *const c_char =
                std::mem::transmute(self.f(GET_STRING_UTF_CHARS));
            let chars = get(self.0, s, std::ptr::null_mut());
            if chars.is_null() {
                self.clear();
                return None;
            }
            let out = CStr::from_ptr(chars).to_string_lossy().into_owned();
            let release: unsafe extern "C" fn(Env, JObject, *const c_char) =
                std::mem::transmute(self.f(RELEASE_STRING_UTF_CHARS));
            release(self.0, s, chars);
            Some(out)
        }
    }
}

/// `context.getSystemService("clipboard")`.
fn clipboard_manager(jni: &Jni, context: JObject) -> Option<JObject> {
    let name = jni.string("clipboard")?;
    jni.call_object(
        context,
        c"getSystemService",
        c"(Ljava/lang/String;)Ljava/lang/Object;",
        &[JValue { l: name }],
    )
}

/// The clipboard's text, through `ClipboardManager.getPrimaryClip()`.
pub(super) unsafe fn clipboard_text(vm: *mut c_void, context: JObject) -> Option<String> {
    unsafe {
        with_env(vm, |jni| {
            let cm = clipboard_manager(jni, context)?;
            let clip =
                jni.call_object(cm, c"getPrimaryClip", c"()Landroid/content/ClipData;", &[])?;
            let item = jni.call_object(
                clip,
                c"getItemAt",
                c"(I)Landroid/content/ClipData$Item;",
                &[JValue { i: 0 }],
            )?;
            let text = jni.call_object(
                item,
                c"coerceToText",
                c"(Landroid/content/Context;)Ljava/lang/CharSequence;",
                &[JValue { l: context }],
            )?;
            let s = jni.call_object(text, c"toString", c"()Ljava/lang/String;", &[])?;
            jni.to_rust(s)
        })
    }
}

/// Replace the clipboard with `text`, as `ClipData.newPlainText`.
pub(super) unsafe fn set_clipboard_text(vm: *mut c_void, context: JObject, text: &str) -> bool {
    unsafe {
        with_env(vm, |jni| {
            let cm = clipboard_manager(jni, context)?;
            let label = jni.string("text")?;
            let text = jni.string(text)?;
            let clip = jni.call_static_object(
                c"android/content/ClipData",
                c"newPlainText",
                c"(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Landroid/content/ClipData;",
                &[JValue { l: label }, JValue { l: text }],
            )?;
            jni.call_void(
                cm,
                c"setPrimaryClip",
                c"(Landroid/content/ClipData;)V",
                &[JValue { l: clip }],
            )
        })
        .is_some()
    }
}

/// The character key `key_code` types on keyboard `device` with `meta_state`
/// held, from the device's `KeyCharacterMap`: what `KeyEvent.getUnicodeChar`
/// returns, which the NDK doesn't expose. `None` for keys that type nothing
/// and for dead keys.
pub(super) unsafe fn unicode_char(
    vm: *mut c_void,
    device: i32,
    key_code: i32,
    meta_state: i32,
) -> Option<char> {
    unsafe {
        with_env(vm, |jni| {
            let map = jni.call_static_object(
                c"android/view/KeyCharacterMap",
                c"load",
                c"(I)Landroid/view/KeyCharacterMap;",
                &[JValue { i: device }],
            )?;
            let c = jni.call_int(
                map,
                c"get",
                c"(II)I",
                &[JValue { i: key_code }, JValue { i: meta_state }],
            )?;
            // KeyCharacterMap.COMBINING_ACCENT marks a dead key.
            if c <= 0 || c as u32 & 0x8000_0000 != 0 {
                return None;
            }
            char::from_u32(c as u32)
        })
    }
}

/// The screen density in dpi and whether night mode is on, from
/// `context.getResources().getConfiguration()`.
pub(super) unsafe fn configuration(vm: *mut c_void, context: JObject) -> Option<(i32, bool)> {
    // Configuration.UI_MODE_NIGHT_MASK and UI_MODE_NIGHT_YES.
    const NIGHT_MASK: i32 = 0x30;
    const NIGHT_YES: i32 = 0x20;
    unsafe {
        with_env(vm, |jni| {
            let res = jni.call_object(
                context,
                c"getResources",
                c"()Landroid/content/res/Resources;",
                &[],
            )?;
            let config = jni.call_object(
                res,
                c"getConfiguration",
                c"()Landroid/content/res/Configuration;",
                &[],
            )?;
            let dpi = jni.int_field(config, c"densityDpi")?;
            let ui_mode = jni.int_field(config, c"uiMode")?;
            Some((dpi, ui_mode & NIGHT_MASK == NIGHT_YES))
        })
    }
}

/// The system's animator duration scale, `Settings.Global.ANIMATOR_DURATION_SCALE`
/// (developer options, and *Remove animations* in the accessibility settings,
/// which sets it to 0). 1 when unset.
pub(super) unsafe fn animator_duration_scale(vm: *mut c_void, context: JObject) -> Option<f32> {
    unsafe {
        with_env(vm, |jni| {
            let resolver = jni.call_object(
                context,
                c"getContentResolver",
                c"()Landroid/content/ContentResolver;",
                &[],
            )?;
            let name = jni.string("animator_duration_scale")?;
            jni.call_static_float(
                c"android/provider/Settings$Global",
                c"getFloat",
                c"(Landroid/content/ContentResolver;Ljava/lang/String;F)F",
                &[
                    JValue { l: resolver },
                    JValue { l: name },
                    JValue { f: 1.0 },
                ],
            )
        })
    }
}

/// Color the status and navigation bars `argb` (the app's background), with
/// dark icons on a light one. Call on the main thread: these are `View` and
/// `Window` methods.
pub(super) unsafe fn style_system_bars(
    vm: *mut c_void,
    activity: JObject,
    sdk: i32,
    argb: u32,
    light: bool,
) -> bool {
    // WindowManager.LayoutParams.FLAG_DRAWS_SYSTEM_BAR_BACKGROUNDS.
    const DRAWS_SYSTEM_BAR_BACKGROUNDS: i32 = 0x8000_0000_u32 as i32;
    // View.SYSTEM_UI_FLAG_LIGHT_STATUS_BAR (API 23), _LIGHT_NAVIGATION_BAR (26).
    const LIGHT_STATUS_BAR: i32 = 0x2000;
    const LIGHT_NAVIGATION_BAR: i32 = 0x10;
    unsafe {
        with_env(vm, |jni| {
            let window =
                jni.call_object(activity, c"getWindow", c"()Landroid/view/Window;", &[])?;
            jni.call_void(
                window,
                c"addFlags",
                c"(I)V",
                &[JValue {
                    i: DRAWS_SYSTEM_BAR_BACKGROUNDS,
                }],
            )?;
            let color = [JValue { i: argb as i32 }];
            jni.call_void(window, c"setStatusBarColor", c"(I)V", &color)?;
            jni.call_void(window, c"setNavigationBarColor", c"(I)V", &color)?;
            let decor = jni.call_object(window, c"getDecorView", c"()Landroid/view/View;", &[])?;
            let mut mask = LIGHT_STATUS_BAR;
            if sdk >= 26 {
                mask |= LIGHT_NAVIGATION_BAR;
            }
            let flags = jni.call_int(decor, c"getSystemUiVisibility", c"()I", &[])?;
            let flags = if light { flags | mask } else { flags & !mask };
            jni.call_void(
                decor,
                c"setSystemUiVisibility",
                c"(I)V",
                &[JValue { i: flags }],
            )
        })
        .is_some()
    }
}

/// Show or hide the soft keyboard through `InputMethodManager`, for the
/// activity's decor view. Call on the main thread.
pub(super) unsafe fn soft_input(vm: *mut c_void, activity: JObject, show: bool) -> bool {
    unsafe {
        with_env(vm, |jni| {
            let name = jni.string("input_method")?;
            let imm = jni.call_object(
                activity,
                c"getSystemService",
                c"(Ljava/lang/String;)Ljava/lang/Object;",
                &[JValue { l: name }],
            )?;
            let window =
                jni.call_object(activity, c"getWindow", c"()Landroid/view/Window;", &[])?;
            let decor = jni.call_object(window, c"getDecorView", c"()Landroid/view/View;", &[])?;
            if show {
                jni.call_bool(
                    imm,
                    c"showSoftInput",
                    c"(Landroid/view/View;I)Z",
                    &[JValue { l: decor }, JValue { i: 0 }],
                )
            } else {
                let token =
                    jni.call_object(decor, c"getWindowToken", c"()Landroid/os/IBinder;", &[])?;
                jni.call_bool(
                    imm,
                    c"hideSoftInputFromWindow",
                    c"(Landroid/os/IBinder;I)Z",
                    &[JValue { l: token }, JValue { i: 0 }],
                )
            }
        })
        .unwrap_or(false)
    }
}

/// The system accent color as `0xAARRGGBB`: Material You's
/// `system_accent1_600` (light) or `_200` (dark), on Android 12 and later.
pub(super) unsafe fn accent_color(vm: *mut c_void, context: JObject, dark: bool) -> Option<u32> {
    unsafe {
        with_env(vm, |jni| {
            let res = jni.call_object(
                context,
                c"getResources",
                c"()Landroid/content/res/Resources;",
                &[],
            )?;
            let name = jni.string(if dark {
                "system_accent1_200"
            } else {
                "system_accent1_600"
            })?;
            let kind = jni.string("color")?;
            let package = jni.string("android")?;
            let id = jni.call_int(
                res,
                c"getIdentifier",
                c"(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)I",
                &[
                    JValue { l: name },
                    JValue { l: kind },
                    JValue { l: package },
                ],
            )?;
            if id == 0 {
                return None;
            }
            let color = jni.call_int(
                res,
                c"getColor",
                c"(ILandroid/content/res/Resources$Theme;)I",
                &[
                    JValue { i: id },
                    JValue {
                        l: std::ptr::null_mut(),
                    },
                ],
            )?;
            Some(color as u32)
        })
    }
}

/// Open `url` with whatever handles it: `startActivity(new
/// Intent(ACTION_VIEW, Uri.parse(url)))`, in a new task. `false` when no
/// activity can (`ActivityNotFoundException`) or the URL doesn't parse.
pub(super) unsafe fn open_url(vm: *mut c_void, activity: JObject, url: &str) -> bool {
    unsafe {
        with_env(vm, |jni| {
            let s = jni.string(url)?;
            let uri = jni.call_static_object(
                c"android/net/Uri",
                c"parse",
                c"(Ljava/lang/String;)Landroid/net/Uri;",
                &[JValue { l: s }],
            )?;
            let action = jni.string("android.intent.action.VIEW")?;
            let class = jni.find_class(c"android/content/Intent")?;
            let init = jni.method(class, c"<init>", c"(Ljava/lang/String;Landroid/net/Uri;)V")?;
            let new_object: unsafe extern "C" fn(Env, JObject, JMethod, *const JValue) -> JObject =
                std::mem::transmute(jni.f(NEW_OBJECT_A));
            let args = [JValue { l: action }, JValue { l: uri }];
            let intent = jni.ok(new_object(jni.0, class, init, args.as_ptr()))?;
            // FLAG_ACTIVITY_NEW_TASK.
            jni.call_object(
                intent,
                c"addFlags",
                c"(I)Landroid/content/Intent;",
                &[JValue { i: 0x1000_0000 }],
            )?;
            jni.call_void(
                activity,
                c"startActivity",
                c"(Landroid/content/Intent;)V",
                &[JValue { l: intent }],
            )
        })
        .is_some()
    }
}

/// The calls the classes Stipple defines at run time need (see `classes`).
impl Jni {
    /// Whether an exception is pending: if so, it goes to logcat (with
    /// `what`) and is cleared.
    pub(super) fn failed(&self, what: &str) -> bool {
        unsafe {
            let check: unsafe extern "C" fn(Env) -> u8 =
                std::mem::transmute(self.f(EXCEPTION_CHECK));
            if check(self.0) == 0 {
                return false;
            }
            let describe: unsafe extern "C" fn(Env) =
                std::mem::transmute(self.f(EXCEPTION_DESCRIBE));
            describe(self.0);
        }
        self.clear();
        super::log(&format!("stipple: {what} threw"));
        true
    }

    pub(super) fn call_float(
        &self,
        obj: JObject,
        name: &CStr,
        sig: &CStr,
        args: &[JValue],
    ) -> Option<f32> {
        let m = self.method(self.class_of(obj)?, name, sig)?;
        unsafe {
            let f: unsafe extern "C" fn(Env, JObject, JMethod, *const JValue) -> f32 =
                std::mem::transmute(self.f(CALL_FLOAT_METHOD_A));
            let r = f(self.0, obj, m, args.as_ptr());
            (!self.clear()).then_some(r)
        }
    }

    /// `new class(args)`, with the constructor of signature `sig`.
    pub(super) fn new_object(
        &self,
        class: JObject,
        sig: &CStr,
        args: &[JValue],
    ) -> Option<JObject> {
        let init = self.method(class, c"<init>", sig)?;
        unsafe {
            let f: unsafe extern "C" fn(Env, JObject, JMethod, *const JValue) -> JObject =
                std::mem::transmute(self.f(NEW_OBJECT_A));
            self.ok(f(self.0, class, init, args.as_ptr()))
        }
    }

    /// [`new_object`](Self::new_object) of the class named `class`.
    pub(super) fn new_named(&self, class: &CStr, sig: &CStr, args: &[JValue]) -> Option<JObject> {
        let class = self.find_class(class)?;
        self.new_object(class, sig, args)
    }

    /// A global reference to `obj`, which outlives the current native call.
    pub(super) fn global(&self, obj: JObject) -> JObject {
        unsafe {
            let f: unsafe extern "C" fn(Env, JObject) -> JObject =
                std::mem::transmute(self.f(NEW_GLOBAL_REF));
            f(self.0, obj)
        }
    }

    pub(super) fn delete_global(&self, obj: JObject) {
        if obj.is_null() {
            return;
        }
        unsafe {
            let f: unsafe extern "C" fn(Env, JObject) =
                std::mem::transmute(self.f(DELETE_GLOBAL_REF));
            f(self.0, obj);
        }
    }

    /// Set the `int` field `name` of `obj`.
    pub(super) fn set_int_field(&self, obj: JObject, name: &CStr, value: i32) -> Option<()> {
        let class = self.class_of(obj)?;
        unsafe {
            let get_id: unsafe extern "C" fn(
                Env,
                JObject,
                *const c_char,
                *const c_char,
            ) -> JMethod = std::mem::transmute(self.f(GET_FIELD_ID));
            let id = self.ok(get_id(self.0, class, name.as_ptr(), c"I".as_ptr()))?;
            let set: unsafe extern "C" fn(Env, JObject, JMethod, i32) =
                std::mem::transmute(self.f(SET_INT_FIELD));
            set(self.0, obj, id, value);
        }
        (!self.clear()).then_some(())
    }

    /// A Java string from `s`, through UTF-16, so characters beyond the BMP
    /// (emoji) survive, unlike [`string`](Self::string)'s modified UTF-8.
    pub(super) fn string16(&self, s: &str) -> Option<JObject> {
        let units: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            let f: unsafe extern "C" fn(Env, *const u16, i32) -> JObject =
                std::mem::transmute(self.f(NEW_STRING));
            self.ok(f(self.0, units.as_ptr(), units.len() as i32))
        }
    }

    /// A Java string's text, read as UTF-16 (see [`string16`](Self::string16)).
    pub(super) fn to_rust16(&self, s: JObject) -> Option<String> {
        if s.is_null() {
            return None;
        }
        unsafe {
            let len: unsafe extern "C" fn(Env, JObject) -> i32 =
                std::mem::transmute(self.f(GET_STRING_LENGTH));
            let n = len(self.0, s).max(0) as usize;
            let get: unsafe extern "C" fn(Env, JObject, *mut u8) -> *const u16 =
                std::mem::transmute(self.f(GET_STRING_CHARS));
            let chars = get(self.0, s, std::ptr::null_mut());
            if chars.is_null() {
                self.clear();
                return None;
            }
            let out = String::from_utf16_lossy(std::slice::from_raw_parts(chars, n));
            let release: unsafe extern "C" fn(Env, JObject, *const u16) =
                std::mem::transmute(self.f(RELEASE_STRING_CHARS));
            release(self.0, s, chars);
            Some(out)
        }
    }

    /// A `CharSequence`'s text (`toString()`), or `None` for `null`.
    pub(super) fn char_sequence(&self, cs: JObject) -> Option<String> {
        if cs.is_null() {
            return None;
        }
        let s = self.call_object(cs, c"toString", c"()Ljava/lang/String;", &[])?;
        self.to_rust16(s)
    }

    /// A new `int[n]`.
    pub(super) fn int_array(&self, n: usize) -> Option<JObject> {
        unsafe {
            let f: unsafe extern "C" fn(Env, i32) -> JObject =
                std::mem::transmute(self.f(NEW_INT_ARRAY));
            self.ok(f(self.0, n as i32))
        }
    }

    /// The first `N` elements of an `int[]`.
    pub(super) fn ints<const N: usize>(&self, array: JObject) -> Option<[i32; N]> {
        let mut out = [0i32; N];
        unsafe {
            let f: unsafe extern "C" fn(Env, JObject, i32, i32, *mut i32) =
                std::mem::transmute(self.f(GET_INT_ARRAY_REGION));
            f(self.0, array, 0, N as i32, out.as_mut_ptr());
        }
        (!self.clear()).then_some(out)
    }

    /// Bind `methods` (name, JNI signature, function) as `class`'s natives.
    pub(super) fn register_natives(
        &self,
        class: JObject,
        methods: &[(std::ffi::CString, std::ffi::CString, *const c_void)],
    ) -> bool {
        #[repr(C)]
        struct NativeMethod {
            name: *const c_char,
            signature: *const c_char,
            func: *const c_void,
        }
        let table: Vec<NativeMethod> = methods
            .iter()
            .map(|(name, signature, func)| NativeMethod {
                name: name.as_ptr(),
                signature: signature.as_ptr(),
                func: *func,
            })
            .collect();
        unsafe {
            let f: unsafe extern "C" fn(Env, JObject, *const NativeMethod, i32) -> i32 =
                std::mem::transmute(self.f(REGISTER_NATIVES));
            let r = f(self.0, class, table.as_ptr(), table.len() as i32);
            !self.failed("RegisterNatives") && r == JNI_OK
        }
    }

    /// A `java.nio.ByteBuffer` over `bytes`.
    pub(super) fn direct_buffer(&self, bytes: &'static [u8]) -> Option<JObject> {
        unsafe {
            let f: unsafe extern "C" fn(Env, *mut c_void, i64) -> JObject =
                std::mem::transmute(self.f(NEW_DIRECT_BYTE_BUFFER));
            self.ok(f(self.0, bytes.as_ptr() as *mut c_void, bytes.len() as i64))
        }
    }

    /// Whether `a` and `b` refer to the same object.
    pub(super) fn same(&self, a: JObject, b: JObject) -> bool {
        unsafe {
            let f: unsafe extern "C" fn(Env, JObject, JObject) -> u8 =
                std::mem::transmute(self.f(IS_SAME_OBJECT));
            f(self.0, a, b) != 0
        }
    }

    /// The `static int` field `name` of the class named `class`.
    pub(super) fn static_int(&self, class: &CStr, name: &CStr) -> Option<i32> {
        let class = self.find_class(class)?;
        unsafe {
            let get_id: unsafe extern "C" fn(
                Env,
                JObject,
                *const c_char,
                *const c_char,
            ) -> JMethod = std::mem::transmute(self.f(GET_STATIC_FIELD_ID));
            let id = self.ok(get_id(self.0, class, name.as_ptr(), c"I".as_ptr()))?;
            let get: unsafe extern "C" fn(Env, JObject, JMethod) -> i32 =
                std::mem::transmute(self.f(GET_STATIC_INT_FIELD));
            let v = get(self.0, class, id);
            (!self.clear()).then_some(v)
        }
    }

    /// A `String[]` of `items`.
    pub(super) fn string_array(&self, items: &[&str]) -> Option<JObject> {
        let class = self.find_class(c"java/lang/String")?;
        unsafe {
            let new: unsafe extern "C" fn(Env, i32, JObject, JObject) -> JObject =
                std::mem::transmute(self.f(NEW_OBJECT_ARRAY));
            let array = self.ok(new(self.0, items.len() as i32, class, std::ptr::null_mut()))?;
            let set: unsafe extern "C" fn(Env, JObject, i32, JObject) =
                std::mem::transmute(self.f(SET_OBJECT_ARRAY_ELEMENT));
            for (i, item) in items.iter().enumerate() {
                set(self.0, array, i as i32, self.string16(item)?);
            }
            (!self.clear()).then_some(array)
        }
    }

    /// Every element of an `int[]` (none for `null`).
    pub(super) fn int_vec(&self, array: JObject) -> Vec<i32> {
        if array.is_null() {
            return Vec::new();
        }
        unsafe {
            let len: unsafe extern "C" fn(Env, JObject) -> i32 =
                std::mem::transmute(self.f(GET_ARRAY_LENGTH));
            let n = len(self.0, array).max(0) as usize;
            let mut out = vec![0i32; n];
            let f: unsafe extern "C" fn(Env, JObject, i32, i32, *mut i32) =
                std::mem::transmute(self.f(GET_INT_ARRAY_REGION));
            f(self.0, array, 0, n as i32, out.as_mut_ptr());
            if self.clear() {
                return Vec::new();
            }
            out
        }
    }

    /// Pop the local frame, keeping `result` alive in the caller's frame.
    fn pop_frame_keeping(&self, result: JObject) -> JObject {
        unsafe {
            let f: unsafe extern "C" fn(Env, JObject) -> JObject =
                std::mem::transmute(self.f(POP_LOCAL_FRAME));
            f(self.0, result)
        }
    }
}

/// Run `f` with the env a native method was called with, inside a local
/// reference frame, logging and clearing any exception it leaves.
pub(super) unsafe fn in_native<R>(env: Env, f: impl FnOnce(&Jni) -> Option<R>) -> Option<R> {
    if env.is_null() {
        return None;
    }
    let jni = Jni(env);
    if !jni.push_frame() {
        return None;
    }
    let r = f(&jni);
    jni.failed("a native method");
    jni.pop_frame();
    r
}

/// [`in_native`] for a native method that returns an object: the object `f`
/// makes survives the frame. `null` when it makes none.
pub(super) unsafe fn in_native_object(
    env: Env,
    f: impl FnOnce(&Jni) -> Option<JObject>,
) -> JObject {
    if env.is_null() {
        return std::ptr::null_mut();
    }
    let jni = Jni(env);
    if !jni.push_frame() {
        return std::ptr::null_mut();
    }
    let r = f(&jni).unwrap_or(std::ptr::null_mut());
    if jni.failed("a native method") {
        jni.pop_frame();
        return std::ptr::null_mut();
    }
    jni.pop_frame_keeping(r)
}
