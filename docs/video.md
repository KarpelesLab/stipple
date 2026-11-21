# Video

`stipple::video` plays a video file: its frames are decoded (in hardware
where the platform has a decoder Stipple drives), put back in presentation
order, shown each at its time, and drawn by the renderer. Where the
decoder's buffers can be imported by the GPU renderer, they reach the
screen without a copy: on macOS and iOS, VideoToolbox's `CVPixelBuffer`s are
`IOSurface`s that Metal binds as textures and converts in its shader; on
Android, `MediaCodec`'s pictures are `AHardwareBuffer`s the GPU renderer
samples; on Windows, Media Foundation's pictures become shared NV12 textures
Direct3D 11 samples.

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
| Windows | MP4, Matroska | H.264, HEVC (where the HEVC Video Extensions are installed) | Media Foundation: the system's decoder, on the GPU (DXVA) where there's a hardware Direct3D 11 device, else in software | while Direct3D 11 draws: a shared NV12 texture (copied on the GPU from DXVA's, or uploaded once from the software decoder's memory) sampled through two plane views; else NV12 read once, converted, uploaded |
| | | MPEG-1/2 | oxideav (software) | RGBA pixels, uploaded |
| Android | MP4, Matroska | H.264, HEVC | `MediaCodec`: the device's decoder (the emulator's is software) | while the GPU renderer imports YUV buffers: the `AImageReader`'s `AHardwareBuffer`, sampled by GLES as an external texture (Vulkan through a sampler YCbCr conversion); else its planes read once, converted, uploaded |
| | | MPEG-1/2 | oxideav (software) | RGBA pixels, uploaded |
| Web | what the browser plays | what the browser plays | the browser, in a `<video>` | the `<video>` itself: imported each frame by WebGPU (`importExternalTexture`), handed to WebGL2's `texImage2D`, drawn into a 2D canvas and read back by the software renderer |

Natively everything needs the **`video` feature**: oxideav's MP4 and
Matroska demuxers (`oxideav-mp4`, `oxideav-mkv`), its pure-Rust MPEG-1/2
decoder (`oxideav-mpeg12video`) and, on Linux, its VA-API bridge
(`oxideav-vaapi`, only its H.264 front end and the run-time libva loader).
VideoToolbox, Media Foundation (`mfplat.dll`, `d3d11.dll`) and `MediaCodec`
(`libmediandk.so`) are reached directly, loaded at run time like the
camera's AVFoundation and Camera2. Without the feature, `Video::open` says `Unsupported`
everywhere but the web, which needs nothing.

Not yet:

- **Sound.** The audio track is ignored (the web's `<video>` plays muted).
  Playing it through `stipple::audio`, the clock following the audio, is
  next.
- **H.264 and HEVC in software.** oxideav has no working H.264 decoder yet
  (its VP8, VP9, AV1 and Theora decoders are scaffolds too), so without the
  platform's decoder (a Linux machine without VA-API, a Windows without
  Media Foundation, or `VideoOptions::hardware(false)`) only MPEG-1/2
  plays. HEVC on Linux would need the bridge's HEVC front end.
- **Windows without a copy.** A DXVA picture is copied once on the GPU into
  a texture of its own (the decoder's are an array it reuses, not shared),
  and a software-decoded one uploaded; a pool of shared textures opened
  once by the renderer would spare the opening of a new one per frame.
- **Android buffers with padding.** A picture whose visible part is
  smaller than its buffer (a decoder that aligns its rows, crops the rest)
  is converted on the CPU: the external image has no crop rectangle yet.
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

## Media Foundation and MediaCodec

Both take Annex B: the demuxer's length-prefixed units get start codes, and
the `avcC`/`hvcC` record's parameter sets go first (in-band to Media
Foundation, as `csd-0`/`csd-1` to `MediaCodec`), and in-band again after a
seek. Both give pictures in presentation order with the times they were
given. A stream that changes size mid-way (`MF_E_TRANSFORM_STREAM_CHANGE`,
`INFO_OUTPUT_FORMAT_CHANGED`) is followed; the end of the stream drains
what they hold (`MFT_MESSAGE_COMMAND_DRAIN`, `BUFFER_FLAG_END_OF_STREAM`)
and a seek flushes them.

