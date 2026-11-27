//! Linux: ALSA's `default` PCM, through `libasound.so.2` loaded at run time
//! (so the program starts on systems without it, and plays silence there).
//!
//! On desktops `default` is PulseAudio's or PipeWire's ALSA plugin, so this
//! reaches the sound server too. A thread of ours writes interleaved float
//! frames with `snd_pcm_writei`, which blocks while the device's buffer is
//! full, and recovers from underruns and suspends with `snd_pcm_recover`.

use super::{Device, Render};
use std::ffi::{c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The rate asked for; ALSA's plug layer (or the sound server) converts if
/// the hardware runs at another.
const RATE: u32 = 48_000;
/// The latency asked for, in microseconds.
const LATENCY_US: c_uint = 40_000;

const SND_PCM_STREAM_PLAYBACK: c_int = 0;
const SND_PCM_FORMAT_FLOAT_LE: c_int = 14;
const SND_PCM_ACCESS_RW_INTERLEAVED: c_int = 3;
type Pcm = *mut c_void;

dlsym::library! {
    /// The few libasound entry points used.
    struct Lib {
        unsafe extern "C" {
            #[link_name = "snd_pcm_open"]
            fn open(pcm: *mut Pcm, name: *const c_char, stream: c_int, mode: c_int) -> c_int;
            #[link_name = "snd_pcm_set_params"]
            fn set_params(
                pcm: Pcm,
                format: c_int,
                access: c_int,
                channels: c_uint,
                rate: c_uint,
                resample: c_int,
                latency_us: c_uint,
            ) -> c_int;
            #[link_name = "snd_pcm_get_params"]
            fn get_params(pcm: Pcm, buffer: *mut c_ulong, period: *mut c_ulong) -> c_int;
            #[link_name = "snd_pcm_writei"]
            fn writei(pcm: Pcm, buf: *const c_void, frames: c_ulong) -> c_long;
            #[link_name = "snd_pcm_recover"]
            fn recover(pcm: Pcm, err: c_int, silent: c_int) -> c_int;
            #[link_name = "snd_pcm_drop"]
            fn drop(pcm: Pcm) -> c_int;
            #[link_name = "snd_pcm_prepare"]
            fn prepare(pcm: Pcm) -> c_int;
            #[link_name = "snd_pcm_close"]
            fn close(pcm: Pcm) -> c_int;
        }
    }
}

/// libasound, or `None` where it isn't installed (or lacks one of these).
fn lib() -> Option<&'static Lib> {
    static LIB: std::sync::OnceLock<Option<Lib>> = std::sync::OnceLock::new();
    LIB.get_or_init(|| {
        // SAFETY: a system library, trusted like the rest of the platform.
        unsafe { <Lib as dlsym::Bindings>::open("libasound.so.2") }.ok()
    })
    .as_ref()
}

struct PcmHandle(Pcm);

// SAFETY: the handle is used by one thread at a time (the writer, then the
// closer after it's joined).
unsafe impl Send for PcmHandle {}

struct Ctl {
    stop: AtomicBool,
    paused: AtomicBool,
}

struct Alsa {
    ctl: Arc<Ctl>,
    thread: Option<std::thread::JoinHandle<()>>,
}

pub(super) fn open(make: &mut dyn FnMut(u32) -> Render) -> Option<Box<dyn Device>> {
    let lib = lib()?;
    let mut pcm: Pcm = std::ptr::null_mut();
    unsafe {
        if lib.open(&mut pcm, c"default".as_ptr(), SND_PCM_STREAM_PLAYBACK, 0) < 0 || pcm.is_null()
        {
            return None;
        }
        if lib.set_params(
            pcm,
            SND_PCM_FORMAT_FLOAT_LE,
            SND_PCM_ACCESS_RW_INTERLEAVED,
            2,
            RATE,
            1,
            LATENCY_US,
        ) < 0
        {
            lib.close(pcm);
            return None;
        }
    }
    let mut buffer: c_ulong = 0;
    let mut period: c_ulong = 0;
    unsafe { lib.get_params(pcm, &mut buffer, &mut period) };
    let period = if period == 0 { 512 } else { period as usize }.clamp(64, 8192);
    let mut render = make(RATE);
    let ctl = Arc::new(Ctl {
        stop: AtomicBool::new(false),
        paused: AtomicBool::new(false),
    });
    let c = ctl.clone();
    let handle = PcmHandle(pcm);
    let thread = std::thread::Builder::new()
        .name("stipple-audio".into())
        .spawn(move || {
            let handle = handle;
            let pcm = handle.0;
            let mut buf = vec![0.0f32; period * 2];
            let mut dropped = false;
            let mut lost = false;
            while !c.stop.load(Ordering::Relaxed) {
                if lost {
                    // The device went away: keep the mix moving at about
                    // real-time pace, as the null output does, so sounds
                    // still end.
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    let n = (RATE as usize / 100).min(period) * 2;
                    super::render_guarded(&mut render, &mut buf[..n]);
                    continue;
                }
                if c.paused.load(Ordering::Relaxed) {
                    if !dropped {
                        unsafe { lib.drop(pcm) };
                        dropped = true;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    continue;
                }
                if dropped {
                    unsafe { lib.prepare(pcm) };
                    dropped = false;
                }
                super::render_guarded(&mut render, &mut buf);
                let mut done = 0usize;
                while done < period && !lost && !c.stop.load(Ordering::Relaxed) {
                    let n = unsafe {
                        lib.writei(
                            pcm,
                            buf[done * 2..].as_ptr().cast(),
                            (period - done) as c_ulong,
                        )
                    };
                    if n >= 0 {
                        done += n as usize;
                    } else if unsafe { lib.recover(pcm, n as c_int, 1) } < 0 {
                        lost = true;
                    }
                }
            }
            unsafe {
                lib.drop(pcm);
                lib.close(pcm);
            }
        })
        .ok()?;
    Some(Box::new(Alsa {
        ctl,
        thread: Some(thread),
    }))
}

impl Device for Alsa {
    fn name(&self) -> &'static str {
        "alsa"
    }

    fn set_paused(&mut self, paused: bool) {
        self.ctl.paused.store(paused, Ordering::Relaxed);
    }
}

impl Drop for Alsa {
    fn drop(&mut self) {
        self.ctl.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
