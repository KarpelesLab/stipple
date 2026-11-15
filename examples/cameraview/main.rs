//! A viewfinder: the camera's live preview filling the window, with buttons
//! over it for the torch, the zoom and the lens, and a frame counter. A tap
//! on the preview focuses there; a pinch zooms. Every frame also reaches a
//! callback on the capture thread, which measures its brightness the way a
//! barcode decoder would read it (the luminance plane, borrowed, not copied).
//!
//! It asks for the camera permission first (`platform::permission`), and
//! offers the system settings once refused.
//!
//! Runs on iOS, Android, macOS and in the browser; Windows and Linux have no
//! camera support yet, which it says.
//!
//! **Self-test.** With `CAMERAVIEW_SELFTEST` set (`SIMCTL_CHILD_` on the iOS
//! simulator), it logs the lenses and the permission, opens the back camera,
//! and ends with `cameraview: selftest passed` once 10 frames that aren't
//! blank arrived, or once opening failed cleanly where there's no camera
//! (the iOS simulator), else `cameraview: selftest FAILED: ...`. It logs to
//! stdout and, on iOS, to `cameraview.log` in the app's tmp directory. On
//! Android (no environment to read) it always logs a line every 30 frames:
//! `cameraview: frame N WxH luma mean M spread S`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use stipple::camera::{Camera, CameraOptions, CameraStatus, Lens, camera_preview};
use stipple::core::PinchPhase;
use stipple::platform::permission::{self, Permission, PermissionStatus};
use stipple::prelude::*;

#[derive(Default)]
struct State {
    camera: Option<Camera>,
    permission: Option<PermissionStatus>,
    error: Option<String>,
    frames: u64,
    /// Frames that weren't blank, from the callback.
    lively: Arc<AtomicU64>,
    /// The zoom when the pinch began.
    pinch_from: f32,
}

pub fn log(line: &str) {
    println!("{line}");
    if cfg!(target_os = "ios") {
        use std::io::Write;
        let path = std::env::temp_dir().join("cameraview.log");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{line}");
        }
    }
}

fn selftest() -> bool {
    !cfg!(target_arch = "wasm32") && std::env::var_os("CAMERAVIEW_SELFTEST").is_some()
}

/// The luminance's mean and spread (max - min) over a sparse grid.
fn luma_stats(frame: &stipple::camera::Frame<'_>) -> Option<(u32, u32)> {
    let y = frame.luma()?;
    let (w, h) = (frame.size.width as usize, frame.size.height as usize);
    let (mut sum, mut n, mut lo, mut hi) = (0u64, 0u64, 255u8, 0u8);
    for gy in 0..16 {
        for gx in 0..16 {
            let v = y.at(gx * w / 16, gy * h / 16)?;
            sum += u64::from(v);
            n += 1;
            lo = lo.min(v);
            hi = hi.max(v);
        }
    }
    Some(((sum / n.max(1)) as u32, u32::from(hi - lo)))
}

/// Open the back camera and start counting its frames.
fn open(state: &mut State) {
    match Camera::open(CameraOptions::new().lens(Lens::Back).resolution(1280, 720)) {
        Ok(camera) => {
            let lively = state.lively.clone();
            let mut n = 0u64;
            camera.on_frame(move |frame| {
                n += 1;
                let stats = luma_stats(frame);
                // Not blank: some contrast in the picture (the web's RGBA
                // frames count as they come).
                if stats.is_none_or(|(_, spread)| spread > 8) {
                    lively.fetch_add(1, Ordering::Relaxed);
                }
                if n % 30 == 1 {
                    let (mean, spread) = stats.unwrap_or((0, 0));
                    log(&format!(
                        "cameraview: frame {n} {}x{} rotation {} luma mean {mean} spread {spread}",
                        frame.size.width, frame.size.height, frame.rotation
                    ));
                }
            });
            log(&format!(
                "cameraview: opened {:?}, torch {}, zoom {:?}, focus {}",
                camera.lens(),
                camera.has_torch(),
                camera.zoom_range(),
                camera.can_focus()
            ));
            state.camera = Some(camera);
            state.error = None;
        }
        Err(e) => {
            log(&format!("cameraview: open failed: {e:?}"));
            state.error = Some(e.to_string());
        }
    }
}

fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    let Some(camera) = state.camera.clone() else {
        return no_camera(state, cx);
    };
    let tap = camera.clone();
    let pinch = camera.clone();
    let preview = camera_preview(cx, &camera)
        .on_pointer(cx, move |ev, _| {
            if ev.kind == PointerEventKind::Down {
                let _ = tap.focus_at_preview(ev.position, ev.size);
            }
        })
        .on_pinch(cx, move |ev, s: &mut State| {
            if ev.phase == PinchPhase::Began {
                s.pinch_from = pinch.zoom();
            }
            pinch.set_zoom(s.pinch_from * ev.scale as f32);
        });
    let status = match camera.status() {
        CameraStatus::Failed(e) => format!("Failed: {e}"),
        other => format!("{other:?}"),
    };
    let info = format!("{status} · {} frames · {:.1}×", state.frames, camera.zoom());
    let (lo, hi) = camera.zoom_range();
    let torch = camera.clone();
    let zoom_in = camera.clone();
    let zoom_out = camera.clone();
    let lens = camera.clone();
    let mut buttons = Vec::new();
    if camera.has_torch() {
        let label = if camera.torch() {
            "Torch off"
        } else {
            "Torch on"
        };
        buttons.push(button(cx, label).on_tap(cx, move |_| {
            let _ = torch.set_torch(!torch.torch());
        }));
    }
    if hi > lo {
        buttons.push(button(cx, "−").on_tap(cx, move |_| {
            zoom_out.set_zoom(zoom_out.zoom() / 1.5);
        }));
        buttons.push(button(cx, "+").on_tap(cx, move |_| {
            zoom_in.set_zoom(zoom_in.zoom() * 1.5);
        }));
    }
    if Camera::lenses().len() > 1 {
        buttons.push(button(cx, "Switch lens").on_tap(cx, move |s: &mut State| {
            // The next kind of camera there is, round.
            let lenses = Camera::lenses();
            let at = lenses.iter().position(|&l| l == lens.lens()).unwrap_or(0);
            let next = lenses[(at + 1) % lenses.len()];
            if let Err(e) = lens.set_lens(next) {
                s.error = Some(e.to_string());
            }
        }));
    }
    let safe = cx.safe_area();
    let bar = column(vec![label(cx, info), row(buttons).gap(theme.spacing.sm)])
        .gap(theme.spacing.sm)
        .padding(Insets::new(
            theme.spacing.md,
            theme.spacing.md + safe.right,
            theme.spacing.md + safe.bottom,
            theme.spacing.md + safe.left,
        ))
        .fill(theme.palette.surface.with_alpha(220));
    Element::layers(vec![
        preview,
        column(vec![spacer(), bar]).align(Align::Start, Align::Stretch),
    ])
    .grow(1.0)
}