- **Media Foundation.** `CLSID_MSH264DecoderMFT` (HEVC:
  `CLSID_MSH265DecoderMFT`, else the first synchronous decoder `MFTEnumEx`
  lists) driven as an `IMFTransform` through its vtable. It's made on a
  thread of its own in the multithreaded apartment, so the caller's thread
  (the UI's, single-threaded for OLE) is left alone; the player's thread
  joins the MTA. Where a hardware Direct3D 11 device with video support
  comes up and the decoder is `MF_SA_D3D11_AWARE`, it gets the device
  through an `IMFDXGIDeviceManager` and decodes on the GPU into textures;
  otherwise in software into memory. Each picture is handed over by what
  the renderer is doing at the time: while Direct3D 11 imports NV12 (it drew
  in the last half second), a shared NV12 texture (`D3D11_RESOURCE_MISC_SHARED`,
  its legacy handle an `ExternalImage::dxgi_shared_handle`); otherwise NV12
  read once (a staging texture for DXVA's) and converted. A Windows without
  Media Foundation (the N editions without the Media Feature Pack, a Server
  without the feature) says `Unsupported`. `STIPPLE_VIDEO_NO_DXVA` keeps it
  in software.
- **MediaCodec.** `AMediaCodec_createDecoderByType` (`video/avc`,
  `video/hevc`) in synchronous mode, rendering into an `AImageReader` of
  `YUV_420_888` images the GPU can sample and the CPU read
  (`AImageReader_newWithUsage`, Android 8). Each picture is acquired before
  the next is rendered (the decoder's surface drops a queued picture for a
  newer one). While the renderer imports `ExternalFormat::Yuv420` (as for
  the camera), the image's `AHardwareBuffer` is the frame and the `AImage`
  stays out of the reader until the frame goes; otherwise its planes are
  converted at once and the image given back. Without a reader (older
  systems, a decoder that won't render into one; `STIPPLE_VIDEO_NO_SURFACE`
  forces it) the decoder writes into its own buffers, planar or semi-planar
  as its output format says, converted the same way. A native executable
  has no binder threads for the media service to call back on:
  `ABinderProcess_startThreadPool` starts them (a no-op in an app).

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
- Elsewhere the H.264 clip plays through the platform's decoder (VA-API,
  Media Foundation, `MediaCodec`): every frame in presentation order at its
  time with the right colors, seeking (to a frame, between two, past the
  end), and a `Video` played to its last frame; where there's no such
  decoder it says `Unsupported` (a failure with `STIPPLE_REQUIRE_VIDEO`
  set); with `hardware(false)` it's always `Unsupported`.
- On Windows, while Direct3D 11 imports NV12, every frame is a shared NV12
  texture, and with the `d3d` feature Direct3D 11 draws them (nothing
  uploaded) with the right colors.
- On Android, while the renderer imports YUV buffers, every frame is an
  `AHardwareBuffer` with the right colors when its planes are read, and
  with the `gl` feature GLES draws them (nothing uploaded).

```sh
cargo test --features video video::
cargo test --features video,mtl video::      # with Metal, on a Mac
cargo test --features video,d3d video::      # with Direct3D 11, on Windows
tools/gpu-mobile.sh android video gl         # on an Android device or emulator
cargo run --example videoplayer --features video -- --selftest [DIR]
```

`videoplayer --selftest` opens each bundled clip this platform plays (the
MPEG-2 one everywhere) in the app's window off screen, checks the picture
in it, plays half a second and checks again.

Run so far: all of the above on an Apple M5 Max (macOS: VideoToolbox, the
software decoder, Metal and the software renderer). CI runs the MPEG-2 tests
and the self-test on Linux; on its Windows runner (non-blocking, the
`visual-windows` job) Media Foundation decodes the H.264 clip in software
(the runner has only WARP) and Direct3D 11 on WARP draws its shared
textures; on the API 34 emulator (non-blocking, `gpu-android`, through
`tools/gpu-mobile.sh android video gl`) `MediaCodec`'s software decoder
renders into the image reader, GLES draws the hardware buffers, and the
`ByteBuffer` fallback decodes the clip too. VA-API's H.264 ran on an NVIDIA
GPU through `nvidia-vaapi-driver` (all frames, colors, seeking). Not run
yet: DXVA (a Windows machine with a GPU), `MediaCodec` on a phone's
hardware decoder, HEVC on Windows and Android, iOS and the web's `<video>`
glue (compile-checked).
