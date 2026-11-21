//! Playing the committed clips: one second at 10 frames per second, 160×120,
//! red on the left and blue on the right for half a second, then lime and
//! yellow. `bars-mpeg2.mkv` (MPEG-2 in Matroska, closed GOPs, B-frames)
//! decodes in software everywhere; `bars-h264.mp4` (H.264 Main with
//! B-frames, in MP4) needs the platform's decoder: VideoToolbox, VA-API,
//! Media Foundation or `MediaCodec`. With `STIPPLE_REQUIRE_VIDEO` set, a
//! platform decoder that isn't there fails the H.264 tests instead of
//! skipping them (CI's Windows and Android runs).

use super::player::Stream;
use super::*;
use crate::geometry::{Rect, ScaleFactor, Size};
use crate::render::{Color, Pixmap, Scene, SoftwareRenderer};
use std::time::Instant;

const MPEG2: &[u8] = include_bytes!("testdata/bars-mpeg2.mkv");
const H264: &[u8] = include_bytes!("testdata/bars-h264.mp4");

const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const LIME: [u8; 3] = [0, 255, 0];
const YELLOW: [u8; 3] = [255, 255, 0];

/// `image` drawn into a scene its size.
fn scene_of(image: &Arc<ExternalImage>) -> Scene {
    let size = image.size();
    let (w, h) = (size.width as f64, size.height as f64);
    let mut scene = Scene::new(Size::new(w, h));
    scene.draw_external(Rect::from_xywh(0.0, 0.0, w, h), image);
    scene
}

fn on_cpu(image: &Arc<ExternalImage>) -> Pixmap {
    SoftwareRenderer::new()
        .background(Color::BLACK)
        .render(scene_of(image), ScaleFactor::IDENTITY)
}

/// The left and right halves' colors are about `left` and `right`.
fn assert_halves(p: &Pixmap, left: [u8; 3], right: [u8; 3], what: &str) {
    let (w, h) = (p.size().width, p.size().height);
    for (x, want) in [(w / 4, left), (w * 3 / 4, right)] {
        let px = p.pixel(x, h / 2).expect("a pixel");
        let off = (0..3).map(|i| px[i].abs_diff(want[i])).max().unwrap();
        assert!(
            off <= 40,
            "{what}: pixel ({x}, {}) is {px:?}, not about {want:?}",
            h / 2
        );
    }
}

