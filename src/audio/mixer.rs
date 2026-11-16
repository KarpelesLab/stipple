//! The mixer: voices summed into one stereo stream at the device's rate.
//!
//! The game's thread talks to the mixer through [`Shared`]: commands go into a
//! short queue (locked only to push or to swap it out), group volumes and the
//! pause flags are atomics. The real-time side, [`Renderer`], owns the voices;
//! the output callback calls [`Renderer::render`], which only `try_lock`s the
//! queue, so a game thread holding it never stalls the audio thread.

use super::sound::SoundData;
use super::stream::Consumer;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

/// How many volume groups there can be, the three built-in ones included.
pub const MAX_GROUPS: usize = 32;

/// Frames mixed per block. Gains (volume, pan, fades, group volumes) move in
/// a straight line across each block, so no change clicks.
const BLOCK: usize = 256;

/// A volume group: every voice plays in one, and each group has a volume (for
/// a settings screen's sliders) and can be paused on its own.
///
/// [`MASTER`](Self::MASTER) scales every group. [`MUSIC`](Self::MUSIC) and
/// [`EFFECTS`](Self::EFFECTS) are there from the start; make more with
/// [`Audio::group`](super::Audio::group).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Group(pub(crate) u8);

impl Group {
    /// The master volume, over every other group.
    pub const MASTER: Group = Group(0);
    /// Music ([`Audio::play_music`](super::Audio::play_music)'s default).
    pub const MUSIC: Group = Group(1);
    /// Sound effects ([`Audio::play`](super::Audio::play)'s default).
    pub const EFFECTS: Group = Group(2);

    /// The group's index, from 0 (master) up to [`MAX_GROUPS`].
    pub fn index(self) -> usize {
        usize::from(self.0)
    }
}

/// How to play a sound (see [`Audio::play`](super::Audio::play)).
///
/// ```
/// use stipple::audio::{Group, PlayParams};
/// let p = PlayParams::default().volume(0.5).pan(-1.0).speed(1.5);
/// assert_eq!(p.group, Group::EFFECTS);
/// ```
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayParams {
    /// Linear gain: 1 plays the sound as recorded, 0 is silent. Above 1
    /// amplifies (the output clips at full scale).
    pub volume: f32,
    /// Stereo position, from -1 (left) through 0 (centre) to 1 (right).
    pub pan: f32,
    /// Playback speed: 2 plays twice as fast and an octave higher, 0.5 half
    /// as fast and an octave lower.
    pub speed: f32,
    /// Start over at the end until stopped.
    pub looping: bool,
    /// The group whose volume scales this sound.
    pub group: Group,
    /// Rise from silence over this long instead of starting at full volume.
    pub fade_in: Duration,
}

impl Default for PlayParams {
    fn default() -> Self {
        Self {
            volume: 1.0,
            pan: 0.0,
            speed: 1.0,
            looping: false,
            group: Group::EFFECTS,
            fade_in: Duration::ZERO,
        }
    }
}

impl PlayParams {
    /// Set [`volume`](Self::volume).
    pub fn volume(mut self, volume: f32) -> Self {
        self.volume = volume;
        self
    }
    /// Set [`pan`](Self::pan).
    pub fn pan(mut self, pan: f32) -> Self {
        self.pan = pan;
        self
    }
    /// Set [`speed`](Self::speed).
    pub fn speed(mut self, speed: f32) -> Self {
        self.speed = speed;
        self
    }
    /// Set [`looping`](Self::looping).
    pub fn looping(mut self, looping: bool) -> Self {
        self.looping = looping;
        self
    }
    /// Set [`group`](Self::group).
    pub fn group(mut self, group: Group) -> Self {
        self.group = group;
        self
    }
    /// Set [`fade_in`](Self::fade_in).
    pub fn fade_in(mut self, fade_in: Duration) -> Self {
        self.fade_in = fade_in;
        self
    }
}

/// A sound that is playing (or was): stop it, change its volume, pan or
/// speed, or fade it, from any thread.
///
/// Dropping the handle leaves the sound playing; it ends on its own (or, when
/// looping, plays until [`stop`](Self::stop) or
/// [`Audio::stop_all`](super::Audio::stop_all)).
#[derive(Clone)]
pub struct Voice {
    id: u64,
    state: Arc<VoiceState>,
    shared: Weak<Shared>,
}

impl fmt::Debug for Voice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Voice")
            .field("id", &self.id)
            .field("playing", &self.is_playing())
            .finish()
    }
}

