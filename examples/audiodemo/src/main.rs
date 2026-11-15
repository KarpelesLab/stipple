//! `stipple::audio` without any audio files: sound effects synthesized from
//! waveforms, a short tune generated note by note and streamed as looping
//! music, and a volume slider per group (master, music, effects).
//!
//! `--selftest` opens the output, plays an effect and the music for half a
//! second, checks the output pulled about as many frames as that takes, and
//! prints `audiodemo selftest: ok` (CI runs it; with no sound device the null
//! output stands in). `--expect-backend alsa` (or `coreaudio`, `null`, ...)
//! also fails unless that output opened.

use std::time::{Duration, Instant};
use stipple::audio::{
    Audio, Group, MusicParams, PlayParams, Sound, Tone, Voice, Waveform, note_frequency,
};
use stipple::prelude::*;

/// The effects, made once.
struct Effects {
    coin: Sound,
    jump: Sound,
    laser: Sound,
    explosion: Sound,
}

impl Effects {
    fn new(rate: u32) -> Self {
        let ms = Duration::from_millis;
        let coin = Sound::concat(&[
            Tone::new(Waveform::Square, 988.0, ms(70))
                .volume(0.25)
                .render(rate),
            Tone::new(Waveform::Square, 1319.0, ms(220))
                .volume(0.25)
                .release(ms(180))
                .render(rate),
        ]);
        let jump = Tone::new(Waveform::Square, 220.0, ms(220))
            .sweep_to(880.0)
            .volume(0.25)
            .release(ms(60))
            .render(rate);
        let laser = Tone::new(Waveform::Sawtooth, 1800.0, ms(180))
            .sweep_to(300.0)
            .volume(0.25)
            .render(rate);
        let explosion = Tone::new(Waveform::Noise, 1200.0, ms(700))
            .sweep_to(60.0)
            .volume(0.5)
            .release(ms(600))
            .render(rate);
        Self {
            coin,
            jump,
            laser,
            explosion,
        }
    }
}

/// Eight bars of a little tune at 140 bpm: a square-wave lead over a triangle
/// bass, as a WAV file in memory, to stream as music. The loop starts at bar
/// 3, after a two-bar intro.
fn tune(rate: u32) -> (Vec<u8>, Duration) {
    let beat = Duration::from_secs_f64(60.0 / 140.0 / 2.0); // eighth notes
    // MIDI notes, 0 for a rest; eight eighths per bar.
    let lead: [u8; 64] = [
        // intro
        72, 0, 76, 0, 79, 0, 76, 0, 72, 0, 76, 0, 79, 0, 84, 0, //
        // loop
        76, 79, 84, 79, 76, 72, 74, 76, 77, 81, 84, 81, 77, 74, 72, 71, //
        72, 76, 79, 76, 72, 67, 69, 71, 72, 0, 79, 0, 72, 0, 0, 0, //
        74, 77, 81, 77, 74, 71, 72, 74, 76, 79, 84, 79, 76, 0, 72, 0,
    ];
    let bass: [u8; 16] = [
        48, 48, 53, 55, 48, 45, 53, 55, 48, 45, 41, 43, 50, 43, 48, 48,
    ];
    let note = |wave: Waveform, n: u8, len: Duration, volume: f32| {
        if n == 0 {
            Sound::silence(rate, len)
        } else {
            Tone::new(wave, note_frequency(n), len)
                .volume(volume)
                .release(len / 3)
                .render(rate)
        }
    };
    let lead = Sound::concat(
        &lead
            .iter()
            .map(|&n| note(Waveform::Square, n, beat, 0.12))
            .collect::<Vec<_>>(),
    );
    let bass = Sound::concat(
        &bass
            .iter()
            .map(|&n| note(Waveform::Triangle, n, beat * 4, 0.35))
            .collect::<Vec<_>>(),
    );
    let music = Sound::mix(&[lead, bass]);
    (music.to_wav(), beat * 16)
}

struct Demo {
    audio: Audio,
    fx: Effects,
    tune: (Vec<u8>, Duration),
    music: Option<Voice>,
    master: f64,
    music_volume: f64,
    effects: f64,
    pan: f64,
    paused: bool,
}

impl Demo {
    fn new(audio: Audio) -> Self {
        let rate = audio.rate();
        Demo {
            fx: Effects::new(rate),
            tune: tune(rate),
            audio,
            music: None,
            master: 0.8,
            music_volume: 0.6,
            effects: 1.0,
            pan: 0.5,
            paused: false,
        }
    }

    fn play(&self, sound: &Sound) {
        let pan = (self.pan * 2.0 - 1.0) as f32;
        self.audio.play(sound, PlayParams::default().pan(pan));
    }

    fn toggle_music(&mut self) {
        match self.music.take() {
            Some(v) if v.is_playing() => v.fade_out(Duration::from_millis(600)),
            _ => {
                let (bytes, loop_start) = &self.tune;
                let params = MusicParams::default()
                    .loop_points(*loop_start, None)
                    .fade_in(Duration::from_millis(200));
                self.music = self.audio.play_music(bytes.clone(), params).ok();
            }
        }
    }

    fn apply_volumes(&self) {
        self.audio.set_volume(Group::MASTER, self.master as f32);
        self.audio
            .set_volume(Group::MUSIC, self.music_volume as f32);
        self.audio.set_volume(Group::EFFECTS, self.effects as f32);
    }
}

