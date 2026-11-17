//! Mixer, decoder and output tests.

use super::*;
use std::time::Duration;

const RATE: u32 = 48_000;

/// Render `frames` frames from an offline mixer.
fn render(audio: &Audio, frames: usize) -> Vec<f32> {
    let mut out = vec![0.0; frames * 2];
    assert!(audio.render(&mut out));
    out
}

fn ramp(n: usize) -> Vec<f32> {
    (0..n).map(|i| (i as f32 + 1.0) / 1024.0).collect()
}

#[test]
fn plays_a_sound_exactly_at_matching_rates() {
    let audio = Audio::offline(RATE);
    let s = Sound::from_samples(RATE, 2, ramp(400));
    let v = audio.play(&s, PlayParams::default());
    assert!(v.is_playing());
    let out = render(&audio, 150);
    assert_eq!(&out[..], &s.samples()[..300]);
    let out = render(&audio, 150);
    assert_eq!(&out[..100], &s.samples()[300..]);
    assert!(out[100..].iter().all(|&x| x == 0.0));
    assert!(!v.is_playing(), "the voice ends with its sound");
    assert_eq!(audio.stats().voices, 0);
}

#[test]
fn voices_end_after_exactly_their_frames() {
    for n in [1usize, 2, 3, 4, 255, 256, 257, 1000] {
        let audio = Audio::offline(RATE);
        let s = Sound::from_samples(RATE, 1, vec![0.5; n]);
        let v = audio.play(&s, PlayParams::default().pan(-1.0));
        let out = render(&audio, n + 10);
        let heard = out.chunks(2).filter(|f| f[0] != 0.0).count();
        assert_eq!(heard, n, "{n} frames");
        assert!(!v.is_playing());
    }
}

#[test]
fn mixes_voices_by_summing() {
    let audio = Audio::offline(RATE);
    let a = Sound::from_samples(RATE, 2, vec![0.25; 128]);
    let b = Sound::from_samples(RATE, 2, vec![-0.125; 64]);
    audio.play(&a, PlayParams::default());
    audio.play(&b, PlayParams::default());
    let out = render(&audio, 64);
    assert_eq!(out[0], 0.125);
    assert_eq!(out[63], 0.125);
    assert_eq!(out[62], 0.125);
    assert_eq!(out[64], 0.25);
    // Clipped at full scale.
    let audio = Audio::offline(RATE);
    for _ in 0..4 {
        audio.play(&a, PlayParams::default());
    }
    assert_eq!(render(&audio, 1)[0], 1.0);
}