impl Voice {
    fn send(&self, cmd: Cmd) {
        if let Some(shared) = self.shared.upgrade() {
            shared.push(cmd);
        }
    }

    /// Stop now (over 5 ms, so it doesn't click).
    pub fn stop(&self) {
        self.fade_out(Duration::from_millis(5));
    }

    /// Fade to silence over `duration`, then stop.
    pub fn fade_out(&self, duration: Duration) {
        let frames = self.frames(duration);
        self.send(Cmd::Fade {
            id: self.id,
            to: 0.0,
            frames,
            stop: true,
        });
    }

    /// Move the volume to `volume` over `duration`, and keep playing.
    pub fn fade_to(&self, volume: f32, duration: Duration) {
        let frames = self.frames(duration);
        self.send(Cmd::Fade {
            id: self.id,
            to: sanitize_volume(volume),
            frames,
            stop: false,
        });
    }

    /// Set the volume (see [`PlayParams::volume`]).
    pub fn set_volume(&self, volume: f32) {
        self.fade_to(volume, Duration::ZERO);
    }

    /// Set the stereo position (see [`PlayParams::pan`]).
    pub fn set_pan(&self, pan: f32) {
        self.send(Cmd::Pan(self.id, sanitize_pan(pan)));
    }

    /// Set the playback speed (see [`PlayParams::speed`]).
    pub fn set_speed(&self, speed: f32) {
        self.send(Cmd::Speed(self.id, sanitize_speed(speed)));
    }

    /// Hold this voice where it is; [`resume`](Self::resume) carries on.
    pub fn pause(&self) {
        self.state.paused.store(true, Ordering::Release);
        self.send(Cmd::Pause(self.id, true));
    }

    /// Carry on after [`pause`](Self::pause).
    pub fn resume(&self) {
        self.state.paused.store(false, Ordering::Release);
        self.send(Cmd::Pause(self.id, false));
    }

    /// Whether the voice is still playing (or paused): `false` once it ended
    /// or was stopped.
    pub fn is_playing(&self) -> bool {
        self.state.playing.load(Ordering::Acquire)
    }

    /// Whether the voice is held by [`pause`](Self::pause) (as far as the
    /// handles know: set by `pause`, cleared by `resume`).
    pub fn is_paused(&self) -> bool {
        self.state.paused.load(Ordering::Acquire)
    }

    /// Where in its sound the voice is playing: the time into the sound (or,
    /// for streamed music, the time played since it started, loops
    /// included), as of the output's last buffer. A player's elapsed time
    /// and seek bar read it.
    pub fn position(&self) -> Duration {
        let frames = self.state.position.load(Ordering::Acquire);
        Duration::from_secs_f64(frames as f64 / f64::from(self.state.rate.max(1)))
    }

    /// Jump to `at` into the sound (clamped to its end, which ends the
    /// voice), keeping the volume, pan, speed and pause. Only sounds played
    /// from memory ([`Audio::play`](super::Audio::play)) can seek; streamed
    /// music ignores it.
    pub fn seek(&self, at: Duration) {
        let frame = (at.as_secs_f64() * f64::from(self.state.rate.max(1))).round() as u64;
        if self.state.seekable {
            self.state.position.store(frame, Ordering::Release);
        }
        self.send(Cmd::Seek(self.id, frame));
    }

    fn frames(&self, d: Duration) -> u32 {
        let rate = self.shared.upgrade().map_or(48_000, |s| s.rate);
        duration_frames(d, rate)
    }
}

fn duration_frames(d: Duration, rate: u32) -> u32 {
    (d.as_secs_f64() * f64::from(rate))
        .round()
        .min(f64::from(u32::MAX)) as u32
}

fn sanitize_speed(speed: f32) -> f32 {
    if speed.is_finite() {
        speed.clamp(1.0 / 64.0, 64.0)
    } else {
        1.0
    }
}

fn sanitize_volume(volume: f32) -> f32 {
    if volume.is_finite() {
        volume.max(0.0)
    } else {
        0.0
    }
}

