//! Thumbnails and previews, decoded off the UI thread and cached.
//!
//! The view asks with [`Loader::get`] for what it shows, every time it's
//! built; what isn't there yet is queued, and worker threads decode it and
//! tell the app through its `AppHandle`, which builds the view again. A job
//! still queued when a build no longer asks for it (its row scrolled away,
//! the selection moved on) is dropped instead of decoded: the cancellation.
//! On the web, which has no threads, the UI thread decodes one job per
//! timer tick instead.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use stipple::audio::Sound;
use stipple::prelude::*;
use stipple::widgets::{Block, BlockKind, Inline, Marks, RichDocument};

use super::vfs::{Kind, Vfs};

/// Longest side of a thumbnail, in pixels.
pub const THUMB: u32 = 96;
/// Longest side of a preview image: larger pictures are scaled down.
const PREVIEW_MAX: u32 = 1600;
/// The most text a preview reads.
const TEXT_MAX: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Job {
    Thumb,
    Preview,
}

/// A Markdown block, made `Send` for the trip back from a worker: its kind
/// and its runs of text with their marks.
pub type MdBlock = (BlockKind, Vec<(String, Marks)>);

/// What a job produced.
#[derive(Debug)]
pub enum Loaded {
    /// A picture: the (scaled) pixels and the file's own size.
    Image {
        pixels: Arc<Pixmap>,
        width: u32,
        height: u32,
    },
    Sound(Sound),
    Text {
        text: String,
        lines: usize,
        words: usize,
    },
    Markdown(Vec<MdBlock>),
    /// The file couldn't be read or decoded.
    Failed(String),
}

#[derive(Debug)]
enum Slot {
    /// Waiting; `wanted` is the last view build that asked.
    Pending {
        wanted: u64,
    },
    Ready(Arc<Loaded>),
}

/// Counters, for the status bar and the selftest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub decoded: u64,
    pub cancelled: u64,
    pub failed: u64,
    pub queued: usize,
}

type Key = (Job, String);

#[derive(Debug, Default)]
struct State {
    vfs: Option<Vfs>,
    /// Bumped by `reset`: results of older jobs are thrown away.
    generation: u64,
    /// Counts view builds.
    build: u64,
    slots: HashMap<Key, Slot>,
    /// Jobs, newest last: the newest (what was just scrolled to) goes first.
    queue: Vec<Key>,
    /// Jobs a worker has taken and not finished yet.
    running: usize,
    stats: Stats,
}

type Wake = Box<dyn Fn() + Send + Sync>;

struct Inner {
    state: Mutex<State>,
    ready: Condvar,
    /// Tells the app something finished (`AppHandle::update`).
    notify: OnceLock<Wake>,
    /// On the web: schedules a UI-thread tick of `pump`.
    schedule: OnceLock<Wake>,
}

/// The cache and its workers. Clones share them.
#[derive(Clone)]
pub struct Loader {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Loader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Loader")
            .field("stats", &self.stats())
            .finish()
    }
}

