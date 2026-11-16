//! The web backend: a `wasm32-unknown-unknown` module driven by a page script.
//!
//! A browser can't block in an event loop, so `run` doesn't loop: it keeps
//! the App's event handler and returns, and the page calls into the module
//! (the exports [`web_app!`](crate::web_app) generates, which land in the
//! `#[doc(hidden)]` functions at the bottom of this file) for each DOM event.
//! Each call turns into an [`Event`] for the handler, exactly as a desktop
//! backend's loop would deliver it.
//!
//! The other direction goes through a handful of functions the module imports
//! from the page, under the import module `"stipple"`. `web/stipple.js` in the
//! repository implements them:
//!
//! | import | does |
//! |---|---|
//! | `now() -> f64` | `performance.now()`, the clock behind [`time::Instant`](crate::platform::time::Instant) |
//! | `present(ptr, w, h, rects, n)` | `putImageData` of the RGBA frame, limited to `n` damage rects (`x, y, w, h` as `u32`s) unless `n` is 0 |
//! | `schedule(ms)` | one `setTimeout` that calls `stipple_wake`; replaces the previous one, and a negative `ms` just cancels it |
//! | `request_frame()` | `requestAnimationFrame`, which calls `stipple_frame` |
//! | `set_cursor(ptr, len)` | the canvas's `style.cursor` |
//! | `pointer_lock(on)` | `canvas.requestPointerLock` (unadjusted movement where the browser has it) or `document.exitPointerLock`; motion comes back as `stipple_pointer` kind 4, the lock ending as kind 5 |
//! | `set_title(ptr, len)` | `document.title` |
//! | `clipboard_write(ptr, len)` | `navigator.clipboard.writeText` (or `write`, with the HTML), and the text a `copy` / `cut` event hands over |
//! | `clipboard_write_html(ptr, len)` | the HTML (empty: none) that goes with the next `clipboard_write` |
//! | `open_file(id, ptr, len)` | an `<input type=file>` (`accept` = the string); the file comes back through `stipple_file` |
//! | `save_file(name, name_len, data, data_len)` | a download of a `Blob` |
//! | `log(level, ptr, len)` | `console.log` / `console.error` (panics land here) |
//! | `set_a11y(ptr, len)` | the ARIA overlay, from the accessibility tree as JSON (see [`aria_json`](crate::platform::access::aria_json)) |
//! | `set_ime(active, x, y, w, h)` | moves the hidden `<textarea>` that takes composition to the caret (CSS pixels) |
//! | `set_text_input(active)` | the textarea's `inputmode`, which shows or hides a touch keyboard |
//! | `set_input_hints(ptr, len)` | the focused field's keyboard hints, as `name=value;…` attributes for the textarea (`inputmode`, `enterkeyhint`, `autocapitalize`, `autocorrect`, `spellcheck`, `autocomplete`) |
//! | `open_url(ptr, len)` | `window.open(url, "_blank", "noopener")` |
//! | `notify(title, title_len, body, body_len)` | a `Notification`, asking for permission first if needed |
//! | `save_state(ptr, len)` | keeps the app's saved state in the tab's `sessionStorage` (`len` 0 removes it) |
//! | `gamepad_rumble(index, low, high, ms) -> u32` | `vibrationActuator.playEffect("dual-rumble")` on pad `index`; 0 if it has none |
//! | `fetch_asset(id, ptr, len)` | `fetch` of the packaged file at that path under the page's assets URL; the bytes (or the failure) come back through `stipple_asset` |
//! | `keep_awake(on)` | a Screen Wake Lock (`navigator.wakeLock.request("screen")`), asked again whenever the page shows, or released |
//!
//! The page also reports the canvas's safe area, the part of it under a
//! phone's notch, status bar or home indicator (`env(safe-area-inset-*)`,
//! nonzero only on a page with `viewport-fit=cover`), through
//! `stipple_safe_area(top, right, bottom, left)`, and how far up from its
//! bottom the on-screen keyboard reaches (from `visualViewport`) through
//! `stipple_keyboard_inset(px)`.
//! | `date_now() -> f64` | `Date.now()` |
//! | `share_file(name, name_len, mime, mime_len, data, data_len)` | keeps a `File` for the next `share` |
//! | `share(id, title, title_len, text, text_len, url, url_len)` | `navigator.share` with the files kept; answers `stipple_answer(id, 0 shared / 1 cancelled / 2 unavailable)` |
//! | `vibrate(ptr, n)` | `navigator.vibrate` with `n` `u32` milliseconds |
//! | `schedule_notification(id, id_len, title, title_len, body, body_len, delay_ms, repeat_ms)` | a timer (then an interval) showing a `Notification` tagged with the id; a click calls `stipple_notification(id)` |
//! | `cancel_notification(id, id_len)` | clears that id's timer (an empty id: all) |
//! | `permission_status(kind) -> u32` | what the Permissions API last said for kind 0 camera, 1 microphone, 2 notifications (`Notification.permission`), 3 location: 0 granted, 1 denied, 2 not asked |
//! | `permission_request(id, kind)` | asks: `getUserMedia` (its stream stopped at once), `Notification.requestPermission` or `getCurrentPosition`, answering `stipple_answer(id, status)` (3: no device) |
//! | `camera_count() -> u32` | the video inputs `enumerateDevices` listed (`u32::MAX` until it answered) |
//! | `camera_open(id, facing, width, height)` | `getUserMedia` (`facingMode` `user` for 1, else `environment`), playing in a hidden `<video>` (registered as an external source); reports `stipple_camera_caps(id, flags, zoom_min, zoom_max)` (flags: 1 torch, 2 focus point, 4 facing the user) or `stipple_camera_failed(id, code)`, then offers each new frame as the video through `stipple_camera_video(id, source, width, height, ms)` (1: taken, drawn by the GPU renderer from the video), else hands its RGBA over through `stipple_camera_frame(id, ptr, len, width, height, ms)` |
//! | `external_read(id, ptr, width, height) -> u32` | draws external source `id` (`ExternalImage::web_source`, the camera's video) into a 2D canvas and copies its RGBA into `ptr`, for the CPU renderer; 0 without one |
//! | `external_release(id)` | the page forgets external source `id` |
//! | `camera_pause(id, paused)` | stops handing frames over and disables the track, or the reverse |
//! | `camera_close(id)` | stops the stream's tracks |
//! | `camera_torch(id, on)`, `camera_zoom(id, factor)`, `camera_focus(id, x, y)` | `applyConstraints` with `torch`, `zoom`, `pointsOfInterest` |
//!
//! With the `gpu` feature the module also imports `gpu_init`, `gpu_texture`,
//! `gpu_free`, `gpu_frame` and `gpu_release`, which `web/gpu.js` provides (see
//! `src/gpu/web.rs`): the window's GPU layer is the canvas itself.
//!
//! Waking and timers: [`wake`](crate::platform::wake) and the frame clock call
//! back into `rearm`, which asks the page for one timer at the earliest
//! deadline; `stipple_wake` then delivers [`Event::Wake`]. The frame clock's
//! display source is `requestAnimationFrame`.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::sync::Arc;

