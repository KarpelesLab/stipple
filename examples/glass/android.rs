//! This example as an Android app: `main.rs` as a module, started from
//! `ANativeActivity_onCreate`. It's the cdylib example `glass_android` in
//! Cargo.toml; package it with
//!
//! ```sh
//! STIPPLE_APK_EXAMPLE=glass_android \
//!   STIPPLE_APK_MANIFEST=examples/glass/AndroidManifest.xml \
//!   bash examples/androiddemo/build-apk.sh x86_64 glass.apk
//! ```
//!
//! (`STIPPLE_APK_FEATURES=vk` or `gl` to draw with a GPU renderer.) The
//! window is translucent: the wallpaper shows through its background, at
//! 70% opacity. `examples/glass/test-android.sh` checks it on a device.

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