fn wait_until(what: &str, mut ok: impl FnMut() -> bool) {
    let start = Instant::now();
    while !ok() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn an_mpeg2_stream_decodes_in_software_in_presentation_order() {
    let mut s = Stream::open(Source::Bytes(MPEG2.into()), true).unwrap();
    assert_eq!(s.decoder_name(), "software");
    assert_eq!(s.size, PhysicalSize::new(160, 120));
    assert_eq!(s.duration, Some(Duration::from_secs(1)));
    let mut times = Vec::new();
    while let Some(f) = s.next().unwrap() {
        if times.is_empty() {
            assert_halves(&on_cpu(&f.image), RED, BLUE, "the first frame");
        }
        times.push(f.time.as_millis());
        if times.len() == 10 {
            assert_halves(&on_cpu(&f.image), LIME, YELLOW, "the last frame");
        }
    }
    assert_eq!(times, (0..10).map(|i| i * 100).collect::<Vec<_>>());

    // Seeking lands on the frame at (or just after) the time asked for.
    let f = s.seek(Duration::from_millis(700)).unwrap().unwrap();
    assert_eq!(f.time, Duration::from_millis(700));
    assert_halves(&on_cpu(&f.image), LIME, YELLOW, "after seeking");
    let f = s.seek(Duration::from_millis(260)).unwrap().unwrap();
    assert_eq!(f.time, Duration::from_millis(300));
    assert_halves(&on_cpu(&f.image), RED, BLUE, "after seeking back");
}

#[test]
fn a_video_shows_its_first_frame_plays_to_the_end_and_seeks() {
    let video = Video::open(MPEG2, VideoOptions::new()).unwrap();
    assert_eq!(video.decoder(), "software");
    plays_to_the_end_and_seeks(video);
}

/// The player's part, whichever decoder: the first frame shown paused,
/// played to the last frame in about the clip's time, in order, then a
/// seek while ended.
fn plays_to_the_end_and_seeks(video: Video) {
    assert_eq!(video.size(), PhysicalSize::new(160, 120));
    wait_until("the first frame", || video.frame().is_some());
    assert_eq!(video.status(), VideoStatus::Paused);
    assert!(!video.is_playing());
    let first = video.frame().unwrap();
    assert_eq!(first.timestamp, Duration::ZERO);
    assert_halves(&on_cpu(&first.image), RED, BLUE, "the first frame");

    let shown = Arc::new(Mutex::new(Vec::new()));
    let seen = shown.clone();
    video.on_frame(move |f| seen.lock().unwrap().push(f.timestamp));
    let start = Instant::now();
    video.play();
    wait_until("the end", || video.status() == VideoStatus::Ended);
    let took = start.elapsed();
    // Nine more frames, a tenth of a second apart: about 0.9 s.
    assert!(
        took >= Duration::from_millis(800) && took < Duration::from_secs(3),
        "played in {took:?}"
    );
    let times = shown.lock().unwrap().clone();
    assert!(times.len() >= 5, "only {times:?} shown");
    assert!(
        times.windows(2).all(|w| w[0] < w[1]),
        "out of order: {times:?}"
    );
    assert_eq!(video.position(), Duration::from_millis(900));
    assert!(!video.is_playing());

    video.seek(Duration::from_millis(200));
    wait_until("the seek", || {
        video.position() == Duration::from_millis(200)
    });
    assert_halves(&on_cpu(&video.frame().unwrap().image), RED, BLUE, "seeked");
}

#[test]
fn a_looping_video_starts_again() {
    let video = Video::open(MPEG2, VideoOptions::new().autoplay(true).looping(true)).unwrap();
    let wrapped = Arc::new(Mutex::new((Duration::ZERO, false)));
    let w = wrapped.clone();
    video.on_frame(move |f| {
        let mut w = w.lock().unwrap();
        if f.timestamp < w.0 {
            w.1 = true;
        }
        w.0 = f.timestamp;
    });
    wait_until("the loop", || wrapped.lock().unwrap().1);
    assert!(video.is_playing());
    assert_eq!(video.status(), VideoStatus::Playing);
}

#[test]
fn h264_needs_a_platform_decoder() {
    match Video::open(H264, VideoOptions::new().hardware(false)) {
        Err(VideoError::Unsupported(_)) => {}
        other => panic!("{other:?}"),
    }
    let not_video = Video::open(&b"not a video at all"[..], VideoOptions::new());
    assert!(matches!(not_video, Err(VideoError::Unsupported(_))));
}

/// The platform's H.264 decoder here, and what it's called.
fn platform_decoder() -> Option<&'static str> {
    if cfg!(any(target_os = "macos", target_os = "ios")) {
        Some("VideoToolbox")
    } else if cfg!(target_os = "linux") {
        Some("VA-API")
    } else if cfg!(target_os = "windows") {
        Some("Media Foundation")
    } else if cfg!(target_os = "android") {
        Some("MediaCodec")
    } else {
        None
    }
}

/// The H.264 clip opened with the platform's decoder; `None` (the test
/// skips) where there's none, unless `STIPPLE_REQUIRE_VIDEO` says there
/// must be.
fn open_h264_stream() -> Option<Stream> {
    match Stream::open(Source::Bytes(H264.into()), true) {
        Ok(s) => {
            assert_eq!(Some(s.decoder_name()), platform_decoder());
            Some(s)
        }
        Err(VideoError::Unsupported(why))
            if std::env::var_os("STIPPLE_REQUIRE_VIDEO").is_none() =>
        {
            // A Linux machine without VA-API, a Mac VM without the media
            // engine, a Windows without Media Foundation.
            eprintln!("skipping: no H.264 decoder here: {why}");
            None
        }
        Err(e) => panic!("{e}"),
    }
}

/// Every frame of the H.264 clip in presentation order, at its time, the
/// right colors (on the CPU), each checked by `each` too; then seeking.
fn check_h264_stream(mut s: Stream, mut each: impl FnMut(&Arc<ExternalImage>)) {
    let mut times = Vec::new();
    while let Some(f) = s.next().unwrap() {
        each(&f.image);
        let cpu = on_cpu(&f.image);
        if f.time < Duration::from_millis(500) {
            assert_halves(&cpu, RED, BLUE, "an early frame");
        } else {
            assert_halves(&cpu, LIME, YELLOW, "a late frame");
        }
        times.push(f.time.as_millis());
    }
    assert_eq!(times, (0..10).map(|i| i * 100).collect::<Vec<_>>());
    let f = s.seek(Duration::from_millis(600)).unwrap().unwrap();
    assert_eq!(f.time, Duration::from_millis(600));
    assert_halves(&on_cpu(&f.image), LIME, YELLOW, "after seeking");
    let f = s.seek(Duration::from_millis(260)).unwrap().unwrap();
    assert_eq!(f.time, Duration::from_millis(300));
    assert_halves(&on_cpu(&f.image), RED, BLUE, "after seeking back");
    // Past the end: the last frame.
    let f = s.seek(Duration::from_secs(5)).unwrap().unwrap();
    assert_eq!(f.time, Duration::from_millis(900));
}

