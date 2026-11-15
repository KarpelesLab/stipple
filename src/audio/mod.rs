//! Sound for games and apps: sound effects, streamed music, volume groups,
//! on every platform through the OS's own audio API.
//!
//! ```no_run
//! use std::time::Duration;
//! use stipple::audio::{Audio, Group, PlayParams, Sound, Waveform};
//!
//! let audio = Audio::shared(); // opens the default output once per process
//! let coin = Sound::tone(Waveform::Square, 1320.0, Duration::from_millis(80));
//! audio.play(&coin, PlayParams::default().volume(0.4));
//!
//! // Music streams from its encoded bytes, decoded as it plays.
//! # let bytes: Vec<u8> = coin.to_wav();
//! let music = audio
//!     .play_music(bytes, stipple::audio::MusicParams::default().looping(true))
//!     .unwrap();
//! audio.set_volume(Group::MUSIC, 0.5);
//! music.fade_out(Duration::from_secs(2));
//! ```
//!
//! - [`Audio`] is the mixer and its output device. It's cheap to clone and
//!   can be used from any thread (a game calls it from its frame update).
//! - [`Sound`] is decoded audio in memory, shared by every voice playing it:
//!   decoded from a file's bytes ([`Sound::decode`], [`Audio::load`]) or
//!   synthesized ([`Tone`], [`Sound::tone`]).
//! - [`Audio::play`] starts a [`Voice`]: a handle to stop it, fade it, or
//!   change its volume, pan and speed while it plays.
//! - [`Audio::play_music`] streams: it decodes a little ahead of what plays,
//!   on a worker thread, and loops between loop points.
//! - Every voice plays in a [`Group`]: [`Group::MASTER`] over
//!   [`Group::MUSIC`], [`Group::EFFECTS`] and any made with [`Audio::group`],
//!   each with a volume and a pause.
//!
//! Formats: PCM and float WAV always; Ogg Vorbis, MP3, FLAC and Opus with
//! the `audio` feature, which brings oxideav's decoders. Output stops while a
//! phone app is in the background, and starts again when it comes back. See
//! `docs/audio.md`.

mod decode;
mod mixer;
mod output;
mod resample;
mod sound;
mod stream;
mod synth;
#[cfg(test)]
mod tests;

pub use mixer::{AudioStats, Group, MAX_GROUPS, PlayParams, Voice};
pub use sound::Sound;
pub use synth::{Tone, Waveform, note_frequency};

use mixer::{Renderer, Shared, Source};
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// Why a sound couldn't be decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioError {
    /// A format (or a WAV encoding) this build can't decode; most need the
    /// `audio` feature.
    Unsupported(String),
    /// The data is damaged or not what it claims to be.
    Decode(String),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioError::Unsupported(m) => write!(f, "unsupported audio: {m}"),
            AudioError::Decode(m) => write!(f, "audio decode error: {m}"),
        }
    }
}

impl std::error::Error for AudioError {}

/// How to play streamed music (see [`Audio::play_music`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MusicParams {
    /// Linear gain (see [`PlayParams::volume`]).
    pub volume: f32,
    /// Start over at the loop end (or the end), from the loop start.
    pub looping: bool,
    /// Where a loop starts again. The first time through plays from the top,
    /// so an intro can come before the loop.
    pub loop_start: Duration,
    /// Where a loop ends (and, when not looping, where playing stops);
    /// `None` for the end.
    pub loop_end: Option<Duration>,
    /// The group whose volume scales the music.
    pub group: Group,
    /// Rise from silence over this long.
    pub fade_in: Duration,
}

impl Default for MusicParams {
    fn default() -> Self {
        Self {
            volume: 1.0,
            looping: false,
            loop_start: Duration::ZERO,
            loop_end: None,
            group: Group::MUSIC,
            fade_in: Duration::ZERO,
        }
    }
}