fn sanitize_pan(pan: f32) -> f32 {
    if pan.is_finite() {
        pan.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

/// What a voice shares with its handles.
#[derive(Debug)]
struct VoiceState {
    playing: AtomicBool,
    paused: AtomicBool,
    /// Source frames into the sound, as of the last buffer.
    position: AtomicU64,
    /// The source's frame rate.
    rate: u32,
    /// Whether the source can seek (a sound in memory).
    seekable: bool,
}

/// A request from the game's side to the real-time side.
enum Cmd {
    Play(Box<VoiceRt>),
    Fade {
        id: u64,
        to: f32,
        frames: u32,
        stop: bool,
    },
    Pan(u64, f32),
    Speed(u64, f32),
    Pause(u64, bool),
    Seek(u64, u64),
    StopAll(u32),
}

/// Mixer counters, for diagnostics and tests (see
/// [`Audio::stats`](super::Audio::stats)).
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioStats {
    /// Frames (one sample per channel) the output pulled from the mixer.
    pub frames: u64,
    /// Times the output asked for audio.
    pub callbacks: u64,
    /// Voices playing (or paused) when the output last asked.
    pub voices: u32,
    /// Frames of streamed music that weren't decoded in time (played as
    /// silence).
    pub underruns: u64,
}

/// The game's side of the mixer.
pub(crate) struct Shared {
    pub(crate) rate: u32,
    cmds: Mutex<Vec<Cmd>>,
    group_volume: [AtomicU32; MAX_GROUPS],
    group_paused: [AtomicBool; MAX_GROUPS],
    group_names: Mutex<Vec<String>>,
    paused: AtomicBool,
    next_id: AtomicU64,
    frames: AtomicU64,
    callbacks: AtomicU64,
    voices: AtomicU32,
    pub(crate) underruns: AtomicU64,
    /// Voices that ended, handed back so their buffers are freed here and not
    /// on the audio thread.
    #[allow(clippy::vec_box)]
    trash: Mutex<Vec<Box<VoiceRt>>>,
}

impl fmt::Debug for Shared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Shared").field("rate", &self.rate).finish()
    }
}

impl Shared {
    pub(crate) fn new(rate: u32) -> Arc<Self> {
        Arc::new(Self {
            rate,
            cmds: Mutex::new(Vec::with_capacity(64)),
            group_volume: std::array::from_fn(|_| AtomicU32::new(1.0f32.to_bits())),
            group_paused: std::array::from_fn(|_| AtomicBool::new(false)),
            group_names: Mutex::new(vec!["master".into(), "music".into(), "effects".into()]),
            paused: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
            frames: AtomicU64::new(0),
            callbacks: AtomicU64::new(0),
            voices: AtomicU32::new(0),
            underruns: AtomicU64::new(0),
            trash: Mutex::new(Vec::with_capacity(64)),
        })
    }

    fn push(&self, cmd: Cmd) {
        lock(&self.cmds).push(cmd);
    }

    /// Free the voices the audio thread handed back.
    fn collect_trash(&self) {
        let dead = std::mem::take(&mut *lock(&self.trash));
        drop(dead);
    }

    /// Start a voice playing `source`, whose frames run at `source_rate`.
    pub(crate) fn play(
        self: &Arc<Self>,
        source: Source,
        source_rate: u32,
        mono: bool,
        p: &PlayParams,
    ) -> Voice {
        self.collect_trash();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let state = Arc::new(VoiceState {
            playing: AtomicBool::new(true),
            paused: AtomicBool::new(false),
            position: AtomicU64::new(0),
            rate: source_rate.max(1),
            seekable: matches!(source, Source::Buffer { .. }),
        });
        let fade_frames = duration_frames(p.fade_in, self.rate);
        let volume = sanitize_volume(p.volume);
        let v = VoiceRt {
            id,
            state: state.clone(),
            source,
            mono,
            group: p.group.index().min(MAX_GROUPS - 1),
            volume: if fade_frames > 0 { 0.0 } else { volume },
            pan: sanitize_pan(p.pan),
            speed: sanitize_speed(p.speed),
            base_step: f64::from(source_rate.max(1)) / f64::from(self.rate),
            fade: (fade_frames > 0).then_some(Fade {
                from: 0.0,
                to: volume,
                total: fade_frames,
                done: 0,
                stop: false,
            }),
            paused: false,
            hist: [[0.0; 2]; 4],
            frac: 0.0,
            tail: 0,
            primed: false,
            last_gain: None,
            advanced: 0,
            ended: false,
            done: false,
        };
        self.push(Cmd::Play(Box::new(v)));
        Voice {
            id,
            state,
            shared: Arc::downgrade(self),
        }
    }

    pub(crate) fn stop_all(&self, fade: Duration) {
        self.push(Cmd::StopAll(duration_frames(fade, self.rate)));
    }

