//! A native Android backend over the NDK's C API (`libandroid`), with no NDK
//! crate (`ndk`, `ndk-glue`, `android-activity`): the "talk to the OS directly"
//! policy. Android-only.
//!
//! Apps run in a `NativeActivity`. Android calls the app's
//! `ANativeActivity_onCreate` on its main (Java) thread; the app hands that to
//! [`start`], which registers the activity callbacks and runs the app's own
//! `main` on a thread of its own, the UI thread. There [`App::run`] reaches
//! [`run`], which drives everything from an `ALooper`:
//!
//! - **Lifecycle.** Each activity callback queues a command and writes a byte
//!   to a pipe the looper watches. Callbacks the framework needs finished before
//!   it continues (the window or input queue going away, a redraw, the activity
//!   being destroyed) wait until the UI thread has handled them, the same
//!   handshake as the NDK's `android_native_app_glue`.
//! - **Input.** The `AInputQueue` is attached to the looper. Touches become
//!   pointer events for the first finger, the mouse its buttons, hover and
//!   wheel, and keys go through the device's key character map (over JNI) so
//!   any layout types the right characters.
//! - **Wakeups.** [`wake`](crate::platform::wake) interrupts the poll with
//!   `ALooper_wake`, and timers bound its timeout.
//! - **Frames.** `AChoreographer` paces animations at the display's refresh.
//! - **The window.** The software [`Pixmap`] is copied into the
//!   `ANativeWindow`'s buffer (`ANativeWindow_lock`), at the content rectangle
//!   the system leaves free of its bars; the rest takes the background color.
//!
//! The scale factor is the screen density over 160 dpi, and the native theme
//! follows the configuration's night mode and, on Android 12 and later, the
//! wallpaper's accent color.
//!
//! [`App::run`]: crate::App::run
#![allow(unsafe_code)]

use core::ffi::{c_char, c_int, c_long, c_void};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Instant;

use crate::geometry::{PhysicalSize, Point, Rect, ScaleFactor};
use crate::platform::ControlFlow;
use crate::platform::error::PlatformError;
use crate::platform::event::{ButtonState, Event, PointerButton, WindowId};
use crate::platform::input;
use crate::platform::window::{Window, WindowAttributes};
use crate::render::{Pixmap, Surface};

/// Hand-written JNI for the clipboard, key characters and the accent color.
mod jni;

// ---- NDK FFI ----------------------------------------------------------------

/// WINDOW_FORMAT_RGBA_8888: straight R,G,B,A bytes, matching our Pixmap.
const WINDOW_FORMAT_RGBA_8888: i32 = 1;
/// WINDOW_FORMAT_RGBX_8888: the same layout, shown opaque.
const WINDOW_FORMAT_RGBX_8888: i32 = 2;

/// `ANativeWindow_Buffer`: the locked back buffer. `stride` is in **pixels**.
#[repr(C)]
struct ANativeWindowBuffer {
    width: i32,
    height: i32,
    stride: i32,
    format: i32,
    bits: *mut c_void,
    reserved: [u32; 6],
}

/// `ARect`, as `onContentRectChanged` reports it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ARect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

const ALOOPER_PREPARE_ALLOW_NON_CALLBACKS: c_int = 1;
const ALOOPER_EVENT_INPUT: c_int = 1;
const ALOOPER_POLL_CALLBACK: c_int = -2;
const ALOOPER_POLL_ERROR: c_int = -4;
/// AWINDOW_FLAG_FULLSCREEN: hide the status bar.
const AWINDOW_FLAG_FULLSCREEN: u32 = 0x400;
/// Looper idents for the command pipe and the input queue.
const LOOPER_ID_COMMAND: c_int = 1;
const LOOPER_ID_INPUT: c_int = 2;

const AINPUT_EVENT_TYPE_KEY: i32 = 1;
const AINPUT_EVENT_TYPE_MOTION: i32 = 2;
const AKEY_EVENT_ACTION_DOWN: i32 = 0;
const AKEY_EVENT_ACTION_UP: i32 = 1;
const AMOTION_EVENT_ACTION_MASK: i32 = 0xff;
const AMOTION_EVENT_ACTION_DOWN: i32 = 0;
const AMOTION_EVENT_ACTION_UP: i32 = 1;
const AMOTION_EVENT_ACTION_MOVE: i32 = 2;
const AMOTION_EVENT_ACTION_CANCEL: i32 = 3;
const AMOTION_EVENT_ACTION_POINTER_UP: i32 = 6;
const AMOTION_EVENT_ACTION_HOVER_MOVE: i32 = 7;
const AMOTION_EVENT_ACTION_SCROLL: i32 = 8;
const AMOTION_EVENT_ACTION_HOVER_ENTER: i32 = 9;
const AMOTION_EVENT_ACTION_HOVER_EXIT: i32 = 10;
const AMOTION_EVENT_AXIS_VSCROLL: i32 = 9;
const AMOTION_EVENT_AXIS_HSCROLL: i32 = 10;
const AMOTION_EVENT_BUTTON_SECONDARY: i32 = 2;
const AMOTION_EVENT_BUTTON_TERTIARY: i32 = 4;
const AMOTION_EVENT_TOOL_TYPE_MOUSE: i32 = 3;
/// Meta-state bits for Ctrl and Meta (either side), which make a key a
/// shortcut rather than typing.
const AMETA_CTRL_MASK: i32 = 0x7000;
const AMETA_META_MASK: i32 = 0x7_0000;

const ACONFIGURATION_DENSITY_DEFAULT: i32 = 0;
const ACONFIGURATION_DENSITY_MEDIUM: i32 = 160;
const ACONFIGURATION_DENSITY_ANY: i32 = 0xfffe;
const ACONFIGURATION_UI_MODE_NIGHT_YES: i32 = 2;