impl MusicParams {
    /// Set [`volume`](Self::volume).
    pub fn volume(mut self, volume: f32) -> Self {
        self.volume = volume;
        self
    }
    /// Set [`looping`](Self::looping).
    pub fn looping(mut self, looping: bool) -> Self {
        self.looping = looping;
        self
    }
    /// Loop between `start` and `end` (`None` for the end of the track).
    pub fn loop_points(mut self, start: Duration, end: Option<Duration>) -> Self {
        self.looping = true;
        self.loop_start = start;
        self.loop_end = end;
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

/// The mixer and the output it plays through. Clones share both; the output
/// closes when the last clone is dropped.
///
/// Most apps use one, [`Audio::shared`]. Every method can be called from any
/// thread and returns at once: changes reach the audio thread within one of
/// its buffers (a few milliseconds).
#[derive(Clone)]
pub struct Audio {
    inner: Arc<Inner>,
}

struct Inner {
    shared: Arc<Shared>,
    backend: &'static str,
    /// The output; `None` for an offline mixer.
    _device: Option<Arc<Mutex<Box<dyn output::Device>>>>,
    /// An offline mixer's renderer, driven by [`Audio::render`].
    offline: Option<Mutex<Renderer>>,
}

impl fmt::Debug for Audio {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Audio")
            .field("backend", &self.inner.backend)
            .field("rate", &self.inner.shared.rate)
            .finish()
    }
}

impl Default for Audio {
    fn default() -> Self {
        Self::new()
    }
}

impl Audio {
    /// Open the default output device. Where there's none (no sound card, no
    /// audio library, or `STIPPLE_AUDIO=null` in the environment), a silent
    /// null output plays instead, so the game works the same without sound.
    pub fn new() -> Audio {
        Self::open(false)
    }

    /// An `Audio` on the null output, whatever devices there are.
    #[cfg(test)]
    pub(crate) fn with_null_output() -> Audio {
        Self::open(true)
    }

    fn open(null: bool) -> Audio {
        let mut shared = None;
        let device = output::open(
            &mut |rate| {
                let s = Shared::new(rate);
                shared = Some(s.clone());
                let mut renderer = Renderer::new(s);
                Box::new(move |buf: &mut [f32]| renderer.render(buf))
            },
            null,
        );
        let backend = output::lock(&device).name();
        let shared = shared.expect("the output asked for a renderer");
        Audio {
            inner: Arc::new(Inner {
                shared,
                backend,
                _device: Some(device),
                offline: None,
            }),
        }
    }

    /// The process's audio, opened on first use and kept open: the handle a
    /// game reaches for from anywhere.
    pub fn shared() -> Audio {
        static SHARED: OnceLock<Audio> = OnceLock::new();
        SHARED.get_or_init(Audio::new).clone()
    }

    /// A mixer with no output, at `rate` frames per second: nothing plays
    /// until [`render`](Self::render) pulls the mix (to write it to a file,
    /// or in tests).
    pub fn offline(rate: u32) -> Audio {
        let shared = Shared::new(rate.max(1));
        Audio {
            inner: Arc::new(Inner {
                shared: shared.clone(),
                backend: "offline",
                _device: None,
                offline: Some(Mutex::new(Renderer::new(shared))),
            }),
        }
    }

    /// Mix into `out` (interleaved stereo, so two samples per frame), for an
    /// [`offline`](Self::offline) mixer. Returns `false`, leaving `out`
    /// alone, for a mixer playing through a device.
    pub fn render(&self, out: &mut [f32]) -> bool {
        match &self.inner.offline {
            Some(r) => {
                output::lock(r).render(out);
                true
            }
            None => false,
        }
    }

    /// The output's sample rate, in frames per second (48 000 on most
    /// devices).
    pub fn rate(&self) -> u32 {
        self.inner.shared.rate
    }

    /// Which output this plays through: `"coreaudio"`, `"wasapi"`,
    /// `"alsa"`, `"aaudio"`, `"webaudio"`, `"null"` (no device) or
    /// `"offline"`.
    pub fn backend(&self) -> &'static str {
        self.inner.backend
    }

