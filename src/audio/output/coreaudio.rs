//! macOS and iOS: an output `AudioUnit` (the default output on macOS,
//! RemoteIO on iOS) with a render callback, through AudioToolbox.
//!
//! On iOS the app's `AVAudioSession` is set to the ambient category first:
//! game audio mixes with other apps' (the user's music keeps playing) and
//! follows the silent switch.

use super::{Device, Render};
use std::ffi::c_void;

#[repr(C)]
struct AudioComponentDescription {
    component_type: u32,
    component_sub_type: u32,
    component_manufacturer: u32,
    component_flags: u32,
    component_flags_mask: u32,
}

#[repr(C)]
#[derive(Default)]
struct AudioStreamBasicDescription {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

#[repr(C)]
struct AudioBuffer {
    number_channels: u32,
    data_byte_size: u32,
    data: *mut c_void,
}

#[repr(C)]
struct AudioBufferList {
    number_buffers: u32,
    buffers: [AudioBuffer; 1],
}

type RenderProc = unsafe extern "C" fn(
    refcon: *mut c_void,
    flags: *mut u32,
    timestamp: *const c_void,
    bus: u32,
    frames: u32,
    data: *mut AudioBufferList,
) -> i32;

#[repr(C)]
struct AuRenderCallbackStruct {
    input_proc: RenderProc,
    input_proc_ref_con: *mut c_void,
}

type AudioUnit = *mut c_void;

#[link(name = "AudioToolbox", kind = "framework")]
unsafe extern "C" {
    fn AudioComponentFindNext(
        component: *mut c_void,
        desc: *const AudioComponentDescription,
    ) -> *mut c_void;
    fn AudioComponentInstanceNew(component: *mut c_void, out: *mut AudioUnit) -> i32;
    fn AudioComponentInstanceDispose(unit: AudioUnit) -> i32;
    fn AudioUnitInitialize(unit: AudioUnit) -> i32;
    fn AudioUnitUninitialize(unit: AudioUnit) -> i32;
    fn AudioUnitSetProperty(
        unit: AudioUnit,
        id: u32,
        scope: u32,
        element: u32,
        data: *const c_void,
        size: u32,
    ) -> i32;
    fn AudioUnitGetProperty(
        unit: AudioUnit,
        id: u32,
        scope: u32,
        element: u32,
        data: *mut c_void,
        size: *mut u32,
    ) -> i32;
    fn AudioOutputUnitStart(unit: AudioUnit) -> i32;
    fn AudioOutputUnitStop(unit: AudioUnit) -> i32;
}

const fn fourcc(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}

const K_AUDIO_UNIT_TYPE_OUTPUT: u32 = fourcc(b"auou");
#[cfg(target_os = "macos")]
const K_AUDIO_UNIT_SUB_TYPE: u32 = fourcc(b"def "); // DefaultOutput
#[cfg(target_os = "ios")]
const K_AUDIO_UNIT_SUB_TYPE: u32 = fourcc(b"rioc"); // RemoteIO
const K_AUDIO_UNIT_MANUFACTURER_APPLE: u32 = fourcc(b"appl");
const K_AUDIO_FORMAT_LINEAR_PCM: u32 = fourcc(b"lpcm");
const K_AUDIO_FORMAT_FLAG_IS_FLOAT: u32 = 1;
const K_AUDIO_FORMAT_FLAG_IS_PACKED: u32 = 8;
const K_AUDIO_UNIT_PROPERTY_STREAM_FORMAT: u32 = 8;
const K_AUDIO_UNIT_PROPERTY_SET_RENDER_CALLBACK: u32 = 23;
const K_AUDIO_UNIT_SCOPE_INPUT: u32 = 1;
const K_AUDIO_UNIT_SCOPE_OUTPUT: u32 = 2;

/// The render callback: fill the unit's buffer from the mixer.
unsafe extern "C" fn render_cb(
    refcon: *mut c_void,
    _flags: *mut u32,
    _timestamp: *const c_void,
    _bus: u32,
    frames: u32,
    data: *mut AudioBufferList,
) -> i32 {
    // SAFETY: `refcon` is the `Box<Render>` the unit was given, alive until
    // the unit is disposed; the unit calls this on one thread at a time.
    let render = unsafe { &mut *refcon.cast::<Render>() };
    let list = unsafe { &mut *data };
    if list.number_buffers == 0 {
        return 0;
    }
    let buf = &mut list.buffers[0];
    let n = (frames as usize * 2).min(buf.data_byte_size as usize / 4);
    if buf.data.is_null() || n == 0 {
        return 0;
    }
    // SAFETY: the unit hands a buffer of `data_byte_size` bytes of
    // interleaved f32 (the format set on its input).
    let out = unsafe { std::slice::from_raw_parts_mut(buf.data.cast::<f32>(), n) };
    super::render_guarded(render, out);
    0
}

struct CoreAudio {
    unit: AudioUnit,
    render: *mut Render,
    running: bool,
}

// SAFETY: the unit may be started, stopped and disposed from any thread.
unsafe impl Send for CoreAudio {}

pub(super) fn open(make: &mut dyn FnMut(u32) -> Render) -> Option<Box<dyn Device>> {
    #[cfg(target_os = "ios")]
    let session_rate = ios::activate_session();
    #[cfg(not(target_os = "ios"))]
    let session_rate: Option<f64> = None;
    unsafe {
        let desc = AudioComponentDescription {
            component_type: K_AUDIO_UNIT_TYPE_OUTPUT,
            component_sub_type: K_AUDIO_UNIT_SUB_TYPE,
            component_manufacturer: K_AUDIO_UNIT_MANUFACTURER_APPLE,
            component_flags: 0,
            component_flags_mask: 0,
        };
        let component = AudioComponentFindNext(std::ptr::null_mut(), &desc);
        if component.is_null() {
            return None;
        }
        let mut unit: AudioUnit = std::ptr::null_mut();
        if AudioComponentInstanceNew(component, &mut unit) != 0 || unit.is_null() {
            return None;
        }
        // The hardware's rate: the output side of the unit.
        let mut hw = AudioStreamBasicDescription::default();
        let mut size = std::mem::size_of::<AudioStreamBasicDescription>() as u32;
        let got = AudioUnitGetProperty(
            unit,
            K_AUDIO_UNIT_PROPERTY_STREAM_FORMAT,
            K_AUDIO_UNIT_SCOPE_OUTPUT,
            0,
            (&raw mut hw).cast(),
            &mut size,
        );
        let rate = session_rate
            .filter(|r| *r >= 8000.0)
            .or((got == 0 && hw.sample_rate >= 8000.0).then_some(hw.sample_rate))
            .unwrap_or(48_000.0)
            .round() as u32;
        let format = AudioStreamBasicDescription {
            sample_rate: f64::from(rate),
            format_id: K_AUDIO_FORMAT_LINEAR_PCM,
            format_flags: K_AUDIO_FORMAT_FLAG_IS_FLOAT | K_AUDIO_FORMAT_FLAG_IS_PACKED,
            bytes_per_packet: 8,
            frames_per_packet: 1,
            bytes_per_frame: 8,
            channels_per_frame: 2,
            bits_per_channel: 32,
            reserved: 0,
        };
        if AudioUnitSetProperty(
            unit,
            K_AUDIO_UNIT_PROPERTY_STREAM_FORMAT,
            K_AUDIO_UNIT_SCOPE_INPUT,
            0,
            (&raw const format).cast(),
            std::mem::size_of::<AudioStreamBasicDescription>() as u32,
        ) != 0
        {
            AudioComponentInstanceDispose(unit);
            return None;
        }
        let render: *mut Render = Box::into_raw(Box::new(make(rate)));
        let callback = AuRenderCallbackStruct {
            input_proc: render_cb,
            input_proc_ref_con: render.cast(),
        };
        let fail = |unit: AudioUnit, render: *mut Render| {
            AudioComponentInstanceDispose(unit);
            drop(Box::from_raw(render));
        };
        if AudioUnitSetProperty(
            unit,
            K_AUDIO_UNIT_PROPERTY_SET_RENDER_CALLBACK,
            K_AUDIO_UNIT_SCOPE_INPUT,
            0,
            (&raw const callback).cast(),
            std::mem::size_of::<AuRenderCallbackStruct>() as u32,
        ) != 0
            || AudioUnitInitialize(unit) != 0
        {
            fail(unit, render);
            return None;
        }
        if AudioOutputUnitStart(unit) != 0 {
            AudioUnitUninitialize(unit);
            fail(unit, render);
            return None;
        }
        Some(Box::new(CoreAudio {
            unit,
            render,
            running: true,
        }))
    }
}

impl Device for CoreAudio {
    fn name(&self) -> &'static str {
        "coreaudio"
    }

