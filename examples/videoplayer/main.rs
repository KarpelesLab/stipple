//! A video player: a clip (or the file given) playing in a `video_view`,
//! with play and pause, a seek bar, looping, and what decodes it.
//!
//! ```sh
//! cargo run --example videoplayer --features video            # the bundled clip
//! cargo run --example videoplayer --features video -- FILE    # an MP4 or Matroska file
//! cargo run --example videoplayer --features video,mtl        # drawn by Metal (macOS)
//! cargo run --example videoplayer --features video -- --selftest [DIR]
//! ```
//!
//! The bundled clip is H.264 in MP4 (`clip.mp4`), which VideoToolbox decodes
//! on macOS and iOS (its buffers drawn by Metal without a copy) and VA-API on
//! Linux; where neither can, the same clip in MPEG-2 (`clip.mkv`) plays
//! through oxideav's software decoder. In the browser, the MP4 plays in a
//! `<video>` the GPU renderer draws from.
//!
//! **Self-test** (`--selftest`): off screen, each clip this platform plays
//! (the MPEG-2 one everywhere) must show its first frame in the window (the
//! picture's middle not black, and colorful), then play: its position has to
//! move on. Prints `videoplayer: selftest ok (...)` with the decoders used;
//! with a directory, writes each clip's window there as a PNG.

use std::time::Duration;
use stipple::prelude::*;
use stipple::video::{Video, VideoError, VideoOptions, VideoStatus, video_view};

const CLIP_H264: &[u8] = include_bytes!("clip.mp4");
#[cfg(not(target_arch = "wasm32"))]
const CLIP_MPEG2: &[u8] = include_bytes!("clip.mkv");
/// The browser plays the MP4 (and no browser plays MPEG-2).
#[cfg(target_arch = "wasm32")]
const CLIP_MPEG2: &[u8] = &[];

struct State {
    video: Option<Video>,
    error: Option<String>,
    /// The frame showing's time (from the video's frame callback).
    position: Duration,
}

/// The file given, else the H.264 clip, else (no hardware decoder) the
/// MPEG-2 one.
fn open(path: Option<&str>, options: VideoOptions) -> Result<Video, VideoError> {
    if let Some(path) = path {
        return Video::open_file(path, options);
    }
    match Video::open(CLIP_H264, options) {
        Err(VideoError::Unsupported(why)) if !cfg!(target_arch = "wasm32") => {
            println!("videoplayer: H.264 unsupported here ({why}); playing the MPEG-2 clip");
            Video::open(CLIP_MPEG2, options)
        }
        other => other,
    }
}

fn secs(d: Duration) -> String {
    format!("{:.1}", d.as_secs_f64())
}

fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    let Some(video) = state.video.clone() else {
        return column(vec![
            heading(cx, "Video"),
            label(
                cx,
                state.error.clone().unwrap_or_else(|| "No video.".into()),
            ),
        ])
        .gap(theme.spacing.md)
        .padding(Insets::uniform(theme.spacing.lg))
        .grow(1.0);
    };
    let duration = video.duration().unwrap_or(Duration::ZERO);
    let fraction = if duration.is_zero() {
        0.0
    } else {
        state.position.as_secs_f64() / duration.as_secs_f64()
    };
    let toggle = video.clone();
    let seek = video.clone();
    let looping = video.clone();
    let size = video.size();
    let status = match video.status() {
        VideoStatus::Failed(e) => format!("failed: {e}"),
        other => format!("{other:?}").to_lowercase(),
    };
    let info = format!(
        "{} / {} s · {}×{} · {} · {status}",
        secs(state.position),
        secs(duration),
        size.width,
        size.height,
        video.decoder(),
    );
    let play_label = if video.is_playing() { "Pause" } else { "Play" };
    column(vec![
        video_view(cx, &video),
        row(vec![
            button(cx, play_label).on_tap(cx, move |_| {
                if toggle.is_playing() {
                    toggle.pause();
                } else {
                    toggle.play();
                }
            }),
            slider(cx, fraction, move |_, f| {
                seek.seek(duration.mul_f64(f.clamp(0.0, 1.0)));
            })
            .a11y_label("Position")
            .grow(1.0),
            checkbox(cx, "Loop", video.is_looping(), move |_| {
                looping.set_looping(!looping.is_looping());
            }),
        ])
        .gap(theme.spacing.md)
        .align(Align::Center, Align::Center),
        label(cx, info),
    ])
    .gap(theme.spacing.sm)
    .padding(Insets::uniform(theme.spacing.md))
    .grow(1.0)
}

