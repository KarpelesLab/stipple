//! This example as an Android app: `main.rs` as a module, started from
//! `ANativeActivity_onCreate`. It's the cdylib example `cameraview_android`
//! in Cargo.toml; package it with
//!
//! ```sh
//! STIPPLE_APK_EXAMPLE=cameraview_android \
//!   STIPPLE_APK_MANIFEST=examples/cameraview/AndroidManifest.xml \
//!   bash examples/androiddemo/build-apk.sh x86_64 cameraview.apk
//! ```
//!
//! Its `println!`s go to logcat under the `stipple` tag, among them a line
//! every 30 frames (`cameraview: frame N WxH ... luma mean M spread S`),
//! which CI's emulator run reads back.

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
#[path = "main.rs"]
mod example;

#[cfg(target_os = "android")]
mod app {
    use std::ffi::c_void;
    use stipple::platform::backend::android::{self, ANativeActivity};

    /// The symbol Android resolves to start a NativeActivity.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn ANativeActivity_onCreate(
        activity: *mut ANativeActivity,
        saved_state: *mut c_void,
        saved_state_size: usize,
    ) {
        unsafe {
            android::start(
                activity,
                saved_state,
                saved_state_size,
                super::example::main,
            )
        };
    }
}
