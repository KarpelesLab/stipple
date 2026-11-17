//! Streamed music: decoded a little ahead of the playback position into a
//! ring buffer.
//!
//! On threaded targets a worker thread decodes into the ring and the audio
//! callback reads from it; neither ever waits for the other. The ring is a
//! single-producer, single-consumer queue of `f32` bit patterns in atomics,
//! so it needs no lock and no `unsafe`. On the web, which has no threads, the
//! audio callback (already on the page's main thread) decodes into the ring
//! itself when it runs low.

use super::AudioError;
use super::decode::PcmStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// Frames of decoded music kept ahead of the playback position (about 0.7 s
/// at 48 kHz).
const RING_FRAMES: usize = 1 << 15;

/// Frames decoded before music starts, on the caller's thread.
const PREFILL_FRAMES: usize = 4096;

/// A lock-free single-producer, single-consumer ring of stereo frames.
#[derive(Debug)]
pub(crate) struct Ring {
    slots: Box<[AtomicU32]>,
    mask: usize,
    /// Samples written / read so far (they only grow).
    head: AtomicUsize,
    tail: AtomicUsize,
    /// The producer reached the end and won't write again.
    ended: AtomicBool,
    /// The consumer is gone: the producer should stop.
    closed: AtomicBool,
}

impl Ring {
    pub(crate) fn new(frames: usize) -> Arc<Self> {
        let samples = (frames * 2).next_power_of_two();
        Arc::new(Self {
            slots: (0..samples).map(|_| AtomicU32::new(0)).collect(),
            mask: samples - 1,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            ended: AtomicBool::new(false),
            closed: AtomicBool::new(false),
        })
    }

    /// Frames there's room for.
    pub(crate) fn free_frames(&self) -> usize {
        let used = self
            .head
            .load(Ordering::Relaxed)
            .wrapping_sub(self.tail.load(Ordering::Acquire));
        (self.slots.len() - used) / 2
    }

    /// Frames waiting to be read.
    pub(crate) fn len_frames(&self) -> usize {
        self.head
            .load(Ordering::Acquire)
            .wrapping_sub(self.tail.load(Ordering::Relaxed))
            / 2
    }

    /// Write as many whole frames of `samples` (interleaved stereo) as fit;
    /// returns how many samples went in. Producer only.
    pub(crate) fn push(&self, samples: &[f32]) -> usize {
        let n = (self.free_frames() * 2).min(samples.len() & !1);
        let head = self.head.load(Ordering::Relaxed);
        for (i, s) in samples[..n].iter().enumerate() {
            self.slots[head.wrapping_add(i) & self.mask].store(s.to_bits(), Ordering::Relaxed);
        }
        self.head.store(head.wrapping_add(n), Ordering::Release);
        n
    }

    /// Read one frame. Consumer only.
    pub(crate) fn pop(&self) -> Option<[f32; 2]> {
        let tail = self.tail.load(Ordering::Relaxed);
        if self.head.load(Ordering::Acquire).wrapping_sub(tail) < 2 {
            return None;
        }
        let l = f32::from_bits(self.slots[tail & self.mask].load(Ordering::Relaxed));
        let r =
            f32::from_bits(self.slots[tail.wrapping_add(1) & self.mask].load(Ordering::Relaxed));
        self.tail.store(tail.wrapping_add(2), Ordering::Release);
        Some([l, r])
    }

    pub(crate) fn set_ended(&self) {
        self.ended.store(true, Ordering::Release);
    }

    pub(crate) fn is_ended(&self) -> bool {
        self.ended.load(Ordering::Acquire)
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}

/// How streamed music loops.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LoopPoints {
    pub looping: bool,
    /// The frame the loop goes back to.
    pub start: u64,
    /// The frame the loop ends at (exclusive); `None` for the end of the
    /// stream.
    pub end: Option<u64>,
}

/// What [`Feeder::step`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// Wrote some frames; call again.
    Progress,
    /// The ring is full: wait for the consumer.
    Full,
    /// The stream ended (or failed): nothing more will come.
    Done,
}

/// Decodes a stream into a ring, looping as asked.
pub(crate) struct Feeder {
    stream: Box<dyn PcmStream>,
    ring: Arc<Ring>,
    loops: LoopPoints,
    /// Decoded stereo samples not yet in the ring.
    pending: Vec<f32>,
    offset: usize,
    /// The stream frame the next decoded chunk starts at.
    pos: u64,
    /// Frames produced since the last rewind, to stop a loop that makes
    /// nothing from spinning.
    since_rewind: u64,
    /// Looped back at least once (so passes start at the loop start).
    looped: bool,
}

impl Feeder {
    pub(crate) fn new(stream: Box<dyn PcmStream>, ring: Arc<Ring>, loops: LoopPoints) -> Self {
        Self {
            stream,
            ring,
            loops,
            pending: Vec::new(),
            offset: 0,
            pos: 0,
            since_rewind: 0,
            looped: false,
        }
    }