use crate::geometry::{Insets, PhysicalSize, Point, Rect, ScaleFactor, Size};
use crate::platform::access::{A11yNode, aria_json};
use crate::platform::event::Ime;
use crate::platform::event::{
    ButtonState, Event, KeyCode, PointerButton, Touch, TouchPhase, WindowId,
};
use crate::platform::input::{css_cursor, web_button, web_key, web_modifiers, web_wheel};
use crate::platform::native::SystemLook;
use crate::platform::time::Instant;
use crate::platform::window::{CursorIcon, ImeState, Window, WindowAttributes};
use crate::platform::{ControlFlow, frame, wake};
use crate::render::{Font, Pixmap, Surface};

mod js {
    #[link(wasm_import_module = "stipple")]
    unsafe extern "C" {
        pub fn now() -> f64;
        pub fn present(ptr: *const u8, width: u32, height: u32, rects: *const u32, n: usize);
        pub fn schedule(ms: f64);
        pub fn request_frame();
        pub fn set_cursor(ptr: *const u8, len: usize);
        pub fn pointer_lock(on: u32);
        pub fn set_title(ptr: *const u8, len: usize);
        pub fn clipboard_write(ptr: *const u8, len: usize);
        pub fn clipboard_write_html(ptr: *const u8, len: usize);
        pub fn open_file(id: u32, accept: *const u8, accept_len: usize);
        pub fn save_file(name: *const u8, name_len: usize, data: *const u8, data_len: usize);
        pub fn log(level: u32, ptr: *const u8, len: usize);
        pub fn set_a11y(ptr: *const u8, len: usize);
        pub fn set_ime(active: u32, x: f64, y: f64, width: f64, height: f64);
        pub fn set_text_input(active: u32);
        pub fn set_input_hints(ptr: *const u8, len: usize);
        pub fn open_url(ptr: *const u8, len: usize);
        pub fn notify(title: *const u8, title_len: usize, body: *const u8, body_len: usize);
        pub fn message_box(confirm: u32, ptr: *const u8, len: usize) -> u32;
        pub fn save_state(ptr: *const u8, len: usize);
        pub fn gamepad_rumble(index: u32, low: f64, high: f64, ms: f64) -> u32;
        pub fn fetch_asset(id: u32, ptr: *const u8, len: usize);
        pub fn keep_awake(on: u32);
        pub fn date_now() -> f64;
        pub fn share_file(
            name: *const u8,
            name_len: usize,
            mime: *const u8,
            mime_len: usize,
            data: *const u8,
            data_len: usize,
        );
        pub fn share(
            id: u32,
            title: *const u8,
            title_len: usize,
            text: *const u8,
            text_len: usize,
            url: *const u8,
            url_len: usize,
        );
        pub fn vibrate(ptr: *const u32, n: usize);
        pub fn schedule_notification(
            id: *const u8,
            id_len: usize,
            title: *const u8,
            title_len: usize,
            body: *const u8,
            body_len: usize,
            delay_ms: f64,
            repeat_ms: f64,
        );
        pub fn cancel_notification(id: *const u8, id_len: usize);
        pub fn permission_status(kind: u32) -> u32;
        pub fn permission_request(id: u32, kind: u32);
        pub fn camera_count() -> u32;
        pub fn camera_open(id: u32, facing: u32, width: u32, height: u32);
        pub fn camera_pause(id: u32, paused: u32);
        pub fn camera_close(id: u32);
        pub fn camera_torch(id: u32, on: u32);
        pub fn camera_zoom(id: u32, factor: f64);
        pub fn camera_focus(id: u32, x: f64, y: f64);
    }
}

/// `performance.now()`, in milliseconds.
pub(crate) fn now_ms() -> f64 {
    unsafe { js::now() }
}

/// Write `msg` to the browser console; `error` uses `console.error`.
pub(crate) fn log(msg: &str, error: bool) {
    unsafe { js::log(u32::from(error), msg.as_ptr(), msg.len()) }
}

/// Put `text`, and `html` if given, on the system clipboard (best effort: the
/// browser may refuse outside a user gesture, but a `copy` event still picks
/// it up).
pub(crate) fn clipboard_write(text: &str, html: Option<&str>) {
    let html = html.unwrap_or("");
    unsafe {
        js::clipboard_write_html(html.as_ptr(), html.len());
        js::clipboard_write(text.as_ptr(), text.len());
    }
}

/// The text the last `paste` event carried.
pub(crate) fn clipboard_read() -> Option<String> {
    HOST.with(|h| h.pasted.borrow().clone())
}

/// The HTML the last `paste` event carried.
pub(crate) fn clipboard_read_html() -> Option<String> {
    HOST.with(|h| h.pasted_html.borrow().clone())
}

/// Show the browser's file picker for request `id`, filtered by `accept`.
pub(crate) fn open_file(id: u32, accept: &str) {
    unsafe { js::open_file(id, accept.as_ptr(), accept.len()) }
}

/// `window.confirm(text)` (whether OK was pressed) when `confirm`, else
/// `window.alert(text)` (`true`).
pub(crate) fn message_box(confirm: bool, text: &str) -> bool {
    unsafe { js::message_box(u32::from(confirm), text.as_ptr(), text.len()) != 0 }
}

/// Offer `data` as a download named `name`.
pub(crate) fn save_file(name: &str, data: &[u8]) {
    unsafe { js::save_file(name.as_ptr(), name.len(), data.as_ptr(), data.len()) }
}