#[link(name = "android")]
unsafe extern "C" {
    fn ANativeWindow_setBuffersGeometry(
        window: *mut c_void,
        width: i32,
        height: i32,
        format: i32,
    ) -> i32;
    fn ANativeWindow_lock(
        window: *mut c_void,
        out_buffer: *mut ANativeWindowBuffer,
        in_out_dirty_bounds: *mut c_void,
    ) -> i32;
    fn ANativeWindow_unlockAndPost(window: *mut c_void) -> i32;
    /// The window's current drawable width in pixels.
    pub fn ANativeWindow_getWidth(window: *mut c_void) -> i32;
    /// The window's current drawable height in pixels.
    pub fn ANativeWindow_getHeight(window: *mut c_void) -> i32;
    fn ANativeWindow_acquire(window: *mut c_void);
    fn ANativeWindow_release(window: *mut c_void);

    fn ANativeActivity_finish(activity: *mut ANativeActivity);
    fn ANativeActivity_showSoftInput(activity: *mut ANativeActivity, flags: u32);
    fn ANativeActivity_hideSoftInput(activity: *mut ANativeActivity, flags: u32);
    fn ANativeActivity_setWindowFlags(activity: *mut ANativeActivity, add: u32, remove: u32);

    fn ALooper_prepare(opts: c_int) -> *mut c_void;
    fn ALooper_forThread() -> *mut c_void;
    fn ALooper_acquire(looper: *mut c_void);
    fn ALooper_release(looper: *mut c_void);
    fn ALooper_wake(looper: *mut c_void);
    fn ALooper_addFd(
        looper: *mut c_void,
        fd: c_int,
        ident: c_int,
        events: c_int,
        callback: *const c_void,
        data: *mut c_void,
    ) -> c_int;
    fn ALooper_removeFd(looper: *mut c_void, fd: c_int) -> c_int;
    fn ALooper_pollOnce(
        timeout_ms: c_int,
        out_fd: *mut c_int,
        out_events: *mut c_int,
        out_data: *mut *mut c_void,
    ) -> c_int;

    fn AInputQueue_attachLooper(
        queue: *mut c_void,
        looper: *mut c_void,
        ident: c_int,
        callback: *const c_void,
        data: *mut c_void,
    );
    fn AInputQueue_detachLooper(queue: *mut c_void);
    fn AInputQueue_getEvent(queue: *mut c_void, out_event: *mut *mut c_void) -> i32;
    fn AInputQueue_preDispatchEvent(queue: *mut c_void, event: *mut c_void) -> i32;
    fn AInputQueue_finishEvent(queue: *mut c_void, event: *mut c_void, handled: c_int);

    fn AInputEvent_getType(event: *const c_void) -> i32;
    fn AInputEvent_getDeviceId(event: *const c_void) -> i32;
    fn AKeyEvent_getAction(event: *const c_void) -> i32;
    fn AKeyEvent_getKeyCode(event: *const c_void) -> i32;
    fn AKeyEvent_getMetaState(event: *const c_void) -> i32;
    fn AMotionEvent_getAction(event: *const c_void) -> i32;
    fn AMotionEvent_getMetaState(event: *const c_void) -> i32;
    fn AMotionEvent_getButtonState(event: *const c_void) -> i32;
    fn AMotionEvent_getPointerCount(event: *const c_void) -> usize;
    fn AMotionEvent_getPointerId(event: *const c_void, index: usize) -> i32;
    fn AMotionEvent_getToolType(event: *const c_void, index: usize) -> i32;
    fn AMotionEvent_getX(event: *const c_void, index: usize) -> f32;
    fn AMotionEvent_getY(event: *const c_void, index: usize) -> f32;
    fn AMotionEvent_getAxisValue(event: *const c_void, axis: i32, index: usize) -> f32;

    fn AConfiguration_new() -> *mut c_void;
    fn AConfiguration_delete(config: *mut c_void);
    fn AConfiguration_fromAssetManager(config: *mut c_void, assets: *mut c_void);
    fn AConfiguration_getDensity(config: *const c_void) -> i32;
    fn AConfiguration_getUiModeNight(config: *const c_void) -> i32;
}

#[link(name = "log")]
unsafe extern "C" {
    fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}

#[link(name = "dl")]
unsafe extern "C" {
    fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
}

unsafe extern "C" {
    fn pipe2(fds: *mut c_int, flags: c_int) -> c_int;
    fn dup2(old: c_int, new: c_int) -> c_int;
    fn read(fd: c_int, buf: *mut c_void, n: usize) -> isize;
    fn write(fd: c_int, buf: *const c_void, n: usize) -> isize;
    fn close(fd: c_int) -> c_int;
}

const O_NONBLOCK: c_int = 0o4000;
const O_CLOEXEC: c_int = 0o2_000_000;
const RTLD_NOW: c_int = 2;

/// `ANativeActivityCallbacks` — the lifecycle/window function-pointer table a
/// `NativeActivity` invokes. Only the fields a backend wires up need non-null
/// pointers; the rest stay null. Field order matches `native_activity.h`.
#[repr(C)]
#[derive(Debug)]
pub struct ANativeActivityCallbacks {
    pub on_start: Option<unsafe extern "C" fn(*mut ANativeActivity)>,
    pub on_resume: Option<unsafe extern "C" fn(*mut ANativeActivity)>,
    pub on_save_instance_state:
        Option<unsafe extern "C" fn(*mut ANativeActivity, *mut usize) -> *mut c_void>,
    pub on_pause: Option<unsafe extern "C" fn(*mut ANativeActivity)>,
    pub on_stop: Option<unsafe extern "C" fn(*mut ANativeActivity)>,
    pub on_destroy: Option<unsafe extern "C" fn(*mut ANativeActivity)>,
    pub on_window_focus_changed: Option<unsafe extern "C" fn(*mut ANativeActivity, i32)>,
    pub on_native_window_created: Option<unsafe extern "C" fn(*mut ANativeActivity, *mut c_void)>,
    pub on_native_window_resized: Option<unsafe extern "C" fn(*mut ANativeActivity, *mut c_void)>,
    pub on_native_window_redraw_needed:
        Option<unsafe extern "C" fn(*mut ANativeActivity, *mut c_void)>,
    pub on_native_window_destroyed: Option<unsafe extern "C" fn(*mut ANativeActivity, *mut c_void)>,
    pub on_input_queue_created: Option<unsafe extern "C" fn(*mut ANativeActivity, *mut c_void)>,
    pub on_input_queue_destroyed: Option<unsafe extern "C" fn(*mut ANativeActivity, *mut c_void)>,
    pub on_content_rect_changed: Option<unsafe extern "C" fn(*mut ANativeActivity, *const ARect)>,
    pub on_configuration_changed: Option<unsafe extern "C" fn(*mut ANativeActivity)>,
    pub on_low_memory: Option<unsafe extern "C" fn(*mut ANativeActivity)>,
}

/// `ANativeActivity` — the activity handed to `ANativeActivity_onCreate`.
/// Field order matches `native_activity.h`.
#[repr(C)]
#[derive(Debug)]
pub struct ANativeActivity {
    pub callbacks: *mut ANativeActivityCallbacks,
    pub vm: *mut c_void,
    pub env: *mut c_void,
    pub clazz: *mut c_void,
    pub internal_data_path: *const c_char,
    pub external_data_path: *const c_char,
    pub sdk_version: i32,
    pub instance: *mut c_void,
    pub asset_manager: *mut c_void,
    pub obb_path: *const c_char,
}

/// Write `msg` to logcat under the `stipple` tag.
pub fn log(msg: &str) {
    const ANDROID_LOG_INFO: c_int = 4;
    if let Ok(text) = std::ffi::CString::new(msg) {
        unsafe { __android_log_write(ANDROID_LOG_INFO, c"stipple".as_ptr(), text.as_ptr()) };
    }
}

/// Send the process's stdout and stderr to logcat, line by line, so `println!`
/// and Stipple's own diagnostics show up in `adb logcat`. Done once.
fn redirect_stdio() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let mut fds = [0 as c_int; 2];
        if unsafe { pipe2(fds.as_mut_ptr(), O_CLOEXEC) } != 0 {
            return;
        }
        let (rd, wr) = (fds[0], fds[1]);
        unsafe {
            dup2(wr, 1);
            dup2(wr, 2);
            close(wr);
        }
        let _ = std::thread::Builder::new()
            .name("stipple-stdio".into())
            .spawn(move || {
                let mut pending = Vec::new();
                let mut buf = [0u8; 1024];
                loop {
                    let n = unsafe { read(rd, buf.as_mut_ptr().cast(), buf.len()) };
                    if n <= 0 {
                        break;
                    }
                    pending.extend_from_slice(&buf[..n as usize]);
                    while let Some(i) = pending.iter().position(|&b| b == b'\n') {
                        let line: Vec<u8> = pending.drain(..=i).collect();
                        log(String::from_utf8_lossy(&line[..i]).trim_end());
                    }
                }
            });
    });
}

// ---- The activity, shared with the Java main thread -------------------------

/// A lifecycle event from an activity callback, handled on the UI thread.
#[derive(Clone, Copy, Debug)]
enum Command {
    Start,
    Resume,
    Pause,
    Stop,
    Destroy,
    Focus(bool),
    /// An `ANativeWindow*`, acquired by the callback for the UI thread.
    WindowCreated(usize),
    WindowResized,
    RedrawNeeded,
    WindowDestroyed,
    /// An `AInputQueue*`.
    InputQueueCreated(usize),
    InputQueueDestroyed,
    ContentRect(ARect),
    ConfigChanged,
    LowMemory,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// The UI thread hasn't reached [`run`] yet; commands queue up.
    Starting,
    Running,
    /// The loop has ended (or never will): callbacks no longer wait.
    Exited,
}

