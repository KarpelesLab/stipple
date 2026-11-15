//! Windows: WASAPI in shared mode, event-driven, on a render thread of ours.
//!
//! COM by hand: each interface is a pointer to a vtable, called by slot
//! (`IUnknown`'s three methods first, then the interface's in declaration
//! order). Everything COM happens on the render thread, which joins the
//! multi-threaded apartment; the device's mix rate is sent back to `open`.
//! When the device goes away (unplugged, the default changes) the thread
//! opens the new default and carries on at the same rate; WASAPI converts.

use super::{Device, Render};
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

type Hresult = i32;
type Handle = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct Guid(u32, u16, u16, [u8; 8]);

const CLSID_MM_DEVICE_ENUMERATOR: Guid = Guid(
    0xBCDE_0395,
    0xE52F,
    0x467C,
    [0x8E, 0x3D, 0xC4, 0x57, 0x92, 0x91, 0x69, 0x2E],
);
const IID_IMM_DEVICE_ENUMERATOR: Guid = Guid(
    0xA956_64D2,
    0x9614,
    0x4F35,
    [0xA7, 0x46, 0xDE, 0x8D, 0xB6, 0x36, 0x17, 0xE6],
);
const IID_IAUDIO_CLIENT: Guid = Guid(
    0x1CB9_AD4C,
    0xDBFA,
    0x4C32,
    [0xB1, 0x78, 0xC2, 0xF5, 0x68, 0xA7, 0x03, 0xB2],
);
const IID_IAUDIO_RENDER_CLIENT: Guid = Guid(
    0xF294_ACFC,
    0x3146,
    0x4483,
    [0xA7, 0xBF, 0xAD, 0xDC, 0xA7, 0xC2, 0x60, 0xE2],
);
const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: Guid = Guid(
    0x0000_0003,
    0x0000,
    0x0010,
    [0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71],
);

const CLSCTX_ALL: u32 = 0x17;
const COINIT_MULTITHREADED: u32 = 0;
const E_RENDER: u32 = 0;
const E_CONSOLE: u32 = 0;
const AUDCLNT_SHAREMODE_SHARED: u32 = 0;
const AUDCLNT_STREAMFLAGS_EVENTCALLBACK: u32 = 0x0004_0000;
const AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM: u32 = 0x8000_0000;
const AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY: u32 = 0x0800_0000;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
const WAIT_OBJECT_0: u32 = 0;
/// 20 ms, in 100-nanosecond units.
const BUFFER_DURATION: i64 = 200_000;

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct WaveFormatEx {
    format_tag: u16,
    channels: u16,
    samples_per_sec: u32,
    avg_bytes_per_sec: u32,
    block_align: u16,
    bits_per_sample: u16,
    cb_size: u16,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct WaveFormatExtensible {
    format: WaveFormatEx,
    valid_bits_per_sample: u16,
    channel_mask: u32,
    sub_format: Guid,
}

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, coinit: u32) -> Hresult;
    fn CoUninitialize();
    fn CoCreateInstance(
        clsid: *const Guid,
        outer: *mut c_void,
        context: u32,
        iid: *const Guid,
        out: *mut *mut c_void,
    ) -> Hresult;
    fn CoTaskMemFree(p: *mut c_void);
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateEventW(attrs: *mut c_void, manual: i32, initial: i32, name: *const u16) -> Handle;
    fn WaitForSingleObject(handle: Handle, ms: u32) -> u32;
    fn CloseHandle(handle: Handle) -> i32;
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *const c_void;
}

/// An owned COM interface pointer, released on drop.
struct Com(*mut c_void);

impl Com {
    /// Method `slot` of the vtable.
    unsafe fn method(&self, slot: usize) -> *const c_void {
        unsafe {
            let vtbl = *(self.0 as *const *const *const c_void);
            *vtbl.add(slot)
        }
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let release: unsafe extern "system" fn(*mut c_void) -> u32 =
                    std::mem::transmute(self.method(2));
                release(self.0);
            }
        }
    }
}

fn ok(hr: Hresult) -> Result<(), Hresult> {
    if hr < 0 { Err(hr) } else { Ok(()) }
}

/// An initialized audio client on the default render device.
struct Client {
    // Field order is drop order: the render client before the audio client.
    render: Com,
    client: Com,
    event: Handle,
    buffer_frames: u32,
}

impl Drop for Client {
    fn drop(&mut self) {
        unsafe {
            let stop: unsafe extern "system" fn(*mut c_void) -> Hresult =
                std::mem::transmute(self.client.method(11));
            stop(self.client.0);
            CloseHandle(self.event);
        }
    }
}

