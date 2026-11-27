//! Audio output: a stereo `f32` stream at the device's preferred rate, pulled
//! by the OS (or a thread of ours) through a render callback.
//!
//! | Platform | API |
//! |---|---|
//! | macOS | AudioToolbox: the default output `AudioUnit` |
//! | iOS | AudioToolbox: the RemoteIO `AudioUnit`, in an ambient `AVAudioSession` |
//! | Windows | WASAPI, shared mode, event-driven |
//! | Linux | ALSA's `default` device (`libasound.so.2`, loaded at run time) |
//! | Linux, static (fullrust) | the same, loaded through the system's `ld.so` |
//! | Android | AAudio (`libaaudio.so`, loaded at run time; Android 8+) |
//! | Web | Web Audio, pulled by `web/stipple.js` |
//!
//! Where none opens, a null output plays silence at real-time pace.

use std::sync::{Arc, Mutex, OnceLock, Weak};

/// The mixer's render function: fills interleaved stereo `f32` frames.
pub(crate) type Render = Box<dyn FnMut(&mut [f32]) + Send>;

/// An open output stream.
pub(crate) trait Device: Send {
    /// Which backend this is ("coreaudio", "wasapi", ...).
    fn name(&self) -> &'static str;
    /// Stop pulling audio (the app went to the background), or start again.
    fn set_paused(&mut self, paused: bool);
}

#[cfg(target_os = "android")]
#[allow(unsafe_code)]
mod aaudio;
#[cfg(any(target_os = "linux", target_os = "fullrust"))]
#[allow(unsafe_code)]
mod alsa;
#[cfg(any(target_os = "macos", target_os = "ios"))]
#[allow(unsafe_code)]
mod coreaudio;
mod null;
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod wasapi;
#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)]
mod web;

/// The platform's output, if one opens. `make` gets the device's rate and
/// returns the render function.
#[allow(unused_variables, clippy::needless_return)]
fn open_platform(make: &mut dyn FnMut(u32) -> Render) -> Option<Box<dyn Device>> {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    return coreaudio::open(make);
    #[cfg(target_os = "windows")]
    return wasapi::open(make);
    #[cfg(any(target_os = "linux", target_os = "fullrust"))]
    return alsa::open(make);
    #[cfg(target_os = "android")]
    return aaudio::open(make);
    #[cfg(target_arch = "wasm32")]
    return web::open(make);
    #[allow(unreachable_code)]
    None
}

/// Open the default output, falling back to the null output (straight away
/// with `null`, or when `STIPPLE_AUDIO=null` is set in the environment).
pub(crate) fn open(make: &mut dyn FnMut(u32) -> Render, null: bool) -> Arc<Mutex<Box<dyn Device>>> {
    let forced_null = null || std::env::var_os("STIPPLE_AUDIO").is_some_and(|v| v == "null");
    let device = (!forced_null)
        .then(|| open_platform(make))
        .flatten()
        .unwrap_or_else(|| null::open(make));
    let device = Arc::new(Mutex::new(device));
    let mut list = lock(registry());
    list.retain(|w| w.strong_count() > 0);
    list.push(Arc::downgrade(&device));
    if background() {
        lock(&device).set_paused(true);
    }
    device
}

type Registry = Mutex<Vec<Weak<Mutex<Box<dyn Device>>>>>;

/// Every open device, to pause them with the app.
fn registry() -> &'static Registry {
    static DEVICES: OnceLock<Registry> = OnceLock::new();
    DEVICES.get_or_init(|| Mutex::new(Vec::new()))
}

static BACKGROUND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn background() -> bool {
    BACKGROUND.load(std::sync::atomic::Ordering::Relaxed)
}

/// The app went to the background (`true`) or came back: stop or restart
/// every output.
pub(crate) fn set_background(bg: bool) {
    BACKGROUND.store(bg, std::sync::atomic::Ordering::Relaxed);
    let devices: Vec<_> = lock(registry()).iter().filter_map(Weak::upgrade).collect();
    for d in devices {
        lock(&d).set_paused(bg);
    }
}

/// The null output, alone.
#[cfg(test)]
pub(crate) fn null_for_tests(render: Render) -> Box<dyn Device> {
    let mut render = Some(render);
    null::open(&mut |_| render.take().expect("one render function"))
}

pub(crate) fn lock<T: ?Sized>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Run `render` on `buf`, and play silence instead if it panics (a panic must
/// not unwind into the OS's audio thread).
pub(crate) fn render_guarded(render: &mut Render, buf: &mut [f32]) {
    let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| render(buf))).is_ok();
    if !ok {
        buf.fill(0.0);
    }
}