#[derive(Debug)]
struct Shared {
    queue: VecDeque<Command>,
    /// How many commands were queued, and how many the UI thread handled.
    posted: u64,
    done: u64,
    phase: Phase,
}

/// One activity: what its callbacks share with the UI thread. The callbacks
/// reach it through `ANativeActivity::instance`, the UI thread through an
/// `Arc` of its own.
#[derive(Debug)]
struct Instance {
    activity: *mut ANativeActivity,
    /// The command pipe: callbacks write a byte, the looper watches the read end.
    pipe: (c_int, c_int),
    shared: Mutex<Shared>,
    cond: Condvar,
    /// The way back to the main thread, for the calls Android wants made
    /// there: a pipe its looper watches, and what the UI thread asked for.
    main_looper: *mut c_void,
    main_pipe: (c_int, c_int),
    requests: Mutex<MainRequests>,
}

/// Work the UI thread hands to the main thread.
#[derive(Debug, Default)]
struct MainRequests {
    /// Color the system bars: the background as ARGB, and whether it's light.
    bars: Option<(u32, bool)>,
    /// Show (`true`) or hide the soft keyboard.
    soft_input: Option<bool>,
}

// The activity pointer is only dereferenced while the activity lives (until
// the UI thread has handled `Destroy`, which `onDestroy` waits for), and the
// rest is behind the mutex.
unsafe impl Send for Instance {}
unsafe impl Sync for Instance {}

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            close(self.pipe.0);
            close(self.pipe.1);
            close(self.main_pipe.0);
            close(self.main_pipe.1);
        }
    }
}

impl Instance {
    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Queue `cmd` for the UI thread; with `sync`, wait until it has been
    /// handled. Returns `false` if the loop has ended, so nobody will.
    fn post(&self, cmd: Command, sync: bool) -> bool {
        let mut s = self.lock();
        if s.phase == Phase::Exited {
            return false;
        }
        s.queue.push_back(cmd);
        s.posted += 1;
        let seq = s.posted;
        unsafe { write(self.pipe.1, [1u8].as_ptr().cast(), 1) };
        while sync && s.done < seq && s.phase != Phase::Exited {
            s = self.cond.wait(s).unwrap_or_else(|e| e.into_inner());
        }
        true
    }

    /// Ask the main thread to do something (see [`MainRequests`]).
    fn request(&self, f: impl FnOnce(&mut MainRequests)) {
        f(&mut self.requests.lock().unwrap_or_else(|e| e.into_inner()));
        unsafe { write(self.main_pipe.1, [1u8].as_ptr().cast(), 1) };
    }

    fn next(&self) -> Option<Command> {
        self.lock().queue.pop_front()
    }

    /// The UI thread finished a command taken with [`next`](Self::next).
    fn handled(&self) {
        self.lock().done += 1;
        self.cond.notify_all();
    }

    fn set_phase(&self, phase: Phase) {
        let mut s = self.lock();
        s.phase = phase;
        if phase == Phase::Exited {
            // Nobody will handle what's left; let go of the windows in it.
            for cmd in s.queue.drain(..) {
                if let Command::WindowCreated(w) = cmd {
                    unsafe { ANativeWindow_release(w as *mut c_void) };
                }
            }
        }
        drop(s);
        self.cond.notify_all();
    }
}

/// The running activity, for the clipboard and the system look, which can be
/// asked for from any thread.
static CURRENT: Mutex<Option<Arc<Instance>>> = Mutex::new(None);

thread_local! {
    /// The activity of the UI thread [`start`] spawned, until [`run`] takes it.
    static PENDING: RefCell<Option<Arc<Instance>>> = const { RefCell::new(None) };
}

/// Run `f` on the current activity, if one is alive.
fn with_activity<R>(f: impl FnOnce(&ANativeActivity) -> Option<R>) -> Option<R> {
    let current = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
    let instance = current.as_ref()?;
    // Valid while CURRENT holds it: `Destroy` clears CURRENT first.
    f(unsafe { &*instance.activity })
}

/// Start a Stipple app from `ANativeActivity_onCreate`: register the activity
/// callbacks and run `main` on a new thread, the UI thread, where it builds an
/// [`App`](crate::App) and calls [`App::run`](crate::App::run).
///
/// ```ignore
/// #[unsafe(no_mangle)]
/// pub unsafe extern "C" fn ANativeActivity_onCreate(
///     activity: *mut ANativeActivity,
///     saved_state: *mut c_void,
///     saved_state_size: usize,
/// ) {
///     unsafe { android::start(activity, saved_state, saved_state_size, main) };
/// }
/// ```
///
/// Android may destroy the activity and create a new one in the same process
/// (on a configuration change the manifest doesn't declare in
/// `android:configChanges`, for one): `App::run` then returns, and `main` runs
/// again on a fresh thread for the new activity.
///
/// The process's stdout and stderr are sent to logcat, under the `stipple` tag.
///
/// # Safety
/// `activity` must be the live activity Android passed to
/// `ANativeActivity_onCreate`, and this must be called from that function.
pub unsafe fn start<F>(
    activity: *mut ANativeActivity,
    _saved_state: *mut c_void,
    _saved_state_size: usize,
    main: F,
) where
    F: FnOnce() + Send + 'static,
{
    if activity.is_null() {
        return;
    }
    redirect_stdio();
    let (mut fds, mut main_fds) = ([0 as c_int; 2], [0 as c_int; 2]);
    unsafe {
        if pipe2(fds.as_mut_ptr(), O_NONBLOCK | O_CLOEXEC) != 0 {
            log("stipple: can't create the command pipe");
            return;
        }
        if pipe2(main_fds.as_mut_ptr(), O_NONBLOCK | O_CLOEXEC) != 0 {
            log("stipple: can't create the main-thread pipe");
            close(fds[0]);
            close(fds[1]);
            return;
        }
    }
    let instance = Arc::new(Instance {
        activity,
        pipe: (fds[0], fds[1]),
        shared: Mutex::new(Shared {
            queue: VecDeque::new(),
            posted: 0,
            done: 0,
            phase: Phase::Starting,
        }),
        cond: Condvar::new(),
        // onCreate runs on the main thread, so this is its looper.
        main_looper: unsafe { ALooper_forThread() },
        main_pipe: (main_fds[0], main_fds[1]),
        requests: Mutex::new(MainRequests::default()),
    });
    if !instance.main_looper.is_null() {
        unsafe {
            ALooper_acquire(instance.main_looper);
            ALooper_addFd(
                instance.main_looper,
                instance.main_pipe.0,
                ALOOPER_POLL_CALLBACK,
                ALOOPER_EVENT_INPUT,
                on_main_request as *const c_void,
                Arc::as_ptr(&instance) as *mut c_void,
            );
        }
    }
    unsafe {
        let cb = (*activity).callbacks;
        if !cb.is_null() {
            let cb = &mut *cb;
            cb.on_start = Some(on_start);
            cb.on_resume = Some(on_resume);
            cb.on_pause = Some(on_pause);
            cb.on_stop = Some(on_stop);
            cb.on_destroy = Some(on_destroy);
            cb.on_window_focus_changed = Some(on_window_focus_changed);
            cb.on_native_window_created = Some(on_native_window_created);
            cb.on_native_window_resized = Some(on_native_window_resized);
            cb.on_native_window_redraw_needed = Some(on_native_window_redraw_needed);
            cb.on_native_window_destroyed = Some(on_native_window_destroyed);
            cb.on_input_queue_created = Some(on_input_queue_created);
            cb.on_input_queue_destroyed = Some(on_input_queue_destroyed);
            cb.on_content_rect_changed = Some(on_content_rect_changed);
            cb.on_configuration_changed = Some(on_configuration_changed);
            cb.on_low_memory = Some(on_low_memory);
        }
        (*activity).instance = Arc::into_raw(instance.clone()) as *mut c_void;
    }
    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(instance.clone());

    /// Marks the loop ended when the UI thread finishes, however it finishes,
    /// so callbacks never wait on a thread that's gone.
    struct Exit(Arc<Instance>);
    impl Drop for Exit {
        fn drop(&mut self) {
            self.0.set_phase(Phase::Exited);
        }
    }
    let spawned = std::thread::Builder::new()
        .name("stipple-main".into())
        .spawn(move || {
            let _exit = Exit(instance.clone());
            PENDING.with(|p| *p.borrow_mut() = Some(instance));
            main();
        });
    if spawned.is_err() {
        log("stipple: can't start the UI thread");
    }
}