/// The default render device's mix rate.
unsafe fn mix_rate() -> Result<u32, Hresult> {
    unsafe {
        let device = default_device()?;
        let client = activate(&device)?;
        let get_mix_format: unsafe extern "system" fn(
            *mut c_void,
            *mut *mut WaveFormatEx,
        ) -> Hresult = std::mem::transmute(client.method(8));
        let mut fmt: *mut WaveFormatEx = std::ptr::null_mut();
        ok(get_mix_format(client.0, &mut fmt))?;
        let rate = std::ptr::addr_of!((*fmt).samples_per_sec).read_unaligned();
        CoTaskMemFree(fmt.cast());
        Ok(rate)
    }
}

unsafe fn default_device() -> Result<Com, Hresult> {
    unsafe {
        let mut p = std::ptr::null_mut();
        ok(CoCreateInstance(
            &CLSID_MM_DEVICE_ENUMERATOR,
            std::ptr::null_mut(),
            CLSCTX_ALL,
            &IID_IMM_DEVICE_ENUMERATOR,
            &mut p,
        ))?;
        let enumerator = Com(p);
        // IMMDeviceEnumerator::GetDefaultAudioEndpoint
        let get_default: unsafe extern "system" fn(
            *mut c_void,
            u32,
            u32,
            *mut *mut c_void,
        ) -> Hresult = std::mem::transmute(enumerator.method(4));
        let mut d = std::ptr::null_mut();
        ok(get_default(enumerator.0, E_RENDER, E_CONSOLE, &mut d))?;
        Ok(Com(d))
    }
}

unsafe fn activate(device: &Com) -> Result<Com, Hresult> {
    unsafe {
        // IMMDevice::Activate
        let activate: unsafe extern "system" fn(
            *mut c_void,
            *const Guid,
            u32,
            *mut c_void,
            *mut *mut c_void,
        ) -> Hresult = std::mem::transmute(device.method(3));
        let mut c = std::ptr::null_mut();
        ok(activate(
            device.0,
            &IID_IAUDIO_CLIENT,
            CLSCTX_ALL,
            std::ptr::null_mut(),
            &mut c,
        ))?;
        Ok(Com(c))
    }
}

/// Open the default device as stereo float at `rate` (WASAPI converts to the
/// device's own format), event-driven, and start it.
unsafe fn open_client(rate: u32) -> Result<Client, Hresult> {
    unsafe {
        let device = default_device()?;
        let client = activate(&device)?;
        let format = WaveFormatExtensible {
            format: WaveFormatEx {
                format_tag: WAVE_FORMAT_EXTENSIBLE,
                channels: 2,
                samples_per_sec: rate,
                avg_bytes_per_sec: rate * 8,
                block_align: 8,
                bits_per_sample: 32,
                cb_size: 22,
            },
            valid_bits_per_sample: 32,
            channel_mask: 0x3, // front left, front right
            sub_format: KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
        };
        // IAudioClient::Initialize
        let initialize: unsafe extern "system" fn(
            *mut c_void,
            u32,
            u32,
            i64,
            i64,
            *const WaveFormatEx,
            *const Guid,
        ) -> Hresult = std::mem::transmute(client.method(3));
        ok(initialize(
            client.0,
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
            BUFFER_DURATION,
            0,
            (&raw const format).cast(),
            std::ptr::null(),
        ))?;
        let get_buffer_size: unsafe extern "system" fn(*mut c_void, *mut u32) -> Hresult =
            std::mem::transmute(client.method(4));
        let mut buffer_frames = 0;
        ok(get_buffer_size(client.0, &mut buffer_frames))?;
        let event = CreateEventW(std::ptr::null_mut(), 0, 0, std::ptr::null());
        if event.is_null() {
            return Err(-1);
        }
        let set_event: unsafe extern "system" fn(*mut c_void, Handle) -> Hresult =
            std::mem::transmute(client.method(13));
        if let Err(e) = ok(set_event(client.0, event)) {
            CloseHandle(event);
            return Err(e);
        }
        let get_service: unsafe extern "system" fn(
            *mut c_void,
            *const Guid,
            *mut *mut c_void,
        ) -> Hresult = std::mem::transmute(client.method(14));
        let mut r = std::ptr::null_mut();
        if let Err(e) = ok(get_service(client.0, &IID_IAUDIO_RENDER_CLIENT, &mut r)) {
            CloseHandle(event);
            return Err(e);
        }
        let c = Client {
            render: Com(r),
            client,
            event,
            buffer_frames,
        };
        let start: unsafe extern "system" fn(*mut c_void) -> Hresult =
            std::mem::transmute(c.client.method(10));
        ok(start(c.client.0))?;
        Ok(c)
    }
}

