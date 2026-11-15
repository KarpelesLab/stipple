//! No device: a thread that pulls audio at real-time pace and drops it, so
//! voices still advance and end as they would on a device. (On the web there
//! are no threads, so nothing is pulled there.)

use super::{Device, Render};

/// The rate the null output runs at.
pub(crate) const RATE: u32 = 48_000;

pub(crate) struct Null {
    #[cfg(not(target_arch = "wasm32"))]
    ctl: std::sync::Arc<Ctl>,
    #[cfg(not(target_arch = "wasm32"))]
    thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct Ctl {
    stop: std::sync::atomic::AtomicBool,
    paused: std::sync::atomic::AtomicBool,
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn open(make: &mut dyn FnMut(u32) -> Render) -> Box<dyn Device> {
    drop(make(RATE));
    Box::new(Null {})
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn open(make: &mut dyn FnMut(u32) -> Render) -> Box<dyn Device> {
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};
    let mut render = make(RATE);
    let ctl = std::sync::Arc::new(Ctl::default());
    let c = ctl.clone();
    let thread = std::thread::Builder::new()
        .name("stipple-audio-null".into())
        .spawn(move || {
            let mut buf = vec![0.0f32; 2 * RATE as usize / 10];
            let mut start = Instant::now();
            let mut done = 0u64;
            while !c.stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
                if c.paused.load(Ordering::Relaxed) {
                    start = Instant::now();
                    done = 0;
                    continue;
                }
                let due = (start.elapsed().as_secs_f64() * f64::from(RATE)) as u64;
                let mut todo = due.saturating_sub(done);
                // After a long stall, don't try to catch up more than 100 ms.
                if todo > buf.len() as u64 / 2 {
                    done = due - buf.len() as u64 / 2;
                    todo = buf.len() as u64 / 2;
                }
                let n = todo as usize * 2;
                super::render_guarded(&mut render, &mut buf[..n]);
                done += todo;
            }
        })
        .ok();
    Box::new(Null { ctl, thread })
}

impl Device for Null {
    fn name(&self) -> &'static str {
        "null"
    }

    fn set_paused(&mut self, _paused: bool) {
        #[cfg(not(target_arch = "wasm32"))]
        self.ctl
            .paused
            .store(_paused, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Drop for Null {
    fn drop(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.ctl
                .stop
                .store(true, std::sync::atomic::Ordering::Relaxed);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }
}