    fn set_paused(&mut self, paused: bool) {
        if paused == !self.running {
            return;
        }
        unsafe {
            if paused {
                AudioOutputUnitStop(self.unit);
                #[cfg(target_os = "ios")]
                ios::set_active(false);
            } else {
                // Back from the background (or an interruption the
                // background covered): the session must be active again.
                #[cfg(target_os = "ios")]
                ios::set_active(true);
                AudioOutputUnitStart(self.unit);
            }
        }
        self.running = !paused;
    }
}

impl Drop for CoreAudio {
    fn drop(&mut self) {
        unsafe {
            // Stopping waits for a callback in progress, so the render
            // function is free to go after it.
            AudioOutputUnitStop(self.unit);
            AudioUnitUninitialize(self.unit);
            AudioComponentInstanceDispose(self.unit);
            drop(Box::from_raw(self.render));
        }
    }
}

#[cfg(target_os = "ios")]
mod ios {
    //! The app's `AVAudioSession`.

    use crate::platform::native::objc::{Id, class, msg};

    #[link(name = "AVFoundation", kind = "framework")]
    unsafe extern "C" {
        static AVAudioSessionCategoryAmbient: Id;
    }

    fn session() -> Id {
        unsafe { msg!(class("AVAudioSession"), "sharedInstance", () -> Id) }
    }

    /// Set the ambient category and activate the session; the hardware's
    /// sample rate.
    pub(super) fn activate_session() -> Option<f64> {
        let s = session();
        if s.is_null() {
            return None;
        }
        unsafe {
            let mut err: Id = std::ptr::null_mut();
            let e = &raw mut err;
            let category = AVAudioSessionCategoryAmbient;
            let _: bool = msg!(s, "setCategory:error:", (category: Id, e: *mut Id) -> bool);
            set_active(true);
            let rate: f64 = msg!(s, "sampleRate", () -> f64);
            Some(rate)
        }
    }

    pub(super) fn set_active(active: bool) {
        let s = session();
        if s.is_null() {
            return;
        }
        let mut err: Id = std::ptr::null_mut();
        let e = &raw mut err;
        unsafe {
            let _: bool = msg!(s, "setActive:error:", (active: bool, e: *mut Id) -> bool);
        }
    }
}