/// The instance a callback's activity carries.
unsafe fn instance<'a>(activity: *mut ANativeActivity) -> Option<&'a Instance> {
    unsafe {
        let ptr = activity.as_ref()?.instance as *const Instance;
        ptr.as_ref()
    }
}

unsafe fn post(activity: *mut ANativeActivity, cmd: Command, sync: bool) {
    if let Some(i) = unsafe { instance(activity) } {
        i.post(cmd, sync);
    }
}

unsafe extern "C" fn on_start(a: *mut ANativeActivity) {
    unsafe { post(a, Command::Start, false) }
}
unsafe extern "C" fn on_resume(a: *mut ANativeActivity) {
    unsafe { post(a, Command::Resume, false) }
}
unsafe extern "C" fn on_pause(a: *mut ANativeActivity) {
    unsafe { post(a, Command::Pause, true) }
}
unsafe extern "C" fn on_stop(a: *mut ANativeActivity) {
    unsafe { post(a, Command::Stop, false) }
}
unsafe extern "C" fn on_destroy(a: *mut ANativeActivity) {
    unsafe {
        post(a, Command::Destroy, true);
        // The UI thread is done with the activity; drop the callbacks' share.
        let ptr = (*a).instance as *const Instance;
        (*a).instance = std::ptr::null_mut();
        if !ptr.is_null() {
            let instance = Arc::from_raw(ptr);
            if !instance.main_looper.is_null() {
                ALooper_removeFd(instance.main_looper, instance.main_pipe.0);
                ALooper_release(instance.main_looper);
            }
            drop(instance);
        }
    }
}

/// On the main thread, when the UI thread has asked for something: color the
/// system bars, or show or hide the soft keyboard.
unsafe extern "C" fn on_main_request(fd: c_int, _events: c_int, data: *mut c_void) -> c_int {
    let mut buf = [0u8; 64];
    while unsafe { read(fd, buf.as_mut_ptr().cast(), buf.len()) } > 0 {}
    // Valid until onDestroy removes this callback, before dropping it.
    let Some(instance) = (unsafe { (data as *const Instance).as_ref() }) else {
        return 0;
    };
    let requests =
        std::mem::take(&mut *instance.requests.lock().unwrap_or_else(|e| e.into_inner()));
    let activity = instance.activity;
    let (vm, clazz, sdk) = unsafe { ((*activity).vm, (*activity).clazz, (*activity).sdk_version) };
    if let Some((argb, light)) = requests.bars {
        unsafe { jni::style_system_bars(vm, clazz, sdk, argb, light) };
    }
    if let Some(show) = requests.soft_input {
        // InputMethodManager, for the decor view: the NDK's own call asks for
        // the activity's content view, which Android 13 and later refuse to
        // serve. The NDK's call stays as the fallback.
        unsafe {
            if !jni::soft_input(vm, clazz, show) {
                if show {
                    ANativeActivity_showSoftInput(activity, 0);
                } else {
                    ANativeActivity_hideSoftInput(activity, 0);
                }
            }
        }
    }
    1
}
unsafe extern "C" fn on_window_focus_changed(a: *mut ANativeActivity, focused: i32) {
    unsafe { post(a, Command::Focus(focused != 0), false) }
}
unsafe extern "C" fn on_native_window_created(a: *mut ANativeActivity, window: *mut c_void) {
    unsafe {
        // Hold a reference for the UI thread, which releases it on destroy.
        ANativeWindow_acquire(window);
        let posted =
            instance(a).is_some_and(|i| i.post(Command::WindowCreated(window as usize), true));
        if !posted {
            ANativeWindow_release(window);
        }
    }
}
unsafe extern "C" fn on_native_window_resized(a: *mut ANativeActivity, _window: *mut c_void) {
    unsafe { post(a, Command::WindowResized, false) }
}
unsafe extern "C" fn on_native_window_redraw_needed(a: *mut ANativeActivity, _window: *mut c_void) {
    unsafe { post(a, Command::RedrawNeeded, true) }
}
unsafe extern "C" fn on_native_window_destroyed(a: *mut ANativeActivity, _window: *mut c_void) {
    unsafe { post(a, Command::WindowDestroyed, true) }
}
unsafe extern "C" fn on_input_queue_created(a: *mut ANativeActivity, queue: *mut c_void) {
    unsafe { post(a, Command::InputQueueCreated(queue as usize), true) }
}
unsafe extern "C" fn on_input_queue_destroyed(a: *mut ANativeActivity, _queue: *mut c_void) {
    unsafe { post(a, Command::InputQueueDestroyed, true) }
}
unsafe extern "C" fn on_content_rect_changed(a: *mut ANativeActivity, rect: *const ARect) {
    if let Some(rect) = unsafe { rect.as_ref() } {
        unsafe { post(a, Command::ContentRect(*rect), false) }
    }
}
unsafe extern "C" fn on_configuration_changed(a: *mut ANativeActivity) {
    unsafe { post(a, Command::ConfigChanged, false) }
}
unsafe extern "C" fn on_low_memory(a: *mut ANativeActivity) {
    unsafe { post(a, Command::LowMemory, false) }
}

// ---- Configuration and the system look --------------------------------------

/// What the stack reads from the activity's configuration.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Config {
    /// Physical pixels per logical pixel: the density over 160 dpi.
    scale: f64,
    dark: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            scale: 1.0,
            dark: false,
        }
    }
}

/// The density bucket's scale factor. 160 dpi is 1x.
fn density_scale(density: i32) -> f64 {
    match density {
        ACONFIGURATION_DENSITY_DEFAULT => 1.0,
        d if d <= 0 || d >= ACONFIGURATION_DENSITY_ANY => 1.0,
        d => f64::from(d) / f64::from(ACONFIGURATION_DENSITY_MEDIUM),
    }
}

/// Read the configuration from the activity's `Resources` over JNI, which
/// follow changes; the asset manager's copy that `AConfiguration` reads can lag
/// behind `onConfigurationChanged`. The NDK's is the fallback.
fn read_config(activity: &ANativeActivity) -> Config {
    if let Some((dpi, dark)) = unsafe { jni::configuration(activity.vm, activity.clazz) } {
        return Config {
            scale: density_scale(dpi),
            dark,
        };
    }
    if activity.asset_manager.is_null() {
        return Config::default();
    }
    unsafe {
        let config = AConfiguration_new();
        if config.is_null() {
            return Config::default();
        }
        AConfiguration_fromAssetManager(config, activity.asset_manager);
        let out = Config {
            scale: density_scale(AConfiguration_getDensity(config)),
            dark: AConfiguration_getUiModeNight(config) == ACONFIGURATION_UI_MODE_NIGHT_YES,
        };
        AConfiguration_delete(config);
        out
    }
}