/// Open `url` in a new tab (browsers allow it from a user gesture).
pub(crate) fn open_url(url: &str) {
    unsafe { js::open_url(url.as_ptr(), url.len()) }
}

/// Show a notification through the `Notification` API.
pub(crate) fn notify(title: &str, body: &str) {
    unsafe { js::notify(title.as_ptr(), title.len(), body.as_ptr(), body.len()) }
}

type Handler = Box<dyn FnMut(Event, &dyn Window) -> ControlFlow>;

/// The page-side state: the App's handler, events waiting for it, and the
/// canvas's size.
struct Host {
    handler: RefCell<Option<Handler>>,
    queue: RefCell<VecDeque<Event>>,
    /// The handler is running; events that arrive meanwhile wait in `queue`.
    busy: Cell<bool>,
    /// The canvas's size in CSS (logical) pixels; zero until the page says.
    css: Cell<(f64, f64)>,
    /// `devicePixelRatio`.
    scale: Cell<f64>,
    pasted: RefCell<Option<String>>,
    pasted_html: RefCell<Option<String>>,
    cursor: Cell<CursorIcon>,
    font: RefCell<Option<Font>>,
    /// The accessibility tree last sent to the page, so an unchanged one isn't
    /// sent again.
    a11y: RefCell<Option<A11yNode>>,
    text_input: Cell<bool>,
    /// The first finger down, which is also the pointer until it lifts.
    primary: Cell<Option<u64>>,
    /// Whether the page started the app ([`start`]), so its canvas sets the
    /// size.
    started: Cell<bool>,
    /// The canvas's safe area, in CSS pixels.
    safe_area: Cell<Insets>,
    /// How far up from the canvas's bottom the on-screen keyboard reaches.
    keyboard: Cell<f64>,
}

thread_local! {
    static HOST: Host = const {
        Host {
            handler: RefCell::new(None),
            queue: RefCell::new(VecDeque::new()),
            busy: Cell::new(false),
            css: Cell::new((0.0, 0.0)),
            scale: Cell::new(1.0),
            pasted: RefCell::new(None),
            pasted_html: RefCell::new(None),
            cursor: Cell::new(CursorIcon::Default),
            font: RefCell::new(None),
            a11y: RefCell::new(None),
            text_input: Cell::new(false),
            primary: Cell::new(None),
            started: Cell::new(false),
            safe_area: Cell::new(Insets::ZERO),
            keyboard: Cell::new(0.0),
        }
    };
}

/// Keep `handler` and paint the first frame. Returns at once: from here on the
/// page drives the app through the exports.
pub(crate) fn run<H>(attrs: WindowAttributes, handler: H)
where
    H: FnMut(Event, &dyn Window) -> ControlFlow + 'static,
{
    HOST.with(|h| {
        if h.css.get().0 <= 0.0 {
            let s = attrs.logical_size;
            h.css.set((s.width, s.height));
        }
        *h.handler.borrow_mut() = Some(Box::new(handler));
    });
    if attrs.title != WindowAttributes::default().title {
        WebWindow.set_title(&attrs.title);
    }
    wake::install(Arc::new(rearm), Some(Arc::new(|_| rearm())));
    frame::name_source("requestAnimationFrame");
    frame::install(Arc::new(|| unsafe { js::request_frame() }), None);
    deliver(Event::RedrawRequested);
}

/// Queue `event` for the handler and run it, unless the handler is already
/// running (an event raised from inside it), in which case the outer call
/// delivers it next.
fn deliver(event: Event) {
    HOST.with(|h| h.queue.borrow_mut().push_back(event));
    drain();
}

fn drain() {
    if HOST.with(|h| h.busy.replace(true)) {
        return;
    }
    while let Some(event) = HOST.with(|h| h.queue.borrow_mut().pop_front()) {
        let Some(mut handler) = HOST.with(|h| h.handler.borrow_mut().take()) else {
            HOST.with(|h| h.queue.borrow_mut().clear());
            break;
        };
        if handler(event, &WebWindow) != ControlFlow::Exit {
            HOST.with(|h| *h.handler.borrow_mut() = Some(handler));
        }
    }
    HOST.with(|h| h.busy.set(false));
    rearm();
}

/// Ask the page for one timer at the earliest thing the loop waits for: a
/// pending wake (now), the App's next timer, or a frame the display owes.
fn rearm() {
    let ms = match wake::timeout(Instant::now()) {
        Some(t) => t.as_secs_f64() * 1000.0,
        None => -1.0,
    };
    unsafe { js::schedule(ms) }
}

/// The one window: the canvas.
struct WebWindow;

fn physical(css: (f64, f64), scale: f64) -> PhysicalSize {
    PhysicalSize::new(
        ((css.0 * scale).round() as u32).max(1),
        ((css.1 * scale).round() as u32).max(1),
    )
}

impl Window for WebWindow {
    fn id(&self) -> WindowId {
        WindowId(1)
    }

    fn inner_size(&self) -> PhysicalSize {
        HOST.with(|h| physical(h.css.get(), h.scale.get()))
    }

    fn scale_factor(&self) -> ScaleFactor {
        ScaleFactor::new(HOST.with(|h| h.scale.get()))
    }

    fn request_redraw(&self) {
        deliver(Event::RedrawRequested);
    }

    fn set_title(&self, title: &str) {
        unsafe { js::set_title(title.as_ptr(), title.len()) }
    }

    fn create_surface(&self) -> Box<dyn Surface> {
        Box::new(WebSurface {
            size: self.inner_size(),
        })
    }

    /// The canvas itself: `web/gpu.js` takes it for its WebGPU or WebGL2
    /// context (handing it back to the 2D context on a CPU frame after
    /// [`drop_gpu_layer`](Window::drop_gpu_layer)).
    fn gpu_layer(&self) -> Option<crate::platform::GpuLayer> {
        cfg!(feature = "gpu").then_some(crate::platform::GpuLayer::Web)
    }

    fn drop_gpu_layer(&self) {
        #[cfg(feature = "gpu")]
        crate::gpu::web::release();
    }

    fn clipboard(&self) -> Option<String> {
        clipboard_read()
    }