    /// Decode and write some frames.
    pub(crate) fn step(&mut self) -> Step {
        if self.ring.is_closed() {
            return Step::Done;
        }
        if self.offset >= self.pending.len() {
            match self.decode() {
                Ok(true) => {}
                Ok(false) => {
                    self.ring.set_ended();
                    return Step::Done;
                }
                Err(_) => {
                    self.ring.set_ended();
                    return Step::Done;
                }
            }
        }
        let n = self.ring.push(&self.pending[self.offset..]);
        self.offset += n;
        if n == 0 { Step::Full } else { Step::Progress }
    }

    /// Fill `pending` with the next chunk (as stereo, inside the loop
    /// points); `false` at the end.
    fn decode(&mut self) -> Result<bool, AudioError> {
        self.pending.clear();
        self.offset = 0;
        loop {
            let at_end = self.loops.end.is_some_and(|end| self.pos >= end);
            let chunk = if at_end {
                None
            } else {
                self.stream.next_chunk()?
            };
            let Some(chunk) = chunk else {
                // A pass that made nothing (an empty stream, or a loop start
                // past the end) would spin: stop instead.
                if !self.loops.looping || self.since_rewind == 0 {
                    return Ok(false);
                }
                self.stream.rewind()?;
                self.pos = 0;
                self.since_rewind = 0;
                self.looped = true;
                continue;
            };
            let ch = usize::from(self.stream.channels()).max(1);
            let frames = chunk.len() / ch;
            let first = self.pos;
            self.pos += frames as u64;
            // The first pass plays from the top, later ones from the loop
            // start; every pass stops at the loop end.
            let lo = if self.looped {
                self.loops.start.saturating_sub(first).min(frames as u64) as usize
            } else {
                0
            };
            let hi = match self.loops.end {
                Some(end) => end.saturating_sub(first).min(frames as u64) as usize,
                None => frames,
            };
            if hi <= lo {
                continue;
            }
            self.pending.reserve((hi - lo) * 2);
            for f in lo..hi {
                let l = chunk[f * ch];
                let r = if ch > 1 { chunk[f * ch + 1] } else { l };
                self.pending.push(l);
                self.pending.push(r);
            }
            self.since_rewind += (hi - lo) as u64;
            return Ok(true);
        }
    }
}

/// The audio thread's end of a music stream.
pub(crate) struct Consumer {
    ring: Arc<Ring>,
    /// On the web, the decoder runs here, on the audio callback.
    #[cfg(target_arch = "wasm32")]
    feeder: Option<Feeder>,
}

impl Consumer {
    /// The next frame; silence (counted in `underruns`) if the decoder is
    /// behind, `None` once the stream ended and everything was played.
    pub(crate) fn next(&mut self, underruns: &AtomicU64) -> Option<[f32; 2]> {
        #[cfg(target_arch = "wasm32")]
        if self.ring.len_frames() < RING_FRAMES / 2
            && let Some(feeder) = &mut self.feeder
        {
            // Decode until the ring is full again.
            loop {
                match feeder.step() {
                    Step::Progress => continue,
                    Step::Full => break,
                    Step::Done => {
                        self.feeder = None;
                        break;
                    }
                }
            }
        }
        // In the crate's tests, an offline render waits for the decoder
        // thread instead of racing it, however loaded the runner.
        #[cfg(all(test, not(target_arch = "wasm32")))]
        {
            let waited = std::time::Instant::now();
            while self.ring.len_frames() == 0
                && !self.ring.is_ended()
                && waited.elapsed() < std::time::Duration::from_secs(10)
            {
                std::thread::sleep(std::time::Duration::from_micros(200));
            }
        }
        match self.ring.pop() {
            Some(f) => Some(f),
            None if self.ring.is_ended() => None,
            None => {
                underruns.fetch_add(1, Ordering::Relaxed);
                Some([0.0; 2])
            }
        }
    }
}

impl Drop for Consumer {
    fn drop(&mut self) {
        self.ring.closed.store(true, Ordering::Release);
    }
}

/// Start decoding `stream` ahead: on a worker thread where there are
/// threads, else on the audio callback.
pub(crate) fn start(stream: Box<dyn PcmStream>, loops: LoopPoints) -> Consumer {
    let ring = Ring::new(RING_FRAMES);
    let mut feeder = Feeder::new(stream, ring.clone(), loops);
    // A head start, so the first callbacks have frames.
    let mut step = Step::Progress;
    while step == Step::Progress && ring.len_frames() < PREFILL_FRAMES {
        step = feeder.step();
    }
    #[cfg(target_arch = "wasm32")]
    {
        Consumer {
            ring,
            feeder: (step != Step::Done).then_some(feeder),
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if step == Step::Done {
            return Consumer { ring };
        }
        let spawned = std::thread::Builder::new()
            .name("stipple-music".into())
            .spawn(move || {
                loop {
                    match feeder.step() {
                        Step::Progress => {}
                        Step::Full => std::thread::sleep(std::time::Duration::from_millis(10)),
                        Step::Done => break,
                    }
                }
            });
        if spawned.is_err() {
            ring.set_ended();
        }
        Consumer { ring }
    }
}
