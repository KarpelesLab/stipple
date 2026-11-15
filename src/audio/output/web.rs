//! The web: Web Audio, driven by `web/stipple.js`.
//!
//! The module has no threads, so the page pulls: `audio_open` makes an
//! `AudioContext` with a `ScriptProcessorNode` whose callback (on the page's
//! main thread) calls the exported `stipple_audio_render` for each buffer
//! and copies the mix out of the module's memory. Browsers start a context
//! suspended until the user interacts with the page (the autoplay policy);
//! `stipple.js` resumes it on the first click, tap or key press.

use super::{Device, Render};
use std::cell::RefCell;

#[link(wasm_import_module = "stipple")]
unsafe extern "C" {
    /// Make the page's audio context: its sample rate, or 0 without Web
    /// Audio.
    fn audio_open() -> f64;
    /// Suspend (1) or resume (0) the context.
    fn audio_pause(paused: u32);
}

thread_local! {
    /// The mixer, and the buffer it fills for the page.
    static OUT: RefCell<Option<(Render, Vec<f32>)>> = const { RefCell::new(None) };
}

struct Web;

pub(super) fn open(make: &mut dyn FnMut(u32) -> Render) -> Option<Box<dyn Device>> {
    let rate = unsafe { audio_open() };
    if !(8000.0..=384_000.0).contains(&rate) {
        return None;
    }
    let render = make(rate.round() as u32);
    OUT.with_borrow_mut(|o| *o = Some((render, Vec::new())));
    Some(Box::new(Web))
}

/// Mix `frames` stereo frames for the page; a pointer to them (interleaved
/// `f32`) in the module's memory, valid until the next call, or null when no
/// audio is open.
#[doc(hidden)]
#[unsafe(no_mangle)]
pub extern "C" fn stipple_audio_render(frames: u32) -> *const f32 {
    OUT.with_borrow_mut(|o| match o {
        Some((render, buf)) => {
            buf.resize(frames as usize * 2, 0.0);
            super::render_guarded(render, buf);
            buf.as_ptr()
        }
        None => std::ptr::null(),
    })
}

impl Device for Web {
    fn name(&self) -> &'static str {
        "webaudio"
    }

    fn set_paused(&mut self, paused: bool) {
        unsafe { audio_pause(u32::from(paused)) }
    }
}

impl Drop for Web {
    fn drop(&mut self) {
        unsafe { audio_pause(1) };
        OUT.with_borrow_mut(|o| *o = None);
    }
}