fn app(path: Option<&str>, options: VideoOptions) -> App<State> {
    let (video, error) = match open(path, options) {
        Ok(v) => (Some(v), None),
        Err(e) => (None, Some(e.to_string())),
    };
    let app = App::new(
        State {
            video: video.clone(),
            error,
            position: Duration::ZERO,
        },
        view,
    )
    .title("Stipple Video")
    .logical_size(Size::new(560.0, 420.0));
    if let Some(video) = video {
        let handle = app.handle();
        video.on_frame(move |f| {
            let t = f.timestamp;
            handle.update(move |s| s.position = t);
        });
    }
    app
}

pub fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--selftest") {
        let out = args
            .iter()
            .find(|a| !a.starts_with("--"))
            .map(String::as_str);
        match selftest(out) {
            Ok(used) => println!("videoplayer: selftest ok ({used})"),
            Err(e) => {
                println!("videoplayer: selftest FAILED: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    let path = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .map(String::as_str);
    let app = app(path, VideoOptions::new().autoplay(true).looping(true));
    // Built with a GPU backend, drawn with it: VideoToolbox's buffers then
    // reach Metal as they are.
    #[cfg(any(feature = "mtl", feature = "vk", feature = "gl", feature = "d3d"))]
    let app = app.renderer(stipple::render::Renderer::Gpu);
    app.run();
}

/// Each clip this platform plays shows in the window and plays.
fn selftest(png_dir: Option<&str>) -> Result<String, String> {
    stipple::anim::set_reduced_motion(Some(true));
    let mut used = Vec::new();
    for (name, clip) in [("MPEG-2", CLIP_MPEG2), ("H.264", CLIP_H264)] {
        let video = match Video::open(clip, VideoOptions::new()) {
            Ok(v) => v,
            Err(VideoError::Unsupported(why)) if name == "H.264" => {
                println!("videoplayer: H.264 skipped: {why}");
                continue;
            }
            Err(e) => return Err(format!("{name}: {e}")),
        };
        let mut app = App::new(
            State {
                video: Some(video.clone()),
                error: None,
                position: Duration::ZERO,
            },
            view,
        )
        .logical_size(Size::new(560.0, 420.0));
        wait(&format!("{name}'s first frame"), || video.frame().is_some())?;
        let frame = app.render_once();
        check_picture(&frame).map_err(|e| format!("{name}: {e}"))?;
        video.play();
        wait(&format!("{name} to play"), || {
            video.position() >= Duration::from_millis(500)
        })?;
        if let VideoStatus::Failed(e) = video.status() {
            return Err(format!("{name}: {e}"));
        }
        let frame = app.render_once();
        check_picture(&frame).map_err(|e| format!("{name}, playing: {e}"))?;
        if let Some(dir) = png_dir {
            let path = std::path::Path::new(dir).join(format!(
                "videoplayer-{}.png",
                name.replace(['.', '-'], "").to_lowercase()
            ));
            frame
                .save_png(&path)
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }
        used.push(format!("{name}: {}", video.decoder()));
    }
    Ok(used.join(", "))
}

fn wait(what: &str, mut ok: impl FnMut() -> bool) -> Result<(), String> {
    let start = std::time::Instant::now();
    while !ok() {
        if start.elapsed() > Duration::from_secs(20) {
            return Err(format!("timed out waiting for {what}"));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

/// The video's area (the upper middle of the window) shows a picture: not
/// black, and of several colors (the test pattern's).
fn check_picture(frame: &Pixmap) -> Result<(), String> {
    let (w, h) = (frame.size().width, frame.size().height);
    let mut colors = std::collections::HashSet::new();
    let mut lit = 0;
    let mut n = 0;
    for gy in 1..8 {
        for gx in 1..8 {
            let (x, y) = (w * gx / 8, h * gy / 16 + h / 8);
            let Some(px) = frame.pixel(x, y) else {
                continue;
            };
            n += 1;
            if px[0] as u32 + px[1] as u32 + px[2] as u32 > 60 {
                lit += 1;
            }
            colors.insert([px[0] / 64, px[1] / 64, px[2] / 64]);
        }
    }
    if lit * 2 < n || colors.len() < 4 {
        return Err(format!(
            "no picture: {lit} of {n} samples lit, {} colors",
            colors.len()
        ));
    }
    Ok(())
}

stipple::web_app!(main);