    fn set_clipboard(&self, text: &str) {
        clipboard_write(text, None);
    }

    fn read_clipboard(
        &self,
        kind: crate::platform::clipboard::ClipboardKind,
        formats: crate::platform::clipboard::ClipboardFormats,
    ) -> crate::platform::clipboard::ClipboardRead {
        use crate::platform::clipboard::{ClipboardKind, ClipboardRead, read};
        ClipboardRead::Ready(
            (kind == ClipboardKind::Clipboard)
                .then(|| read(formats))
                .flatten(),
        )
    }

    fn write_clipboard(
        &self,
        kind: crate::platform::clipboard::ClipboardKind,
        content: &crate::platform::clipboard::ClipboardContent,
    ) {
        if kind == crate::platform::clipboard::ClipboardKind::Clipboard {
            crate::platform::clipboard::write(content);
        }
    }

    fn info(&self) -> crate::platform::WindowInfo {
        let mut info =
            crate::platform::WindowInfo::from_size(self.inner_size(), self.scale_factor());
        HOST.with(|h| {
            info.safe_area = h.safe_area.get();
            info.keyboard_inset = h.keyboard.get();
        });
        info
    }

    fn set_keep_awake(&self, on: bool) {
        unsafe { js::keep_awake(u32::from(on)) }
    }

    fn set_pointer_lock(&self, locked: bool) -> bool {
        // Granted (or not) later: a refusal comes back as kind 5.
        unsafe { js::pointer_lock(u32::from(locked)) };
        true
    }

    fn set_cursor(&self, cursor: CursorIcon) {
        if HOST.with(|h| h.cursor.replace(cursor)) != cursor {
            let css = css_cursor(cursor);
            unsafe { js::set_cursor(css.as_ptr(), css.len()) }
        }
    }

    fn set_accessibility_tree(&self, root: &A11yNode) {
        let changed = HOST.with(|h| {
            let mut sent = h.a11y.borrow_mut();
            if sent.as_ref() == Some(root) {
                return false;
            }
            *sent = Some(root.clone());
            true
        });
        if changed {
            let json = aria_json(root);
            unsafe { js::set_a11y(json.as_ptr(), json.len()) }
        }
    }

    fn set_ime(&self, state: Option<&ImeState>) {
        match state {
            Some(s) => {
                let r = s.cursor_area;
                unsafe { js::set_ime(1, r.min_x(), r.min_y(), r.width(), r.height()) }
            }
            None => unsafe { js::set_ime(0, 0.0, 0.0, 0.0, 0.0) },
        }
    }

    fn set_input_hints(&self, hints: &crate::platform::InputHints) {
        let attrs = crate::platform::input_hints::web_attributes(hints);
        unsafe { js::set_input_hints(attrs.as_ptr(), attrs.len()) }
    }

    fn set_text_input_active(&self, active: bool) {
        if HOST.with(|h| h.text_input.replace(active)) != active {
            unsafe { js::set_text_input(u32::from(active)) }
        }
    }
}

/// Presents frames with `putImageData`, straight from wasm memory.
struct WebSurface {
    size: PhysicalSize,
}

impl Surface for WebSurface {
    fn resize(&mut self, size: PhysicalSize) {
        self.size = size;
    }

    fn size(&self) -> PhysicalSize {
        self.size
    }

    fn present(&mut self, pixmap: &Pixmap, damage: &[Rect]) {
        let size = pixmap.size();
        let rects = damage_rects(damage, size);
        if !damage.is_empty() && rects.is_empty() {
            return; // all the damage fell outside the canvas
        }
        unsafe {
            js::present(
                pixmap.as_bytes().as_ptr(),
                size.width,
                size.height,
                rects.as_ptr(),
                rects.len() / 4,
            )
        }
    }
}

/// Damage rects as whole device pixels clipped to `size`, flattened to
/// `x, y, w, h` quadruples.
fn damage_rects(damage: &[Rect], size: PhysicalSize) -> Vec<u32> {
    let mut out = Vec::with_capacity(damage.len() * 4);
    for r in damage {
        let x0 = r.min_x().floor().max(0.0) as u32;
        let y0 = r.min_y().floor().max(0.0) as u32;
        let x1 = (r.max_x().ceil().max(0.0) as u32).min(size.width);
        let y1 = (r.max_y().ceil().max(0.0) as u32).min(size.height);
        if x1 > x0 && y1 > y0 {
            out.extend([x0, y0, x1 - x0, y1 - y0]);
        }
    }
    out
}

// ---- Entry points for the generated exports ------------------------------------
//
// `web_app!` expands to `#[no_mangle] extern "C"` functions in the app's crate
// that call these. They're public for that reason only.

/// Allocate `len` bytes for the page to write into. The export that receives
/// the bytes takes them back with [`take_bytes`].
#[doc(hidden)]
pub fn alloc(len: usize) -> *mut u8 {
    Box::into_raw(vec![0u8; len].into_boxed_slice()).cast()
}

/// Take back `len` bytes at `ptr`, from one [`alloc`] of `len`.
///
/// # Safety
/// `ptr` and `len` must come from a single `alloc(len)` not already taken back.
#[doc(hidden)]
pub unsafe fn take_bytes(ptr: *mut u8, len: usize) -> Vec<u8> {
    if ptr.is_null() || len == 0 {
        return Vec::new();
    }
    unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)) }.into_vec()
}

