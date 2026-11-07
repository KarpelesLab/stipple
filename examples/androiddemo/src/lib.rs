//! A `NativeActivity` entry point that renders a Stipple frame onto Android's
//! `ANativeWindow` surface — the runtime half of the Android backend, built as a
//! cdylib (`libstipple_android.so`) that an APK loads via its NativeActivity.
//!
//! Android calls `ANativeActivity_onCreate`; we register an
//! `onNativeWindowCreated` callback, and when the surface arrives we build a
//! Stipple `App`, render one frame, and blit it to the window with
//! `stipple::platform::backend::android::present_to_native_window`. A marker file
//! in the app's data dir lets CI confirm the frame was presented on the
//! emulator.
//!
//! Non-Android builds compile to an empty cdylib so `cargo build --workspace`
//! works everywhere.

#[cfg(target_os = "android")]
mod app {
    use std::ffi::{CString, c_char, c_void};
    use stipple::platform::backend::android::{
        ANativeActivity, ANativeWindow_getHeight, ANativeWindow_getWidth, present_to_native_window,
    };
    use stipple::prelude::*;

    // ANDROID_LOG_INFO. `__android_log_write` is the non-variadic logcat writer.
    const ANDROID_LOG_INFO: i32 = 4;
    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(prio: i32, tag: *const c_char, text: *const c_char) -> i32;
    }

    /// Emit `msg` to logcat under the `stipple` tag (CI greps `adb logcat` for it).
    fn logcat(msg: &str) {
        if let (Ok(tag), Ok(text)) = (CString::new("stipple"), CString::new(msg)) {
            unsafe { __android_log_write(ANDROID_LOG_INFO, tag.as_ptr(), text.as_ptr()) };
        }
    }

    fn view(_state: &(), cx: &mut Cx<()>) -> Element {
        let theme = *cx.theme();
        let card = panel(
            &theme,
            vec![
                label(&theme, "Stipple on Android"),
                divider(&theme),
                setting_row(&theme, Color::rgb(0xef, 0x68, 0x68), "Edit"),
                setting_row(&theme, Color::rgb(0x34, 0xd3, 0x99), "Edit"),
                button_labeled(&theme, "OK"),
            ],
        )
        .width(360.0);
        column(vec![card])
            .grow(1.0)
            .align(Align::Center, Align::Center)
    }

    /// Render a Stipple frame at the surface size and blit it to the window.
    unsafe extern "C" fn on_window_created(_activity: *mut ANativeActivity, window: *mut c_void) {
        unsafe {
            let w = ANativeWindow_getWidth(window).max(1) as f64;
            let h = ANativeWindow_getHeight(window).max(1) as f64;
            let mut app = App::new((), view)
                .theme(Theme::dark())
                .logical_size(Size::new(w, h));
            if let Some(font) = Font::system_default() {
                app = app.font(font);
            }
            let pixmap = app.render_once();
            let ok = present_to_native_window(window, &pixmap);

            // Emit a marker to logcat so CI can confirm the frame was presented.
            logcat(&format!(
                "Stipple Android: window {}x{} presented={}",
                pixmap.size().width,
                pixmap.size().height,
                ok
            ));
        }
    }

    /// The symbol the Android runtime resolves to start a NativeActivity. We only
    /// wire the window-created callback; the rest stay at their defaults.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn ANativeActivity_onCreate(
        activity: *mut ANativeActivity,
        _saved_state: *mut c_void,
        _saved_state_size: usize,
    ) {
        unsafe {
            if activity.is_null() {
                return;
            }
            let cb = (*activity).callbacks;
            if !cb.is_null() {
                (*cb).on_native_window_created = Some(on_window_created);
            }
        }
    }
}