/// What shows before the camera is open: asking for the permission, or why
/// there's no camera.
fn no_camera(state: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    let mut items = vec![heading(cx, "Camera")];
    match state.permission {
        Some(PermissionStatus::NotDetermined) | None => {
            items.push(label(cx, "The camera needs your permission."));
            let handle = cx.handle();
            items.push(button(cx, "Allow the camera").on_tap(cx, move |_| ask(&handle)));
        }
        Some(PermissionStatus::Granted) => {}
        Some(other) => {
            items.push(label(cx, format!("The camera permission is {other:?}.")));
            items.push(button(cx, "Open the settings").on_tap(cx, |_| {
                let _ = permission::open_settings();
            }));
        }
    }
    if let Some(e) = &state.error {
        items.push(label(cx, e.clone()));
    }
    column(items)
        .gap(theme.spacing.md)
        .padding(Insets::uniform(theme.spacing.lg))
        .grow(1.0)
}

/// Ask for the camera, and open it once granted.
fn ask(handle: &AppHandle<State>) {
    handle.request_permission(Permission::Camera, |s, status| {
        log(&format!("cameraview: permission answered {status:?}"));
        s.permission = Some(status);
        if status.is_granted() {
            open(s);
        }
    });
}

pub fn main() {
    let mut state = State::default();
    let status = permission::status(Permission::Camera);
    log(&format!(
        "cameraview: lenses {:?}, permission {status:?}",
        Camera::lenses()
    ));
    state.permission = Some(status);
    let lenses = Camera::lenses();
    let mut ask_now = false;
    // On the web the browser asks as the camera opens.
    if status.is_granted() || cfg!(target_arch = "wasm32") {
        open(&mut state);
    } else if status == PermissionStatus::NotDetermined && !lenses.is_empty() && selftest() {
        ask_now = true;
    } else if selftest() {
        // Where there's no camera: opening must say so, cleanly.
        open(&mut state);
    } else if lenses.is_empty() {
        state.error = Some("There's no camera here.".into());
    }
    let app = App::new(state, view)
        .title("Stipple Camera")
        .logical_size(Size::new(480.0, 640.0))
        .edge_to_edge(true)
        .on_lifecycle(|_, change| log(&format!("cameraview: lifecycle {change:?}")));
    let handle = app.handle();
    if ask_now {
        ask(&handle);
    }
    // The frame counter, twice a second.
    handle.every(Duration::from_millis(500), |s: &mut State| {
        s.frames = s.camera.as_ref().map_or(0, Camera::frame_count);
    });
    if selftest() {
        run_selftest(&handle);
    }
    app.run();
}

/// Check frames arrive (or that there's cleanly no camera), then quit.
fn run_selftest(handle: &AppHandle<State>) {
    let started = std::time::Instant::now();
    let h = handle.clone();
    handle.every(Duration::from_millis(250), move |s: &mut State| {
        let lively = s.lively.load(Ordering::Relaxed);
        let verdict = match (&s.camera, &s.error) {
            // A snapshot waits for the exposure to settle.
            _ if lively
                >= if std::env::var_os("CAMERAVIEW_SNAPSHOT").is_some() {
                    60
                } else {
                    10
                } =>
            {
                Some(Ok(format!("{lively} frames with a picture")))
            }
            (None, Some(_)) if Camera::lenses().is_empty() => {
                Some(Ok("no camera, and opening said so".to_string()))
            }
            (None, Some(e)) => Some(Err(format!("opening failed: {e}"))),
            (Some(c), _) if started.elapsed() > Duration::from_secs(20) => Some(Err(format!(
                "{} frames, {lively} with a picture, status {:?}",
                c.frame_count(),
                c.status()
            ))),
            (None, None) if started.elapsed() > Duration::from_secs(20) => {
                Some(Err(format!("never opened (permission {:?})", s.permission)))
            }
            _ => None,
        };
        match verdict {
            Some(Ok(why)) => {
                log(&format!("cameraview: selftest passed ({why})"));
                // The preview as last shown, for a look.
                if let Some(path) = std::env::var_os("CAMERAVIEW_SNAPSHOT")
                    && let Some(image) = s.camera.as_ref().and_then(Camera::preview_image)
                {
                    let _ = image.save_png(path);
                }
                s.camera = None;
                h.exit();
            }
            Some(Err(why)) => {
                log(&format!("cameraview: selftest FAILED: {why}"));
                s.camera = None;
                h.exit();
            }
            None => {}
        }
    });
}

stipple::web_app!(main);