/// [`take_bytes`] as UTF-8 text (invalid sequences replaced).
///
/// # Safety
/// As for [`take_bytes`].
#[doc(hidden)]
pub unsafe fn take_string(ptr: *mut u8, len: usize) -> String {
    let bytes = unsafe { take_bytes(ptr, len) };
    String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// A font the page loaded: the first is the one the app draws with unless it
/// sets its own; each later one becomes its fallback (a CJK or emoji font,
/// say), in order.
#[doc(hidden)]
pub fn set_font(bytes: Vec<u8>) {
    match Font::from_bytes(bytes) {
        Ok(font) => HOST.with(|h| {
            let mut slot = h.font.borrow_mut();
            *slot = Some(match slot.take() {
                Some(primary) => primary.with_fallback(&font),
                None => font,
            });
        }),
        Err(_) => log("stipple: the font could not be read", true),
    }
}

/// The font the page loaded ([`set_font`]), which [`Font::system_default`]
/// returns on the web.
pub(crate) fn page_font() -> Option<Font> {
    HOST.with(|h| h.font.borrow().clone())
}

/// Call `make`, which builds the app and runs it (or returns it to run), in a
/// canvas of `width` × `height` CSS pixels at `scale` (`devicePixelRatio`).
#[doc(hidden)]
pub fn start<R: crate::web_app::WebEntry>(
    width: f64,
    height: f64,
    scale: f64,
    make: impl FnOnce() -> R,
) {
    std::panic::set_hook(Box::new(|info| log(&format!("stipple: {info}"), true)));
    HOST.with(|h| {
        h.css.set((width.max(1.0), height.max(1.0)));
        h.scale.set(ScaleFactor::new(scale).get());
        h.started.set(true);
    });
    make().launch();
}

/// What [`App::run`](crate::App::run) does to an app on the web before it
/// runs: once the page started it, it fills the canvas, whatever size the
/// app asked for.
pub(crate) fn adopt<S: 'static>(app: crate::App<S>) -> crate::App<S> {
    match HOST.with(|h| h.started.get().then(|| h.css.get())) {
        Some((w, h)) => app.logical_size(Size::new(w, h)),
        None => app,
    }
}

/// The canvas changed size or moved to a screen with another pixel ratio.
#[doc(hidden)]
pub fn resize(width: f64, height: f64, scale: f64) {
    let css = (width.max(1.0), height.max(1.0));
    let scale = ScaleFactor::new(scale).get();
    let (old_css, old_scale) = HOST.with(|h| (h.css.replace(css), h.scale.replace(scale)));
    if scale != old_scale {
        deliver(Event::ScaleFactorChanged(ScaleFactor::new(scale)));
    }
    if css != old_css || scale != old_scale {
        deliver(Event::Resized(physical(css, scale)));
    }
}

/// A pointer event at `(x, y)` CSS pixels: `kind` 0 moves, 1 presses, 2
/// releases, 3 leaves the canvas; while the pointer is locked, 4 is its
/// motion (`movementX`/`movementY` in `x` and `y`), and 5 says the lock
/// ended or was refused. `button` is the DOM's (0 left, 1 middle, 2 right);
/// `mods` packs the modifiers (see [`key`]).
#[doc(hidden)]
pub fn pointer(kind: u32, button: u32, x: f64, y: f64, mods: u32) {
    let position = Point::new(x, y);
    let button = web_button(button);
    // Moves record the modifiers too (and the wheel's, which the page
    // precedes with a move).
    if kind < 3 || kind == 4 {
        crate::platform::set_pointer_modifiers(web_modifiers(mods));
    }
    let state = match kind {
        0 => return deliver(Event::PointerMoved { position }),
        4 => return deliver(Event::PointerDelta { dx: x, dy: y }),
        5 => return deliver(Event::PointerLockLost),
        1 => ButtonState::Pressed,
        2 => ButtonState::Released,
        _ => return deliver(Event::PointerLeft),
    };
    deliver(Event::PointerButton {
        button,
        state,
        position,
    });
}

/// A finger of a touch event (each of its `changedTouches`): `phase` 0 for
/// `touchstart`, 1 `touchmove`, 2 `touchend`, 3 `touchcancel`; `id` is the
/// `Touch.identifier`, `(x, y)` in CSS pixels. Each finger is an
/// [`Event::Touch`] (the app's gestures: scrolling, long press, pinch); the
/// first one down is also the pointer until it lifts, as on iOS and Android.
#[doc(hidden)]
pub fn touch(phase: u32, id: u32, x: f64, y: f64) {
    let position = Point::new(x, y);
    let id = u64::from(id);
    let phase = match phase {
        0 => TouchPhase::Started,
        1 => TouchPhase::Moved,
        2 => TouchPhase::Ended,
        _ => TouchPhase::Cancelled,
    };
    deliver(Event::Touch(Touch {
        id,
        phase,
        position,
    }));
    let primary = HOST.with(|h| h.primary.get());
    let button = |state, position| Event::PointerButton {
        button: PointerButton::Left,
        state,
        position,
    };
    match phase {
        TouchPhase::Started if primary.is_none() => {
            HOST.with(|h| h.primary.set(Some(id)));
            crate::platform::set_pointer_modifiers(Default::default());
            deliver(Event::PointerMoved { position });
            deliver(button(ButtonState::Pressed, position));
        }
        TouchPhase::Moved if primary == Some(id) => deliver(Event::PointerMoved { position }),
        TouchPhase::Ended | TouchPhase::Cancelled if primary == Some(id) => {
            HOST.with(|h| h.primary.set(None));
            // A cancelled touch is released far away, so it doesn't click.
            let at = if phase == TouchPhase::Cancelled {
                Point::new(-1.0e9, -1.0e9)
            } else {
                position
            };
            deliver(button(ButtonState::Released, at));
            // A finger that lifts leaves no hover behind.
            deliver(Event::PointerLeft);
        }
        _ => {}
    }
}

/// A `wheel` event (`deltaX`, `deltaY`, `deltaMode`).
#[doc(hidden)]
pub fn wheel(dx: f64, dy: f64, mode: u32) {
    let page = HOST.with(|h| h.css.get().1);
    deliver(Event::Scroll {
        delta: web_wheel(dx, dy, mode, page),
    });
}

