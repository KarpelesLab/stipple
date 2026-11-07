//! Stipple on the web: a wasm module exposing a small C ABI that a hand-written
//! JS shim drives — no `wasm-bindgen`, no JS framework (workspace policy in
//! `ROADMAP.md` §1).
//!
//! The module holds a persistent [`App`] so state survives across events. JS:
//! 1. `stipple_alloc(len)` → write font bytes into wasm memory → `stipple_set_font`,
//! 2. `stipple_init(w, h)` to build + render the app,
//! 3. forwards canvas events via `stipple_click` / `stipple_pointer_move` /
//!    `stipple_text`, each of which re-renders,
//! 4. reads the RGBA frame via `stipple_frame_ptr`/`_len`/`_width`/`_height` and
//!    blits it with `putImageData` (a `Pixmap` is straight RGBA8 = `ImageData`).
//!
//! Built for `wasm32-unknown-unknown`; the `Visual` workflow screenshots it in
//! headless Chrome after the page synthesizes clicks.

use std::cell::RefCell;

use stipple::prelude::*;

/// App state: a click counter (the whole canvas is the button).
#[derive(Default)]
struct State {
    clicks: u32,
}

type WebApp = App<State>;

thread_local! {
    static FONT: RefCell<Option<Font>> = const { RefCell::new(None) };
    static APP: RefCell<Option<WebApp>> = const { RefCell::new(None) };
    static FRAME: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static SIZE: RefCell<(u32, u32)> = const { RefCell::new((0, 0)) };
}

fn build(state: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    Element::stack(
        Axis::Horizontal,
        vec![Element::text(
            format!("Clicks: {}", state.clicks),
            48.0,
            theme.palette.on_primary,
        )],
    )
    .fill(theme.palette.primary)
    .align(Align::Center, Align::Center)
    .on_tap(cx, |s: &mut State| s.clicks += 1)
}

fn render_into(app: &mut WebApp) {
    let pixmap = app.render_once();
    let size = pixmap.size();
    FRAME.with(|c| *c.borrow_mut() = pixmap.as_bytes().to_vec());
    SIZE.with(|c| *c.borrow_mut() = (size.width, size.height));
}

fn with_app(f: impl FnOnce(&mut WebApp)) {
    APP.with(|c| {
        if let Some(app) = c.borrow_mut().as_mut() {
            f(app);
        }
    });
}

/// Allocate `len` bytes in wasm memory for JS to write into (e.g. font bytes).
#[unsafe(no_mangle)]
pub extern "C" fn stipple_alloc(len: usize) -> *mut u8 {
    let mut buf = vec![0u8; len];
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}

/// Adopt `len` font bytes previously written at `ptr` (from [`stipple_alloc`]).
///
/// # Safety
/// `ptr`/`len` must come from a single `stipple_alloc(len)` call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn stipple_set_font(ptr: *mut u8, len: usize) {
    let bytes = unsafe { Vec::from_raw_parts(ptr, len, len) };
    if let Ok(font) = Font::from_bytes(bytes) {
        FONT.with(|c| *c.borrow_mut() = Some(font));
    }
}

/// Build the app at `width` × `height` logical pixels and render the first frame.
#[unsafe(no_mangle)]
pub extern "C" fn stipple_init(width: u32, height: u32) {
    let mut app = App::new(State::default(), build)
        .theme(Theme::dark())
        .logical_size(Size::new(width.max(1) as f64, height.max(1) as f64));
    if let Some(font) = FONT.with(|c| c.borrow_mut().take()) {
        app = app.font(font);
    }
    render_into(&mut app);
    APP.with(|c| *c.borrow_mut() = Some(app));
}

/// Deliver a click at `(x, y)` logical pixels and re-render.
#[unsafe(no_mangle)]
pub extern "C" fn stipple_click(x: f64, y: f64) {
    with_app(|app| {
        app.click_at(Point::new(x, y));
        render_into(app);
    });
}

/// Update the hovered element for pointer `(x, y)` and re-render if it changed.
#[unsafe(no_mangle)]
pub extern "C" fn stipple_pointer_move(x: f64, y: f64) {
    with_app(|app| {
        if app.hover_at(Point::new(x, y)) {
            render_into(app);
        }
    });
}

/// Deliver committed text (UTF-8 bytes at `ptr`) to the focused element.
///
/// # Safety
/// `ptr`/`len` must reference `len` valid bytes (e.g. from [`stipple_alloc`]).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn stipple_text(ptr: *const u8, len: usize) {
    let s = unsafe { std::slice::from_raw_parts(ptr, len) };
    let text = String::from_utf8_lossy(s).into_owned();
    with_app(|app| {
        if app.type_text(&text) {
            render_into(app);
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn stipple_frame_ptr() -> *const u8 {
    FRAME.with(|c| c.borrow().as_ptr())
}
#[unsafe(no_mangle)]
pub extern "C" fn stipple_frame_len() -> usize {
    FRAME.with(|c| c.borrow().len())
}
#[unsafe(no_mangle)]
pub extern "C" fn stipple_width() -> u32 {
    SIZE.with(|c| c.borrow().0)
}
#[unsafe(no_mangle)]
pub extern "C" fn stipple_height() -> u32 {
    SIZE.with(|c| c.borrow().1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_and_click_update_state() {
        stipple_init(200, 120);
        assert_eq!((stipple_width(), stipple_height()), (200, 120));
        assert_eq!(stipple_frame_len(), 200 * 120 * 4);
        // Whole canvas is the button; a click increments the counter.
        stipple_click(100.0, 60.0);
        APP.with(|c| assert_eq!(c.borrow().as_ref().unwrap().state().clicks, 1));
    }
}
