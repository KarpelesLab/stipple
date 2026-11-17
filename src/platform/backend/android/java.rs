//! JNI access for apps, behind [`platform::android`](crate::platform::android):
//! the process's `JavaVM`, the current activity, the application context,
//! activities started for a result, and work run on Android's main thread.
//!
//! - **The `JavaVM`** is kept from the first of Stipple's entry points that
//!   runs in the process (the activity's `ANativeActivity_onCreate`, or one of
//!   the APK's components: the `Alarm` receiver, the `Tapped` activity, the
//!   `Files` provider), and otherwise asked of `libnativehelper`
//!   (`JNI_GetCreatedJavaVMs`, public from Android 12).
//! - **Results.** An intent is started from the [`Results`](crate::platform::dex::RESULTS)
//!   fragment ([`classes::results_fragment`]) with a request code of the
//!   app's own ([`requests`]); its callback waits in a static under that code,
//!   so the answer finds it even when it's delivered to an activity that
//!   replaced the one that asked: [`classes::install`] then adds the new
//!   activity's fragment at once (it has the same `mWho`,
//!   `android:fragment:0`, since its saved state drops the old one), before
//!   the framework delivers results (before `onResume`).
//! - **The main thread.** `NativeActivity` runs the native callbacks on the
//!   Java main thread, but Stipple's UI thread is a thread of its own. Work
//!   for the main thread goes through a pipe the main thread's `ALooper`
//!   watches, registered the first time Stipple runs on it.

use std::ffi::{c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicI32, AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, Once};

use super::jni::{self, Env, JObject, JValue, Jni};
use super::requests::{self, Requests};
use super::{ANativeActivity, classes, log};

/// `Activity.RESULT_OK`: the started activity did what it was asked.
pub const RESULT_OK: i32 = -1;
/// `Activity.RESULT_CANCELED`: it was backed out of, or it failed, or it
/// couldn't be started.
pub const RESULT_CANCELED: i32 = 0;

/// The JNI `JavaVM` (opaque: use it through your own JNI bindings).
pub type JavaVM = c_void;

/// A JNI object reference, as `jni.h`'s `jobject`.
#[allow(non_camel_case_types)]
pub type jobject = *mut c_void;

/// What [`start_activity_for_result`]'s caller hears.
type Callback = Box<dyn FnOnce(i32, Option<jobject>) + Send>;

/// Work for the main thread.
type Job = Box<dyn FnOnce() + Send>;

/// The callbacks waiting for an answer, by request code.
static WAITING: Mutex<Requests<Callback>> = Mutex::new(Requests::new());
/// The process's `JavaVM`, once known.
static VM: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
/// A global reference to the application context, once known.
static APP_CONTEXT: AtomicUsize = AtomicUsize::new(0);
/// The write end of the main thread's job pipe, once registered (-1 before).
static MAIN_PIPE: AtomicI32 = AtomicI32::new(-1);
/// The main thread's jobs, waiting for its looper to run them.
static JOBS: Mutex<Vec<Job>> = Mutex::new(Vec::new());

unsafe extern "C" {
    fn gettid() -> c_int;
    fn getpid() -> c_int;
}

fn waiting() -> MutexGuard<'static, Requests<Callback>> {
    WAITING.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether the calling thread is the process's main thread (whose thread id
/// is the process id).
fn is_main_thread() -> bool {
    unsafe { gettid() == getpid() }
}

// ---- Entry points ------------------------------------------------------------

