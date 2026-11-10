//! [`web_app!`]: the exports a browser page calls.

/// Make this crate a web app: generate the C ABI that `web/stipple.js` (in
/// Stipple's repository) calls to run `$make`'s [`App`](crate::App) in a
/// `<canvas>`.
///
/// `$make` is a function (or closure) that returns the app. Call the macro
/// once, at the top level of a `cdylib` crate built for
/// `wasm32-unknown-unknown`; on other targets it expands to nothing, so the
/// crate still builds and tests natively.
///
/// ```ignore
/// use stipple::prelude::*;
///
/// fn app() -> App<State> {
///     App::new(State::default(), view).theme(Theme::dark())
/// }
///
/// stipple::web_app!(app);
/// ```
///
/// The page loads the module with `stipple.js`:
///
/// ```js
/// import { start } from "./stipple.js";
/// await start(document.getElementById("app"), "myapp.wasm", { font: "font.ttf" });
/// ```
///
/// The exports, all prefixed `stipple_`, are an implementation detail shared
/// with `stipple.js`; see `docs/platforms/web.md`.
#[macro_export]
macro_rules! web_app {
    ($make:expr) => {
        #[cfg(target_arch = "wasm32")]
        #[doc(hidden)]
        pub mod __stipple_web_exports {
            #[allow(unused_imports)]
            use super::*;
            use $crate::platform::backend::web as glue;

            #[unsafe(no_mangle)]
            pub extern "C" fn stipple_alloc(len: usize) -> *mut u8 {
                glue::alloc(len)
            }

            /// # Safety
            /// `ptr`/`len` come from one `stipple_alloc(len)`.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn stipple_set_font(ptr: *mut u8, len: usize) {
                glue::set_font(unsafe { glue::take_bytes(ptr, len) });
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn stipple_start(width: f64, height: f64, scale: f64) {
                glue::start(width, height, scale, || ($make)());
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn stipple_resize(width: f64, height: f64, scale: f64) {
                glue::resize(width, height, scale);
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn stipple_pointer(kind: u32, button: u32, x: f64, y: f64, mods: u32) {
                glue::pointer(kind, button, x, y, mods);
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn stipple_wheel(dx: f64, dy: f64, mode: u32) {
                glue::wheel(dx, dy, mode);
            }

            /// # Safety
            /// `ptr`/`len` come from one `stipple_alloc(len)`.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn stipple_key(ptr: *mut u8, len: usize, mods: u32) -> u32 {
                let key = unsafe { glue::take_string(ptr, len) };
                u32::from(glue::key(&key, mods))
            }

            /// # Safety
            /// `ptr`/`len` come from one `stipple_alloc(len)`.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn stipple_paste(ptr: *mut u8, len: usize) {
                glue::paste(unsafe { glue::take_string(ptr, len) });
            }

            /// # Safety
            /// `ptr`/`len` come from one `stipple_alloc(len)`.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn stipple_paste_html(ptr: *mut u8, len: usize) {
                glue::paste_html(unsafe { glue::take_string(ptr, len) });
            }

            /// # Safety
            /// `ptr`/`len` come from one `stipple_alloc(len)`.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn stipple_ime(kind: u32, ptr: *mut u8, len: usize) {
                glue::ime(kind, unsafe { glue::take_string(ptr, len) });
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn stipple_color_scheme(dark: u32, accent: i32) {
                glue::color_scheme(dark != 0, accent);
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn stipple_reduced_motion(reduced: u32) {
                glue::reduced_motion(reduced != 0);
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn stipple_focus(focused: u32) {
                glue::focus(focused != 0);
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn stipple_wake() {
                glue::on_wake();
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn stipple_frame() {
                glue::on_frame();
            }

            /// # Safety
            /// Each pointer/length pair comes from one `stipple_alloc`.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn stipple_file(
                id: u32,
                name: *mut u8,
                name_len: usize,
                data: *mut u8,
                data_len: usize,
            ) {
                let name = unsafe { glue::take_string(name, name_len) };
                let data = unsafe { glue::take_bytes(data, data_len) };
                glue::file_picked(id, Some((name, data)));
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn stipple_file_cancel(id: u32) {
                glue::file_picked(id, None);
            }
        }

        // Natively there's nothing to export, but `$make` still counts as used.
        #[cfg(not(target_arch = "wasm32"))]
        const _: () = {
            let _ = $make;
        };
    };
}
