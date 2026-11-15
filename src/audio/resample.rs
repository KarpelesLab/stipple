//! Sample-rate conversion for whole sounds: a windowed-sinc filter.
//!
//! Sounds are converted to the device's rate once, when loaded
//! ([`Sound::resampled`](super::Sound::resampled), [`Audio::load`](super::Audio::load)),
//! with a filter much better than the cubic interpolation the mixer uses for
//! speed changes and for sounds played at another rate.

use std::sync::OnceLock;

/// Zero crossings of the sinc on each side of the centre.
const ZEROS: usize = 24;
/// Table entries per zero crossing.
const RES: usize = 512;
/// Where the low-pass sits, as a fraction of the lower Nyquist frequency.
const CUTOFF: f64 = 0.95;

/// The windowed sinc (Blackman window) from 0 to `ZEROS`, `RES` entries per
/// unit, plus one past the end.
fn kernel() -> &'static [f32] {
    static TABLE: OnceLock<Vec<f32>> = OnceLock::new();
    TABLE.get_or_init(|| {
        (0..=ZEROS * RES + 1)
            .map(|i| {
                let x = i as f64 / RES as f64;
                if x >= ZEROS as f64 {
                    return 0.0;
                }
                let sinc = if i == 0 {
                    1.0
                } else {
                    let px = std::f64::consts::PI * x;
                    px.sin() / px
                };
                // Blackman, over [-ZEROS, ZEROS].
                let w = std::f64::consts::PI * (x / ZEROS as f64 + 1.0);
                let window = 0.42 - 0.5 * w.cos() + 0.08 * (2.0 * w).cos();
                (sinc * window) as f32
            })
            .collect()
    })
}

fn tap(table: &[f32], x: f64) -> f32 {
    let x = x.abs() * RES as f64;
    let i = x as usize;
    if i + 1 >= table.len() {
        return 0.0;
    }
    let f = (x - i as f64) as f32;
    table[i] + (table[i + 1] - table[i]) * f
}

/// Convert `input` (interleaved, `channels` per frame) from `from` Hz to
/// `to` Hz.
pub(crate) fn resample(input: &[f32], channels: usize, from: u32, to: u32) -> Vec<f32> {
    if from == to || from == 0 || to == 0 || channels == 0 {
        return input.to_vec();
    }
    let table = kernel();
    let frames_in = input.len() / channels;
    let ratio = f64::from(to) / f64::from(from);
    let frames_out = (frames_in as f64 * ratio).round() as usize;
    // Lowering the rate lowers the cutoff with it, which widens the filter.
    let scale = ratio.min(1.0) * CUTOFF;
    let radius = ZEROS as f64 / scale;
    let mut out = vec![0.0f32; frames_out * channels];
    let mut acc = vec![0.0f32; channels];
    for j in 0..frames_out {
        let t = j as f64 / ratio;
        let lo = (t - radius).ceil().max(0.0) as usize;
        let hi = ((t + radius).floor() as usize).min(frames_in.saturating_sub(1));
        acc.fill(0.0);
        let mut i = lo;
        while i <= hi {
            let w = tap(table, (t - i as f64) * scale);
            let frame = &input[i * channels..(i + 1) * channels];
            for (a, s) in acc.iter_mut().zip(frame) {
                *a += s * w;
            }
            i += 1;
        }
        for (o, a) in out[j * channels..(j + 1) * channels].iter_mut().zip(&acc) {
            *o = a * scale as f32;
        }
    }
    out
}