    /// The group named `name`, made if there's none yet (`None` once all
    /// [`MAX_GROUPS`] are taken).
    pub(crate) fn group(&self, name: &str) -> Option<Group> {
        let mut names = lock(&self.group_names);
        if let Some(i) = names.iter().position(|n| n == name) {
            return Some(Group(i as u8));
        }
        if names.len() >= MAX_GROUPS {
            return None;
        }
        names.push(name.to_owned());
        Some(Group((names.len() - 1) as u8))
    }

    pub(crate) fn set_volume(&self, g: Group, volume: f32) {
        if let Some(slot) = self.group_volume.get(g.index()) {
            slot.store(sanitize_volume(volume).to_bits(), Ordering::Relaxed);
        }
    }

    pub(crate) fn volume(&self, g: Group) -> f32 {
        self.group_volume
            .get(g.index())
            .map_or(0.0, |v| f32::from_bits(v.load(Ordering::Relaxed)))
    }

    pub(crate) fn set_group_paused(&self, g: Group, paused: bool) {
        if g == Group::MASTER {
            self.set_paused(paused);
        } else if let Some(slot) = self.group_paused.get(g.index()) {
            slot.store(paused, Ordering::Relaxed);
        }
    }

    pub(crate) fn group_paused(&self, g: Group) -> bool {
        if g == Group::MASTER {
            return self.paused();
        }
        self.group_paused
            .get(g.index())
            .is_some_and(|p| p.load(Ordering::Relaxed))
    }

