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
            let mut pace = Pace::new(Instant::now());
            while !c.stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
                if c.paused.load(Ordering::Relaxed) {
                    pace = Pace::new(Instant::now());
                    continue;
                }
                let todo = pace.take(Instant::now(), buf.len() as u64 / 2);
                super::render_guarded(&mut render, &mut buf[..todo as usize * 2]);
            }
        })
        .ok();
    Box::new(Null { ctl, thread })
}

/// What the null output owes at real-time pace: the frames due since it
/// started (or resumed) less those it has pulled.
#[cfg(not(target_arch = "wasm32"))]
struct Pace {
    start: std::time::Instant,
    done: u64,
}

#[cfg(not(target_arch = "wasm32"))]
impl Pace {
    fn new(start: std::time::Instant) -> Self {
        Self { start, done: 0 }
    }

    /// The frames to pull at `now`, at most `cap` (after a long stall it
    /// doesn't try to catch up more than that), counted as pulled.
    fn take(&mut self, now: std::time::Instant, cap: u64) -> u64 {
        let elapsed = now.saturating_duration_since(self.start);
        let due = (elapsed.as_secs_f64() * f64::from(RATE)) as u64;
        let todo = due.saturating_sub(self.done).min(cap);
        // Past the cap, what was missed is dropped rather than owed.
        self.done = self.done.max(due);
        todo
    }
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

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn it_owes_what_real_time_pace_asks_and_drops_a_long_stall() {
        let t0 = Instant::now();
        let us = Duration::from_micros;
        let cap = u64::from(RATE) / 10; // 100 ms
        let mut pace = Pace::new(t0);
        assert_eq!(pace.take(t0, cap), 0);
        // (Times a binary fraction of a second, so the frames are exact.)
        assert_eq!(pace.take(t0 + us(62_500), cap), 3000);
        assert_eq!(pace.take(t0 + us(62_500), cap), 0, "pulled already");
        assert_eq!(pace.take(t0 + us(125_000), cap), 3000);
        // A second's stall: 100 ms now, and the rest isn't owed later.
        assert_eq!(pace.take(t0 + us(1_250_000), cap), cap);
        assert_eq!(pace.take(t0 + us(1_312_500), cap), 3000);
    }
}
