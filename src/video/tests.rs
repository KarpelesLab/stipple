//! Playing the committed clips: one second at 10 frames per second, 160×120,
//! red on the left and blue on the right for half a second, then lime and
//! yellow. `bars-mpeg2.mkv` (MPEG-2 in Matroska, closed GOPs, B-frames)
//! decodes in software everywhere; `bars-h264.mp4` (H.264 Main with
//! B-frames, in MP4) needs VideoToolbox or VA-API.

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
fn h264_needs_a_hardware_decoder() {
    match Video::open(H264, VideoOptions::new().hardware(false)) {
        Err(VideoError::Unsupported(_)) => {}
        other => panic!("{other:?}"),
    }
    let not_video = Video::open(&b"not a video at all"[..], VideoOptions::new());
    assert!(matches!(not_video, Err(VideoError::Unsupported(_))));
}

/// Elsewhere than Apple's platforms, H.264 plays through VA-API where
/// there's a device that decodes it, else it's unsupported.
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
#[test]
fn h264_plays_through_va_api_or_says_unsupported() {
    match Video::open(H264, VideoOptions::new()) {
        Ok(video) => {
            assert_eq!(video.decoder(), "VA-API");
            wait_until("the first frame", || video.frame().is_some());
            let f = video.frame().unwrap();
            assert_halves(&on_cpu(&f.image), RED, BLUE, "VA-API's first frame");
        }
        Err(VideoError::Unsupported(why)) => eprintln!("no hardware H.264 here: {why}"),
        Err(e) => panic!("{e}"),
    }
}

/// VideoToolbox decodes the H.264 clip into `IOSurface`s: NV12 buffers
/// (no copy made), drawn by the software renderer from the locked surface.
#[cfg(any(target_os = "macos", target_os = "ios"))]
#[test]
fn h264_decodes_through_videotoolbox_into_io_surfaces() {
    let mut s = match Stream::open(Source::Bytes(H264.into()), true) {
        Ok(s) => s,
        Err(VideoError::Unsupported(why)) => {
            // A virtual machine without the media engine.
            eprintln!("skipping: {why}");
            return;
        }
        Err(e) => panic!("{e}"),
    };
    assert_eq!(s.decoder_name(), "VideoToolbox");
    let mut times = Vec::new();
    while let Some(f) = s.next().unwrap() {
        assert_eq!(f.image.format(), crate::render::ExternalFormat::Nv12);
        assert!(format!("{:?}", f.image).contains("IOSurface"));
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
