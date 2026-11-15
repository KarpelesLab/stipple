//! Android: AAudio (Android 8, API 26, and later), from `libaaudio.so`
//! loaded at run time, so older devices start fine and play silence.
//!
//! A low-latency shared float stream with a data callback. When the stream
//! is disconnected (headphones unplugged, the route changed), it's reopened
//! on another thread, as AAudio asks, at the same rate.

use super::{Device, Render};
use std::ffi::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

type Stream = *mut c_void;
type Builder = *mut c_void;
type DataCallback = unsafe extern "C" fn(Stream, *mut c_void, *mut c_void, i32) -> i32;
type ErrorCallback = unsafe extern "C" fn(Stream, *mut c_void, i32);

const AAUDIO_FORMAT_PCM_FLOAT: i32 = 2;
const AAUDIO_PERFORMANCE_MODE_LOW_LATENCY: i32 = 12;
const AAUDIO_SHARING_MODE_SHARED: i32 = 1;
const AAUDIO_USAGE_GAME: i32 = 14;
const AAUDIO_CALLBACK_RESULT_CONTINUE: i32 = 0;
const AAUDIO_ERROR_DISCONNECTED: i32 = -899;
const RTLD_NOW: c_int = 2;

#[link(name = "dl")]
unsafe extern "C" {
    fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
}

struct Lib {
    create_builder: unsafe extern "C" fn(*mut Builder) -> i32,
    set_format: unsafe extern "C" fn(Builder, i32),
    set_channel_count: unsafe extern "C" fn(Builder, i32),
    set_sample_rate: unsafe extern "C" fn(Builder, i32),
    set_performance_mode: unsafe extern "C" fn(Builder, i32),
    set_sharing_mode: unsafe extern "C" fn(Builder, i32),
    /// API 28 and later.
    set_usage: Option<unsafe extern "C" fn(Builder, i32)>,
    set_data_callback: unsafe extern "C" fn(Builder, DataCallback, *mut c_void),
    set_error_callback: unsafe extern "C" fn(Builder, ErrorCallback, *mut c_void),
    open_stream: unsafe extern "C" fn(Builder, *mut Stream) -> i32,
    delete_builder: unsafe extern "C" fn(Builder) -> i32,
    get_sample_rate: unsafe extern "C" fn(Stream) -> i32,
    request_start: unsafe extern "C" fn(Stream) -> i32,
    request_pause: unsafe extern "C" fn(Stream) -> i32,
    request_stop: unsafe extern "C" fn(Stream) -> i32,
    close: unsafe extern "C" fn(Stream) -> i32,
}

/// The function `name` in library `h`, as the function pointer type `F`.
///
/// # Safety
/// `F` must be a function pointer type matching the symbol's signature.
unsafe fn sym<F: Copy>(h: *mut c_void, name: &std::ffi::CStr) -> Option<F> {
    assert_eq!(std::mem::size_of::<F>(), std::mem::size_of::<*mut c_void>());
    let p = unsafe { dlsym(h, name.as_ptr()) };
    // SAFETY: a non-null symbol address, as the caller's function type.
    (!p.is_null()).then(|| unsafe { std::mem::transmute_copy::<*mut c_void, F>(&p) })
}

fn lib() -> Option<&'static Lib> {
    static LIB: std::sync::OnceLock<Option<Lib>> = std::sync::OnceLock::new();
    LIB.get_or_init(|| unsafe {
        let h = dlopen(c"libaaudio.so".as_ptr(), RTLD_NOW);
        if h.is_null() {
            return None;
        }
        Some(Lib {
            create_builder: sym(h, c"AAudio_createStreamBuilder")?,
            set_format: sym(h, c"AAudioStreamBuilder_setFormat")?,
            set_channel_count: sym(h, c"AAudioStreamBuilder_setChannelCount")?,
            set_sample_rate: sym(h, c"AAudioStreamBuilder_setSampleRate")?,
            set_performance_mode: sym(h, c"AAudioStreamBuilder_setPerformanceMode")?,
            set_sharing_mode: sym(h, c"AAudioStreamBuilder_setSharingMode")?,
            set_usage: sym(h, c"AAudioStreamBuilder_setUsage"),
            set_data_callback: sym(h, c"AAudioStreamBuilder_setDataCallback")?,
            set_error_callback: sym(h, c"AAudioStreamBuilder_setErrorCallback")?,
            open_stream: sym(h, c"AAudioStreamBuilder_openStream")?,
            delete_builder: sym(h, c"AAudioStreamBuilder_delete")?,
            get_sample_rate: sym(h, c"AAudioStream_getSampleRate")?,
            request_start: sym(h, c"AAudioStream_requestStart")?,
            request_pause: sym(h, c"AAudioStream_requestPause")?,
            request_stop: sym(h, c"AAudioStream_requestStop")?,
            close: sym(h, c"AAudioStream_close")?,
        })
    })
    .as_ref()
}

/// What the callbacks share with the device.
struct State {
    render: Mutex<Render>,
    /// The open stream (null while reopening or after a failed reopen).
    stream: Mutex<StreamPtr>,
    rate: u32,
    paused: AtomicBool,
    closed: AtomicBool,
}

struct StreamPtr(Stream);

// SAFETY: AAudio streams may be controlled from any thread.
unsafe impl Send for StreamPtr {}

