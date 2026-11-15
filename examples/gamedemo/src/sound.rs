//! Sound, all synthesized at startup: a few effects and a looping night
//! tune.

use std::time::Duration;

use stipple::audio::{Audio, Group, PlayParams, Sound, Tone, Voice, Waveform, note_frequency};

#[derive(Debug)]
pub struct Sounds {
    pub audio: Audio,
    ember: Sound,
    hit: Sound,
    flare: Sound,
    fizzle: Sound,
    over: Sound,
    music: Sound,
    playing: Option<Voice>,
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

impl Sounds {
    pub fn new(audio: Audio) -> Sounds {
        let rate = audio.rate();
        let ember = Sound::concat(&[
            Tone::new(Waveform::Triangle, 1046.5, ms(50))
                .volume(0.35)
                .render(rate),
            Tone::new(Waveform::Triangle, 1568.0, ms(140))
                .volume(0.35)
                .release(ms(120))
                .render(rate),
        ]);
        let hit = Sound::mix(&[
            Tone::new(Waveform::Noise, 900.0, ms(320))
                .sweep_to(80.0)
                .volume(0.45)
                .release(ms(260))
                .render(rate),
            Tone::new(Waveform::Square, 220.0, ms(200))
                .sweep_to(70.0)
                .volume(0.2)
                .release(ms(150))
                .render(rate),
        ]);
        let flare = Sound::mix(&[
            Tone::new(Waveform::Sawtooth, 300.0, ms(420))
                .sweep_to(1400.0)
                .volume(0.18)
                .release(ms(300))
                .render(rate),
            Tone::new(Waveform::Noise, 3000.0, ms(380))
                .volume(0.18)
                .release(ms(340))
                .render(rate),
        ]);
        let fizzle = Tone::new(Waveform::Square, 180.0, ms(120))
            .sweep_to(120.0)
            .volume(0.15)
            .release(ms(80))
            .render(rate);
        let over = Sound::concat(
            &[67u8, 63, 60, 55]
                .iter()
                .map(|&n| {
                    Tone::new(Waveform::Triangle, note_frequency(n), ms(260))
                        .volume(0.35)
                        .release(ms(200))
                        .render(rate)
                })
                .collect::<Vec<_>>(),
        );
        Sounds {
            ember,
            hit,
            flare,
            fizzle,
            over,
            music: tune(rate),
            playing: None,
            audio,
        }
    }

    /// The volumes of the three groups, from the settings.
    pub fn set_volumes(&self, master: f32, music: f32, effects: f32) {
        self.audio.set_volume(Group::MASTER, master);
        self.audio.set_volume(Group::MUSIC, music);
        self.audio.set_volume(Group::EFFECTS, effects);
    }

    /// A pan from a position across the screen (0 to 320).
    fn pan(x: f32) -> f32 {
        ((x / 160.0) - 1.0).clamp(-1.0, 1.0) * 0.6
    }

    /// An ember caught: higher with each in a row.
    pub fn ember(&self, x: f32, combo: u32) {
        let speed = 1.0 + 0.125 * combo.saturating_sub(1) as f32;
        let params = PlayParams::default().pan(Self::pan(x)).speed(speed);
        self.audio.play(&self.ember, params);
    }

    pub fn hit(&self, x: f32) {
        self.audio
            .play(&self.hit, PlayParams::default().pan(Self::pan(x)));
    }

    pub fn flare(&self) {
        self.audio.play(&self.flare, PlayParams::default());
    }

    pub fn fizzle(&self) {
        self.audio.play(&self.fizzle, PlayParams::default());
    }

    pub fn over(&self) {
        self.audio.play(&self.over, PlayParams::default());
    }

    /// Start the music, once.
    pub fn music(&mut self) {
        if self.playing.as_ref().is_some_and(Voice::is_playing) {
            return;
        }
        let params = PlayParams::default()
            .group(Group::MUSIC)
            .looping(true)
            .volume(0.8)
            .fade_in(ms(1500));
        self.playing = Some(self.audio.play(&self.music, params));
    }
}

/// Sixteen bars at 84 bpm in A minor: a plucked arpeggio over a soft bass,
/// with a slow bell melody coming in on the second half.
fn tune(rate: u32) -> Sound {
    let eighth = Duration::from_secs_f64(60.0 / 84.0 / 2.0);
    // Chords (root, third, fifth) per bar: Am, F, C, G, twice; then Am, Dm, F, E.
    let chords: [[u8; 3]; 8] = [
        [57, 60, 64],
        [53, 57, 60],
        [55, 60, 64],
        [55, 59, 62],
        [57, 60, 64],
        [50, 53, 57],
        [53, 57, 60],
        [52, 56, 59],
    ];
    let note = |wave: Waveform, n: u8, len: Duration, volume: f32| {
        if n == 0 {
            Sound::silence(rate, len)
        } else {
            Tone::new(wave, note_frequency(n), len)
                .volume(volume)
                .attack(ms(4))
                .release(len.mul_f32(0.7))
                .render(rate)
        }
    };
    let mut arp = Vec::new();
    let mut bass = Vec::new();
    for pass in 0..2 {
        for (i, c) in chords.iter().enumerate() {
            // Up and down the chord, an octave up on the second pass.
            let lift = if pass == 1 && i % 2 == 1 { 12 } else { 0 };
            for k in [0, 1, 2, 1, 0, 1, 2, 1] {
                arp.push(note(Waveform::Triangle, c[k] + 12 + lift, eighth, 0.12));
            }
            bass.push(note(Waveform::Sine, c[0] - 12, eighth * 8, 0.3));
        }
    }
    // The bell: whole and half notes, the second half only.
    let bell_notes: [(u8, u32); 12] = [
        (0, 64),
        (76, 8),
        (74, 4),
        (72, 4),
        (69, 8),
        (72, 8),
        (71, 8),
        (69, 4),
        (64, 4),
        (69, 8),
        (68, 6),
        (0, 2),
    ];
    let bell: Vec<Sound> = bell_notes
        .iter()
        .map(|&(n, eighths)| note(Waveform::Sine, n, eighth * eighths, 0.1))
        .collect();
    Sound::mix(&[
        Sound::concat(&arp),
        Sound::concat(&bass),
        Sound::concat(&bell),
    ])
}