impl Loader {
    /// A loader reading from `vfs`, with `workers` threads (none on the web).
    pub fn new(vfs: Vfs, workers: usize) -> Self {
        let loader = Loader {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    vfs: Some(vfs),
                    ..State::default()
                }),
                ready: Condvar::new(),
                notify: OnceLock::new(),
                schedule: OnceLock::new(),
            }),
        };
        if !cfg!(target_arch = "wasm32") {
            for i in 0..workers {
                let l = loader.clone();
                std::thread::Builder::new()
                    .name(format!("mediabrowser-loader-{i}"))
                    .spawn(move || l.work())
                    .expect("loader thread");
            }
        }
        loader
    }

    /// Call `notify` (on any thread) whenever a job finished.
    pub fn on_ready(&self, notify: impl Fn() + Send + Sync + 'static) {
        let _ = self.inner.notify.set(Box::new(notify));
    }

    /// On the web: call `schedule` to have `pump` run soon on the UI thread.
    pub fn on_schedule(&self, schedule: impl Fn() + Send + Sync + 'static) {
        let _ = self.inner.schedule.set(Box::new(schedule));
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Browse another file system: forget everything cached and queued.
    pub fn reset(&self, vfs: Vfs) {
        let mut st = self.lock();
        st.vfs = Some(vfs);
        st.generation += 1;
        st.slots.clear();
        st.queue.clear();
    }

    /// Forget what's cached for `path` (it changed on disk).
    pub fn forget_all(&self) {
        let mut st = self.lock();
        st.generation += 1;
        st.slots.clear();
        st.queue.clear();
    }

    /// A view build starts: what it doesn't ask for again goes stale.
    pub fn begin_build(&self) {
        self.lock().build += 1;
    }

    /// What `job` made of `path`, if it's done; otherwise queue it (or keep
    /// it queued) and return `None`.
    pub fn get(&self, job: Job, path: &str) -> Option<Arc<Loaded>> {
        let mut st = self.lock();
        let build = st.build;
        let key = (job, path.to_string());
        match st.slots.get_mut(&key) {
            Some(Slot::Ready(done)) => return Some(done.clone()),
            Some(Slot::Pending { wanted }) => {
                *wanted = build;
                return None;
            }
            None => {}
        }
        st.slots
            .insert(key.clone(), Slot::Pending { wanted: build });
        st.queue.push(key);
        st.stats.queued = st.queue.len();
        drop(st);
        self.inner.ready.notify_one();
        if let Some(schedule) = self.inner.schedule.get() {
            schedule();
        }
        None
    }

    pub fn stats(&self) -> Stats {
        self.lock().stats
    }

    /// Whether nothing is queued or being loaded.
    pub fn idle(&self) -> bool {
        let st = self.lock();
        st.queue.is_empty() && st.running == 0
    }

    /// Take the next job still wanted, dropping (cancelling) the stale ones
    /// on the way.
    fn next_job(st: &mut State) -> Option<(Key, Vfs, u64)> {
        while let Some(key) = st.queue.pop() {
            st.stats.queued = st.queue.len();
            let fresh = match st.slots.get(&key) {
                // Asked for by this build or the one before.
                Some(Slot::Pending { wanted }) => wanted + 1 >= st.build,
                _ => false,
            };
            if fresh {
                let vfs = st.vfs.clone()?;
                st.running += 1;
                return Some((key, vfs, st.generation));
            }
            // Gone from the screen before its turn: drop it, so it's asked
            // for again if it comes back.
            if matches!(st.slots.get(&key), Some(Slot::Pending { .. })) {
                st.slots.remove(&key);
                st.stats.cancelled += 1;
            }
        }
        None
    }

    fn finish(&self, key: Key, generation: u64, done: Loaded) {
        let mut st = self.lock();
        st.running -= 1;
        if st.generation != generation {
            return;
        }
        if matches!(done, Loaded::Failed(_)) {
            st.stats.failed += 1;
        } else {
            st.stats.decoded += 1;
        }
        st.slots.insert(key, Slot::Ready(Arc::new(done)));
        drop(st);
        if let Some(notify) = self.inner.notify.get() {
            notify();
        }
    }

    /// A worker thread's loop.
    fn work(&self) {
        loop {
            let job = {
                let mut st = self.lock();
                loop {
                    if let Some(job) = Self::next_job(&mut st) {
                        break job;
                    }
                    st = self
                        .inner
                        .ready
                        .wait_timeout(st, Duration::from_secs(1))
                        .unwrap_or_else(|e| e.into_inner())
                        .0;
                }
            };
            let ((kind, path), vfs, generation) = job;
            let done = load(&vfs, kind, &path);
            self.finish((kind, path), generation, done);
        }
    }

    /// On the web: run one job on this (the UI) thread, and schedule the
    /// next if there is one.
    pub fn pump(&self) {
        let job = Self::next_job(&mut self.lock());
        if let Some(((kind, path), vfs, generation)) = job {
            let done = load(&vfs, kind, &path);
            self.finish((kind, path), generation, done);
        }
        if !self.idle()
            && let Some(schedule) = self.inner.schedule.get()
        {
            schedule();
        }
    }
}

/// Decode `path` for `job`.
pub fn load(vfs: &Vfs, job: Job, path: &str) -> Loaded {
    let bytes = match vfs.read(path) {
        Ok(b) => b,
        Err(e) => return Loaded::Failed(e),
    };
    match (job, Kind::of(path)) {
        (_, Kind::Image) => match decode_image(&bytes) {
            Ok(full) => {
                let size = full.size();
                let max = if job == Job::Thumb {
                    THUMB
                } else {
                    PREVIEW_MAX
                };
                Loaded::Image {
                    pixels: Arc::new(fit(&full, max)),
                    width: size.width,
                    height: size.height,
                }
            }
            Err(e) => Loaded::Failed(e),
        },
        (Job::Preview, Kind::Audio) => match Sound::decode(&bytes) {
            Ok(sound) => Loaded::Sound(sound),
            Err(e) => Loaded::Failed(e.to_string()),
        },
        (Job::Preview, Kind::Text) => {
            let text = String::from_utf8_lossy(&bytes[..bytes.len().min(TEXT_MAX)]).into_owned();
            Loaded::Text {
                lines: text.lines().count(),
                words: text.split_whitespace().count(),
                text,
            }
        }
        (Job::Preview, Kind::Markdown) => {
            let md = String::from_utf8_lossy(&bytes[..bytes.len().min(TEXT_MAX)]).into_owned();
            Loaded::Markdown(markdown_blocks(&RichDocument::from_markdown(&md)))
        }
        _ => Loaded::Failed("no preview".into()),
    }
}

