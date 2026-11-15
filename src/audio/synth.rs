//! Synthesized sounds: retro-style effects and notes without audio files.

use super::Sound;
use std::time::Duration;

/// The shape of a [`Tone`]'s wave.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Waveform {
    /// A pure tone.
    #[default]
    Sine,
    /// Hollow and buzzy, like an old console's pulse channel.
    Square,
    /// Softer than a square; a classic bass.
    Triangle,
    /// Bright and brassy.
    Sawtooth,
    /// Random values held for half a period each: a hiss at high
    /// frequencies, a rumble at low ones (explosions, footsteps).
    Noise,
}

/// A synthesized note or effect: a waveform, a pitch (sliding from
/// `frequency` to `end_frequency`), and an attack and release so it doesn't
/// click.
///
/// ```
/// use std::time::Duration;
/// use stipple::audio::{Tone, Waveform};
/// // A laser: a square wave sliding down two octaves.
/// let zap = Tone::new(Waveform::Square, 1760.0, Duration::from_millis(150))
///     .sweep_to(440.0)
///     .volume(0.3)
///     .render(48_000);
/// assert_eq!(zap.frames(), 7_200);
/// ```
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tone {
    pub waveform: Waveform,
    /// Starting pitch, in Hz.
    pub frequency: f32,
    /// Ending pitch, in Hz (the same as `frequency` for a steady note); the
    /// pitch slides between them evenly in octaves.
    pub end_frequency: f32,
    pub duration: Duration,
    /// Rise from silence over this long.
    pub attack: Duration,
    /// Fall to silence over this long, at the end.
    pub release: Duration,
    /// Peak amplitude, from 0 to 1.
    pub volume: f32,
}

impl Tone {
    /// A `frequency` Hz note of `waveform` lasting `duration`, at half
    /// volume, with a 5 ms attack and a 20 ms release.
    pub fn new(waveform: Waveform, frequency: f32, duration: Duration) -> Self {
        Self {
            waveform,
            frequency,
            end_frequency: frequency,
            duration,
            attack: Duration::from_millis(5),
            release: Duration::from_millis(20),
            volume: 0.5,
        }
    }

    /// Slide the pitch to `frequency` by the end.
    pub fn sweep_to(mut self, frequency: f32) -> Self {
        self.end_frequency = frequency;
        self
    }

    /// Set the attack.
    pub fn attack(mut self, attack: Duration) -> Self {
        self.attack = attack;
        self
    }

    /// Set the release.
    pub fn release(mut self, release: Duration) -> Self {
        self.release = release;
        self
    }

    /// Set the peak amplitude.
    pub fn volume(mut self, volume: f32) -> Self {
        self.volume = volume;
        self
    }

    /// Render the tone as a mono sound at `rate` frames per second.
    pub fn render(&self, rate: u32) -> Sound {
        let rate = rate.max(1);
        let sr = f64::from(rate);
        let frames = (self.duration.as_secs_f64() * sr).round() as usize;
        let attack = (self.attack.as_secs_f64() * sr).round() as usize;
        let release = (self.release.as_secs_f64() * sr).round() as usize;
        let f0 = f64::from(self.frequency.max(0.0));
        let f1 = f64::from(self.end_frequency.max(0.0));
        let mut phase = 0.0f64;
        let mut noise = 0x2545_F491u32;
        let mut held = 0.0f32;
        let mut samples = Vec::with_capacity(frames);
        for i in 0..frames {
            let t = if frames > 1 {
                i as f64 / (frames - 1) as f64
            } else {
                0.0
            };
            let freq = if f0 > 0.0 && f1 > 0.0 && f0 != f1 {
                f0 * (f1 / f0).powf(t)
            } else {
                f0 + (f1 - f0) * t
            };
            let dt = (freq / sr).min(0.5);
            let p = phase as f32;
            let d = dt as f32;
            let v = match self.waveform {
                Waveform::Sine => (std::f32::consts::TAU * p).sin(),
                Waveform::Square => {
                    let naive = if p < 0.5 { 1.0 } else { -1.0 };
                    naive + poly_blep(p, d) - poly_blep((p + 0.5).fract(), d)
                }
                Waveform::Triangle => 1.0 - 4.0 * (p - 0.5).abs(),
                Waveform::Sawtooth => 2.0 * p - 1.0 - poly_blep(p, d),
                Waveform::Noise => held,
            };
            phase += dt;
            let wrapped_half = (phase >= 0.5 && phase - dt < 0.5) || phase >= 1.0;
            if phase >= 1.0 {
                phase -= 1.0;
            }
            if self.waveform == Waveform::Noise && (wrapped_half || i == 0) {
                // xorshift32
                noise ^= noise << 13;
                noise ^= noise >> 17;
                noise ^= noise << 5;
                held = (noise as f32 / u32::MAX as f32) * 2.0 - 1.0;
            }
            let env_a = if attack > 0 && i < attack {
                i as f32 / attack as f32
            } else {
                1.0
            };
            let left = frames - i;
            let env_r = if release > 0 && left <= release {
                (left - 1) as f32 / release as f32
            } else {
                1.0
            };
            samples.push(v * self.volume * env_a.min(env_r));
        }
        Sound::from_samples(rate, 1, samples)
    }
}

/// PolyBLEP: smooths a jump at phase 0 so square and sawtooth waves don't
/// alias much.
fn poly_blep(t: f32, dt: f32) -> f32 {
    if dt <= 0.0 {
        0.0
    } else if t < dt {
        let x = t / dt;
        x + x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + x + x + 1.0
    } else {
        0.0
    }
}

impl Sound {
    /// A `frequency` Hz note of `waveform` lasting `duration`, at 48 kHz (see
    /// [`Tone`] for slides and envelopes).
    pub fn tone(waveform: Waveform, frequency: f32, duration: Duration) -> Sound {
        Tone::new(waveform, frequency, duration).render(48_000)
    }
}

/// The frequency of a MIDI note number (69 is A4, 440 Hz; 60 is middle C).
pub fn note_frequency(note: u8) -> f32 {
    440.0 * 2f32.powf((f32::from(note) - 69.0) / 12.0)
}