fn slider_row(
    cx: &mut Cx<Demo>,
    theme: &Theme,
    name: &str,
    value: f64,
    set: impl Fn(&mut Demo, f64) + 'static,
) -> Element {
    row(vec![
        label(cx, name).width(90.0),
        slider(cx, value, move |s, v| {
            set(s, v);
            s.apply_volumes();
        }),
        label(cx, format!("{:.0}%", value * 100.0)),
    ])
    .gap(theme.spacing.md)
}

fn view(state: &Demo, cx: &mut Cx<Demo>) -> Element {
    let theme = *cx.theme();
    let effect = |cx: &mut Cx<Demo>, name: &str, pick: fn(&Effects) -> &Sound| {
        button(cx, name).on_tap(cx, move |s| s.play(pick(&s.fx)))
    };
    let music_on = state.music.as_ref().is_some_and(Voice::is_playing);
    let stats = state.audio.stats();
    panel(cx).children(vec![
        heading(cx, "Audio"),
        label(
            cx,
            format!(
                "Output: {} at {} Hz · {} voices",
                state.audio.backend(),
                state.audio.rate(),
                stats.voices
            ),
        ),
        row(vec![
            effect(cx, "Coin", |f| &f.coin),
            effect(cx, "Jump", |f| &f.jump),
            effect(cx, "Laser", |f| &f.laser),
            effect(cx, "Explosion", |f| &f.explosion),
        ])
        .gap(theme.spacing.sm),
        slider_row(cx, &theme, "Pan", state.pan, |s, v| s.pan = v),
        row(vec![
            button(cx, if music_on { "Stop music" } else { "Play music" })
                .on_tap(cx, |s| s.toggle_music()),
            button(
                cx,
                if state.paused {
                    "Resume all"
                } else {
                    "Pause all"
                },
            )
            .on_tap(cx, |s| {
                s.paused = !s.paused;
                if s.paused {
                    s.audio.pause_all();
                } else {
                    s.audio.resume_all();
                }
            }),
        ])
        .gap(theme.spacing.sm),
        slider_row(cx, &theme, "Master", state.master, |s, v| s.master = v),
        slider_row(cx, &theme, "Music", state.music_volume, |s, v| {
            s.music_volume = v
        }),
        slider_row(cx, &theme, "Effects", state.effects, |s, v| s.effects = v),
    ])
}

/// Play for half a second and check the output kept pace.
fn selftest(expect_backend: Option<&str>) -> Result<String, String> {
    let audio = Audio::new();
    if let Some(want) = expect_backend
        && audio.backend() != want
    {
        return Err(format!("the output is {}, not {want}", audio.backend()));
    }
    let mut demo = Demo::new(audio.clone());
    demo.apply_volumes();
    let before = audio.stats();
    let start = Instant::now();
    demo.play(&demo.fx.coin);
    demo.toggle_music();
    std::thread::sleep(Duration::from_millis(500));
    let elapsed = start.elapsed().as_secs_f64();
    let stats = audio.stats();
    let frames = stats.frames - before.frames;
    let expected = elapsed * f64::from(audio.rate());
    let line = format!(
        "backend={} rate={} frames={frames} (expected ~{expected:.0}) callbacks={} voices={} underruns={}",
        audio.backend(),
        audio.rate(),
        stats.callbacks - before.callbacks,
        stats.voices,
        stats.underruns,
    );
    // Devices pull ahead to fill their buffers, so allow some slack above;
    // below, the output fell behind.
    if (frames as f64) < expected * 0.5 || (frames as f64) > expected + f64::from(audio.rate()) {
        return Err(line);
    }
    if !demo.music.as_ref().is_some_and(Voice::is_playing) {
        return Err(format!("{line}; the music stopped"));
    }
    Ok(line)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--selftest") {
        let expect = args
            .iter()
            .position(|a| a == "--expect-backend")
            .and_then(|i| args.get(i + 1))
            .cloned();
        match selftest(expect.as_deref()) {
            Ok(line) => println!("audiodemo selftest: ok: {line}"),
            Err(line) => {
                println!("audiodemo selftest: FAILED: {line}");
                std::process::exit(1);
            }
        }
        return;
    }
    let demo = Demo::new(Audio::shared());
    demo.apply_volumes();
    let app = App::new(demo, view)
        .title("Stipple Audio")
        .logical_size(Size::new(560.0, 420.0));
    app.run();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tune_loops_after_its_intro() {
        let (wav, loop_start) = tune(48_000);
        let s = Sound::decode(&wav).unwrap();
        assert!((s.duration().as_secs_f64() - 64.0 * 60.0 / 280.0).abs() < 0.01);
        assert!((loop_start.as_secs_f64() - 16.0 * 60.0 / 280.0).abs() < 1e-6);
    }

    #[test]
    fn the_window_renders() {
        let mut app =
            App::new(Demo::new(Audio::offline(48_000)), view).logical_size(Size::new(560.0, 420.0));
        let frame = app.render_once();
        assert_eq!(frame.size().width, 560);
    }

    #[test]
    fn effects_play_to_the_end_offline() {
        let audio = Audio::offline(48_000);
        let demo = Demo::new(audio.clone());
        demo.play(&demo.fx.explosion);
        let mut buf = vec![0.0; 2 * 48_000];
        audio.render(&mut buf);
        assert!(buf.iter().any(|x| x.abs() > 0.05));
        assert_eq!(audio.stats().voices, 0);
    }
}