/// The system's light/dark mode, and its accent color on Android 12 and later,
/// for [`native::system_look`](crate::platform::native::system_look).
pub(crate) fn system_look() -> crate::platform::native::SystemLook {
    with_activity(|a| {
        let config = read_config(a);
        let accent = (a.sdk_version >= 31)
            .then(|| unsafe { jni::accent_color(a.vm, a.clazz, config.dark) })
            .flatten()
            .map(|argb| {
                crate::render::Color::rgb((argb >> 16) as u8, (argb >> 8) as u8, argb as u8)
            });
        Some(crate::platform::native::SystemLook {
            dark: config.dark,
            accent,
        })
    })
    .unwrap_or_default()
}

/// The clipboard's text, through `ClipboardManager` over JNI.
pub(crate) fn clipboard_text() -> Option<String> {
    with_activity(|a| unsafe { jni::clipboard_text(a.vm, a.clazz) })
}

/// Replace the clipboard's text, through `ClipboardManager` over JNI.
pub(crate) fn set_clipboard_text(text: &str) -> bool {
    with_activity(|a| unsafe { jni::set_clipboard_text(a.vm, a.clazz, text) }.then_some(()))
        .is_some()
}

/// Open `url` in the app that handles it, through an `ACTION_VIEW` intent.
pub(crate) fn open_url(url: &str) -> bool {
    with_activity(|a| unsafe { jni::open_url(a.vm, a.clazz, url) }.then_some(())).is_some()
}

// ---- The frame clock: AChoreographer -----------------------------------------

/// Animation frames from `AChoreographer`, resolved at run time: it appeared
/// in API 24 and its 64-bit callback in API 29, above the API 21 the NDK
/// links against.
mod choreographer {
    use super::{RTLD_NOW, c_long, c_void, dlopen, dlsym};
    use std::sync::Arc;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, Ordering};

    type Callback64 = unsafe extern "C" fn(i64, *mut c_void);
    type Callback = unsafe extern "C" fn(c_long, *mut c_void);

    struct Api {
        get_instance: unsafe extern "C" fn() -> *mut c_void,
        post64: Option<unsafe extern "C" fn(*mut c_void, Callback64, *mut c_void)>,
        post: Option<unsafe extern "C" fn(*mut c_void, Callback, *mut c_void)>,
    }

    fn api() -> Option<&'static Api> {
        static API: OnceLock<Option<Api>> = OnceLock::new();
        API.get_or_init(|| unsafe {
            let lib = dlopen(c"libandroid.so".as_ptr(), RTLD_NOW);
            if lib.is_null() {
                return None;
            }
            let get_instance = dlsym(lib, c"AChoreographer_getInstance".as_ptr());
            if get_instance.is_null() {
                return None;
            }
            let post64 = dlsym(lib, c"AChoreographer_postFrameCallback64".as_ptr());
            let post = dlsym(lib, c"AChoreographer_postFrameCallback".as_ptr());
            Some(Api {
                get_instance: std::mem::transmute::<
                    *mut c_void,
                    unsafe extern "C" fn() -> *mut c_void,
                >(get_instance),
                post64: (!post64.is_null()).then(|| {
                    std::mem::transmute::<
                        *mut c_void,
                        unsafe extern "C" fn(*mut c_void, Callback64, *mut c_void),
                    >(post64)
                }),
                post: (!post.is_null()).then(|| {
                    std::mem::transmute::<
                        *mut c_void,
                        unsafe extern "C" fn(*mut c_void, Callback, *mut c_void),
                    >(post)
                }),
            })
        })
        .as_ref()
    }

    /// Whether a frame callback is outstanding, so one is posted at a time.
    static ARMED: AtomicBool = AtomicBool::new(false);

    fn ticked() {
        ARMED.store(false, Ordering::Release);
        crate::platform::frame::tick();
    }

    unsafe extern "C" fn on_frame64(_nanos: i64, _data: *mut c_void) {
        ticked();
    }

    unsafe extern "C" fn on_frame(_nanos: c_long, _data: *mut c_void) {
        ticked();
    }

    /// Install the choreographer of the calling thread (the UI thread, which
    /// has a looper) as the frame source. Without one, the frame clock's timer
    /// paces frames instead.
    pub(super) fn install() {
        let Some(api) = api() else {
            return;
        };
        let choreographer = unsafe { (api.get_instance)() };
        if choreographer.is_null() {
            return;
        }
        let choreographer = choreographer as usize;
        ARMED.store(false, Ordering::Release);
        crate::platform::frame::install(
            Arc::new(move || {
                if ARMED.swap(true, Ordering::AcqRel) {
                    return;
                }
                let ch = choreographer as *mut c_void;
                unsafe {
                    // The 32-bit build's `long` callback overflows its
                    // nanoseconds, which only matters if they're read.
                    if let Some(post64) = api.post64 {
                        post64(ch, on_frame64, std::ptr::null_mut());
                    } else if let Some(post) = api.post {
                        post(ch, on_frame, std::ptr::null_mut());
                    }
                }
            }),
            None,
        );
    }
}

// ---- The window --------------------------------------------------------------

/// The window state the event loop, the [`Window`] and the [`Surface`] share
/// on the UI thread.
#[derive(Debug)]
struct View {
    instance: Arc<Instance>,
    /// The `ANativeWindow*`, while there is one.
    window: Cell<*mut c_void>,
    /// The window's full size, in pixels.
    window_size: Cell<(i32, i32)>,
    /// Where the app draws: the window less the system bars.
    content: Cell<Option<ARect>>,
    scale: Cell<f64>,
    /// The color around the content rectangle and behind the system bars: the
    /// app's background, read from the corner of each frame.
    background: Cell<[u8; 4]>,
    redraw: Cell<bool>,
    /// Whether a text input has focus, and whether the soft keyboard was asked
    /// to show for it.
    text_input: Cell<bool>,
    soft_input: Cell<bool>,
    /// Whether a frame has reached the screen yet (logged once, for CI).
    presented: Cell<bool>,
}

impl View {
    /// The content rectangle, clipped to the window; the whole window until
    /// the system reports one.
    fn content_rect(&self) -> ARect {
        let (w, h) = self.window_size.get();
        let full = ARect {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        };
        match self.content.get() {
            Some(r) => {
                let r = ARect {
                    left: r.left.clamp(0, w),
                    top: r.top.clamp(0, h),
                    right: r.right.clamp(0, w),
                    bottom: r.bottom.clamp(0, h),
                };
                if r.right > r.left && r.bottom > r.top {
                    r
                } else {
                    full
                }
            }
            None => full,
        }
    }

    fn inner_size(&self) -> PhysicalSize {
        let r = self.content_rect();
        PhysicalSize::new(
            (r.right - r.left).max(1) as u32,
            (r.bottom - r.top).max(1) as u32,
        )
    }

    /// A window-relative position in pixels as a logical, content-relative point.
    fn to_logical(&self, x: f32, y: f32) -> Point {
        let r = self.content_rect();
        let s = self.scale.get();
        Point::new(
            (f64::from(x) - f64::from(r.left)) / s,
            (f64::from(y) - f64::from(r.top)) / s,
        )
    }

    /// Re-read the window's size. Returns whether it changed.
    fn update_size(&self) -> bool {
        let window = self.window.get();
        if window.is_null() {
            return false;
        }
        let size = unsafe {
            (
                ANativeWindow_getWidth(window),
                ANativeWindow_getHeight(window),
            )
        };
        self.window_size.replace(size) != size
    }