#[test]
fn pan_law_is_constant_power_for_mono_and_a_balance_for_stereo() {
    let check = |pan: f32| {
        let audio = Audio::offline(RATE);
        let s = Sound::from_samples(RATE, 1, vec![1.0; 16]);
        audio.play(&s, PlayParams::default().pan(pan));
        let out = render(&audio, 1);
        (out[0], out[1])
    };
    let (l, r) = check(0.0);
    assert!((l - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
    assert!((r - l).abs() < 1e-6);
    let (l, r) = check(-1.0);
    assert!((l - 1.0).abs() < 1e-6 && r.abs() < 1e-6);
    let (l, r) = check(1.0);
    assert!(l.abs() < 1e-6 && (r - 1.0).abs() < 1e-6);
    for pan in [-0.7f32, -0.2, 0.3, 0.9] {
        let (l, r) = check(pan);
        assert!((l * l + r * r - 1.0).abs() < 1e-5, "power at {pan}");
    }
    // Stereo: full at the centre, the other side fades as it moves.
    let g = mixer::pan_gains(false, 0.0, 1.0);
    assert_eq!(g, [1.0, 1.0]);
    let g = mixer::pan_gains(false, 0.5, 1.0);
    assert_eq!(g, [0.5, 1.0]);
}

#[test]
fn volumes_and_groups_multiply() {
    let audio = Audio::offline(RATE);
    let s = Sound::from_samples(RATE, 2, vec![0.5; 4096]);
    let voice = audio.play(&s, PlayParams::default().volume(0.5));
    let music = audio.group("music").unwrap();
    assert_eq!(music, Group::MUSIC);
    let ui = audio.group("ui").unwrap();
    assert_eq!(ui.index(), 3);
    assert_eq!(audio.group("ui"), Some(ui));
    audio.play(&s, PlayParams::default().group(ui));
    assert_eq!(render(&audio, 1)[0], 0.25 + 0.5);
    audio.set_volume(Group::EFFECTS, 0.5);
    audio.set_volume(ui, 0.0);
    // Group changes ramp over one block, then hold.
    render(&audio, 256);
    assert_eq!(render(&audio, 1)[0], 0.125);
    audio.set_volume(Group::MASTER, 0.5);
    render(&audio, 256);
    assert_eq!(render(&audio, 1)[0], 0.0625);
    assert_eq!(audio.volume(Group::MASTER), 0.5);
    voice.set_volume(1.0);
    render(&audio, 256);
    assert_eq!(render(&audio, 1)[0], 0.125);
}

#[test]
fn gain_changes_ramp_instead_of_jumping() {
    let audio = Audio::offline(RATE);
    let s = Sound::from_samples(RATE, 2, vec![1.0; 4096]);
    audio.play(&s, PlayParams::default());
    render(&audio, 256);
    audio.set_volume(Group::EFFECTS, 0.0);
    let out = render(&audio, 256);
    let left: Vec<f32> = out.chunks(2).map(|f| f[0]).collect();
    assert!(left.windows(2).all(|w| w[1] <= w[0]));
    assert!(left[0] > 0.99 && left[255] == 0.0);
    assert!(left.windows(2).all(|w| (w[0] - w[1]) < 0.01), "no jump");
}

#[test]
fn looping_sounds_wrap_and_stop_on_request() {
    let audio = Audio::offline(RATE);
    let s = Sound::from_samples(RATE, 1, vec![0.1, 0.2, 0.3]);
    let v = audio.play(&s, PlayParams::default().looping(true).pan(-1.0));
    let out = render(&audio, 10);
    let left: Vec<f32> = out.chunks(2).map(|f| f[0]).collect();
    let want = [0.1, 0.2, 0.3, 0.1, 0.2, 0.3, 0.1, 0.2, 0.3, 0.1];
    for (a, b) in left.iter().zip(want) {
        assert!((a - b).abs() < 1e-6);
    }
    v.stop();
    render(&audio, 48_000 / 100);
    assert!(!v.is_playing());
}

#[test]
fn fades_reach_their_target_and_fade_out_stops() {
    let audio = Audio::offline(RATE);
    let s = Sound::from_samples(RATE, 2, vec![1.0; 48_000]);
    let v = audio.play(&s, PlayParams::default().fade_in(Duration::from_millis(10)));
    let out = render(&audio, 480);
    assert!(out[0] < 0.01, "starts silent");
    assert!((out[2 * 479] - 1.0).abs() < 1e-3, "ends at full");
    v.fade_to(0.5, Duration::from_millis(10));
    render(&audio, 480 + 256);
    assert!((render(&audio, 1)[0] - 0.5).abs() < 1e-6);
    v.fade_out(Duration::from_millis(10));
    render(&audio, 480 + 256);
    assert!(!v.is_playing());
}

#[test]
fn pause_holds_voices_where_they_are() {
    let audio = Audio::offline(RATE);
    let s = Sound::from_samples(RATE, 1, ramp(1000));
    audio.play(&s, PlayParams::default().pan(-1.0));
    let a = render(&audio, 10);
    audio.pause_all();
    assert!(render(&audio, 100).iter().all(|&x| x == 0.0));
    audio.resume_all();
    let b = render(&audio, 1);
    assert!((b[0] - s.samples()[10]).abs() < 1e-6);
    assert!((a[18] - s.samples()[9]).abs() < 1e-6);
    // A paused group holds; the others play on.
    audio.pause_group(Group::EFFECTS);
    assert!(audio.is_group_paused(Group::EFFECTS));
    assert!(render(&audio, 10).iter().all(|&x| x == 0.0));
    audio.resume_group(Group::EFFECTS);
    assert!((render(&audio, 1)[0] - s.samples()[11]).abs() < 1e-6);
}

#[test]
fn stop_all_stops_everything() {
    let audio = Audio::offline(RATE);
    let s = Sound::from_samples(RATE, 1, vec![0.5; 4]);
    let a = audio.play(&s, PlayParams::default().looping(true));
    let b = audio.play(&s, PlayParams::default().looping(true));
    render(&audio, 10);
    audio.stop_all(Duration::ZERO);
    render(&audio, 10);
    assert!(!a.is_playing() && !b.is_playing());
    assert_eq!(audio.stats().voices, 0);
}

/// The largest difference between `got` (mono, at `rate`) and a sine of
/// `freq` Hz, away from the edges.
fn sine_error(got: &[f32], rate: u32, freq: f64, amp: f64) -> f64 {
    let skip = got.len() / 8;
    got.iter()
        .enumerate()
        .skip(skip)
        .take(got.len() - 2 * skip)
        .map(|(i, &v)| {
            let want = amp * (std::f64::consts::TAU * freq * i as f64 / f64::from(rate)).sin();
            (f64::from(v) - want).abs()
        })
        .fold(0.0, f64::max)
}

fn sine(rate: u32, freq: f64, frames: usize, amp: f64) -> Vec<f32> {
    (0..frames)
        .map(|i| (amp * (std::f64::consts::TAU * freq * i as f64 / f64::from(rate)).sin()) as f32)
        .collect()
}

#[test]
fn resampling_is_accurate() {
    for (from, to) in [
        (44_100, 48_000),
        (48_000, 44_100),
        (22_050, 48_000),
        (96_000, 48_000),
    ] {
        let s = Sound::from_samples(from, 1, sine(from, 1000.0, from as usize / 4, 0.5));
        let r = s.resampled(to);
        assert_eq!(r.rate(), to);
        let expect = (s.frames() as f64 * f64::from(to) / f64::from(from)).round() as usize;
        assert_eq!(r.frames(), expect);
        let err = sine_error(r.samples(), to, 1000.0, 0.5);
        assert!(err < 2e-3, "{from} -> {to}: {err}");
    }
}

#[test]
fn the_mixer_converts_rates_and_speed_on_the_fly() {
    // A 44.1 kHz sine played on a 48 kHz mixer, through the cubic.
    let audio = Audio::offline(RATE);
    let s = Sound::from_samples(44_100, 1, sine(44_100, 440.0, 22_050, 0.5));
    audio.play(&s, PlayParams::default().pan(-1.0));
    let out = render(&audio, 20_000);
    let left: Vec<f32> = out.chunks(2).map(|f| f[0]).collect();
    assert!(sine_error(&left, RATE, 440.0, 0.5) < 2e-3);
    // Double speed: an octave up, and done in half the time.
    let audio = Audio::offline(RATE);
    let s = Sound::from_samples(RATE, 1, sine(RATE, 440.0, 48_000, 0.5));
    let v = audio.play(&s, PlayParams::default().pan(-1.0).speed(2.0));
    let out = render(&audio, 20_000);
    let left: Vec<f32> = out.chunks(2).map(|f| f[0]).collect();
    assert!(sine_error(&left, RATE, 880.0, 0.5) < 2e-3);
    render(&audio, 4_100);
    assert!(!v.is_playing());
}

/// A WAV file of `samples` in the given format.
fn wav(samples: &[f32], channels: u16, rate: u32, tag: u16, bits: u16) -> Vec<u8> {
    let mut data = Vec::new();
    for &s in samples {
        match (tag, bits) {
            (1, 8) => data.push(((s * 127.0).round() as i32 + 128) as u8),
            (1, 16) => data.extend_from_slice(&((s * 32767.0).round() as i16).to_le_bytes()),
            (1, 24) => {
                data.extend_from_slice(&((s * 8_388_607.0).round() as i32).to_le_bytes()[..3])
            }
            (1, 32) => data.extend_from_slice(
                &((f64::from(s) * 2_147_483_647.0).round() as i32).to_le_bytes(),
            ),
            (3, 32) => data.extend_from_slice(&s.to_le_bytes()),
            (3, 64) => data.extend_from_slice(&f64::from(s).to_le_bytes()),
            _ => unreachable!(),
        }
    }
    let align = channels * bits / 8;
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(4 + 26 + 8 + 8 + data.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    // An unknown chunk to skip, of odd size (padded).
    out.extend_from_slice(b"LIST\x01\x00\x00\x00x\x00");
    out.extend_from_slice(b"fmt \x10\x00\x00\x00");
    out.extend_from_slice(&tag.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * u32::from(align)).to_le_bytes());
    out.extend_from_slice(&align.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&data);
    out
}

#[test]
fn wav_round_trips_in_every_plain_encoding() {
    let src: Vec<f32> = (0..64).map(|i| ((i as f32) * 0.3).sin() * 0.9).collect();
    for (tag, bits, tol) in [
        (1, 8, 1.0 / 64.0),
        (1, 16, 1.0 / 16_000.0),
        (1, 24, 1e-6),
        (1, 32, 1e-6),
        (3, 32, 0.0),
        (3, 64, 1e-7),
    ] {
        for channels in [1u16, 2] {
            let bytes = wav(&src, channels, 22_050, tag, bits);
            let s = Sound::decode(&bytes).unwrap();
            assert_eq!((s.channels(), s.rate()), (channels, 22_050));
            assert_eq!(s.samples().len(), src.len(), "{tag}/{bits}");
            for (a, b) in s.samples().iter().zip(&src) {
                assert!((a - b).abs() <= tol, "{tag}/{bits}: {a} vs {b}");
            }
        }
    }
    // Sound::to_wav and back.
    let s = Sound::from_samples(RATE, 2, src.clone());
    let back = Sound::decode(&s.to_wav()).unwrap();
    assert_eq!((back.channels(), back.rate(), back.frames()), (2, RATE, 32));
    // More than two channels keep the first two.
    let quad: Vec<f32> = (0..16).map(|i| i as f32 / 16.0).collect();
    let s = Sound::decode(&wav(&quad, 4, RATE, 3, 32)).unwrap();
    assert_eq!(s.channels(), 2);
    assert_eq!(
        s.samples(),
        &[
            0.0,
            1.0 / 16.0,
            4.0 / 16.0,
            5.0 / 16.0,
            8.0 / 16.0,
            9.0 / 16.0,
            12.0 / 16.0,
            13.0 / 16.0
        ]
    );
}

#[test]
fn bad_data_is_an_error() {
    assert!(Sound::decode(b"RIFF\x04\x00\x00\x00WAVE").is_err());
    let err = Sound::decode(b"definitely not audio").unwrap_err();
    assert!(matches!(err, AudioError::Unsupported(_)), "{err:?}");
}

#[test]
fn audio_load_converts_to_the_output_rate() {
    let audio = Audio::offline(RATE);
    let bytes = Sound::from_samples(24_000, 1, sine(24_000, 300.0, 2400, 0.5)).to_wav();
    let s = audio.load(&bytes).unwrap();
    assert_eq!((s.rate(), s.frames()), (RATE, 4800));
}

#[test]
fn music_streams_loops_and_ends() {
    // A 0.1 s ramp, streamed: plays to the end and stops.
    let src = Sound::from_samples(RATE, 1, (0..4800).map(|i| i as f32 / 8192.0).collect());
    let audio = Audio::offline(RATE);
    let v = audio
        .play_music(src.to_wav(), MusicParams::default())
        .unwrap();
    let out = collect_stream(&audio, 6000);
    let left: Vec<f32> = out.chunks(2).map(|f| f[0]).collect();
    for (i, &x) in left.iter().take(4800).enumerate() {
        assert!((x - src.samples()[i]).abs() < 1e-3, "frame {i}");
    }
    assert!(left[4800..].iter().all(|&x| x == 0.0));
    assert!(!v.is_playing());

    // Looping between loop points: the intro once, then the loop.
    let audio = Audio::offline(RATE);
    let params = MusicParams::default().loop_points(
        Duration::from_secs_f64(1000.0 / 48_000.0),
        Some(Duration::from_secs_f64(3000.0 / 48_000.0)),
    );
    let v = audio.play_music(src.to_wav(), params).unwrap();
    let out = collect_stream(&audio, 7000);
    let left: Vec<f32> = out.chunks(2).map(|f| f[0]).collect();
    let expect = |i: usize| {
        let pos = if i < 3000 {
            i
        } else {
            1000 + (i - 3000) % 2000
        };
        src.samples()[pos]
    };
    for (i, &x) in left.iter().enumerate() {
        assert!(
            (x - expect(i)).abs() < 1e-3,
            "frame {i}: {x} vs {}",
            expect(i)
        );
    }
    assert!(v.is_playing());
    v.stop();
    collect_stream(&audio, 1000);
    assert!(!v.is_playing());
}

/// Render `frames` of streamed music. (An offline mixer runs faster than real
/// time; in tests it waits for the decoder thread rather than run dry.)
fn collect_stream(audio: &Audio, frames: usize) -> Vec<f32> {
    let before = audio.stats().underruns;
    let out = render(audio, frames);
    assert_eq!(audio.stats().underruns, before, "the decoder kept up");
    out
}

#[test]
fn synthesized_tones_have_their_pitch_and_length() {
    let s = Tone::new(Waveform::Sine, 440.0, Duration::from_millis(500))
        .attack(Duration::ZERO)
        .release(Duration::ZERO)
        .volume(0.5)
        .render(RATE);
    assert_eq!(s.frames(), 24_000);
    assert!(sine_error(s.samples(), RATE, 440.0, 0.5) < 1e-3);
    for w in [
        Waveform::Square,
        Waveform::Triangle,
        Waveform::Sawtooth,
        Waveform::Noise,
    ] {
        let s = Tone::new(w, 220.0, Duration::from_millis(100))
            .sweep_to(110.0)
            .render(RATE);
        assert!(s.samples().iter().all(|x| x.abs() <= 0.6), "{w:?}");
        assert!(s.samples().iter().any(|x| x.abs() > 0.2), "{w:?}");
        // The release brings it to silence.
        assert_eq!(*s.samples().last().unwrap(), 0.0);
    }
    assert!((note_frequency(69) - 440.0).abs() < 1e-3);
    assert!((note_frequency(81) - 880.0).abs() < 1e-2);
    let both = Sound::concat(&[Sound::silence(RATE, Duration::from_millis(10)), s.clone()]);
    assert_eq!(both.frames(), 480 + s.frames());
    let mixed = Sound::mix(&[s.clone(), s.clone().to_stereo()]);
    assert_eq!((mixed.channels(), mixed.frames()), (2, s.frames()));
}

#[test]
fn the_null_output_pulls_no_faster_than_real_time_until_paused() {
    // Its pace is tested with fixed times (`output::null`); this is the thread,
    // asserting nothing about how promptly a loaded machine runs it.
    use std::sync::atomic::Ordering::Relaxed;
    let pulled = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let p = pulled.clone();
    let opened = std::time::Instant::now();
    let mut device = output::null_for_tests(Box::new(move |buf: &mut [f32]| {
        p.fetch_add(buf.len() / 2, Relaxed);
        buf.fill(0.0);
    }));
    assert_eq!(device.name(), "null");
    while pulled.load(Relaxed) == 0 && opened.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(5));
    }
    let n = pulled.load(Relaxed);
    let most = opened.elapsed().as_secs_f64() * 48_000.0; // its rate
    assert!(n > 0, "it pulls");
    assert!(
        n as f64 <= most,
        "{n} frames, ahead of real time ({most:.0})"
    );
    // Paused: a pull under way may still finish, then nothing more comes.
    device.set_paused(true);
    let paused = std::time::Instant::now();
    loop {
        let a = pulled.load(Relaxed);
        std::thread::sleep(Duration::from_millis(60));
        if pulled.load(Relaxed) == a {
            break;
        }
        assert!(paused.elapsed() < Duration::from_secs(10), "still pulling");
    }
}

