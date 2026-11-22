//! An Android app with Java classes of its own: a `Service` and a
//! `BroadcastReceiver` (in `java/`, compiled by `cargo-stipple` with `javac`
//! and `d8`), declared in `manifest-fragment.xml`, which `cargo-stipple`
//! merges into the manifest it writes. Both call into this library through
//! `native` methods, the two ways JNI binds them:
//!
//! - `PingReceiver.ping` by name: the exported
//!   `Java_org_stipple_servicedemo_PingReceiver_ping` below.
//! - `PingService.started` with `RegisterNatives`, from this library's
//!   `JNI_OnLoad`. Stipple doesn't define `JNI_OnLoad`, so the app may.
//!
//! The process can start for either with no activity at all (`am
//! startservice` on a process that isn't running): `JNI_OnLoad` and the
//! native methods then run without `ANativeActivity_onCreate` having run, and
//! without Stipple's backend started. They log through liblog themselves
//! (Stipple sends `println!` to logcat only once its activity starts), under
//! the `androidservice` tag:
//!
//! - `androidservice: JNI_OnLoad (activity started: false)`
//! - `androidservice: service started "TEXT" id=N (activity started: false)`
//! - `androidservice: receiver ping "TEXT" (activity started: false)`
//!
//! An app widget (`PingWidget`), whose layout, label and provider info are
//! the app's own resources (`res/`, compiled by `cargo-stipple` with
//! `aapt2`), logs `androidservice: widget update label="Stipple ping"
//! layout text="Not pinged yet"` when updated. Packaged with libraries
//! (`package-libraries.sh`: androidx.startup's AAR and its dependencies),
//! androidx.startup runs `PingInitializer` as the process starts:
//! `androidservice: startup initializer ran (androidx.startup's R.string:
//! "androidx.startup")`.
//!
//! `examples/androidservice/test-on-device.sh` checks those on an emulator.
//! The activity itself is an ordinary Stipple app.
//!
//! ```sh
//! tools/package/cargo-stipple android --example androidservice \
//!     --config examples/androidservice/package.toml
//! ```

#[cfg(target_os = "android")]
mod app {
    use std::ffi::{CStr, CString, c_char, c_int, c_void};
    use std::sync::atomic::{AtomicBool, Ordering};
    use stipple::platform::backend::android::{self, ANativeActivity};
    use stipple::prelude::*;