    /// Copy `pixmap` into the window at the content rectangle, filling the rest
    /// with the background. With `damage` (in the pixmap's pixels), only that
    /// part is copied, and the window keeps the rest of its last frame.
    fn present(&self, pixmap: &Pixmap, damage: &[Rect]) {
        let window = self.window.get();
        if window.is_null() {
            return;
        }
        let content = self.content_rect();
        let new_background = pixmap
            .pixel(0, 0)
            .is_some_and(|[r, g, b, _]| self.set_background([r, g, b, 255]));
        // The part of the window to redraw: the damage, moved to the content's
        // place, or everything.
        let mut dirty = (!damage.is_empty() && !new_background).then(|| {
            let (x0, y0, x1, y1) = damage.iter().fold(
                (f64::MAX, f64::MAX, f64::MIN, f64::MIN),
                |(x0, y0, x1, y1), r| {
                    (
                        x0.min(r.min_x()),
                        y0.min(r.min_y()),
                        x1.max(r.max_x()),
                        y1.max(r.max_y()),
                    )
                },
            );
            ARect {
                left: content.left + x0.floor() as i32,
                top: content.top + y0.floor() as i32,
                right: content.left + x1.ceil() as i32,
                bottom: content.top + y1.ceil() as i32,
            }
        });
        let mut buf = ANativeWindowBuffer {
            width: 0,
            height: 0,
            stride: 0,
            format: 0,
            bits: std::ptr::null_mut(),
            reserved: [0; 6],
        };
        let bounds = dirty
            .as_mut()
            .map_or(std::ptr::null_mut(), |d| d as *mut ARect as *mut c_void);
        if unsafe { ANativeWindow_lock(window, &mut buf, bounds) } != 0 || buf.bits.is_null() {
            return;
        }
        let (bw, bh) = (buf.width.max(0), buf.height.max(0));
        // On return the bounds are what must be redrawn, which can be more.
        let area = dirty.unwrap_or(ARect {
            left: 0,
            top: 0,
            right: bw,
            bottom: bh,
        });
        let (ax0, ax1) = (
            area.left.clamp(0, bw) as usize,
            area.right.clamp(0, bw) as usize,
        );
        let (ay0, ay1) = (
            area.top.clamp(0, bh) as usize,
            area.bottom.clamp(0, bh) as usize,
        );
        let dst_stride = buf.stride.max(0) as usize * 4;
        let dst = unsafe {
            std::slice::from_raw_parts_mut(buf.bits as *mut u8, dst_stride * bh as usize)
        };
        let size = pixmap.size();
        let (left, top) = (content.left.max(0) as usize, content.top.max(0) as usize);
        let (pw, ph) = (size.width as usize, size.height as usize);
        // The window columns the pixmap covers.
        let (cx0, cx1) = (left.min(bw as usize), (left + pw).min(bw as usize));
        let src = pixmap.as_bytes();
        let src_stride = pixmap.stride();
        let background = self.background.get();
        let fill = |row: &mut [u8], from: usize, to: usize| {
            if from < to {
                for px in row[from * 4..to * 4].as_chunks_mut::<4>().0 {
                    px.copy_from_slice(&background);
                }
            }
        };
        for y in ay0..ay1 {
            let row = &mut dst[y * dst_stride..y * dst_stride + bw as usize * 4];
            if y < top || y >= top + ph {
                fill(row, ax0, ax1);
                continue;
            }
            fill(row, ax0, ax1.min(cx0));
            let (x0, x1) = (ax0.max(cx0), ax1.min(cx1));
            if x0 < x1 {
                let s = (y - top) * src_stride + (x0 - left) * 4;
                row[x0 * 4..x1 * 4].copy_from_slice(&src[s..s + (x1 - x0) * 4]);
            }
            fill(row, ax0.max(cx1), ax1);
        }
        let ok = unsafe { ANativeWindow_unlockAndPost(window) } == 0;
        if ok && !self.presented.replace(true) {
            // CI looks for this line in logcat.
            log(&format!(
                "Stipple Android: window {bw}x{bh} presented=true content {pw}x{ph} at ({left},{top}) scale {}",
                self.scale.get()
            ));
        }
    }

    /// Show or hide the soft keyboard to match whether a text input has focus.
    fn sync_soft_input(&self) {
        let want = self.text_input.get();
        if want != self.soft_input.replace(want) {
            self.instance.request(|r| r.soft_input = Some(want));
        }
    }

    /// Fill around the content with `rgba`, and color the system bars to match.
    /// Returns whether it changed.
    fn set_background(&self, rgba: [u8; 4]) -> bool {
        if self.background.replace(rgba) == rgba {
            return false;
        }
        let [r, g, b, _] = rgba.map(u32::from);
        let argb = 0xff00_0000 | (r << 16) | (g << 8) | b;
        let light = 299 * r + 587 * g + 114 * b > 128_000;
        self.instance.request(|req| req.bars = Some((argb, light)));
        true
    }
}

#[derive(Debug)]
struct AndroidWindow {
    view: Rc<View>,
}

impl Window for AndroidWindow {
    fn id(&self) -> WindowId {
        WindowId(1)
    }
    fn inner_size(&self) -> PhysicalSize {
        self.view.inner_size()
    }
    fn scale_factor(&self) -> ScaleFactor {
        ScaleFactor::new(self.view.scale.get())
    }
    fn request_redraw(&self) {
        self.view.redraw.set(true);
    }
    fn set_title(&self, _title: &str) {}
    fn clipboard(&self) -> Option<String> {
        clipboard_text()
    }
    fn set_clipboard(&self, text: &str) {
        set_clipboard_text(text);
    }
    fn set_text_input_active(&self, active: bool) {
        self.view.text_input.set(active);
    }
    fn set_fullscreen(&self, fullscreen: bool) {
        set_fullscreen(self.view.instance.activity, fullscreen);
    }
    fn create_surface(&self) -> Box<dyn Surface> {
        Box::new(AndroidSurface {
            view: self.view.clone(),
        })
    }
}

#[derive(Debug)]
struct AndroidSurface {
    view: Rc<View>,
}

impl Surface for AndroidSurface {
    fn resize(&mut self, _size: PhysicalSize) {}
    fn size(&self) -> PhysicalSize {
        self.view.inner_size()
    }
    fn present(&mut self, pixmap: &Pixmap, damage: &[Rect]) {
        self.view.present(pixmap, damage);
    }
}

// ---- The event loop ----------------------------------------------------------

/// The UI thread's loop state.
struct Loop<'h> {
    instance: Arc<Instance>,
    looper: *mut c_void,
    queue: *mut c_void,
    view: Rc<View>,
    window: AndroidWindow,
    config: Config,
    handler: &'h mut dyn FnMut(Event, &dyn Window) -> ControlFlow,
    /// The handler asked to exit; the activity is finishing.
    exiting: bool,
    /// The pointer (touch) id being followed, and the button it pressed.
    pointer: Option<(i32, PointerButton)>,
}

