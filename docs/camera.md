# The camera and runtime permissions

`stipple::camera` opens a camera, shows a live preview in the layout, and
hands every frame to the app on a capture thread: what a barcode scanner, a
document scanner or a video call needs. `stipple::platform::permission` asks
the user for the camera first (and for the microphone, notifications, the
photo library and the location). The `cameraview` example puts the two
together:

```sh
cargo run --example cameraview
```

<!-- live: cameraview -->

## Permissions

```rust,no_run
use stipple::platform::permission::{self, Permission, PermissionStatus};

match permission::status(Permission::Camera) {
    PermissionStatus::Granted => { /* open the camera */ }
    PermissionStatus::NotDetermined => {
        // At a moment that makes sense to the user: the system's prompt.
        permission::request(Permission::Camera, |status| println!("{status:?}"));
    }
    // Denied (or Restricted): asking again shows nothing. Only the settings
    // can change it.
    _ => { let _ = permission::open_settings(); }
}
```

From a view, `AppHandle::request_permission` asks and applies the answer to
the state on the UI thread:

```rust,no_run
# use stipple::prelude::*;
# use stipple::platform::permission::Permission;
# struct State { camera_allowed: bool }
# fn view(_: &State, cx: &mut Cx<State>) -> Element {
let handle = cx.handle();
button(cx, "Allow the camera").on_tap(cx, move |_| {
    handle.request_permission(Permission::Camera, |s, status| {
        s.camera_allowed = status.is_granted();
    });
})
# }
```

`status` answers at once; `request` shows the prompt only if the user hasn't
decided yet, and otherwise answers with what they said. `PermissionStatus` is
`Granted`, `Denied`, `NotDetermined` (not asked yet) or `Restricted` (not the
user's to give: parental controls, a device policy).

| | Camera, microphone | Notifications | Photos | Location | `open_settings` |
|---|---|---|---|---|---|
| iOS | `AVCaptureDevice` | `UNUserNotificationCenter` | `PHPhotoLibrary` (read and write) | `CLLocationManager`, while in use | the app's page in Settings |
| Android | `CAMERA`, `RECORD_AUDIO` | `POST_NOTIFICATIONS` (13+) | `READ_MEDIA_IMAGES` (13+), else `READ_EXTERNAL_STORAGE` | `ACCESS_FINE_LOCATION` or `ACCESS_COARSE_LOCATION` | the app's details page |
| Web | the Permissions API; asking opens a `getUserMedia` stream and stops it | `Notification` | granted | the Permissions API; asking reads the position once | `Err` |
| macOS | `AVCaptureDevice` | granted | granted | granted | the Privacy pane |
| Windows, Linux | granted | granted | granted | granted | Windows' privacy settings; `Err` on Linux |

Where the platform gates nothing the answer is `Granted`.

**Declare what you ask for**, or the request fails, and on iOS and macOS the
process is killed when it touches the camera:

- iOS and macOS: a usage string in `Info.plist` for each:
  `NSCameraUsageDescription`, `NSMicrophoneUsageDescription`,
  `NSPhotoLibraryUsageDescription`, `NSLocationWhenInUseUsageDescription`. A
  binary run from a terminal on macOS is covered by the terminal's (the
  prompt names the terminal).
- Android: `<uses-permission android:name="android.permission.CAMERA"/>` and
  the others in the manifest.

`cargo-stipple` writes both from the app's `Cargo.toml`
(`usage = { camera = "To scan QR codes." }` and
`permissions = ["CAMERA"]`); see [packaging.md](packaging.md).