    pub(crate) fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    pub(crate) fn paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    pub(crate) fn stats(&self) -> AudioStats {
        self.collect_trash();
        AudioStats {
            frames: self.frames.load(Ordering::Relaxed),
            callbacks: self.callbacks.load(Ordering::Relaxed),
            voices: self.voices.load(Ordering::Relaxed),
            underruns: self.underruns.load(Ordering::Relaxed),
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Where a voice's frames come from.
pub(crate) enum Source {
    /// A decoded sound, from memory.
    Buffer {
        sound: Arc<SoundData>,
        pos: usize,
        looping: bool,
    },
    /// Music decoded as it plays.
    Stream(Consumer),
}

impl Source {
    /// The next frame (left, right; a mono sound repeats its sample), or
    /// `None` at the end.
    fn next(&mut self, shared: &Shared) -> Option<[f32; 2]> {
        match self {
            Source::Buffer {
                sound,
                pos,
                looping,
            } => {
                let frames = sound.frames();
                if *pos >= frames {
                    if *looping && frames > 0 {
                        *pos = 0;
                    } else {
                        return None;
                    }
                }
                let f = sound.frame(*pos);
                *pos += 1;
                Some(f)
            }
            Source::Stream(c) => c.next(&shared.underruns),
        }
    }
}

struct Fade {
    from: f32,
    to: f32,
    total: u32,
    done: u32,
    stop: bool,
}

/// A voice, on the real-time side.
pub(crate) struct VoiceRt {
    id: u64,
    state: Arc<VoiceState>,
    source: Source,
    mono: bool,
    group: usize,
    volume: f32,
    pan: f32,
    speed: f32,
    /// Source frames per output frame at speed 1.
    base_step: f64,
    fade: Option<Fade>,
    paused: bool,
    /// Four source frames around the read position, which sits between
    /// `hist[1]` and `hist[2]`, `frac` of the way.
    hist: [[f32; 2]; 4],
    frac: f64,
    /// Silent frames pushed into `hist` since the source ended.
    tail: u8,
    primed: bool,
    last_gain: Option<[f32; 2]>,
    /// Source frames the read position moved past (for a stream's
    /// position).
    advanced: u64,
    /// The source ran out.
    ended: bool,
    /// To be removed.
    done: bool,
}

impl VoiceRt {
    fn pull(&mut self, shared: &Shared) -> [f32; 2] {
        if self.tail > 0 {
            self.tail = self.tail.saturating_add(1);
            return [0.0; 2];
        }
        match self.source.next(shared) {
            Some(f) => f,
            None => {
                self.tail = 1;
                [0.0; 2]
            }
        }
    }

    /// The next output frame, interpolated (exactly the source's frames at
    /// matching rates and speed 1).
    fn next(&mut self, shared: &Shared) -> [f32; 2] {
        if !self.primed {
            // The first output frame is the source's first.
            self.primed = true;
            for i in 1..4 {
                self.hist[i] = self.pull(shared);
            }
        }
        let t = self.frac as f32;
        let h = &self.hist;
        let out = if t == 0.0 {
            h[1]
        } else {
            [
                hermite(h[0][0], h[1][0], h[2][0], h[3][0], t),
                hermite(h[0][1], h[1][1], h[2][1], h[3][1], t),
            ]
        };
        self.frac += self.base_step * f64::from(self.speed);
        while self.frac >= 1.0 {
            self.frac -= 1.0;
            self.advanced += 1;
            self.hist.rotate_left(1);
            self.hist[3] = self.pull(shared);
            // Three silent frames past the end: `hist[1]` is past the last
            // real frame.
            if self.tail >= 3 {
                self.ended = true;
                break;
            }
        }
        out
    }

    /// Tell the handles where the voice is: the frame at the read position.
    fn publish_position(&self) {
        let at = match &self.source {
            // `pos` is the next frame to pull; the read position sits three
            // frames behind it once primed (`hist[1]`, with `hist[2..4]` pulled).
            Source::Buffer { pos, .. } if self.primed => pos.saturating_sub(3) as u64,
            Source::Buffer { pos, .. } => *pos as u64,
            Source::Stream(_) => self.advanced,
        };
        self.state.position.store(at, Ordering::Release);
    }

    /// Jump to source frame `frame` (sounds in memory only).
    fn seek(&mut self, frame: u64) {
        if let Source::Buffer { sound, pos, .. } = &mut self.source {
            *pos = (frame as usize).min(sound.frames());
            self.primed = false;
            self.frac = 0.0;
            self.tail = 0;
            self.hist = [[0.0; 2]; 4];
            self.ended = false;
            self.publish_position();
        }
    }

    /// The gains for the left and right outputs, from volume and pan: a
    /// constant-power pan for mono sounds (−3 dB each side at the centre),
    /// a balance for stereo ones (both at full at the centre).
    fn pan_gains(&self, volume: f32) -> [f32; 2] {
        pan_gains(self.mono, self.pan, volume)
    }

    /// Advance the fade by `n` frames: the volume at the end of them, and
    /// whether the voice stops there.
    fn step_fade(&mut self, n: usize) -> (f32, bool) {
        let mut stop = false;
        if let Some(f) = &mut self.fade {
            f.done = f.done.saturating_add(n as u32).min(f.total);
            let t = if f.total == 0 {
                1.0
            } else {
                f.done as f32 / f.total as f32
            };
            self.volume = f.from + (f.to - f.from) * t;
            if f.done >= f.total {
                stop = f.stop;
                self.fade = None;
            }
        }
        (self.volume, stop)
    }
}

/// See [`VoiceRt::pan_gains`].
pub(crate) fn pan_gains(mono: bool, pan: f32, volume: f32) -> [f32; 2] {
    if mono {
        let theta = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
        [volume * theta.cos(), volume * theta.sin()]
    } else {
        [volume * (1.0 - pan).min(1.0), volume * (1.0 + pan).min(1.0)]
    }
}

/// Catmull-Rom interpolation between `x1` (t = 0) and `x2` (t = 1).
fn hermite(x0: f32, x1: f32, x2: f32, x3: f32, t: f32) -> f32 {
    let c1 = 0.5 * (x2 - x0);
    let c2 = x0 - 2.5 * x1 + 2.0 * x2 - 0.5 * x3;
    let c3 = 0.5 * (x3 - x0) + 1.5 * (x1 - x2);
    ((c3 * t + c2) * t + c1) * t + x1
}

/// The real-time side: owns the voices and fills the output's buffers.
pub(crate) struct Renderer {
    shared: Arc<Shared>,
    // Boxed so voices move in and out (from commands, to the trash) without
    // copying or allocating on the audio thread.
    #[allow(clippy::vec_box)]
    voices: Vec<Box<VoiceRt>>,
    pending: Vec<Cmd>,
    group_gain: [f32; MAX_GROUPS],
}

impl fmt::Debug for Renderer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Renderer")
            .field("voices", &self.voices.len())
            .finish()
    }
}

impl Renderer {
    pub(crate) fn new(shared: Arc<Shared>) -> Self {
        let group_gain = std::array::from_fn(|g| {
            let master = shared.volume(Group::MASTER);
            if g == 0 {
                master
            } else {
                master * shared.volume(Group(g as u8))
            }
        });
        Self {
            shared,
            voices: Vec::with_capacity(64),
            pending: Vec::with_capacity(64),
            group_gain,
        }
    }