impl Loop<'_> {
    fn deliver(&mut self, event: Event) {
        if self.exiting {
            return;
        }
        if (self.handler)(event, &self.window) == ControlFlow::Exit {
            self.exiting = true;
            unsafe { ANativeActivity_finish(self.instance.activity) };
        }
    }

    /// Tell the app the content size changed.
    fn resized(&mut self) {
        if !self.view.window.get().is_null() {
            let size = self.view.inner_size();
            self.deliver(Event::Resized(size));
        }
    }

    /// Handle one lifecycle command. Returns `true` when the loop should end.
    fn command(&mut self, cmd: Command) -> bool {
        match cmd {
            Command::WindowCreated(w) => {
                let window = w as *mut c_void;
                let old = self.view.window.replace(window);
                if !old.is_null() {
                    unsafe { ANativeWindow_release(old) };
                }
                unsafe { ANativeWindow_setBuffersGeometry(window, 0, 0, WINDOW_FORMAT_RGBX_8888) };
                self.view.update_size();
                self.view.presented.set(false);
                self.resized();
            }
            Command::WindowResized => {
                if self.view.update_size() {
                    self.resized();
                }
            }
            Command::RedrawNeeded => {
                if !self.view.window.get().is_null() {
                    self.view.redraw.set(false);
                    self.deliver(Event::RedrawRequested);
                }
            }
            Command::WindowDestroyed => {
                let old = self.view.window.replace(std::ptr::null_mut());
                if !old.is_null() {
                    unsafe { ANativeWindow_release(old) };
                }
            }
            Command::ContentRect(rect) => {
                let before = self.view.inner_size();
                self.view.content.set(Some(rect));
                if self.view.inner_size() != before {
                    self.resized();
                } else {
                    self.view.redraw.set(true);
                }
            }
            Command::InputQueueCreated(q) => {
                self.detach_queue();
                self.queue = q as *mut c_void;
                unsafe {
                    AInputQueue_attachLooper(
                        self.queue,
                        self.looper,
                        LOOPER_ID_INPUT,
                        std::ptr::null(),
                        std::ptr::null_mut(),
                    )
                };
            }
            Command::InputQueueDestroyed => self.detach_queue(),
            Command::Focus(focused) => self.deliver(Event::FocusChanged(focused)),
            Command::ConfigChanged => {
                let config = read_config(unsafe { &*self.instance.activity });
                let old = std::mem::replace(&mut self.config, config);
                if config.scale != old.scale {
                    self.view.scale.set(config.scale);
                    self.deliver(Event::ScaleFactorChanged(ScaleFactor::new(config.scale)));
                    self.resized();
                }
                if config.dark != old.dark {
                    // Delivered as `ThemeChanged` ahead of the wake this causes.
                    crate::platform::native::note_changed();
                }
            }
            Command::Resume | Command::Start => self.view.redraw.set(true),
            Command::Pause | Command::Stop => {}
            Command::LowMemory => crate::platform::native::clear_cache(),
            Command::Destroy => {
                let mut current = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
                if current
                    .as_ref()
                    .is_some_and(|c| Arc::ptr_eq(c, &self.instance))
                {
                    *current = None;
                }
                drop(current);
                self.deliver(Event::CloseRequested);
                return true;
            }
        }
        false
    }

    fn detach_queue(&mut self) {
        if !self.queue.is_null() {
            unsafe { AInputQueue_detachLooper(self.queue) };
            self.queue = std::ptr::null_mut();
        }
    }

    /// Handle every event waiting in the input queue.
    fn input(&mut self) {
        loop {
            let queue = self.queue;
            if queue.is_null() {
                return;
            }
            let mut event = std::ptr::null_mut();
            if unsafe { AInputQueue_getEvent(queue, &mut event) } < 0 {
                return;
            }
            // The IME gets the first look at key events.
            if unsafe { AInputQueue_preDispatchEvent(queue, event) } != 0 {
                continue;
            }
            let handled = match unsafe { AInputEvent_getType(event) } {
                AINPUT_EVENT_TYPE_KEY => self.key(event),
                AINPUT_EVENT_TYPE_MOTION => self.motion(event),
                _ => false,
            };
            unsafe { AInputQueue_finishEvent(queue, event, handled as c_int) };
        }
    }

    /// A key. Named keys become [`Event::Key`]; keys that type a character
    /// become [`Event::Text`], or a `Key` carrying the character while Ctrl or
    /// Meta is held (a shortcut). Keys the app doesn't know (Back, volume)
    /// stay with the system.
    fn key(&mut self, event: *mut c_void) -> bool {
        let (action, code, meta, device) = unsafe {
            (
                AKeyEvent_getAction(event),
                AKeyEvent_getKeyCode(event),
                AKeyEvent_getMetaState(event),
                AInputEvent_getDeviceId(event),
            )
        };
        let state = match action {
            AKEY_EVENT_ACTION_DOWN => ButtonState::Pressed,
            AKEY_EVENT_ACTION_UP => ButtonState::Released,
            _ => return false,
        };
        let modifiers = input::android_modifiers(meta);
        if let Some(code) = input::android_keycode(code) {
            self.deliver(Event::Key {
                code,
                state,
                modifiers,
            });
            return true;
        }
        // The character without Ctrl and Meta, which would map it to nothing.
        let typing = meta & !(AMETA_CTRL_MASK | AMETA_META_MASK);
        let vm = unsafe { (*self.instance.activity).vm };
        let Some(c) = unsafe { jni::unicode_char(vm, device, code, typing) }
            .or_else(|| input::android_ascii(code, modifiers.shift))
            .filter(|c| !c.is_control())
        else {
            return false;
        };
        if modifiers.ctrl || modifiers.meta {
            self.deliver(Event::Key {
                code: crate::platform::event::KeyCode::Char(c.to_ascii_lowercase()),
                state,
                modifiers,
            });
        } else if state == ButtonState::Pressed {
            self.deliver(Event::Text(c.to_string()));
        }
        true
    }

    /// A touch, mouse or stylus event. The first finger down is the pointer
    /// until it lifts; other fingers are ignored.
    fn motion(&mut self, event: *mut c_void) -> bool {
        let raw = unsafe { AMotionEvent_getAction(event) };
        let action = raw & AMOTION_EVENT_ACTION_MASK;
        let index = ((raw >> 8) & 0xff) as usize;
        let at = |i: usize| unsafe {
            self.view
                .to_logical(AMotionEvent_getX(event, i), AMotionEvent_getY(event, i))
        };
        let mouse = unsafe { AMotionEvent_getToolType(event, 0) } == AMOTION_EVENT_TOOL_TYPE_MOUSE;
        crate::platform::set_pointer_modifiers(input::android_modifiers(unsafe {
            AMotionEvent_getMetaState(event)
        }));
        let find = |id: i32| {
            let n = unsafe { AMotionEvent_getPointerCount(event) };
            (0..n).find(|&i| unsafe { AMotionEvent_getPointerId(event, i) } == id)
        };
        match action {
            AMOTION_EVENT_ACTION_DOWN => {
                let buttons = unsafe { AMotionEvent_getButtonState(event) };
                let button = if mouse && buttons & AMOTION_EVENT_BUTTON_SECONDARY != 0 {
                    PointerButton::Right
                } else if mouse && buttons & AMOTION_EVENT_BUTTON_TERTIARY != 0 {
                    PointerButton::Middle
                } else {
                    PointerButton::Left
                };
                let id = unsafe { AMotionEvent_getPointerId(event, 0) };
                self.pointer = Some((id, button));
                let position = at(0);
                self.deliver(Event::PointerMoved { position });
                self.deliver(Event::PointerButton {
                    button,
                    state: ButtonState::Pressed,
                    position,
                });
            }
            AMOTION_EVENT_ACTION_MOVE => {
                if let Some(i) = self.pointer.and_then(|(id, _)| find(id)) {
                    self.deliver(Event::PointerMoved { position: at(i) });
                }
            }
            AMOTION_EVENT_ACTION_UP | AMOTION_EVENT_ACTION_POINTER_UP => {
                let lifted = unsafe { AMotionEvent_getPointerId(event, index) };
                if let Some((id, button)) = self.pointer
                    && id == lifted
                {
                    self.pointer = None;
                    self.deliver(Event::PointerButton {
                        button,
                        state: ButtonState::Released,
                        position: at(index),
                    });
                    if !mouse {
                        // A finger that lifts leaves no hover behind.
                        self.deliver(Event::PointerLeft);
                    }
                }
            }
            AMOTION_EVENT_ACTION_CANCEL => {
                if let Some((_, button)) = self.pointer.take() {
                    // Released far away, so the press doesn't count as a tap.
                    self.deliver(Event::PointerButton {
                        button,
                        state: ButtonState::Released,
                        position: Point::new(-1.0e9, -1.0e9),
                    });
                    self.deliver(Event::PointerLeft);
                }
            }
            AMOTION_EVENT_ACTION_HOVER_MOVE | AMOTION_EVENT_ACTION_HOVER_ENTER => {
                self.deliver(Event::PointerMoved { position: at(0) });
            }
            AMOTION_EVENT_ACTION_HOVER_EXIT => self.deliver(Event::PointerLeft),
            AMOTION_EVENT_ACTION_SCROLL => {
                let (h, v) = unsafe {
                    (
                        AMotionEvent_getAxisValue(event, AMOTION_EVENT_AXIS_HSCROLL, 0),
                        AMotionEvent_getAxisValue(event, AMOTION_EVENT_AXIS_VSCROLL, 0),
                    )
                };
                self.deliver(Event::PointerMoved { position: at(0) });
                self.deliver(Event::Scroll {
                    delta: input::android_scroll(h, v),
                });
            }
            _ => return false,
        }
        true
    }

    /// Drain the command pipe's bytes; the commands themselves are in the queue.
    fn drain_pipe(&self) {
        let mut buf = [0u8; 64];
        while unsafe { read(self.instance.pipe.0, buf.as_mut_ptr().cast(), buf.len()) } > 0 {}
    }
}

