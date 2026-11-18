# Video

`stipple::video` plays a video file: its frames are decoded (in hardware
where the platform has a decoder Stipple drives), put back in presentation
order, shown each at its time, and drawn by the renderer. Where the
decoder's buffers can be imported by the GPU renderer, they reach the
screen without a copy: on macOS and iOS, VideoToolbox's `CVPixelBuffer`s are
`IOSurface`s that Metal binds as textures and converts in its shader.

```sh
cargo run --example videoplayer --features video
cargo run --example videoplayer --features video -- clip.mp4
```

<!-- live: videoplayer -->

## Playing a video

```rust,no_run
use stipple::prelude::*;
use stipple::video::{Video, VideoOptions, video_view};

struct State {
    video: Video,
}

fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let video = state.video.clone();
    column(vec![
        video_view(cx, &state.video),
        button(cx, "Play / pause").on_tap(cx, move |_| {
            if video.is_playing() { video.pause() } else { video.play() }
        }),
    ])
}

let video = Video::open_file("clip.mp4", VideoOptions::new().autoplay(true)).unwrap();
let app = App::new(State { video }, view);
```

- `Video::open(bytes, options)` and `Video::open_file(path, options)` open a
  video. Natively the container and the codec are checked at once: a file
  this build can't play is `VideoError::Unsupported` there and then. The
  first frame is decoded and shown, paused unless `autoplay`.
- `play`, `pause`, `seek(Duration)` and `set_looping` work from any thread
  (a `Video` is a cheap handle; clones share it, and it stops when the last
  one is dropped). `seek` shows the frame at that time (the first at or
  after it, within half a frame), paused or not; `play` after the end starts
  again.
- `position`, `duration`, `size`, `status` (`Loading`, `Paused`, `Playing`,
  `Ended`, `Failed`) and `decoder` say where it stands.