#[test]
fn audio_without_a_device_still_plays_voices_to_the_end() {
    // Not while another test has the app in the background.
    let _lock = crate::platform::frame::SUSPEND_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let audio = Audio::with_null_output();
    assert_eq!(audio.backend(), "null");
    let s = Sound::from_samples(audio.rate(), 1, vec![0.1; 2400]); // 50 ms
    let v = audio.play(&s, PlayParams::default());
    let t = std::time::Instant::now();
    while v.is_playing() && t.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!v.is_playing());
    assert!(audio.stats().frames >= 2400);
}

#[test]
fn output_stops_while_the_app_is_in_the_background() {
    let _lock = crate::platform::frame::SUSPEND_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let audio = Audio::with_null_output();
    let wait_for_frames = || {
        let start = audio.stats().frames;
        let t = std::time::Instant::now();
        while audio.stats().frames == start && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        audio.stats().frames > start
    };
    assert!(wait_for_frames());
    set_app_background(true);
    // A pull under way may still finish, then nothing more comes.
    let stopped = std::time::Instant::now();
    loop {
        let held = audio.stats().frames;
        std::thread::sleep(Duration::from_millis(80));
        if audio.stats().frames == held {
            break;
        }
        assert!(
            stopped.elapsed() < Duration::from_secs(10),
            "still pulling in the background"
        );
    }
    // An output opened while in the background starts stopped too.
    let late = Audio::with_null_output();
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(late.stats().frames, 0);
    set_app_background(false);
    assert!(wait_for_frames(), "and runs again in the foreground");
}

