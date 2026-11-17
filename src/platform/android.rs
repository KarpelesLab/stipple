//! Calling Android APIs Stipple doesn't wrap: the process's `JavaVM`, the
//! current activity, the application `Context`, starting an activity for a
//! result, and running code on Android's main thread.
//!
//! Stipple has no JNI crate in its API: references are raw `jni.h` pointers
//! ([`jobject`], `*mut `[`JavaVM`]), to use with your own JNI bindings (the
//! `jni` crate's `JavaVM::from_raw` and `JObject::from_raw`, or hand-written
//! calls). Every function here is callable from any thread; Stipple attaches
//! the calling thread to the VM for its own calls as it needs to, and
//! detaches it again.
//!
//! ```ignore
//! use stipple::platform::android;
//!
//! // VpnService.prepare(context): null once the user agreed, otherwise the
//! // consent dialog's intent (made with your JNI bindings).
//! let context = android::application_context().expect("a context");
//! if let Some(intent) = vpn_prepare(context) {
//!     unsafe {
//!         android::start_activity_for_result(intent, |code, _data| {
//!             if code == android::RESULT_OK {
//!                 // Start the VpnService.
//!             }
//!         });
//!     }
//! }
//! ```
//!
//! Ownership: [`activity`] returns a new global reference the caller deletes;
//! [`application_context`] returns Stipple's own, valid for the life of the
//! process, which the caller doesn't delete; the data `Intent` a
//! [`start_activity_for_result`] callback gets is a local reference valid
//! during the callback. See `docs/platforms/android.md`, "Calling Android APIs
//! yourself".
//!
//! Android only.
//!
//! [`jobject`]: crate::platform::android::jobject
//! [`JavaVM`]: crate::platform::android::JavaVM
//! [`activity`]: crate::platform::android::activity()
//! [`application_context`]: crate::platform::android::application_context()
//! [`start_activity_for_result`]: crate::platform::android::start_activity_for_result()

pub use crate::platform::backend::android::java::{
    JavaVM, RESULT_CANCELED, RESULT_OK, activity, application_context, java_vm, jobject,
    run_on_ui_thread, start_activity_for_result,
};