- `video_view(cx, &video)` is the element: the current frame letterboxed in
  its box (black bars around it), black until the first frame. It grows to
  fill its parent; size it like any element. It repaints by itself as frames
  come, without the view being rebuilt (it's a canvas the player
  invalidates, as the camera's preview is).
- `Video::frame()` is the current `VideoFrame`: its `timestamp` and its
  `image`, an `ExternalImage` to draw with `Scene::draw_external` (in a
  canvas of your own, rotated, faded, under other things) or to show in a
  viewport with `App::set_viewport_external`. `Video::on_frame` hears each
  frame as it's shown, on the player's thread: to update a time label
  through an `AppHandle`, as the example does.

Each video decodes on a thread of its own, a few frames ahead of the one
shown. Frames come out of a decoder in decode order (B-frames before the
reference they're shown after); the player holds a few back and sorts them
by presentation time. It shows each when its time comes against the clock
from where playback started, and drops a frame that's more than 60 ms late
rather than falling further behind.

## What plays where

| | Containers | Codecs | Decoder | Frames |
|---|---|---|---|---|
| macOS, iOS | MP4, Matroska | H.264, HEVC | VideoToolbox (the media engine) | `IOSurface`-backed NV12 `CVPixelBuffer`s: drawn by Metal without a copy, locked and converted by the software renderer |
| | | MPEG-1/2 | oxideav (software) | RGBA pixels, uploaded |
| Linux | MP4, Matroska | H.264 | VA-API (the GPU), through `oxideav-vaapi` | read back from the surface once (NV12), converted, uploaded |
| | | MPEG-1/2 | oxideav (software) | RGBA pixels, uploaded |
| Windows, Android | MP4, Matroska | MPEG-1/2 | oxideav (software) | RGBA pixels, uploaded |
| Web | what the browser plays | what the browser plays | the browser, in a `<video>` | the `<video>` itself: imported each frame by WebGPU (`importExternalTexture`), handed to WebGL2's `texImage2D`, drawn into a 2D canvas and read back by the software renderer |

Natively everything needs the **`video` feature**: oxideav's MP4 and
Matroska demuxers (`oxideav-mp4`, `oxideav-mkv`), its pure-Rust MPEG-1/2
decoder (`oxideav-mpeg12video`) and, on Linux, its VA-API bridge
(`oxideav-vaapi`, only its H.264 front end and the run-time libva loader).
VideoToolbox is reached directly, loaded at run time like the camera's
AVFoundation. Without the feature, `Video::open` says `Unsupported`
everywhere but the web, which needs nothing.

Not yet:

- **Sound.** The audio track is ignored (the web's `<video>` plays muted).
  Playing it through `stipple::audio`, the clock following the audio, is
  next.
- **H.264 and HEVC in software.** oxideav has no working H.264 decoder yet
  (its VP8, VP9, AV1 and Theora decoders are scaffolds too), so without a
  hardware decoder (Windows, Android, a Linux machine without VA-API, or
  `VideoOptions::hardware(false)`) only MPEG-1/2 plays. Windows would need
  Media Foundation and Android `MediaCodec`.
- **MPEG-2 in MP4.** `oxideav-mp4` labels an `mp4v` track with an MPEG-2
  object type as MPEG-4 Part 2, so MPEG-2 plays from Matroska only. A pure
  MPEG-1 stream (no MPEG-2 extensions) doesn't decode yet either.
- **VA-API without the copy.** Exporting the decoded surface as a `dma-buf`
  (`vaExportSurfaceHandle`, NV12, which the GLES and Vulkan renderers
  already import) would skip the read-back, but `oxideav-vaapi` reuses a
  picture's surface as soon as the next picture is decoded unless it's a
  reference, while the player holds frames ahead and the renderer holds the
  last few. That needs surfaces the player can keep (a pool the bridge
  decodes into, or a GPU copy).

Candidates looked at and left out: `oxideav-videotoolbox` decodes through
the same VideoToolbox API but copies each `CVPixelBuffer` into its own
planes, which is the copy this module avoids; `oxideav-h264`'s software
decoder doesn't decode yet.

## The color space

Video is limited range YUV. VideoToolbox's frames carry their matrix (a
`CVPixelBuffer` attachment): BT.709 for HD, else BT.601, and the image says
so (`ExternalImage::color_space`), which every converter (Metal's shader,
the CPU) uses. Software-decoded frames are taken as BT.601 up to 576 lines
and BT.709 above, as MPEG-2 streams nearly always are.

## Checking it

`src/video/tests.rs` plays two clips committed beside it, a second each at
10 frames per second, 160×120, red and blue halves for half a second, then
lime and yellow:

- `bars-mpeg2.mkv` (MPEG-2, closed GOPs, B-frames) decodes in software,
  every frame in presentation order with the right colors and times;
  seeking lands on the frame asked for; a `Video` shows its first frame,
  plays to the end in about the clip's time, seeks while ended, and loops.
- `bars-h264.mp4` (H.264 Main with B-frames) through VideoToolbox: every
  frame an NV12 `IOSurface`, in order, the right colors on the software
  renderer, and seeking. With the `mtl` feature, Metal draws two of its
  frames straight from the surfaces (no texture uploaded, against one upload
  for the same frame as pixels) within a mean of 3 of 255 of the software
  renderer.
- Elsewhere the H.264 clip either plays through VA-API with the right
  colors, or says `Unsupported`; without hardware decoding it's always
  `Unsupported`.

```sh
cargo test --features video video::
cargo test --features video,mtl video::      # with Metal, on a Mac
cargo run --example videoplayer --features video -- --selftest [DIR]
```

`videoplayer --selftest` opens each bundled clip this platform plays (the
MPEG-2 one everywhere) in the app's window off screen, checks the picture
in it, plays half a second and checks again.

Run so far: all of the above on an Apple M5 Max (macOS: VideoToolbox, the
software decoder, Metal and the software renderer). CI runs the MPEG-2 tests
and the self-test on Linux. VA-API is compile-checked only (no machine
with a VA-API driver has run it yet); iOS, Windows, Android and the web's
`<video>` glue are compile-checked.
