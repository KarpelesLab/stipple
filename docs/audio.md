# Sound

`stipple::audio` plays sound effects and music on every platform, through
each operating system's own audio API: a mixer of any number of voices, each
with its own volume, pan and speed, in volume groups a settings screen can
turn up and down. `examples/audiodemo` plays synthesized effects and a
generated tune, with a slider per group.

```rust,no_run
use std::time::Duration;
use stipple::audio::{Audio, Group, MusicParams, PlayParams, Sound};

let audio = Audio::shared(); // the process's output, opened on first use

// Effects: decode once, play as often as you like (they overlap).
let jump = audio.load(include_bytes!("jump.wav")).unwrap();
audio.play(&jump, PlayParams::default().volume(0.8).pan(-0.3));

// Music: streamed from its encoded bytes, decoded as it plays.
let theme = audio
    .play_music(
        std::fs::read("theme.ogg").unwrap(), // needs the `audio` feature
        MusicParams::default()
            .loop_points(Duration::from_secs(8), None) // an intro, then a loop
            .fade_in(Duration::from_secs(1)),
    )
    .unwrap();

// A settings screen's sliders.
audio.set_volume(Group::MASTER, 0.8);
audio.set_volume(Group::MUSIC, 0.5);

// Later: fade the music out, pause everything for a pause menu.
theme.fade_out(Duration::from_secs(2));
audio.pause_all();
```

## The pieces

- **`Audio`** is the mixer and the output device it plays through. Clones
  share both, and every method can be called from any thread; a game calls
  it from its frame update. Calls return at once: the audio thread picks the
  change up at its next buffer, and moves volume, pan and group changes in a
  straight line over 256 frames (about 5 ms) so nothing clicks.
  - `Audio::shared()` opens the default output once per process and hands
    out clones; `Audio::new()` opens another.
  - `Audio::offline(rate)` has no device: `render(&mut buf)` pulls the mix,
    to write it to a file or in tests.
  - `rate()` is the output's sample rate (48 000 on most devices) and
    `backend()` names the output (`"coreaudio"`, `"wasapi"`, `"alsa"`,
    `"aaudio"`, `"webaudio"`, `"null"`).
  - `stats()` counts the frames the output pulled, its callbacks, the voices
    playing and any music underruns.
- **`Sound`** is decoded audio in memory, mono or stereo `f32`, shared by
  every voice playing it (cloning is cheap). Make one with:
  - `Sound::decode(bytes)`, at the file's own rate, or `Audio::load(bytes)`,
    converted to the output's rate with a windowed-sinc filter so it plays
    without per-sample conversion;
  - `Sound::from_samples(rate, channels, samples)`;
  - the synthesizer: `Tone::new(Waveform::Square, 880.0, duration)` with a
    pitch slide (`sweep_to`), attack, release and volume, rendered with
    `render(rate)`; `Sound::tone` for a plain note; `note_frequency(midi)`.
    Waveforms: sine, square, triangle, sawtooth, and noise (random values
    held for half a period, so a falling sweep is an explosion).
  - `Sound::concat`, `Sound::mix`, `gain`, `resampled`, `to_stereo`, and
    `to_wav` (16-bit PCM) to build longer sounds out of short ones.
- **`Audio::play(&sound, PlayParams)`** starts a **`Voice`**. `PlayParams`
  has a `volume` (linear: 1 as recorded), a `pan` (-1 left to 1 right), a
  `speed` (2 is twice as fast and an octave up), `looping`, the `group`, and
  a `fade_in`. The voice handle can `stop()` (over 5 ms), `fade_out(d)`,
  `fade_to(volume, d)`, `set_volume`, `set_pan`, `set_speed`, `pause()`,
  `resume()`, and tell whether it `is_playing()`. Dropping the handle leaves
  the sound playing to its end.
- **`Audio::play_music(bytes, MusicParams)`** streams: a worker thread
  decodes a little ahead of what plays (about 0.7 s) into a lock-free ring,
  so a long track never sits decoded in memory. `loop_points(start, end)`
  loops between two times: the first pass plays from the top, so an intro
  can come before the loop. Music plays in `Group::MUSIC` unless told
  otherwise, and returns a `Voice` like any sound.
- **Groups.** Every voice plays in one. `Group::MASTER` scales all of them;
  `Group::MUSIC` and `Group::EFFECTS` (the default for `play`) are built in;
  `audio.group("voices")` makes another (up to 32 in all). Each has a
  volume (`set_volume`, `volume`) and can be held on its own
  (`pause_group`, `resume_group`), say the effects under a pause menu while
  its music plays on. `pause_all` and `resume_all` hold everything;
  `stop_all(fade)` ends every voice.

**Pan.** A mono sound pans at constant power: each side at −3 dB (0.707) in
the centre, all on one side at the edge. A stereo sound pans as a balance:
both sides at full in the centre, the far side fading out as it moves.