/// Elsewhere than Apple's platforms (whose test below checks the
/// surfaces), H.264 plays through the platform's decoder where there's
/// one: every frame, in order, the right colors, seeking.
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
#[test]
fn h264_decodes_through_the_platform_decoder() {
    if let Some(s) = open_h264_stream() {
        check_h264_stream(s, |_| {});
    }
}

/// A `Video` of the H.264 clip plays as the MPEG-2 one does: its first
/// frame, to the last frame in about its time, seeking after the end.
#[test]
fn an_h264_video_plays_to_the_end_and_seeks() {
    if open_h264_stream().is_none() {
        return;
    }
    let video = Video::open(H264, VideoOptions::new()).unwrap();
    assert_eq!(Some(video.decoder()), platform_decoder());
    plays_to_the_end_and_seeks(video);
}

/// While the renderer imports NV12 textures (Direct3D 11's), Media
/// Foundation's pictures are DXGI shared textures: uploaded once from the
/// software decoder's memory (or copied on the GPU from DXVA's) and drawn
/// by Direct3D 11 straight from them, the picture as the CPU draws it.
#[cfg(target_os = "windows")]
#[test]
fn media_foundation_hands_shared_textures_to_direct3d() {
    use crate::platform::external::{NATIVE_TEST, note_native};
    use crate::render::ExternalFormat;
    let _lock = NATIVE_TEST.lock().unwrap_or_else(|e| e.into_inner());
    note_native(&[ExternalFormat::Nv12]);
    let Some(mut s) = open_h264_stream() else {
        return;
    };
    #[cfg_attr(not(feature = "d3d"), allow(unused_mut))]
    let mut drawn = 0;
    loop {
        note_native(&[ExternalFormat::Nv12]);
        let Some(f) = s.next().unwrap() else {
            break;
        };
        assert_eq!(f.image.format(), ExternalFormat::Nv12);
        assert!(
            format!("{:?}", f.image).contains("DXGI shared handle"),
            "{:?}",
            f.image
        );
        assert_eq!(f.image.size(), PhysicalSize::new(160, 120));
        #[cfg(feature = "d3d")]
        if f.time.as_millis() % 400 == 0 {
            let scene = scene_of(&f.image);
            let gpu = match crate::gpu::render_scene_scaled(
                &scene,
                Color::BLACK,
                ScaleFactor::IDENTITY,
            ) {
                Ok(p) => p,
                Err(e) if std::env::var_os("STIPPLE_REQUIRE_GPU").is_none() => {
                    eprintln!("skipping the draw: no Direct3D 11 device: {e}");
                    continue;
                }
                Err(e) => panic!("{e}"),
            };
            assert_eq!(
                crate::gpu::stats().uploads,
                0,
                "the frame was uploaded, not imported"
            );
            let (left, right) = if f.time < Duration::from_millis(500) {
                (RED, BLUE)
            } else {
                (LIME, YELLOW)
            };
            assert_halves(&gpu, left, right, "Direct3D 11's frame");
            drawn += 1;
        }
    }
    eprintln!("Media Foundation: shared textures, {drawn} drawn by Direct3D 11");
    note_native(&[]);
}

