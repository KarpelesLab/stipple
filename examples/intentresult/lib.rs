//! An Android app that calls Android APIs Stipple doesn't wrap, through
//! `stipple::platform::android`: it asks `VpnService.prepare(context)` for the
//! VPN consent dialog's intent and starts it for a result, as a VPN client
//! does before it may start its `VpnService`. The JNI here is hand-written (a
//! few calls through the `JNIEnv` table); an app would use its own JNI crate.
//!
//! Package it with
//!
//! ```sh
//! STIPPLE_APK_EXAMPLE=intentresult \
//!   STIPPLE_APK_MANIFEST=examples/intentresult/AndroidManifest.xml \
//!   bash examples/androiddemo/build-apk.sh x86_64 intentresult.apk
//! ```
//!
//! and run `examples/intentresult/test-on-device.sh intentresult.apk`, which
//! reads its `println!`s back from logcat (tag `stipple`):
//!
//! - `intentresult: started N vm=true context=true activity=true` as each
//!   activity starts (N counts them: Android recreates the activity on a
//!   configuration change its manifest doesn't declare, a font scale here);
//! - `intentresult: ui thread main=true` from `run_on_ui_thread`;
//! - `intentresult: unhandled result 0 data=false`: an intent nothing
//!   handles, started for a result once per process, is answered
//!   `RESULT_CANCELED` at once;
//! - `intentresult: vpn result CODE data=BOOL main=true (activity N)` once the
//!   consent dialog the big button opens closes: 0 (`RESULT_CANCELED`) when
//!   it's backed out of, -1 (`RESULT_OK`) when *OK* is tapped, N the activity
//!   running when it answered; `intentresult: vpn already prepared` when
//!   `prepare` returns null (the user agreed already).
//!
//! Non-Android builds compile to an empty cdylib so `cargo build --examples`
//! works everywhere.

#[cfg(target_os = "android")]
mod app {
    use std::ffi::{CStr, c_char, c_void};
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use stipple::platform::android::{self as java, jobject};
    use stipple::platform::backend::android::{self, ANativeActivity};
    use stipple::prelude::*;

    /// Activities started in this process.
    static STARTED: AtomicU32 = AtomicU32::new(0);
    /// Whether the unhandled intent was tried (once per process).
    static TRIED_UNHANDLED: AtomicBool = AtomicBool::new(false);

    #[derive(Default)]
    struct State {
        last: String,
    }

    // ---- A few JNI calls by hand ------------------------------------------

    type Env = *mut c_void;

    #[repr(C)]
    #[derive(Clone, Copy)]
    union JValue {
        l: jobject,
        i: i32,
    }

    const FIND_CLASS: usize = 6;
    const EXCEPTION_CLEAR: usize = 17;
    const DELETE_GLOBAL_REF: usize = 22;
    const DELETE_LOCAL_REF: usize = 23;
    const NEW_OBJECT_A: usize = 30;
    const GET_METHOD_ID: usize = 33;
    const GET_STATIC_METHOD_ID: usize = 113;
    const CALL_STATIC_OBJECT_METHOD_A: usize = 116;
    const NEW_STRING_UTF: usize = 167;
    const EXCEPTION_CHECK: usize = 228;
    const VM_ATTACH_CURRENT_THREAD: usize = 4;

    unsafe extern "C" {
        fn gettid() -> i32;
        fn getpid() -> i32;
    }

    fn on_main_thread() -> bool {
        unsafe { gettid() == getpid() }
    }

    /// Entry `i` of the function table behind a `JNIEnv*` or `JavaVM*`.
    unsafe fn slot<F: Copy>(ptr: *mut c_void, i: usize) -> F {
        unsafe {
            let table = *(ptr as *const *const *const c_void);
            std::mem::transmute_copy(&*table.add(i))
        }
    }

    /// The calling thread's env (attached for good: Stipple's UI thread is
    /// attached while the loop runs anyway, and the main thread always is).
    fn env() -> Option<Env> {
        let vm = java::java_vm();
        if vm.is_null() {
            return None;
        }
        let mut env: Env = std::ptr::null_mut();
        unsafe {
            let attach: unsafe extern "C" fn(*mut c_void, *mut Env, *mut c_void) -> i32 =
                slot(vm, VM_ATTACH_CURRENT_THREAD);
            (attach(vm, &mut env, std::ptr::null_mut()) == 0).then_some(env)
        }
    }

    /// Whether a Java exception is pending (cleared).
    unsafe fn threw(env: Env) -> bool {
        unsafe {
            let check: unsafe extern "C" fn(Env) -> u8 = slot(env, EXCEPTION_CHECK);
            if check(env) == 0 {
                return false;
            }
            let clear: unsafe extern "C" fn(Env) = slot(env, EXCEPTION_CLEAR);
            clear(env);
            true
        }
    }

    unsafe fn find_class(env: Env, name: &CStr) -> Option<jobject> {
        unsafe {
            let f: unsafe extern "C" fn(Env, *const c_char) -> jobject = slot(env, FIND_CLASS);
            let class = f(env, name.as_ptr());
            (!threw(env) && !class.is_null()).then_some(class)
        }
    }

    unsafe fn delete_local(env: Env, obj: jobject) {
        unsafe {
            let f: unsafe extern "C" fn(Env, jobject) = slot(env, DELETE_LOCAL_REF);
            f(env, obj);
        }
    }