/// A `keydown` with `KeyboardEvent.key` and the modifiers packed as 1 Shift,
/// 2 Ctrl, 4 Alt, 8 Meta. Returns whether the page should `preventDefault`:
/// keys Stipple handles, but not the browser's own shortcuts, and not
/// Ctrl/Cmd+C, X or V, whose `copy`, `cut` and `paste` events carry the
/// clipboard.
#[doc(hidden)]
pub fn key(key: &str, mods: u32) -> bool {
    let modifiers = web_modifiers(mods);
    let Some(code) = web_key(key) else {
        return false;
    };
    let command = modifiers.ctrl || modifiers.meta;
    let shortcut = match code {
        KeyCode::Char(c) if command => Some(c.to_ascii_lowercase()),
        _ => None,
    };
    if shortcut == Some('v') {
        return false; // the paste event delivers it, with the text
    }
    // AltGr arrives as Ctrl+Alt on Windows, and types text.
    let text = match code {
        KeyCode::Char(c) if !command || (modifiers.ctrl && modifiers.alt) => Some(c.to_string()),
        KeyCode::Space if !command => Some(" ".to_string()),
        _ => None,
    };
    deliver(Event::Key {
        code,
        state: ButtonState::Pressed,
        modifiers,
    });
    if let Some(text) = text {
        deliver(Event::Text(text));
    }
    match shortcut {
        Some(c) => matches!(c, 'a' | 'z' | 'y'),
        None => !matches!(
            code,
            KeyCode::F1
                | KeyCode::F2
                | KeyCode::F3
                | KeyCode::F4
                | KeyCode::F5
                | KeyCode::F6
                | KeyCode::F7
                | KeyCode::F8
                | KeyCode::F9
                | KeyCode::F10
                | KeyCode::F11
                | KeyCode::F12
        ),
    }
}

/// The HTML a `paste` event carries (empty: none), passed just before
/// [`paste`].
#[doc(hidden)]
pub fn paste_html(html: String) {
    HOST.with(|h| *h.pasted_html.borrow_mut() = Some(html).filter(|h| !h.is_empty()));
}

/// A `paste` event carrying `text`: delivered as Ctrl+V, with the text as the
/// clipboard's.
#[doc(hidden)]
pub fn paste(text: String) {
    HOST.with(|h| *h.pasted.borrow_mut() = Some(text));
    deliver(Event::Key {
        code: KeyCode::Char('v'),
        state: ButtonState::Pressed,
        modifiers: crate::platform::Modifiers {
            ctrl: true,
            ..Default::default()
        },
    });
}

thread_local! {
    /// Left/right modifiers, from the modifier keys' own key events.
    static SIDES: Cell<crate::platform::key::SideTracker> =
        Cell::new(crate::platform::key::SideTracker::default());
}

/// A `keydown` or `keyup` as a raw key: `KeyboardEvent.key` and `.code`, the
/// modifiers packed as for [`key`], and `flags` 1 for a release, 2 for a
/// repeat. Returns whether the app used it; the page then calls
/// `preventDefault` and skips [`key`].
#[doc(hidden)]
pub fn key_raw(key: &str, code: &str, mods: u32, flags: u32) -> bool {
    use crate::platform::input;
    use crate::platform::key::{KeyEvent, PhysicalKey};
    let modifiers = web_modifiers(mods);
    let state = if flags & 1 != 0 {
        ButtonState::Released
    } else {
        ButtonState::Pressed
    };
    let physical = PhysicalKey::from_name(code).unwrap_or_default();
    let pressed = state == ButtonState::Pressed;
    let text = input::web_text(key, modifiers).filter(|_| pressed);
    let ignoring = input::web_text_ignoring_modifiers(code, modifiers.shift, text.as_deref())
        .filter(|_| pressed);
    let named = match key {
        "Shift" => Some(KeyCode::Shift),
        "Control" => Some(KeyCode::Control),
        "Alt" => Some(KeyCode::Alt),
        "Meta" | "OS" => Some(KeyCode::Meta),
        "AltGraph" => Some(KeyCode::AltGraph),
        "CapsLock" => Some(KeyCode::CapsLock),
        "NumLock" => Some(KeyCode::NumLock),
        "ScrollLock" => Some(KeyCode::ScrollLock),
        "PrintScreen" => Some(KeyCode::PrintScreen),
        "Pause" => Some(KeyCode::Pause),
        "ContextMenu" => Some(KeyCode::ContextMenu),
        _ => web_key(key).filter(|k| !matches!(k, KeyCode::Char(_))),
    };
    // The character with Shift as held but without Ctrl/Alt: `key` unless
    // a command modifier changed it (`code` then knows letters and digits).
    let shifted = match (&text, modifiers.ctrl || modifiers.alt || modifiers.meta) {
        (Some(t), false) => Some(t.clone()),
        _ => input::web_text_ignoring_modifiers(code, modifiers.shift, None)
            .or_else(|| input::web_text(key, crate::platform::Modifiers::default())),
    };
    let logical = input::raw_logical(named, physical, shifted.as_deref(), 0);
    let mut ev = KeyEvent::new(state, logical, physical);
    ev.modifiers = modifiers;
    ev.sides = SIDES.with(|s| {
        let mut t = s.get();
        let sides = t.update(physical, state, modifiers);
        s.set(t);
        sides
    });
    ev.repeat = pressed && flags & 2 != 0;
    ev.text = text;
    ev.text_ignoring_modifiers = ignoring;
    ev.scancode = u32::from(physical.0);
    crate::platform::key::deliver_raw(ev, |e| {
        deliver(e);
        ControlFlow::Wait
    })
    .0
}

/// Composition from the page's hidden textarea: `kind` 0 is the preedit so
/// far (`compositionupdate`; empty ends it), 1 commits `text`
/// (`compositionend`, or an `input` event that typed text no `keydown`
/// carried, such as a character composed with a dead key).
#[doc(hidden)]
pub fn ime(kind: u32, text: String) {
    let event = if kind == 0 {
        let end = text.len();
        Ime::Preedit {
            text,
            cursor: Some((end, end)),
        }
    } else {
        Ime::Commit(text)
    };
    deliver(Event::Ime(event));
}

/// The page's `prefers-color-scheme` (`dark`) and accent color (`0xRRGGBB`,
/// or negative for none). Called before `start`, and again when the scheme
/// changes, which a running app sees as [`Event::ThemeChanged`].
#[doc(hidden)]
pub fn color_scheme(dark: bool, accent: i32) {
    let accent = u32::try_from(accent)
        .ok()
        .map(|c| crate::render::Color::rgb((c >> 16) as u8, (c >> 8) as u8, c as u8));
    let changed = crate::platform::native::set_web_look(SystemLook {
        dark,
        accent,
        overlay_scrollers: None,
    });
    if changed && HOST.with(|h| h.handler.borrow().is_some()) {
        deliver(Event::ThemeChanged);
    }
}