    /// Decode `bytes` (see [`Sound::decode`]) and convert the sound to the
    /// output's rate, so it plays without per-sample rate conversion.
    pub fn load(&self, bytes: &[u8]) -> Result<Sound, AudioError> {
        Ok(Sound::decode(bytes)?.resampled(self.rate()))
    }

    /// Play `sound`. Any number of sounds (and the same sound any number of
    /// times) can play at once.
    pub fn play(&self, sound: &Sound, params: PlayParams) -> Voice {
        let data = sound.data.clone();
        let rate = data.rate;
        let mono = data.channels == 1;
        self.inner.shared.play(
            Source::Buffer {
                sound: data,
                pos: 0,
                looping: params.looping,
            },
            rate,
            mono,
            &params,
        )
    }

    /// Stream music from its encoded bytes: it's decoded a little at a time
    /// as it plays (on a worker thread; on the web, as the browser asks for
    /// audio), so a long track doesn't sit decoded in memory. Errors if the
    /// format isn't one this build decodes.
    pub fn play_music(
        &self,
        bytes: impl Into<Arc<[u8]>>,
        params: MusicParams,
    ) -> Result<Voice, AudioError> {
        let stream = decode::open(bytes.into())?;
        let rate = stream.rate();
        let frames = |d: Duration| (d.as_secs_f64() * f64::from(rate)).round() as u64;
        let consumer = stream::start(
            stream,
            stream::LoopPoints {
                looping: params.looping,
                start: frames(params.loop_start),
                end: params.loop_end.map(frames),
            },
        );
        let p = PlayParams {
            volume: params.volume,
            pan: 0.0,
            speed: 1.0,
            looping: false,
            group: params.group,
            fade_in: params.fade_in,
        };
        Ok(self
            .inner
            .shared
            .play(Source::Stream(consumer), rate, false, &p))
    }

    /// The group named `name`, made the first time (`None` once there are
    /// [`MAX_GROUPS`]). `"master"`, `"music"` and `"effects"` name the
    /// built-in groups.
    pub fn group(&self, name: &str) -> Option<Group> {
        self.inner.shared.group(name)
    }

    /// Set a group's volume (linear: 1 is unchanged, 0 silent). Voices
    /// follow smoothly.
    pub fn set_volume(&self, group: Group, volume: f32) {
        self.inner.shared.set_volume(group, volume);
    }

    /// A group's volume.
    pub fn volume(&self, group: Group) -> f32 {
        self.inner.shared.volume(group)
    }

    /// Hold every voice in `group` where it is (pausing
    /// [`Group::MASTER`] holds everything, like [`pause_all`](Self::pause_all)).
    pub fn pause_group(&self, group: Group) {
        self.inner.shared.set_group_paused(group, true);
    }

    /// Carry on after [`pause_group`](Self::pause_group).
    pub fn resume_group(&self, group: Group) {
        self.inner.shared.set_group_paused(group, false);
    }

    /// Whether `group` is paused.
    pub fn is_group_paused(&self, group: Group) -> bool {
        self.inner.shared.group_paused(group)
    }

    /// Hold everything (a pause menu); [`resume_all`](Self::resume_all)
    /// carries on where it was.
    pub fn pause_all(&self) {
        self.inner.shared.set_paused(true);
    }

    /// Carry on after [`pause_all`](Self::pause_all).
    pub fn resume_all(&self) {
        self.inner.shared.set_paused(false);
    }

    /// Whether everything is paused.
    pub fn is_paused(&self) -> bool {
        self.inner.shared.paused()
    }

    /// Stop every voice, fading out over `fade` (zero stops at once).
    pub fn stop_all(&self, fade: Duration) {
        self.inner.shared.stop_all(fade);
    }

    /// Counters: frames pulled, callbacks, voices, music underruns.
    pub fn stats(&self) -> AudioStats {
        self.inner.shared.stats()
    }
}

/// The app went to the background or came back (from the lifecycle events):
/// stop or restart every output.
pub(crate) fn set_app_background(background: bool) {
    output::set_background(background);
}