fn record_vm(vm: *mut c_void) {
    if !vm.is_null() {
        let _ = VM.compare_exchange(
            std::ptr::null_mut(),
            vm,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}

/// From `ANativeActivity_onCreate` (on the main thread): keep the VM and the
/// application context, and watch the main thread's job pipe.
pub(super) fn activity_created(activity: &ANativeActivity) {
    record_vm(activity.vm);
    main_thread_entered();
    if APP_CONTEXT.load(Ordering::Acquire) != 0 {
        return;
    }
    unsafe {
        jni::with_env(activity.vm, |jni| {
            let app = jni.call_object(
                activity.clazz,
                c"getApplicationContext",
                c"()Landroid/content/Context;",
                &[],
            )?;
            keep_context(jni, jni.global(app) as usize);
            Some(())
        });
    }
}

/// From the APK's components' natives (the `Alarm` receiver, the `Tapped`
/// activity, the `Files` provider), which may run in a process where no
/// activity started: keep the VM, and watch the job pipe if on the main
/// thread.
pub(super) fn entered(env: Env) {
    if !env.is_null() {
        record_vm(Jni(env).java_vm());
    }
    main_thread_entered();
}

/// Keep `global` as the application context, unless another thread got
/// there first. Returns the one kept.
fn keep_context(jni: &Jni, global: usize) -> usize {
    if global == 0 {
        return APP_CONTEXT.load(Ordering::Acquire);
    }
    match APP_CONTEXT.compare_exchange(0, global, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => global,
        Err(other) => {
            jni.delete_global(global as JObject);
            other
        }
    }
}

// ---- The app's API -------------------------------------------------------------

/// The process's `JavaVM*`, valid for the life of the process; attach your
/// threads to it with your own JNI bindings (the `jni` crate's
/// `JavaVM::from_raw`, say) as you need them.
///
/// Null only if Stipple hasn't seen the VM: before Android 12, in a process
/// where none of its entry points has run yet (the activity's
/// `ANativeActivity_onCreate`, the `Alarm` receiver, the `Tapped` activity,
/// the `Files` provider). From Android 12 it's asked of `libnativehelper`
/// otherwise. Callable from any thread.
pub fn java_vm() -> *mut JavaVM {
    main_thread_entered();
    vm().unwrap_or(std::ptr::null_mut())
}

fn vm() -> Option<*mut c_void> {
    let vm = VM.load(Ordering::Acquire);
    if !vm.is_null() {
        return Some(vm);
    }
    let found = created_vm()?;
    record_vm(found);
    Some(VM.load(Ordering::Acquire))
}

/// The VM, asked of `libnativehelper` (its `JNI_GetCreatedJavaVMs` is in
/// the NDK from Android 12; it's loaded at run time, so older versions just
/// don't find it).
fn created_vm() -> Option<*mut c_void> {
    type GetCreated = unsafe extern "C" fn(*mut *mut c_void, i32, *mut i32) -> i32;
    unsafe {
        let lib = super::dlopen(c"libnativehelper.so".as_ptr(), super::RTLD_NOW);
        if lib.is_null() {
            return None;
        }
        let sym = super::dlsym(lib, c"JNI_GetCreatedJavaVMs".as_ptr());
        if sym.is_null() {
            return None;
        }
        let f: GetCreated = std::mem::transmute(sym);
        let (mut vm, mut count) = (std::ptr::null_mut(), 0);
        (f(&mut vm, 1, &mut count) == 0 && count > 0 && !vm.is_null()).then_some(vm)
    }
}

/// The current activity (the app's `NativeActivity`), as a **new global
/// reference the caller owns**: delete it (`DeleteGlobalRef`) when done, or
/// it keeps the activity object alive after the activity is destroyed.
///
/// The activity changes when Android recreates it (a configuration change
/// the manifest doesn't declare, "Don't keep activities"), so ask again
/// rather than keeping one. `None` while there is none: before the first
/// `onCreate`, after `onDestroy`, in a process started for a receiver or a
/// service. Callable from any thread.
pub fn activity() -> Option<jobject> {
    main_thread_entered();
    super::with_activity(|a| unsafe {
        jni::with_env(a.vm, |jni| {
            let global = jni.global(a.clazz);
            (!global.is_null()).then_some(global)
        })
    })
}

/// The application `Context` (`getApplicationContext()`), for code that runs
/// without an activity (a service, a receiver, a worker thread while the
/// activity is being recreated).
///
/// It's **Stipple's global reference, kept for the life of the process: don't
/// delete it.** Kept from the first activity; in a process no activity has
/// started in, it's `ActivityThread.currentApplication()`. `None` only when
/// there's no [`java_vm`] or the application isn't created yet. Callable from
/// any thread.
pub fn application_context() -> Option<jobject> {
    main_thread_entered();
    let have = APP_CONTEXT.load(Ordering::Acquire);
    if have != 0 {
        return Some(have as jobject);
    }
    // No activity has started in this process: ask the framework.
    let vm = vm()?;
    let kept = unsafe {
        jni::with_env(vm, |jni| {
            let app = jni.call_static_object(
                c"android/app/ActivityThread",
                c"currentApplication",
                c"()Landroid/app/Application;",
                &[],
            )?;
            Some(keep_context(jni, jni.global(app) as usize))
        })
    }?;
    (kept != 0).then_some(kept as jobject)
}

/// Run `f` on Android's main thread, the Java UI thread, as
/// `Activity.runOnUiThread` does: at once when called on it, otherwise
/// queued for its looper. That's for the Java APIs that must be called there
/// (`View`s, `Toast`, `BiometricPrompt`, a `Handler` without a looper of its
/// own). It isn't Stipple's UI thread, where the `App` runs and `main` was
/// started: that is a thread of its own, and `NativeActivity` calls the
/// native callbacks on the main thread. The main thread is attached to the VM
/// already.
///
/// Works without an activity, once Stipple has run on the main thread in
/// this process (the activity's `onCreate`, or the `Alarm` receiver, the
/// `Tapped` activity or the `Files` provider being created; or a call to
/// any of this module's functions from the main thread). `false` before that,
/// and `f` is dropped. A panic in `f` is caught and logged. Callable from any
/// thread.
pub fn run_on_ui_thread(f: impl FnOnce() + Send + 'static) -> bool {
    post_main(Box::new(f)).is_ok()
}

/// Start the activity `intent` names for a result, and call `callback` with
/// its `resultCode` ([`RESULT_OK`], [`RESULT_CANCELED`] or the activity's
/// own) and its result `Intent`, if it set one: what `onActivityResult`
/// would get, which `NativeActivity` doesn't pass on. For instance the
/// consent dialog `VpnService.prepare(context)` hands back, a
/// `Settings.ACTION_*` screen, `KeyguardManager.createConfirmDeviceCredentialIntent`,
/// `MediaProjectionManager.createScreenCaptureIntent`.
///
/// The intent is started from Stipple's results fragment on the main thread,
/// with a request code of its own. Callable from any thread; the caller still
/// owns `intent` (Stipple takes a global reference of its own for the trip).
///
/// `callback` runs once, on the main thread (attached to the VM). The data
/// `Intent` is a **local reference, valid only during the callback**: make a
/// global reference to keep it, or read what you need from it there.
///
/// The request survives the activity being recreated meanwhile (a
/// configuration change, "Don't keep activities"): the answer goes to the new
/// activity, and `callback` still hears it. It doesn't survive the process
/// ending (Android may kill a backgrounded app while the other activity is up):
/// then the new process gets the result and nobody waits for it. `callback`
/// gets [`RESULT_CANCELED`] and `None` when the intent can't be started:
/// there's no activity, nothing handles the intent
/// (`ActivityNotFoundException`), or `intent` is null.
///
/// # Safety
/// `intent` must be null, a valid local reference of the calling thread or a
/// valid global reference to an `android.content.Intent`.
pub unsafe fn start_activity_for_result(
    intent: jobject,
    callback: impl FnOnce(i32, Option<jobject>) + Send + 'static,
) {
    let mut start = Start {
        intent: 0,
        callback: Some(Box::new(callback)),
    };
    // Each early return drops `start`, which answers RESULT_CANCELED.
    let Some(vm) = vm() else {
        log("stipple: start_activity_for_result: no JavaVM");
        return;
    };
    if intent.is_null() {
        log("stipple: start_activity_for_result: the intent is null");
        return;
    }
    start.intent =
        unsafe { jni::with_env(vm, |jni| Some(jni.global(intent) as usize)) }.unwrap_or_default();
    if start.intent == 0 {
        return;
    }
    // Without an activity the job is dropped at once, and with it `start`.
    super::run_on_main(move |jni, activity| start.start(jni, activity));
}

/// A request on its way to the main thread: the intent (a global
/// reference) and the callback, answered with `RESULT_CANCELED` if dropped
/// before it starts (there is no activity, it went away, nothing handles the
/// intent).
struct Start {
    intent: usize,
    callback: Option<Callback>,
}

impl Start {
    /// On the main thread: start it from the results fragment.
    fn start(mut self, jni: &Jni, activity: JObject) {
        let Some(callback) = self.callback.take() else {
            return;
        };
        let code = waiting().insert(callback);
        let started = classes::results_fragment(jni, activity).and_then(|fragment| {
            jni.call_void(
                fragment,
                c"startActivityForResult",
                c"(Landroid/content/Intent;I)V",
                &[
                    JValue {
                        l: self.intent as JObject,
                    },
                    JValue { i: code },
                ],
            )
        });
        if started.is_some() {
            log(&format!(
                "stipple: activity started for a result (request {code:#x})"
            ));
        } else {
            // No fragment, or no activity handles the intent
            // (ActivityNotFoundException): dropping self answers.
            self.callback = waiting().take(code);
        }
    }
}

impl Drop for Start {
    fn drop(&mut self) {
        if self.intent != 0
            && let Some(vm) = vm()
        {
            let intent = self.intent as JObject;
            unsafe {
                jni::with_env(vm, |jni| {
                    jni.delete_global(intent);
                    Some(())
                })
            };
        }
        if let Some(callback) = self.callback.take() {
            log("stipple: the activity wasn't started for a result");
            let job: Job = Box::new(move || call(callback, RESULT_CANCELED, None));
            if let Err(job) = post_main(job) {
                job();
            }
        }
    }
}

/// Run the app's callback, keeping a panic in it from unwinding into the VM.
fn call(callback: Callback, result: i32, data: Option<jobject>) {
    if catch_unwind(AssertUnwindSafe(|| callback(result, data))).is_err() {
        log("stipple: an activity result's callback panicked");
    }
}

/// `Results.onActivityResult`, on the main thread: hand an answer to one of
/// the app's requests to its callback. `false` if `code` isn't one of them.
pub(super) fn on_activity_result(code: i32, result: i32, data: JObject) -> bool {
    if !requests::is_app_code(code) {
        return false;
    }
    let callback = waiting().take(code);
    log(&format!(
        "stipple: activity result {result} (request {code:#x}, data {})",
        !data.is_null()
    ));
    match callback {
        Some(callback) => call(callback, result, (!data.is_null()).then_some(data)),
        // Asked by a process that has ended since.
        None => log("stipple: nobody waits for that result"),
    }
    true
}

/// Whether an app's request waits for an answer (so a new activity needs
/// its results fragment before the framework delivers results).
pub(super) fn waiting_for_results() -> bool {
    !waiting().is_empty()
}

// ---- The main thread's jobs ----------------------------------------------------

/// On the main thread, the first time: watch a pipe for jobs with its
/// looper. Elsewhere, nothing.
fn main_thread_entered() {
    if MAIN_PIPE.load(Ordering::Acquire) >= 0 || !is_main_thread() {
        return;
    }
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let looper = super::ALooper_forThread();
        if looper.is_null() {
            return;
        }
        let mut fds = [0 as c_int; 2];
        if super::pipe2(fds.as_mut_ptr(), super::O_NONBLOCK | super::O_CLOEXEC) != 0 {
            log("stipple: can't create the main thread's job pipe");
            return;
        }
        // Kept for the process.
        super::ALooper_acquire(looper);
        super::ALooper_addFd(
            looper,
            fds[0],
            super::ALOOPER_POLL_CALLBACK,
            super::ALOOPER_EVENT_INPUT,
            on_jobs as *const c_void,
            std::ptr::null_mut(),
        );
        MAIN_PIPE.store(fds[1], Ordering::Release);
    });
}