/// The canvas's safe area changed (CSS pixels from each edge).
#[doc(hidden)]
pub fn safe_area(top: f64, right: f64, bottom: f64, left: f64) {
    let insets = Insets::new(top.max(0.0), right.max(0.0), bottom.max(0.0), left.max(0.0));
    let changed = HOST.with(|h| h.safe_area.replace(insets)) != insets;
    if changed && HOST.with(|h| h.handler.borrow().is_some()) {
        deliver(Event::WindowChanged);
    }
}

/// How far up from the canvas's bottom the on-screen keyboard reaches (CSS
/// pixels), from the page's `visualViewport`.
#[doc(hidden)]
pub fn keyboard_inset(inset: f64) {
    let inset = inset.max(0.0);
    let changed = HOST.with(|h| h.keyboard.replace(inset)) != inset;
    if changed && HOST.with(|h| h.handler.borrow().is_some()) {
        deliver(Event::WindowChanged);
    }
}

/// The page's `prefers-reduced-motion: reduce`. Called before `start`, and
/// again when it changes, which a running app sees as
/// [`Event::ThemeChanged`] (it reads the setting again then).
#[doc(hidden)]
pub fn reduced_motion(reduced: bool) {
    let changed = crate::platform::native::set_web_reduced_motion(reduced);
    if changed && HOST.with(|h| h.handler.borrow().is_some()) {
        deliver(Event::ThemeChanged);
    }
}

/// The page's languages, clock, first day of the week and time zone offset
/// (see `web/stipple.js`). Called before `start`, and again on
/// `languagechange`, which a running app sees as [`Event::LocaleChanged`].
#[doc(hidden)]
pub fn locale(blob: String) {
    let changed = crate::platform::locale::set_web(&blob);
    if changed && HOST.with(|h| h.handler.borrow().is_some()) {
        deliver(Event::LocaleChanged);
    }
}

/// The page's `renderer` option (`auto`, `software`, `gpu`, `webgpu`,
/// `webgl2`), which overrides the app's [`App::renderer`](crate::App::renderer)
/// as `STIPPLE_RENDERER` does natively. Called before [`start`].
#[doc(hidden)]
pub fn renderer(value: String) {
    crate::app_gpu::set_page_renderer(&value);
}

/// Whether the browser's WebGPU / WebGL2 run on a hardware GPU, as the page
/// found (`web/stipple.js`): then [`Renderer::Auto`](crate::render::Renderer::Auto)
/// draws on it. Called before [`start`].
#[doc(hidden)]
pub fn gpu_hardware(on: bool) {
    crate::app_gpu::set_page_hardware_gpu(on);
}

/// The canvas went out of view (`false`: scrolled away, its iframe scrolled
/// off its page, or the tab hidden) or came back (`true`). Out of view,
/// frames stop, as natively in the background (the frame clock's fallback
/// timer too, which would otherwise drive a game loop on its own); back in
/// view, a whole frame is drawn, and whatever animates asks for frames again.
/// Timers keep running.
#[doc(hidden)]
pub fn visible(on: bool) {
    let was = !frame::suspended();
    frame::set_suspended(!on);
    if on && !was {
        redraw();
    }
}

/// The page wants a whole new frame (its GPU just came up).
#[doc(hidden)]
pub fn redraw() {
    if HOST.with(|h| h.handler.borrow().is_some() || h.busy.get()) {
        deliver(Event::RedrawRequested);
    }
}

/// The canvas gained or lost keyboard focus.
#[doc(hidden)]
pub fn focus(focused: bool) {
    if !focused {
        // Modifier releases go to whatever has focus now.
        SIDES.with(|s| {
            let mut t = s.get();
            t.clear();
            s.set(t);
        });
    }
    deliver(Event::FocusChanged(focused));
}

/// The page's timer fired: deliver a wake if one is due.
#[doc(hidden)]
pub fn on_wake() {
    if wake::take(Instant::now()) {
        deliver(Event::Wake);
    } else {
        drain();
    }
}

/// `requestAnimationFrame` fired: the display is ready for a frame.
#[doc(hidden)]
pub fn on_frame() {
    frame::tick();
    on_wake();
}

/// The state the app saved before the tab was reloaded (or discarded and
/// brought back), from `sessionStorage`. Called before [`start`]; the app's
/// [`restore_state`](crate::App::restore_state) gets it before the first
/// frame.
#[doc(hidden)]
pub fn restore_state(bytes: Vec<u8>) {
    crate::platform::saved_state::set_restored_state((!bytes.is_empty()).then_some(bytes));
}

/// The page is going away or out of sight (`pagehide`, or `visibilitychange`
/// to hidden): ask the app for its state
/// ([`Event::SaveState`](crate::platform::Event::SaveState)) and hand it to
/// the page, which keeps it in `sessionStorage` (nothing saved removes it).
#[doc(hidden)]
pub fn save_state() {
    let bytes = crate::platform::saved_state::deliver_save(deliver).unwrap_or_default();
    unsafe { js::save_state(bytes.as_ptr(), bytes.len()) }
}

/// The file picker for request `id` returned `name` and its `bytes`, or was
/// cancelled (`None`).
#[doc(hidden)]
pub fn file_picked(id: u32, file: Option<(String, Vec<u8>)>) {
    crate::platform::dialog::web_picked(id, file);
    drain();
}

/// Ask the page to fetch the packaged file at `path`, for request `id`; it
/// answers through [`asset`].
pub(crate) fn fetch_asset(id: u32, path: &str) {
    unsafe { js::fetch_asset(id, path.as_ptr(), path.len()) }
}

/// The page fetched the packaged file at `path` for request `id` (0: one it
/// preloaded before starting the app): its bytes, or `status`, the HTTP
/// status (0 for a network error), when it isn't 200.
#[doc(hidden)]
pub fn asset(id: u32, path: String, data: Vec<u8>, status: u32) {
    crate::platform::assets::web::fetched(id, path, data, status);
    drain();
}

/// `Date.now()`: milliseconds since the Unix epoch.
pub(crate) fn date_now() -> f64 {
    unsafe { js::date_now() }
}