**Rate and speed.** A sound at another rate than the output's, or at a speed
other than 1, is converted as it plays with cubic (Catmull-Rom)
interpolation; at matching rates and speed 1 the output is the sound's
samples exactly. For the best quality load effects with `Audio::load`.

**Clipping.** The mix is clamped to full scale. Many loud voices at once
will clip; keep effect volumes below 1 when a lot plays together.

## Formats

| Format | Without features | With `audio` |
|---|---|---|
| WAV: 8/16/24/32-bit PCM, 32/64-bit float | ✅ | ✅ |
| Ogg Vorbis | ❌ | ✅ |
| MP3 | ❌ | ✅ |
| FLAC | ❌ | ✅ |
| Ogg Opus | ❌ | ✅ (always 48 kHz) |

The `audio` Cargo feature brings oxideav's pure-Rust decoders
(`oxideav-basic`, `-ogg`, `-vorbis`, `-mp3`, `-flac`, `-opus`). It's off by
default since they add compile time; the output, the mixer, the synthesizer
and plain WAV need nothing (with the feature, a WAV file the built-in reader
turns down goes to oxideav-basic's). Compressed WAV encodings (ADPCM, µ-law)
aren't decoded. The format is recognized from the bytes, not a file name. Of more than two channels the first two (front left and right)
are kept. Decoding a format the build can't is an `AudioError::Unsupported`,
damaged data an `AudioError::Decode`.

Decode speed matters for streamed music in debug builds: Vorbis, MP3 and
FLAC decode many times faster than real time; Opus only a few times faster
in a release build, so prefer Vorbis for long music.

## Platforms

| Platform | Output | Rate | Buffer |
|---|---|---|---|
| macOS | AudioToolbox: the default output `AudioUnit`, render callback | the device's | the unit's (512 frames, ~11 ms, usually) |
| iOS | AudioToolbox: the RemoteIO `AudioUnit`; the `AVAudioSession` in the ambient category | the session's | the session's |
| Windows | WASAPI, shared mode, event-driven, on a thread of ours (MMCSS "Pro Audio") | the mix format's | 20 ms |
| Linux | ALSA's `default` PCM through `libasound.so.2`, loaded at run time; a thread writes with `snd_pcm_writei` | 48 kHz (ALSA or the sound server converts) | 40 ms |
| Android | AAudio from `libaaudio.so`, loaded at run time (Android 8+), low-latency shared stream, usage "game" | the device's | AAudio's burst |
| Web | Web Audio: a `ScriptProcessorNode` in `stipple.js` pulls the mix from the module | the `AudioContext`'s | 1024 frames (~21 ms) plus the browser's |
| none | the null output: a thread pulls the mix in real time and drops it | 48 kHz | — |

On Linux, PulseAudio and PipeWire both provide ALSA's `default` device, so
the ALSA output reaches them. The Linux and Android libraries are loaded when
audio is first opened: a program without them still starts, and plays
through the null output.

**No device.** When no output opens (no sound card, no `libasound`, Android
before 8, a browser without Web Audio), or `STIPPLE_AUDIO=null` is set in the
environment, the null output plays instead: voices still advance and end on
time, so the game works the same without sound. On the web, with no threads,
nothing advances without an audio context.

**Losing the device.** WASAPI reopens the new default device when the old one
goes (unplugged, the default changed); AAudio reopens its stream when it's
disconnected; ALSA, if it can't recover, carries on like the null output.

**Phones in the background.** Output stops when the app goes to the
background (`Lifecycle::Background`) and starts again when it comes back:
`AudioOutputUnitStop` and the session made inactive on iOS,
`AAudioStream_requestPause` on Android. Voices hold where they were. On iOS,
the ambient category mixes with other apps' audio (the user's music keeps
playing) and follows the silent switch. Interruptions (a phone call) aren't
followed yet beyond that: the unit restarts when the app returns to the
foreground. Android audio focus isn't requested.

## Latency

A sound starts at the output's next buffer: within about 10 ms on macOS and
iOS, 20 ms on Windows, 40 ms on Linux (plus the sound server's own
buffering), a few milliseconds on Android with a low-latency path, and
roughly 30–60 ms on the web (the script processor's buffer and the
browser's). Streamed music starts at once: a first stretch is decoded on the
calling thread before `play_music` returns.

## The web and autoplay

Browsers keep audio silent until the user interacts with the page. The first
`Audio::new` (or `Audio::shared`) makes the page's `AudioContext`, which
starts suspended; `stipple.js` resumes it on the first click, tap or key
press anywhere on the page. Sounds played before that are mixed but not
heard, so start music from a "press to start" screen or a button. The
module has no threads, so streamed music is decoded on the page's main
thread as the browser asks for audio.

## Testing

`Audio::offline(rate)` and `render` give a mixer with no device, rendered
on demand: the mixer's tests use it to check mixing, the pan law, group
volumes, looping, fades, rate conversion, streaming with loop points and
voices ending, exactly. `audiodemo --selftest` opens the real output, plays
for half a second and checks the output pulled about as many frames as that
takes (`--expect-backend alsa` also checks which output opened).