**Android** has no `onRequestPermissionsResult` on a `NativeActivity`: the
backend asks from a headless fragment it adds to the activity (the same one
the file pickers use), and the answer comes back through it. Android doesn't
say whether a refused permission was ever asked for, so Stipple remembers what
it asked (a small file in the app's data directory): not asked is
`NotDetermined`, asked and refused is `Denied`. Android stops showing the
prompt after the second refusal.

**The web** answers `status` from what the Permissions API last said (it asks
once at start and follows changes), so just after the page loads it can still
be `NotDetermined`. Safari doesn't know the `camera` name; there it stays
`NotDetermined` until the camera is opened or asked for.

## The camera

```rust,no_run
use stipple::camera::{Camera, CameraOptions, Lens};

let camera = Camera::open(CameraOptions::new().lens(Lens::Back).resolution(1920, 1080))?;
camera.on_frame(|frame| {
    // On the capture thread. The planes borrow the camera's buffer.
    if let Some(y) = frame.luma() {
        // y.data, y.row_stride: the luminance, for a decoder.
        let _ = (y, frame.size, frame.rotation, frame.timestamp);
    }
});
# Ok::<(), stipple::camera::CameraError>(())
```

`Camera::open` needs the camera permission granted (`NotPermitted`
otherwise), except on the web, where the browser asks as it opens. It starts
capturing at once; `stop` and `start` pause and resume it (the camera's light
goes off). It pauses by itself while the app is in the background
(`Lifecycle::Background`) and starts again when it comes back. It closes when
the last clone of the `Camera` is dropped. `Camera::lenses()` lists the kinds
of camera there are (empty on the iOS simulator), and `status()` says whether
it's `Starting`, `Running`, `Paused`, `Stopped` or `Failed`.

### Frames

`on_frame`'s callback gets every frame on the capture thread (the page's
thread on the web), as a `Frame`:

| | |
|---|---|
| `size` | width and height, as the sensor delivered it (landscape on a phone) |
| `format` | `Yuv420` on iOS, macOS and Android; `Rgba` on the web |
| `planes` | Y, U and V for YUV, each with its `row_stride` and `pixel_stride` (2 for interleaved chroma: NV12 on iOS and macOS, most Android devices); one plane for RGBA |
| `timestamp` | on the camera's clock |
| `rotation` | degrees to turn it clockwise to see it upright on the screen as it's turned now |
| `mirrored` | whether the preview mirrors it (a camera facing the user) |