/// The compressed formats, through oxideav: a 440 Hz sine (at -9 dB; the FLAC
/// one at -6 dB), encoded by the reference tools (oggenc, lame (which picked
/// 24 kHz), flac, opusenc).
#[cfg(feature = "audio")]
#[test]
fn compressed_formats_decode() {
    // (name, bytes, rate, channels, seconds, peak)
    #[allow(clippy::type_complexity)]
    let files: [(&str, &[u8], u32, u16, f64, f32); 4] = [
        (
            "vorbis",
            include_bytes!("testdata/sine440.ogg"),
            44_100,
            2,
            0.25,
            0.35,
        ),
        (
            "mp3",
            include_bytes!("testdata/sine440.mp3"),
            24_000,
            2,
            0.25,
            0.35,
        ),
        (
            "flac",
            include_bytes!("testdata/sine440.flac"),
            44_100,
            1,
            0.1,
            0.5,
        ),
        (
            "opus",
            include_bytes!("testdata/sine440.opus"),
            48_000,
            2,
            0.25,
            0.35,
        ),
    ];
    for (name, bytes, rate, channels, secs, want) in files {
        let s = Sound::decode(bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!((s.rate(), s.channels()), (rate, channels), "{name}");
        let d = s.duration().as_secs_f64();
        // Lossy codecs pad the start and the end a little.
        assert!(d >= secs - 0.001 && d < secs + 0.08, "{name}: {d} s");
        let left: Vec<f32> = s
            .samples()
            .iter()
            .step_by(usize::from(channels))
            .copied()
            .collect();
        let peak = left.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!((peak - want).abs() < 0.05, "{name}: peak {peak}");
        // The pitch, from the rising zero crossings in the middle.
        let mid = &left[left.len() / 4..left.len() * 3 / 4];
        let ups = mid.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        let hz = ups as f64 / (mid.len() as f64 / f64::from(rate));
        assert!((hz - 440.0).abs() < 15.0, "{name}: {hz} Hz");
    }
    // Streamed, as music.
    let audio = Audio::offline(RATE);
    let v = audio
        .play_music(
            &include_bytes!("testdata/sine440.ogg")[..],
            MusicParams::default(),
        )
        .unwrap();
    let out = collect_stream(&audio, 14_400);
    assert!(out.iter().any(|x| x.abs() > 0.3));
    assert!(!v.is_playing());
}

#[test]
fn position_follows_playback_and_seek_jumps() {
    let audio = Audio::offline(RATE);
    // Mono, each sample its own time, to see where the voice reads.
    let samples: Vec<f32> = (0..RATE).map(|i| i as f32 / RATE as f32).collect();
    let s = Sound::from_samples(RATE, 1, samples);
    let v = audio.play(&s, PlayParams::default().pan(-1.0));
    assert_eq!(v.position(), Duration::ZERO);
    render(&audio, 12_000);
    let at = v.position().as_secs_f64();
    assert!((at - 0.25).abs() < 0.001, "{at}");

    v.seek(Duration::from_millis(750));
    assert!((v.position().as_secs_f64() - 0.75).abs() < 0.001);
    let out = render(&audio, 256);
    // The first frame after the seek is the sample at 0.75 s (all on the
    // left: a mono sound panned hard left).
    assert!((out[0] - 0.75).abs() < 0.001, "{}", out[0]);
    assert!(v.is_playing());

    v.pause();
    assert!(v.is_paused());
    render(&audio, 4_800);
    let held = v.position();
    render(&audio, 4_800);
    assert_eq!(v.position(), held, "a paused voice holds its place");
    v.resume();
    assert!(!v.is_paused());

    // Seeking past the end ends it.
    v.seek(Duration::from_secs(5));
    render(&audio, 256);
    assert!(!v.is_playing());
}