/// Fill what's free of the device's buffer. `Err` when the device is gone.
unsafe fn feed(c: &Client, render: &mut Render, silent: bool) -> Result<(), Hresult> {
    unsafe {
        let get_padding: unsafe extern "system" fn(*mut c_void, *mut u32) -> Hresult =
            std::mem::transmute(c.client.method(6));
        let mut padding = 0;
        ok(get_padding(c.client.0, &mut padding))?;
        let frames = c.buffer_frames.saturating_sub(padding);
        if frames == 0 {
            return Ok(());
        }
        // IAudioRenderClient::GetBuffer / ReleaseBuffer
        let get_buffer: unsafe extern "system" fn(*mut c_void, u32, *mut *mut u8) -> Hresult =
            std::mem::transmute(c.render.method(3));
        let release_buffer: unsafe extern "system" fn(*mut c_void, u32, u32) -> Hresult =
            std::mem::transmute(c.render.method(4));
        let mut data = std::ptr::null_mut();
        ok(get_buffer(c.render.0, frames, &mut data))?;
        if data.is_null() {
            return ok(release_buffer(c.render.0, frames, 2));
        }
        let out = std::slice::from_raw_parts_mut(data.cast::<f32>(), frames as usize * 2);
        if silent {
            out.fill(0.0);
        } else {
            super::render_guarded(render, out);
        }
        ok(release_buffer(c.render.0, frames, 0))
    }
}

/// Ask the scheduler to treat this thread as audio ("Pro Audio" in MMCSS),
/// where avrt.dll is there.
unsafe fn boost_thread() {
    unsafe {
        let name: Vec<u16> = "avrt.dll\0".encode_utf16().collect();
        let avrt = LoadLibraryW(name.as_ptr());
        if avrt.is_null() {
            return;
        }
        let f = GetProcAddress(avrt, c"AvSetMmThreadCharacteristicsW".as_ptr().cast());
        if f.is_null() {
            return;
        }
        let set: unsafe extern "system" fn(*const u16, *mut u32) -> Handle = std::mem::transmute(f);
        let task: Vec<u16> = "Pro Audio\0".encode_utf16().collect();
        let mut index = 0;
        set(task.as_ptr(), &mut index);
    }
}

struct Ctl {
    stop: AtomicBool,
    paused: AtomicBool,
}

struct Wasapi {
    ctl: Arc<Ctl>,
    thread: Option<std::thread::JoinHandle<()>>,
}

pub(super) fn open(make: &mut dyn FnMut(u32) -> Render) -> Option<Box<dyn Device>> {
    let (tx, rx) = mpsc::channel::<Option<u32>>();
    let (render_tx, render_rx) = mpsc::channel::<Render>();
    let ctl = Arc::new(Ctl {
        stop: AtomicBool::new(false),
        paused: AtomicBool::new(false),
    });
    let c = ctl.clone();
    let thread = std::thread::Builder::new()
        .name("stipple-audio".into())
        .spawn(move || unsafe {
            let co = CoInitializeEx(std::ptr::null_mut(), COINIT_MULTITHREADED);
            let rate = match mix_rate() {
                Ok(r) if r >= 8000 => r,
                _ => {
                    let _ = tx.send(None);
                    if co >= 0 {
                        CoUninitialize();
                    }
                    return;
                }
            };
            let mut client = open_client(rate).ok();
            let _ = tx.send(client.as_ref().map(|_| rate));
            if client.is_none() {
                if co >= 0 {
                    CoUninitialize();
                }
                return;
            }
            let Ok(mut render) = render_rx.recv() else {
                if co >= 0 {
                    CoUninitialize();
                }
                return;
            };
            boost_thread();
            while !c.stop.load(Ordering::Relaxed) {
                let Some(cl) = &client else {
                    // The device went away: try the (new) default.
                    std::thread::sleep(Duration::from_millis(500));
                    client = open_client(rate).ok();
                    continue;
                };
                if WaitForSingleObject(cl.event, 200) != WAIT_OBJECT_0 {
                    // No event: paused by the system, or gone; check below.
                }
                let paused = c.paused.load(Ordering::Relaxed);
                if feed(cl, &mut render, paused).is_err() {
                    client = None;
                }
            }
            drop(client);
            if co >= 0 {
                CoUninitialize();
            }
        })
        .ok()?;
    match rx.recv() {
        Ok(Some(rate)) => {
            let _ = render_tx.send(make(rate));
            Some(Box::new(Wasapi {
                ctl,
                thread: Some(thread),
            }))
        }
        _ => {
            let _ = thread.join();
            None
        }
    }
}

impl Device for Wasapi {
    fn name(&self) -> &'static str {
        "wasapi"
    }

    fn set_paused(&mut self, paused: bool) {
        // Paused, the thread keeps the stream fed with silence (cheaper and
        // simpler than stopping and restarting the client).
        self.ctl.paused.store(paused, Ordering::Relaxed);
    }
}

impl Drop for Wasapi {
    fn drop(&mut self) {
        self.ctl.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