    /// Fill `out` (interleaved stereo) with the mix.
    pub(crate) fn render(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        let shared = self.shared.clone();
        shared.callbacks.fetch_add(1, Ordering::Relaxed);
        shared
            .frames
            .fetch_add((out.len() / 2) as u64, Ordering::Relaxed);
        if let Ok(mut q) = shared.cmds.try_lock() {
            std::mem::swap(&mut *q, &mut self.pending);
        }
        let mut cmds = std::mem::take(&mut self.pending);
        for cmd in cmds.drain(..) {
            self.apply(cmd);
        }
        self.pending = cmds;
        if !shared.paused() {
            for block in out.chunks_mut(BLOCK * 2) {
                self.mix(block);
            }
            for s in out.iter_mut() {
                *s = s.clamp(-1.0, 1.0);
            }
        }
        // Hand finished voices back to be freed off this thread.
        let mut i = 0;
        while i < self.voices.len() {
            if self.voices[i].done {
                let v = self.voices.swap_remove(i);
                v.state.playing.store(false, Ordering::Release);
                if let Ok(mut t) = shared.trash.try_lock() {
                    t.push(v);
                }
            } else {
                i += 1;
            }
        }
        shared
            .voices
            .store(self.voices.len() as u32, Ordering::Relaxed);
    }

    fn voice(&mut self, id: u64) -> Option<&mut VoiceRt> {
        self.voices
            .iter_mut()
            .find(|v| v.id == id)
            .map(|v| &mut **v)
    }

    fn apply(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Play(v) => self.voices.push(v),
            Cmd::Fade {
                id,
                to,
                frames,
                stop,
            } => {
                if let Some(v) = self.voice(id) {
                    // Stopping a paused voice ends it at once.
                    if stop && v.paused {
                        v.done = true;
                    }
                    v.fade = Some(Fade {
                        from: v.volume,
                        to,
                        total: frames,
                        done: 0,
                        stop,
                    });
                }
            }
            Cmd::Pan(id, pan) => {
                if let Some(v) = self.voice(id) {
                    v.pan = pan;
                }
            }
            Cmd::Speed(id, speed) => {
                if let Some(v) = self.voice(id) {
                    v.speed = speed;
                }
            }
            Cmd::Pause(id, paused) => {
                if let Some(v) = self.voice(id) {
                    v.paused = paused;
                }
            }
            Cmd::Seek(id, frame) => {
                if let Some(v) = self.voice(id) {
                    v.seek(frame);
                }
            }
            Cmd::StopAll(frames) => {
                for v in &mut self.voices {
                    if v.paused || frames == 0 {
                        v.done = true;
                    }
                    v.fade = Some(Fade {
                        from: v.volume,
                        to: 0.0,
                        total: frames,
                        done: 0,
                        stop: true,
                    });
                }
            }
        }
    }

    fn mix(&mut self, block: &mut [f32]) {
        let n = block.len() / 2;
        let shared = &*self.shared;
        let master = shared.volume(Group::MASTER);
        let mut target = [0.0f32; MAX_GROUPS];
        for (g, t) in target.iter_mut().enumerate() {
            *t = if g == 0 {
                master
            } else {
                master * f32::from_bits(shared.group_volume[g].load(Ordering::Relaxed))
            };
        }
        let start_group = self.group_gain;
        self.group_gain = target;
        let inv = 1.0 / n as f32;
        for v in &mut self.voices {
            if v.done || v.paused || shared.group_paused[v.group].load(Ordering::Relaxed) {
                continue;
            }
            let vol0 = v.volume;
            let (vol, stop) = v.step_fade(n);
            let end = v.pan_gains(vol).map(|x| x * target[v.group]);
            let start = v
                .last_gain
                .unwrap_or_else(|| v.pan_gains(vol0).map(|x| x * start_group[v.group]));
            v.last_gain = Some(end);
            for i in 0..n {
                if v.ended {
                    break;
                }
                let t = (i + 1) as f32 * inv;
                let gl = start[0] + (end[0] - start[0]) * t;
                let gr = start[1] + (end[1] - start[1]) * t;
                let f = v.next(shared);
                block[2 * i] += f[0] * gl;
                block[2 * i + 1] += f[1] * gr;
            }
            v.done = v.ended || stop;
            v.publish_position();
        }
    }
}