fn decode_image(bytes: &[u8]) -> Result<Pixmap, String> {
    if bytes.starts_with(b"<svg") || bytes.starts_with(b"<?xml") {
        let svg = Svg::parse(bytes).map_err(|e| format!("{e:?}"))?;
        let size = svg.size();
        let (w, h) = (
            size.width.clamp(1.0, 1024.0),
            size.height.clamp(1.0, 1024.0),
        );
        return Ok(svg.rasterize(PhysicalSize::new(w as u32, h as u32), ImageFit::Contain));
    }
    Pixmap::decode(bytes).map_err(|e| format!("{e:?}"))
}

/// `src` scaled down (box filter) so its longer side is at most `max`.
pub fn fit(src: &Pixmap, max: u32) -> Pixmap {
    let size = src.size();
    let (w, h) = (size.width.max(1), size.height.max(1));
    if w <= max && h <= max {
        return src.clone();
    }
    let scale = f64::from(max) / f64::from(w.max(h));
    let (tw, th) = (
        ((f64::from(w) * scale).round() as u32).max(1),
        ((f64::from(h) * scale).round() as u32).max(1),
    );
    let mut data = Vec::with_capacity((tw * th * 4) as usize);
    for ty in 0..th {
        let (y0, y1) = (ty * h / th, ((ty + 1) * h / th).max(ty * h / th + 1));
        for tx in 0..tw {
            let (x0, x1) = (tx * w / tw, ((tx + 1) * w / tw).max(tx * w / tw + 1));
            let mut sum = [0u32; 4];
            for y in y0..y1.min(h) {
                for x in x0..x1.min(w) {
                    let p = src.pixel(x, y).unwrap_or([0; 4]);
                    for c in 0..4 {
                        sum[c] += u32::from(p[c]);
                    }
                }
            }
            let n = ((y1.min(h) - y0) * (x1.min(w) - x0)).max(1);
            data.extend(sum.map(|s| (s / n) as u8));
        }
    }
    Pixmap::from_rgba8(PhysicalSize::new(tw, th), data)
}

/// The blocks of `doc` as plain data.
fn markdown_blocks(doc: &RichDocument) -> Vec<MdBlock> {
    doc.blocks()
        .into_iter()
        .map(|b: Block| {
            let runs = b
                .inlines
                .into_iter()
                .filter_map(|i| match i {
                    Inline::Text { text, marks, .. } => Some((text, marks)),
                    _ => None,
                })
                .collect();
            (b.kind, runs)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_the_aspect_ratio() {
        let src = Pixmap::from_rgba8(PhysicalSize::new(400, 200), vec![200; 400 * 200 * 4]);
        let t = fit(&src, 96);
        assert_eq!(t.size(), PhysicalSize::new(96, 48));
        assert_eq!(t.pixel(10, 10), Some([200; 4]));
    }

    #[test]
    fn stale_jobs_are_cancelled_not_decoded() {
        let vfs = Vfs::Memory(Arc::new(super::super::vfs::MemFs::new(
            super::super::sample::files(),
        )));
        // No workers: jobs run when `pump` says.
        let loader = Loader::new(vfs, 0);
        loader.begin_build();
        assert!(
            loader
                .get(Job::Thumb, "Pictures/Tiles/tile-00.png")
                .is_none()
        );
        assert!(
            loader
                .get(Job::Thumb, "Pictures/Tiles/tile-01.png")
                .is_none()
        );
        // Two builds later only tile 1 is still on screen.
        loader.begin_build();
        loader.get(Job::Thumb, "Pictures/Tiles/tile-01.png");
        loader.begin_build();
        loader.get(Job::Thumb, "Pictures/Tiles/tile-01.png");
        loader.pump();
        loader.pump();
        let stats = loader.stats();
        assert_eq!((stats.decoded, stats.cancelled), (1, 1), "{stats:?}");
        match loader
            .get(Job::Thumb, "Pictures/Tiles/tile-01.png")
            .as_deref()
        {
            Some(Loaded::Image { pixels, width, .. }) => {
                assert_eq!(*width, 64);
                assert_eq!(pixels.size().width, 64);
            }
            other => panic!("{other:?}"),
        }
    }
}