unsafe extern "C" fn data_cb(_s: Stream, user: *mut c_void, data: *mut c_void, frames: i32) -> i32 {
    // SAFETY: `user` is the `State` the device keeps alive until the stream
    // is closed.
    if user.is_null() || data.is_null() || frames <= 0 {
        return AAUDIO_CALLBACK_RESULT_CONTINUE;
    }
    let state = unsafe { &*user.cast::<State>() };
    // SAFETY: AAudio hands `frames` frames of the stream's format (stereo
    // float).
    let out = unsafe { std::slice::from_raw_parts_mut(data.cast::<f32>(), frames as usize * 2) };
    match state.render.try_lock() {
        Ok(mut r) => super::render_guarded(&mut r, out),
        Err(_) => out.fill(0.0),
    }
    AAUDIO_CALLBACK_RESULT_CONTINUE
}

unsafe extern "C" fn error_cb(_s: Stream, user: *mut c_void, error: i32) {
    if error != AAUDIO_ERROR_DISCONNECTED || user.is_null() {
        return;
    }
    // SAFETY: `user` came from `Arc::as_ptr` on the device's state, alive
    // while its stream is open (closing waits for callbacks); the thread
    // below holds its own reference.
    let state = unsafe {
        Arc::increment_strong_count(user.cast::<State>().cast_const());
        Arc::from_raw(user.cast::<State>().cast_const())
    };
    // The stream can't be closed from its own callback: reopen elsewhere.
    let _ = std::thread::Builder::new()
        .name("stipple-audio-reopen".into())
        .spawn(move || reopen(&state));
}

/// Open a stream at `rate` (0 for the device's own), wired to `state`.
fn open_stream(lib: &Lib, rate: u32, user: *mut c_void) -> Option<Stream> {
    unsafe {
        let mut b: Builder = std::ptr::null_mut();
        if (lib.create_builder)(&mut b) != 0 || b.is_null() {
            return None;
        }
        (lib.set_format)(b, AAUDIO_FORMAT_PCM_FLOAT);
        (lib.set_channel_count)(b, 2);
        if rate > 0 {
            (lib.set_sample_rate)(b, rate as i32);
        }
        (lib.set_performance_mode)(b, AAUDIO_PERFORMANCE_MODE_LOW_LATENCY);
        (lib.set_sharing_mode)(b, AAUDIO_SHARING_MODE_SHARED);
        if let Some(set_usage) = lib.set_usage {
            set_usage(b, AAUDIO_USAGE_GAME);
        }
        (lib.set_data_callback)(b, data_cb, user);
        (lib.set_error_callback)(b, error_cb, user);
        let mut s: Stream = std::ptr::null_mut();
        let r = (lib.open_stream)(b, &mut s);
        (lib.delete_builder)(b);
        (r == 0 && !s.is_null()).then_some(s)
    }
}

fn reopen(state: &State) {
    let Some(lib) = lib() else { return };
    let mut stream = super::lock(&state.stream);
    if state.closed.load(Ordering::Acquire) {
        return;
    }
    unsafe {
        if !stream.0.is_null() {
            (lib.request_stop)(stream.0);
            (lib.close)(stream.0);
        }
    }
    stream.0 = std::ptr::null_mut();
    let user = (state as *const State).cast_mut().cast();
    if let Some(s) = open_stream(lib, state.rate, user) {
        if !state.paused.load(Ordering::Relaxed) {
            unsafe { (lib.request_start)(s) };
        }
        stream.0 = s;
    }
}

struct AAudio {
    state: Arc<State>,
}

pub(super) fn open(make: &mut dyn FnMut(u32) -> Render) -> Option<Box<dyn Device>> {
    let lib = lib()?;
    // Open once to learn the device's rate, then for real with the render
    // function in place.
    let probe = open_stream(lib, 0, std::ptr::null_mut())?;
    let rate = unsafe { (lib.get_sample_rate)(probe) };
    unsafe { (lib.close)(probe) };
    let rate = if rate >= 8000 { rate as u32 } else { 48_000 };
    let state = Arc::new(State {
        render: Mutex::new(make(rate)),
        stream: Mutex::new(StreamPtr(std::ptr::null_mut())),
        rate,
        paused: AtomicBool::new(false),
        closed: AtomicBool::new(false),
    });
    let user = Arc::as_ptr(&state).cast_mut().cast();
    let s = open_stream(lib, rate, user)?;
    if unsafe { (lib.request_start)(s) } != 0 {
        unsafe { (lib.close)(s) };
        return None;
    }
    super::lock(&state.stream).0 = s;
    Some(Box::new(AAudio { state }))
}

impl Device for AAudio {
    fn name(&self) -> &'static str {
        "aaudio"
    }

    fn set_paused(&mut self, paused: bool) {
        let Some(lib) = lib() else { return };
        self.state.paused.store(paused, Ordering::Relaxed);
        let stream = super::lock(&self.state.stream);
        if !stream.0.is_null() {
            unsafe {
                if paused {
                    (lib.request_pause)(stream.0);
                } else {
                    (lib.request_start)(stream.0);
                }
            }
        }
    }
}

impl Drop for AAudio {
    fn drop(&mut self) {
        let Some(lib) = lib() else { return };
        let mut stream = super::lock(&self.state.stream);
        self.state.closed.store(true, Ordering::Release);
        if !stream.0.is_null() {
            unsafe {
                (lib.request_stop)(stream.0);
                (lib.close)(stream.0);
            }
            stream.0 = std::ptr::null_mut();
        }
    }
}
