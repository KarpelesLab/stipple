//! Decoded sounds, kept in memory.

use super::{AudioError, decode, resample};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

/// The samples of a [`Sound`].
#[derive(Debug)]
pub(crate) struct SoundData {
    /// Interleaved, `channels` per frame.
    pub(crate) samples: Vec<f32>,
    /// 1 or 2.
    pub(crate) channels: u16,
    pub(crate) rate: u32,
}

impl SoundData {
    pub(crate) fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels)
    }

    /// Frame `i` as left and right.
    pub(crate) fn frame(&self, i: usize) -> [f32; 2] {
        if self.channels == 1 {
            let s = self.samples[i];
            [s, s]
        } else {
            [self.samples[2 * i], self.samples[2 * i + 1]]
        }
    }
}

/// A sound decoded into memory, ready to play any number of times at once
/// ([`Audio::play`](super::Audio::play)). Cloning is cheap: clones share the
/// samples.
///
/// Sounds are mono or stereo `f32` at some sample rate; the mixer converts the
/// rate as it plays, and [`Audio::load`](super::Audio::load) converts it up
/// front with a better filter.
#[derive(Clone)]
pub struct Sound {
    pub(crate) data: Arc<SoundData>,
}

impl fmt::Debug for Sound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sound")
            .field("channels", &self.data.channels)
            .field("rate", &self.data.rate)
            .field("frames", &self.data.frames())
            .finish()
    }
}

impl Sound {
    /// Decode an encoded sound: WAV (PCM or float) always; with the `audio`
    /// feature also Ogg Vorbis, MP3, FLAC and Opus. The
    /// format is recognized from the bytes.
    ///
    /// More than two channels keep the first two (front left and right).
    pub fn decode(bytes: &[u8]) -> Result<Sound, AudioError> {
        let (samples, channels, rate) = decode::decode_all(Arc::from(bytes))?;
        Ok(Self::from_samples(rate, channels, samples))
    }

    /// A sound from samples: interleaved when `channels` is 2, at `rate`
    /// frames per second.
    ///
    /// # Panics
    /// If `channels` isn't 1 or 2, or `rate` is 0.
    pub fn from_samples(rate: u32, channels: u16, mut samples: Vec<f32>) -> Sound {
        assert!(matches!(channels, 1 | 2), "a sound has 1 or 2 channels");
        assert!(rate > 0, "a sound's rate can't be 0");
        samples.truncate(samples.len() / usize::from(channels) * usize::from(channels));
        Sound {
            data: Arc::new(SoundData {
                samples,
                channels,
                rate,
            }),
        }
    }

    /// `duration` of silence, mono.
    pub fn silence(rate: u32, duration: Duration) -> Sound {
        let frames = (duration.as_secs_f64() * f64::from(rate)).round() as usize;
        Self::from_samples(rate, 1, vec![0.0; frames])
    }

    /// Frames per second.
    pub fn rate(&self) -> u32 {
        self.data.rate
    }

    /// 1 (mono) or 2 (stereo).
    pub fn channels(&self) -> u16 {
        self.data.channels
    }

    /// Length in frames (one sample per channel).
    pub fn frames(&self) -> usize {
        self.data.frames()
    }

    /// Length in time.
    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.frames() as f64 / f64::from(self.rate()))
    }

    /// The samples, interleaved when stereo.
    pub fn samples(&self) -> &[f32] {
        &self.data.samples
    }

    /// This sound at another sample rate (a windowed-sinc conversion).
    pub fn resampled(&self, rate: u32) -> Sound {
        if rate == self.rate() || rate == 0 {
            return self.clone();
        }
        let ch = usize::from(self.channels());
        Self::from_samples(
            rate,
            self.channels(),
            resample::resample(self.samples(), ch, self.rate(), rate),
        )
    }

    /// This sound as stereo (a mono sound's sample in both channels).
    pub fn to_stereo(&self) -> Sound {
        if self.channels() == 2 {
            return self.clone();
        }
        let samples = self.samples().iter().flat_map(|&s| [s, s]).collect();
        Self::from_samples(self.rate(), 2, samples)
    }

    /// The sounds one after the other, at the first one's rate (stereo if any
    /// is).
    pub fn concat(sounds: &[Sound]) -> Sound {
        Self::join(sounds, false)
    }

    /// The sounds played together (summed), as long as the longest, at the
    /// first one's rate (stereo if any is).
    pub fn mix(sounds: &[Sound]) -> Sound {
        Self::join(sounds, true)
    }

    fn join(sounds: &[Sound], overlay: bool) -> Sound {
        let Some(first) = sounds.first() else {
            return Self::from_samples(48_000, 1, Vec::new());
        };
        let rate = first.rate();
        let channels = if sounds.iter().any(|s| s.channels() == 2) {
            2
        } else {
            1
        };
        let parts: Vec<Sound> = sounds
            .iter()
            .map(|s| {
                let s = s.resampled(rate);
                if channels == 2 { s.to_stereo() } else { s }
            })
            .collect();
        let samples = if overlay {
            let len = parts.iter().map(|p| p.samples().len()).max().unwrap_or(0);
            let mut out = vec![0.0f32; len];
            for p in &parts {
                for (o, s) in out.iter_mut().zip(p.samples()) {
                    *o += s;
                }
            }
            out
        } else {
            parts
                .iter()
                .flat_map(|p| p.samples().iter().copied())
                .collect()
        };
        Self::from_samples(rate, channels, samples)
    }

    /// This sound with its volume scaled by `gain`.
    pub fn gain(&self, gain: f32) -> Sound {
        let samples = self.samples().iter().map(|s| s * gain).collect();
        Self::from_samples(self.rate(), self.channels(), samples)
    }

    /// The sound as a 16-bit PCM WAV file (to save it, or to play it back as
    /// streamed music with [`Audio::play_music`](super::Audio::play_music)).
    pub fn to_wav(&self) -> Vec<u8> {
        decode::encode_wav(self.samples(), self.channels(), self.rate())
    }
}