The planes borrow the camera's buffer: nothing is copied, and the buffer goes
back to the camera when the callback returns. A decoder slower than the frame
rate makes the camera drop the frames in between (iOS's
`alwaysDiscardsLateVideoFrames`, Android's `acquireLatestImage`). Copy what
must outlive the call; `frame.to_pixmap()` is an upright RGBA copy, for a
snapshot.

YUV is full range (BT.601). On iOS the front camera's buffers come mirrored
already; `mirrored` accounts for that (the preview doesn't mirror them again).

### The preview

```rust,no_run
# use stipple::prelude::*;
# use stipple::camera::{Camera, camera_preview};
# struct State { camera: Camera }
fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let camera = state.camera.clone();
    Element::layers(vec![
        camera_preview(cx, &state.camera).on_pointer(cx, move |ev, _| {
            // Tap to focus.
            if ev.kind == PointerEventKind::Down {
                let _ = camera.focus_at_preview(ev.position, ev.size);
            }
        }),
        // ...buttons and a scan frame drawn over it.
    ])
}
```

`camera_preview(cx, &camera)` is an element showing the latest frame,
upright, scaled to cover its box (centered, the overflow cropped); it grows
to fill its parent unless sized. It repaints by itself as frames come,
without the view being rebuilt.

`camera.preview_mapping(size)` maps between the frame's pixels and the
preview's coordinates (`to_preview`, `to_frame`), through the rotation, the
mirroring and the crop: to draw over what a decoder found, or to turn a tap
into a focus point (`focus_at_preview` does that).

**What it costs.** This first version draws the preview through Stipple's
renderer. Each frame is converted, on the capture thread, to an upright RGBA
image no bigger than the element's size in pixels and at most 960 pixels on
its long side (nearest-neighbor sampling: about 1.5 ms for 960×540, and
5.5 ms for a whole 1920×1080 frame, on an M-series Mac in a release build),
and the element alone repaints, scaled up to fill. A frame
that arrives before the last one was painted isn't converted, so the preview
runs at whatever rate the renderer keeps up with (with the software renderer
on a phone, below the camera's 30 fps full screen), while `on_frame` still
sees every frame. A path with no copy (the camera's buffer imported as a GPU
texture) is taken while the window's renderer imports the camera's format
(see [gpu.md](gpu.md#external-images-without-a-copy)):

- macOS and iOS, Metal: each frame's `CVPixelBuffer` goes to the preview as
  it is (its `IOSurface`, retained while drawn), and the GPU samples its two
  planes and converts them in the shader.
- Android, GLES: the `AImageReader` is made with `AImageReader_newWithUsage`
  so the GPU can sample its images as well as the CPU read them, and each
  image's `AHardwareBuffer` (`Y8Cb8Cr8_420`, a layout only the driver knows)
  goes to the preview, imported through `eglGetNativeClientBufferANDROID`
  as a `GL_TEXTURE_EXTERNAL_OES` texture that the driver converts as it
  samples; the `AImage` stays out of the reader until the preview lets go
  of it. (The Vulkan renderer doesn't import it: the preview converts there.)
- The web, WebGPU or WebGL2: the camera's `<video>` itself (WebGPU imports
  it as a `texture_external`, WebGL2 hands it to `texImage2D`), and when the
  app has no `on_frame` callback the page doesn't read its pixels back at
  all.

Nothing is converted on the capture thread then; `on_frame` still gets the
frame's planes (on the web, read back only when there is a callback). These
paths are compile-checked; none has run on a device yet. A native preview
layer under the window is still to come.

### Controls

| | |
|---|---|
| `set_torch(on)`, `torch()`, `has_torch()` | the flash, kept on |
| `set_zoom(f)`, `zoom()`, `zoom_range()` | zoom factor; on an iPhone with an ultra-wide lens the range starts below 1 |
| `focus_at(point)`, `focus_at_preview(point, size)`, `can_focus()` | focus and expose at a point (`0..1` of the frame), then keep focusing continuously around it |
| `set_lens(lens)`, `lens()` | `Back`, `Front`, `External`: the camera closes and reopens on the other lens |

Each returns `CameraError::NotAvailable` (or does nothing, for the zoom) where
the lens can't.

## Per platform

| | Capture | Frames | Torch | Zoom | Focus point |
|---|---|---|---|---|---|
| iOS | `AVCaptureSession`, `AVCaptureVideoDataOutput` on a dispatch queue | NV12, the `CVPixelBuffer` locked | `torchMode` | `videoZoomFactor` | `focusPointOfInterest` |
| macOS | the same | NV12 | where the camera has one | where it zooms | where it focuses |
| Android | Camera2 in the NDK (`libcamera2ndk`, `ACameraCaptureSession`) into an `AImageReader` (`libmediandk`); no Java | `YUV_420_888`, the `AImage` planes | `FLASH_MODE_TORCH` | `SCALER_CROP_REGION` | `CONTROL_AF_REGIONS` |
| Web | `getUserMedia`, a hidden `<video>` drawn into a canvas on each new frame (`requestVideoFrameCallback`) | RGBA | `applyConstraints({torch})` where offered | `zoom` where offered | `pointsOfInterest` where offered |
| Windows, Linux | not yet: `Camera::open` returns `Unsupported` | | | | |

- **iOS.** The back lens prefers the virtual triple or dual-wide camera,
  which switches to the ultra-wide (macro) lens by itself when focusing
  close; zoom 1 is the wide lens. The frameworks are loaded when first used.
  The simulator has no camera: `lenses()` is empty and `open` says
  `NoCamera`. The rotation follows the interface orientation.
- **macOS.** The built-in camera counts as `Front` (its preview mirrored),
  others (USB, Continuity Camera) as `External`.
- **Android.** Needs Android 7 (API 24) for the NDK camera. The frame size is
  the `YUV_420_888` output closest to the one asked for; the rotation comes
  from the sensor's orientation and the display's rotation.
- **Web.** The page must be served over HTTPS (or from `localhost`). Frames
  are read back from a canvas, which costs a copy per frame; the browser
  turns the stream upright itself.

## What CI checks

- macOS: none (no camera on the runners). Run
  `CAMERAVIEW_SELFTEST=1 cargo run --example cameraview` on a Mac with a
  camera: it opens it and passes once 10 frames with a picture arrived.
- iOS simulator: `cameraview`'s self-test checks there's no camera and that
  opening says so cleanly, without a crash.
- Android emulator (non-blocking): the APK built from `cameraview_android`,
  the permission granted with `pm grant`, and frames from the emulator's
  virtual camera that aren't blank read back from logcat.
- The frame conversion (rotation, mirroring, color), the preview's size and
  the frame-to-preview mapping are unit-tested on every host.