    /// `VpnService.prepare(context)`: the consent dialog's intent (a local
    /// reference), or `None` if the user agreed already.
    unsafe fn vpn_prepare(env: Env, context: jobject) -> Option<jobject> {
        unsafe {
            let class = find_class(env, c"android/net/VpnService")?;
            let method: unsafe extern "C" fn(
                Env,
                jobject,
                *const c_char,
                *const c_char,
            ) -> *mut c_void = slot(env, GET_STATIC_METHOD_ID);
            let id = method(
                env,
                class,
                c"prepare".as_ptr(),
                c"(Landroid/content/Context;)Landroid/content/Intent;".as_ptr(),
            );
            if threw(env) || id.is_null() {
                return None;
            }
            let call: unsafe extern "C" fn(Env, jobject, *mut c_void, *const JValue) -> jobject =
                slot(env, CALL_STATIC_OBJECT_METHOD_A);
            let intent = call(env, class, id, [JValue { l: context }].as_ptr());
            delete_local(env, class);
            (!threw(env) && !intent.is_null()).then_some(intent)
        }
    }

    /// `new Intent(action)` (a local reference).
    unsafe fn new_intent(env: Env, action: &CStr) -> Option<jobject> {
        unsafe {
            let class = find_class(env, c"android/content/Intent")?;
            let method: unsafe extern "C" fn(
                Env,
                jobject,
                *const c_char,
                *const c_char,
            ) -> *mut c_void = slot(env, GET_METHOD_ID);
            let ctor = method(
                env,
                class,
                c"<init>".as_ptr(),
                c"(Ljava/lang/String;)V".as_ptr(),
            );
            let string: unsafe extern "C" fn(Env, *const c_char) -> jobject =
                slot(env, NEW_STRING_UTF);
            let action = string(env, action.as_ptr());
            let new: unsafe extern "C" fn(Env, jobject, *mut c_void, *const JValue) -> jobject =
                slot(env, NEW_OBJECT_A);
            let intent = new(env, class, ctor, [JValue { l: action }].as_ptr());
            delete_local(env, action);
            delete_local(env, class);
            (!threw(env) && !intent.is_null()).then_some(intent)
        }
    }

    // ---- The app ------------------------------------------------------------

    /// Ask for VPN consent, if it isn't given yet.
    fn ask_for_vpn(handle: AppHandle<State>) {
        let (Some(env), Some(context)) = (env(), java::application_context()) else {
            println!("intentresult: no JNI");
            return;
        };
        let Some(intent) = (unsafe { vpn_prepare(env, context) }) else {
            println!("intentresult: vpn already prepared");
            handle.update(|s| s.last = "Already prepared".into());
            return;
        };
        unsafe {
            java::start_activity_for_result(intent, move |code, data| {
                let activity = STARTED.load(Ordering::Relaxed);
                println!(
                    "intentresult: vpn result {code} data={} main={} (activity {activity})",
                    data.is_some(),
                    on_main_thread()
                );
                let what = match code {
                    java::RESULT_OK => "Consent given".to_string(),
                    java::RESULT_CANCELED => "Consent refused".to_string(),
                    other => format!("Result {other}"),
                };
                handle.update(move |s| s.last = what);
            });
            // Ours to delete; Stipple keeps a reference of its own.
            delete_local(env, intent);
        }
    }

    /// An intent nothing handles: answered RESULT_CANCELED.
    fn start_unhandled() {
        let Some(env) = env() else { return };
        let Some(intent) = (unsafe { new_intent(env, c"org.stipple.intentresult.NOTHING") }) else {
            return;
        };
        unsafe {
            java::start_activity_for_result(intent, |code, data| {
                println!(
                    "intentresult: unhandled result {code} data={}",
                    data.is_some()
                );
            });
            delete_local(env, intent);
        }
    }

    fn view(state: &State, cx: &mut Cx<State>) -> Element {
        let handle = cx.handle();
        let ask = button(cx, "Ask for VPN consent")
            .width(280.0)
            .grow(1.0)
            .on_tap(cx, move |_| {
                println!("intentresult: asking");
                ask_for_vpn(handle.clone());
            });
        column(vec![
            heading(cx, "Activity results"),
            label(cx, format!("Last answer: {}", state.last)),
            ask,
        ])
        .gap(16.0)
        .padding(Insets::uniform(24.0))
    }

    fn main() {
        let n = STARTED.fetch_add(1, Ordering::Relaxed) + 1;
        let activity = java::activity();
        println!(
            "intentresult: started {n} vm={} context={} activity={}",
            !java::java_vm().is_null(),
            java::application_context().is_some(),
            activity.is_some()
        );
        // The activity's reference is ours to delete.
        if let (Some(a), Some(env)) = (activity, env()) {
            unsafe {
                let f: unsafe extern "C" fn(Env, jobject) = slot(env, DELETE_GLOBAL_REF);
                f(env, a);
            }
        }
        let queued = java::run_on_ui_thread(|| {
            println!("intentresult: ui thread main={}", on_main_thread());
        });
        println!("intentresult: run_on_ui_thread queued={queued}");
        if !TRIED_UNHANDLED.swap(true, Ordering::Relaxed) {
            start_unhandled();
        }
        App::new(State::default(), view)
            .theme(Theme::native())
            .edge_to_edge(true)
            .run();
        println!("intentresult: event loop exited (activity {n})");
    }

    /// The symbol Android resolves to start a NativeActivity.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn ANativeActivity_onCreate(
        activity: *mut ANativeActivity,
        saved_state: *mut c_void,
        saved_state_size: usize,
    ) {
        unsafe { android::start(activity, saved_state, saved_state_size, main) };
    }
}