    /// Whether `ANativeActivity_onCreate` has run in this process.
    static ACTIVITY: AtomicBool = AtomicBool::new(false);

    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
    }

    fn log(msg: &str) {
        let msg = format!(
            "androidservice: {msg} (activity started: {})",
            ACTIVITY.load(Ordering::SeqCst)
        );
        if let Ok(text) = CString::new(msg) {
            unsafe { __android_log_write(4, c"androidservice".as_ptr(), text.as_ptr()) };
        }
    }

    // --- A few JNI calls, by their index in the function tables (jni.h). ---

    type Env = *mut *const *const c_void;
    type JavaVm = *mut *const *const c_void;
    type JObject = *mut c_void;

    const JNI_VERSION_1_6: c_int = 0x0001_0006;
    const VM_GET_ENV: usize = 6;
    const FIND_CLASS: usize = 6;
    const NEW_STRING_UTF: usize = 167;
    const GET_STRING_UTF_CHARS: usize = 169;
    const RELEASE_STRING_UTF_CHARS: usize = 170;
    const REGISTER_NATIVES: usize = 215;

    /// The function at `index` in the table `obj` points to.
    unsafe fn func<F: Copy>(obj: *mut *const *const c_void, index: usize) -> F {
        unsafe {
            let table = *obj;
            let f = *table.add(index);
            std::mem::transmute_copy(&f)
        }
    }

    /// A Java string's text (`null` as an empty one).
    unsafe fn string(env: Env, s: JObject) -> String {
        if s.is_null() {
            return String::new();
        }
        unsafe {
            let get: unsafe extern "C" fn(Env, JObject, *mut u8) -> *const c_char =
                func(env, GET_STRING_UTF_CHARS);
            let release: unsafe extern "C" fn(Env, JObject, *const c_char) =
                func(env, RELEASE_STRING_UTF_CHARS);
            let chars = get(env, s, std::ptr::null_mut());
            if chars.is_null() {
                return String::new();
            }
            let text = CStr::from_ptr(chars).to_string_lossy().into_owned();
            release(env, s, chars);
            text
        }
    }

    #[repr(C)]
    struct NativeMethod {
        name: *const c_char,
        signature: *const c_char,
        f: *mut c_void,
    }

    /// Called by the VM when a Java class loads this library with
    /// `System.loadLibrary` (here, `PingService`'s or `PingReceiver`'s static
    /// initializer), once per class loader. Not when the `NativeActivity`
    /// loads it: that goes through `dlopen`, without `JNI_OnLoad`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn JNI_OnLoad(vm: JavaVm, _reserved: *mut c_void) -> c_int {
        log("JNI_OnLoad");
        unsafe {
            let get_env: unsafe extern "C" fn(JavaVm, *mut Env, c_int) -> c_int =
                func(vm, VM_GET_ENV);
            let mut env: Env = std::ptr::null_mut();
            if get_env(vm, &mut env, JNI_VERSION_1_6) != 0 || env.is_null() {
                return -1;
            }
            // FindClass in JNI_OnLoad uses the class loader of the class
            // that loaded the library: the app's, which has PingService.
            let find_class: unsafe extern "C" fn(Env, *const c_char) -> JObject =
                func(env, FIND_CLASS);
            let class = find_class(env, c"org/stipple/servicedemo/PingService".as_ptr());
            if class.is_null() {
                return -1;
            }
            let methods = [NativeMethod {
                name: c"started".as_ptr(),
                signature: c"(Ljava/lang/String;I)V".as_ptr(),
                f: service_started as *mut c_void,
            }];
            let register: unsafe extern "C" fn(Env, JObject, *const NativeMethod, c_int) -> c_int =
                func(env, REGISTER_NATIVES);
            if register(env, class, methods.as_ptr(), methods.len() as c_int) != 0 {
                log("RegisterNatives failed");
                return -1;
            }
        }
        JNI_VERSION_1_6
    }

    /// `PingService.started(String, int)`, bound in `JNI_OnLoad`.
    unsafe extern "C" fn service_started(env: Env, _class: JObject, text: JObject, id: c_int) {
        let text = unsafe { string(env, text) };
        log(&format!("service started {text:?} id={id}"));
    }

    /// `PingReceiver.ping(String)`, found by its name. Returns the reply the
    /// receiver hands back to the broadcast's sender.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn Java_org_stipple_servicedemo_PingReceiver_ping(
        env: Env,
        _class: JObject,
        text: JObject,
    ) -> JObject {
        let text = unsafe { string(env, text) };
        log(&format!("receiver ping {text:?}"));
        unsafe { new_string(env, &format!("pong {text}")) }
    }

    /// A new Java string of `text`.
    unsafe fn new_string(env: Env, text: &str) -> JObject {
        let text = CString::new(text).unwrap_or_default();
        unsafe {
            let new: unsafe extern "C" fn(Env, *const c_char) -> JObject =
                func(env, NEW_STRING_UTF);
            new(env, text.as_ptr())
        }
    }

    /// `PingWidget.update(String, String)`, found by its name: the widget's
    /// label (a string resource of the app's) and the text its layout (a
    /// layout resource) starts with, as `onUpdate` read them. Returns the
    /// widget's new text.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn Java_org_stipple_servicedemo_PingWidget_update(
        env: Env,
        _class: JObject,
        label: JObject,
        text: JObject,
    ) -> JObject {
        let (label, text) = unsafe { (string(env, label), string(env, text)) };
        log(&format!(
            "widget update label={label:?} layout text={text:?}"
        ));
        unsafe { new_string(env, "Pinged") }
    }

    /// `PingInitializer.initialized(String)` (only in the build with
    /// libraries, `package-libraries.sh`): androidx.startup ran the app's
    /// initializer at process start, which read androidx.startup's own
    /// string resource through its R class.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn Java_org_stipple_servicedemo_PingInitializer_initialized(
        env: Env,
        _class: JObject,
        key: JObject,
    ) -> JObject {
        let key = unsafe { string(env, key) };
        log(&format!(
            "startup initializer ran (androidx.startup's R.string: {key:?})"
        ));
        unsafe { new_string(env, "initialized") }
    }

    // --- The activity: an ordinary Stipple app. ---

    fn view(_: &(), cx: &mut Cx<()>) -> Element {
        column(vec![
            label(cx, "Java Service and receiver of its own"),
            label(cx, "adb logcat -s androidservice"),
        ])
        .padding(Insets::uniform(24.0))
        .gap(12.0)
    }

    fn main() {
        println!("androidservice: activity running");
        App::new((), view).theme(Theme::native()).run();
    }

    /// The symbol Android resolves to start a NativeActivity.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn ANativeActivity_onCreate(
        activity: *mut ANativeActivity,
        saved_state: *mut c_void,
        saved_state_size: usize,
    ) {
        ACTIVITY.store(true, Ordering::SeqCst);
        log("ANativeActivity_onCreate");
        unsafe { android::start(activity, saved_state, saved_state_size, main) };
    }
}