/// While the renderer imports Android's YUV hardware buffers, MediaCodec's
/// pictures are the `AImageReader`'s `AHardwareBuffer`s, read on the CPU
/// here (their planes locked) with the right colors.
#[cfg(target_os = "android")]
#[test]
fn media_codec_hands_hardware_buffers_over() {
    use crate::platform::external::{NATIVE_TEST, note_native};
    use crate::render::ExternalFormat;
    let _lock = NATIVE_TEST.lock().unwrap_or_else(|e| e.into_inner());
    note_native(&[ExternalFormat::Yuv420]);
    let Some(mut s) = open_h264_stream() else {
        return;
    };
    let (mut buffers, mut frames) = (0, 0);
    #[cfg_attr(not(feature = "gl"), allow(unused_mut))]
    let mut drawn = 0;
    loop {
        note_native(&[ExternalFormat::Yuv420]);
        let Some(f) = s.next().unwrap() else {
            break;
        };
        frames += 1;
        if format!("{:?}", f.image).contains("AHardwareBuffer") {
            buffers += 1;
            assert_eq!(f.image.format(), ExternalFormat::Yuv420);
            let (left, right) = if f.time < Duration::from_millis(500) {
                (RED, BLUE)
            } else {
                (LIME, YELLOW)
            };
            assert_halves(&on_cpu(&f.image), left, right, "a hardware buffer");
            // OpenGL ES samples it as an external texture.
            #[cfg(feature = "gl")]
            if f.time.as_millis() % 400 == 0 {
                let scene = scene_of(&f.image);
                match crate::gpu::render_scene_scaled(&scene, Color::BLACK, ScaleFactor::IDENTITY) {
                    Ok(gpu) => {
                        assert_eq!(crate::gpu::stats().uploads, 0, "uploaded, not imported");
                        assert_halves(&gpu, left, right, "OpenGL ES's frame");
                        drawn += 1;
                    }
                    Err(e) if std::env::var_os("STIPPLE_REQUIRE_GPU").is_none() => {
                        eprintln!("skipping the draw: no OpenGL ES: {e}");
                    }
                    Err(e) => panic!("{e}"),
                }
            }
        }
    }
    eprintln!(
        "MediaCodec: {buffers} of {frames} frames as hardware buffers, {drawn} drawn on the GPU"
    );
    assert_eq!(frames, 10);
    note_native(&[]);
}

/// VideoToolbox decodes the H.264 clip into `IOSurface`s: NV12 buffers
/// (no copy made), drawn by the software renderer from the locked surface.
#[cfg(any(target_os = "macos", target_os = "ios"))]
#[test]
fn h264_decodes_through_videotoolbox_into_io_surfaces() {
    let Some(s) = open_h264_stream() else {
        return;
    };
    check_h264_stream(s, |image| {
        assert_eq!(image.format(), crate::render::ExternalFormat::Nv12);
        assert!(format!("{image:?}").contains("IOSurface"));
    });
}

/// Metal draws VideoToolbox's buffers straight from their `IOSurface`s:
/// nothing uploaded, the picture as the software renderer draws it.
#[cfg(all(target_os = "macos", feature = "mtl"))]
#[test]
fn metal_draws_videotoolbox_frames_without_a_copy() {
    let video = match Video::open(H264, VideoOptions::new()) {
        Ok(v) => v,
        Err(VideoError::Unsupported(why)) => {
            eprintln!("skipping: {why}");
            return;
        }
        Err(e) => panic!("{e}"),
    };
    for at in [0u64, 700] {
        video.seek(Duration::from_millis(at));
        wait_until("the frame", || {
            video.position() == Duration::from_millis(at) && video.frame().is_some()
        });
        let frame = video.frame().unwrap();
        let scene = scene_of(&frame.image);
        let gpu = match crate::gpu::render_scene_scaled(&scene, Color::BLACK, ScaleFactor::IDENTITY)
        {
            Ok(p) => p,
            Err(e) if std::env::var_os("STIPPLE_REQUIRE_GPU").is_none() => {
                eprintln!("skipping: no Metal device: {e}");
                return;
            }
            Err(e) => panic!("{e}"),
        };
        assert_eq!(
            crate::gpu::stats().uploads,
            0,
            "the frame was uploaded, not imported"
        );
        let (left, right) = if at == 0 { (RED, BLUE) } else { (LIME, YELLOW) };
        assert_halves(&gpu, left, right, "Metal's frame");
        let cpu = on_cpu(&frame.image);
        let mean = gpu
            .as_bytes()
            .iter()
            .zip(cpu.as_bytes())
            .map(|(a, b)| a.abs_diff(*b) as f64)
            .sum::<f64>()
            / gpu.as_bytes().len() as f64;
        assert!(mean < 3.0, "Metal and the CPU differ by {mean} on average");
    }
    // The same frame as pixels (a software decoder's) is an upload: the
    // count above means something.
    let pixels = Arc::new(ExternalImage::from_pixels(Arc::new(on_cpu(
        &video.frame().unwrap().image,
    ))));
    crate::gpu::render_scene_scaled(&scene_of(&pixels), Color::BLACK, ScaleFactor::IDENTITY)
        .unwrap();
    assert_eq!(crate::gpu::stats().uploads, 1);
}