/// Run `job` on the main thread: now if this is it, or through the pipe.
/// Gives it back if the main thread's looper isn't watched yet.
fn post_main(job: Job) -> Result<(), Job> {
    if is_main_thread() {
        main_thread_entered();
        if catch_unwind(AssertUnwindSafe(job)).is_err() {
            log("stipple: a job for the main thread panicked");
        }
        return Ok(());
    }
    let fd = MAIN_PIPE.load(Ordering::Acquire);
    if fd < 0 {
        return Err(job);
    }
    JOBS.lock().unwrap_or_else(|e| e.into_inner()).push(job);
    unsafe { super::write(fd, [1u8].as_ptr().cast(), 1) };
    Ok(())
}

/// The main thread's looper saw the pipe: run the jobs waiting.
unsafe extern "C" fn on_jobs(fd: c_int, _events: c_int, _data: *mut c_void) -> c_int {
    let mut buf = [0u8; 64];
    while unsafe { super::read(fd, buf.as_mut_ptr().cast(), buf.len()) } > 0 {}
    let jobs = std::mem::take(&mut *JOBS.lock().unwrap_or_else(|e| e.into_inner()));
    for job in jobs {
        if catch_unwind(AssertUnwindSafe(job)).is_err() {
            log("stipple: a job for the main thread panicked");
        }
    }
    1
}