/// Hand `share` to `navigator.share`, for share request `id`.
pub(crate) fn share(id: u32, share: &crate::platform::Share) {
    for f in &share.files {
        let mime = f.mime_type();
        unsafe {
            js::share_file(
                f.name.as_ptr(),
                f.name.len(),
                mime.as_ptr(),
                mime.len(),
                f.bytes.as_ptr(),
                f.bytes.len(),
            )
        };
    }
    let (s, t, u) = (&share.subject, &share.text, &share.url);
    unsafe {
        js::share(
            id,
            s.as_ptr(),
            s.len(),
            t.as_ptr(),
            t.len(),
            u.as_ptr(),
            u.len(),
        )
    }
}

/// `navigator.vibrate(pattern)`.
pub(crate) fn vibrate(pattern: &[u32]) {
    unsafe { js::vibrate(pattern.as_ptr(), pattern.len()) }
}

/// Show `entry` in `delay_ms`, then every `repeat_ms` if it repeats.
pub(crate) fn schedule_notification(
    entry: &crate::platform::shell::schedule::Entry,
    delay_ms: u64,
) {
    let (i, t, b) = (&entry.id, &entry.title, &entry.body);
    unsafe {
        js::schedule_notification(
            i.as_ptr(),
            i.len(),
            t.as_ptr(),
            t.len(),
            b.as_ptr(),
            b.len(),
            delay_ms as f64,
            entry.repeat_ms as f64,
        )
    }
}

/// Clear notification `id`'s timer; an empty `id` clears them all.
pub(crate) fn cancel_notification(id: &str) {
    unsafe { js::cancel_notification(id.as_ptr(), id.len()) }
}

/// The page's number for `permission`, if the page gates it.
pub(crate) fn permission_kind(permission: crate::platform::permission::Permission) -> Option<u32> {
    use crate::platform::permission::Permission;
    match permission {
        Permission::Camera => Some(0),
        Permission::Microphone => Some(1),
        Permission::Notifications => Some(2),
        Permission::Location => Some(3),
        _ => None,
    }
}

fn permission_code(code: u32) -> crate::platform::permission::PermissionStatus {
    use crate::platform::permission::PermissionStatus;
    match code {
        0 => PermissionStatus::Granted,
        1 => PermissionStatus::Denied,
        _ => PermissionStatus::NotDetermined,
    }
}

/// What the page last heard of permission `kind`.
pub(crate) fn permission_status(kind: u32) -> crate::platform::permission::PermissionStatus {
    permission_code(unsafe { js::permission_status(kind) })
}

/// Ask for permission `kind`, answering `done`.
pub(crate) fn request_permission(kind: u32, done: crate::platform::permission::Done) {
    let id = crate::platform::shell::sharing::register(move |v| done(permission_code(v)));
    unsafe { js::permission_request(id, kind) }
}

/// The video inputs the page found (`u32::MAX`: not known yet).
pub(crate) fn camera_count() -> u32 {
    unsafe { js::camera_count() }
}

/// Open camera `id`, facing the user (`facing` 1) or not.
pub(crate) fn camera_open(id: u32, facing: u32, width: u32, height: u32) {
    unsafe { js::camera_open(id, facing, width, height) }
}

pub(crate) fn camera_pause(id: u32, paused: bool) {
    unsafe { js::camera_pause(id, u32::from(paused)) }
}

pub(crate) fn camera_close(id: u32) {
    unsafe { js::camera_close(id) }
}

pub(crate) fn camera_torch(id: u32, on: bool) {
    unsafe { js::camera_torch(id, u32::from(on)) }
}

pub(crate) fn camera_zoom(id: u32, factor: f64) {
    unsafe { js::camera_zoom(id, factor) }
}

pub(crate) fn camera_focus(id: u32, x: f64, y: f64) {
    unsafe { js::camera_focus(id, x, y) }
}

/// A frame of camera `id` arrived from the page.
#[doc(hidden)]
pub fn camera_frame(id: u32, pixels: Vec<u8>, width: u32, height: u32, ms: f64) {
    crate::camera::os::frame(id, &pixels, width, height, ms);
    drain();
}

/// A frame of camera `id` arrived as its `<video>` (the page's external
/// source `source`): whether it was taken as that (else the page sends its
/// pixels).
#[doc(hidden)]
pub fn camera_video(id: u32, source: u32, width: u32, height: u32, ms: f64) -> bool {
    let taken = crate::camera::os::video(id, source, width, height, ms);
    if taken {
        drain();
    }
    taken
}

/// Camera `id`'s stream started, and can do this.
#[doc(hidden)]
pub fn camera_caps(id: u32, flags: u32, zoom_min: f64, zoom_max: f64) {
    crate::camera::os::caps(id, flags, zoom_min, zoom_max);
    drain();
}

/// Camera `id` couldn't start.
#[doc(hidden)]
pub fn camera_failed(id: u32, code: u32) {
    crate::camera::os::failed(id, code);
    drain();
}

/// The page answered waiter `id` (a share, a permission request) with
/// `value`.
#[doc(hidden)]
pub fn answer(id: u32, value: u32) {
    crate::platform::shell::sharing::answer(id, value);
    drain();
}

/// The user clicked the notification scheduled under `id`.
#[doc(hidden)]
pub fn notification_tapped(id: String) {
    deliver(Event::NotificationTapped(id));
}

/// Rumble the page's pad `index` (see [`gamepad::set_rumble`](crate::gamepad::set_rumble)).
pub(crate) fn gamepad_rumble(index: u32, low: f32, high: f32, ms: f64) -> bool {
    unsafe { js::gamepad_rumble(index, low as f64, high as f64, ms) != 0 }
}

/// Pad `index` connected, with this `Gamepad.id`.
#[doc(hidden)]
pub fn gamepad_connect(index: u32, id: String) {
    crate::platform::gamepad::web::connect(index, &id);
}

/// Pad `index` disconnected.
#[doc(hidden)]
pub fn gamepad_disconnect(index: u32) {
    crate::platform::gamepad::web::disconnect(index);
}

/// Standard button `button` of pad `index` is at `value`.
#[doc(hidden)]
pub fn gamepad_button(index: u32, button: u32, value: f64, pressed: bool) {
    crate::platform::gamepad::web::set_button(index, button, value, pressed);
}

/// Standard axis `axis` of pad `index` is at `value`.
#[doc(hidden)]
pub fn gamepad_axis(index: u32, axis: u32, value: f64) {
    crate::platform::gamepad::web::set_axis(index, axis, value);
}
