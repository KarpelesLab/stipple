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
//! | `set_title(ptr, len)` | `document.title` |
//! | `clipboard_write(ptr, len)` | `navigator.clipboard.writeText`, and the text a `copy` / `cut` event hands over |
//! | `open_file(id, ptr, len)` | an `<input type=file>` (`accept` = the string); the file comes back through `stipple_file` |
//! | `save_file(name, name_len, data, data_len)` | a download of a `Blob` |
//! | `log(level, ptr, len)` | `console.log` / `console.error` (panics land here) |
//! | `set_a11y(ptr, len)` | the ARIA overlay, from the accessibility tree as JSON (see [`aria_json`](crate::platform::access::aria_json)) |
//! | `set_ime(active, x, y, w, h)` | moves the hidden `<textarea>` that takes composition to the caret (CSS pixels) |
//! | `set_text_input(active)` | the textarea's `inputmode`, which shows or hides a touch keyboard |
//! | `open_url(ptr, len)` | `window.open(url, "_blank", "noopener")` |
//! | `notify(title, title_len, body, body_len)` | a `Notification`, asking for permission first if needed |
//!
//! Waking and timers: [`wake`](crate::platform::wake) and the frame clock call
//! back into `rearm`, which asks the page for one timer at the earliest
//! deadline; `stipple_wake` then delivers [`Event::Wake`]. The frame clock's
//! display source is `requestAnimationFrame`.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::sync::Arc;

use crate::geometry::{PhysicalSize, Point, Rect, ScaleFactor, Size};
use crate::platform::access::{A11yNode, aria_json};
use crate::platform::event::Ime;
use crate::platform::event::{ButtonState, Event, KeyCode, PointerButton, WindowId};
use crate::platform::input::{css_cursor, web_key, web_modifiers, web_wheel};
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
        pub fn set_title(ptr: *const u8, len: usize);
        pub fn clipboard_write(ptr: *const u8, len: usize);
        pub fn open_file(id: u32, accept: *const u8, accept_len: usize);
        pub fn save_file(name: *const u8, name_len: usize, data: *const u8, data_len: usize);
        pub fn log(level: u32, ptr: *const u8, len: usize);
        pub fn set_a11y(ptr: *const u8, len: usize);
        pub fn set_ime(active: u32, x: f64, y: f64, width: f64, height: f64);
        pub fn set_text_input(active: u32);
        pub fn open_url(ptr: *const u8, len: usize);
        pub fn notify(title: *const u8, title_len: usize, body: *const u8, body_len: usize);
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

/// Put `text` on the system clipboard (best effort: the browser may refuse
/// outside a user gesture, but a `copy` event still picks it up).
pub(crate) fn clipboard_write(text: &str) {
    unsafe { js::clipboard_write(text.as_ptr(), text.len()) }
}

/// The text the last `paste` event carried.
pub(crate) fn clipboard_read() -> Option<String> {
    HOST.with(|h| h.pasted.borrow().clone())
}

/// Show the browser's file picker for request `id`, filtered by `accept`.
pub(crate) fn open_file(id: u32, accept: &str) {
    unsafe { js::open_file(id, accept.as_ptr(), accept.len()) }
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
    cursor: Cell<CursorIcon>,
    font: RefCell<Option<Font>>,
    /// The accessibility tree last sent to the page, so an unchanged one isn't
    /// sent again.
    a11y: RefCell<Option<A11yNode>>,
    text_input: Cell<bool>,
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
            cursor: Cell::new(CursorIcon::Default),
            font: RefCell::new(None),
            a11y: RefCell::new(None),
            text_input: Cell::new(false),
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

    fn clipboard(&self) -> Option<String> {
        clipboard_read()
    }

    fn set_clipboard(&self, text: &str) {
        clipboard_write(text);
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

/// The font the page loaded, used by the app unless it set its own.
#[doc(hidden)]
pub fn set_font(bytes: Vec<u8>) {
    match Font::from_bytes(bytes) {
        Ok(font) => HOST.with(|h| *h.font.borrow_mut() = Some(font)),
        Err(_) => log("stipple: the font could not be read", true),
    }
}

/// Build the app with `make` and run it in a canvas of `width` × `height` CSS
/// pixels at `scale` (`devicePixelRatio`).
#[doc(hidden)]
pub fn start<S: 'static>(
    width: f64,
    height: f64,
    scale: f64,
    make: impl FnOnce() -> crate::App<S>,
) {
    std::panic::set_hook(Box::new(|info| log(&format!("stipple: {info}"), true)));
    HOST.with(|h| {
        h.css.set((width.max(1.0), height.max(1.0)));
        h.scale.set(ScaleFactor::new(scale).get());
    });
    let mut app = make();
    if app.font.is_none()
        && let Some(font) = HOST.with(|h| h.font.borrow_mut().take())
    {
        app = app.font(font);
    }
    app = app.logical_size(Size::new(width.max(1.0), height.max(1.0)));
    app.run();
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
/// releases, 3 leaves the canvas. `button` is the DOM's (0 left, 1 middle, 2
/// right); `mods` packs the modifiers (see [`key`]).
#[doc(hidden)]
pub fn pointer(kind: u32, button: u32, x: f64, y: f64, mods: u32) {
    let position = Point::new(x, y);
    let button = match button {
        0 => PointerButton::Left,
        1 => PointerButton::Middle,
        2 => PointerButton::Right,
        b => PointerButton::Other(b as u16),
    };
    let state = match kind {
        0 => return deliver(Event::PointerMoved { position }),
        1 => ButtonState::Pressed,
        2 => ButtonState::Released,
        _ => return deliver(Event::PointerLeft),
    };
    crate::platform::set_pointer_modifiers(web_modifiers(mods));
    deliver(Event::PointerButton {
        button,
        state,
        position,
    });
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
    let changed = crate::platform::native::set_web_look(SystemLook { dark, accent });
    if changed && HOST.with(|h| h.handler.borrow().is_some()) {
        deliver(Event::ThemeChanged);
    }
}

/// The canvas gained or lost keyboard focus.
#[doc(hidden)]
pub fn focus(focused: bool) {
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

/// The file picker for request `id` returned `name` and its `bytes`, or was
/// cancelled (`None`).
#[doc(hidden)]
pub fn file_picked(id: u32, file: Option<(String, Vec<u8>)>) {
    crate::platform::dialog::web_picked(id, file);
    drain();
}