/// Run the event loop on the UI thread [`start`] spawned, until the activity
/// is destroyed or the handler exits (which finishes the activity).
pub fn run<H>(attrs: WindowAttributes, mut handler: H) -> Result<(), PlatformError>
where
    H: FnMut(Event, &dyn Window) -> ControlFlow,
{
    let instance = PENDING
        .with(|p| p.borrow_mut().take())
        .ok_or(PlatformError::NoBackend(
            "no NativeActivity: call App::run from the thread android::start spawned",
        ))?;
    let looper = unsafe { ALooper_prepare(ALOOPER_PREPARE_ALLOW_NON_CALLBACKS) };
    if looper.is_null() {
        instance.set_phase(Phase::Exited);
        return Err(PlatformError::Os("ALooper_prepare failed".into()));
    }
    unsafe {
        ALooper_acquire(looper);
        ALooper_addFd(
            looper,
            instance.pipe.0,
            LOOPER_ID_COMMAND,
            ALOOPER_EVENT_INPUT,
            std::ptr::null(),
            std::ptr::null_mut(),
        );
    }
    let activity = unsafe { &*instance.activity };
    let vm = activity.vm;
    unsafe { jni::attach(vm) };

    // Wakeups and timers interrupt the poll.
    let looper_addr = looper as usize;
    crate::platform::wake::install(
        Arc::new(move || unsafe { ALooper_wake(looper_addr as *mut c_void) }),
        None,
    );
    choreographer::install();

    let config = read_config(activity);
    if attrs.fullscreen {
        set_fullscreen(instance.activity, true);
    }
    let view = Rc::new(View {
        instance: instance.clone(),
        window: Cell::new(std::ptr::null_mut()),
        window_size: Cell::new((1, 1)),
        content: Cell::new(None),
        scale: Cell::new(config.scale),
        background: Cell::new([0, 0, 0, 0]),
        redraw: Cell::new(false),
        text_input: Cell::new(false),
        soft_input: Cell::new(false),
        presented: Cell::new(false),
    });
    let mut lp = Loop {
        instance: instance.clone(),
        looper,
        queue: std::ptr::null_mut(),
        window: AndroidWindow { view: view.clone() },
        view,
        config,
        handler: &mut handler,
        exiting: false,
        pointer: None,
    };
    if let Some(c) = attrs.background {
        lp.view.set_background([c.r, c.g, c.b, 255]);
    }
    instance.set_phase(Phase::Running);

    'run: loop {
        while let Some(cmd) = instance.next() {
            let end = lp.command(cmd);
            instance.handled();
            if end {
                break 'run;
            }
        }
        if crate::platform::wake::take(Instant::now()) {
            lp.deliver(Event::Wake);
        }
        if lp.view.redraw.take() && !lp.view.window.get().is_null() {
            lp.deliver(Event::RedrawRequested);
        }
        lp.view.sync_soft_input();

        let ms = match crate::platform::wake::timeout(Instant::now()) {
            // Round up, so the poll doesn't wake a hair early and spin.
            Some(t) => t.as_micros().div_ceil(1000).min(c_int::MAX as u128) as c_int,
            None => -1,
        };
        let (mut fd, mut events, mut data) = (0, 0, std::ptr::null_mut());
        match unsafe { ALooper_pollOnce(ms, &mut fd, &mut events, &mut data) } {
            LOOPER_ID_COMMAND => lp.drain_pipe(),
            LOOPER_ID_INPUT => lp.input(),
            ALOOPER_POLL_ERROR => break,
            // A wake, a callback (the choreographer's) or the timeout.
            _ => {}
        }
    }

    // Leave nothing pointing at this thread's looper.
    crate::platform::wake::install(Arc::new(|| {}), None);
    crate::platform::frame::install(Arc::new(|| {}), None);
    lp.detach_queue();
    let window = lp.view.window.replace(std::ptr::null_mut());
    drop(lp);
    unsafe {
        if !window.is_null() {
            ANativeWindow_release(window);
        }
        ALooper_removeFd(looper, instance.pipe.0);
        ALooper_release(looper);
        jni::detach(vm);
    }
    instance.set_phase(Phase::Exited);
    Ok(())
}

/// Hide (or show again) the status bar. Safe from any thread: the NDK posts
/// it to the main thread.
fn set_fullscreen(activity: *mut ANativeActivity, fullscreen: bool) {
    let (add, remove) = if fullscreen {
        (AWINDOW_FLAG_FULLSCREEN, 0)
    } else {
        (0, AWINDOW_FLAG_FULLSCREEN)
    };
    unsafe { ANativeActivity_setWindowFlags(activity, add, remove) };
}

/// Blit a software [`Pixmap`] to an `ANativeWindow`: size the buffer to the
/// frame, lock it, copy the pixels row by row (honoring the buffer's pixel
/// stride), and post. Returns whether the post succeeded. [`run`] presents
/// through its own surface; this is for apps that drive a window themselves.
///
/// # Safety
/// `window` must be a live `ANativeWindow*`.
pub unsafe fn present_to_native_window(window: *mut c_void, pixmap: &Pixmap) -> bool {
    if window.is_null() {
        return false;
    }
    let size = pixmap.size();
    let (w, h) = (size.width as i32, size.height as i32);
    unsafe {
        ANativeWindow_setBuffersGeometry(window, w, h, WINDOW_FORMAT_RGBA_8888);
        let mut buf = ANativeWindowBuffer {
            width: 0,
            height: 0,
            stride: 0,
            format: 0,
            bits: core::ptr::null_mut(),
            reserved: [0; 6],
        };
        if ANativeWindow_lock(window, &mut buf, core::ptr::null_mut()) != 0 || buf.bits.is_null() {
            return false;
        }
        // Copy each row into the locked buffer, honoring its pixel stride.
        let src = pixmap.as_bytes();
        let src_stride = pixmap.stride();
        let copy_w = (buf.width.min(w)) as usize * 4;
        let rows = buf.height.min(h) as usize;
        let dst_stride = buf.stride as usize * 4;
        for y in 0..rows {
            let s = &src[y * src_stride..y * src_stride + copy_w];
            let d = (buf.bits as *mut u8).add(y * dst_stride);
            core::ptr::copy_nonoverlapping(s.as_ptr(), d, copy_w);
        }
        ANativeWindow_unlockAndPost(window) == 0
    }
}
