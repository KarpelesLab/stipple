//! Secure keyboard entry while a password field has focus.
//!
//! On macOS, `EnableSecureEventInput` (Carbon's HIToolbox) stops other
//! processes from reading keystrokes through event taps while it is on; the
//! system keeps a count, so every enable is balanced by a disable. Elsewhere
//! there is nothing equivalent to turn on: the IME is turned off for a
//! password field instead (see [`secure_focused`](crate::core::secure_focused)).

/// Turn secure keyboard entry on or off. The App calls this as focus enters
/// and leaves a password field; it does nothing where the platform has no
/// such mode.
pub fn set_secure_input(on: bool) {
    os::set(on);
}

/// Whether secure keyboard entry is on (as far as Stipple turned it on).
pub fn secure_input() -> bool {
    os::get()
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod os {
    use std::sync::atomic::{AtomicBool, Ordering};

    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        fn EnableSecureEventInput() -> i32;
        fn DisableSecureEventInput() -> i32;
    }

    static ON: AtomicBool = AtomicBool::new(false);

    pub(super) fn set(on: bool) {
        // Only on a change, so the system's count stays balanced.
        if ON.swap(on, Ordering::AcqRel) == on {
            return;
        }
        unsafe {
            if on {
                EnableSecureEventInput();
            } else {
                DisableSecureEventInput();
            }
        }
    }

    pub(super) fn get() -> bool {
        ON.load(Ordering::Acquire)
    }
}

#[cfg(not(target_os = "macos"))]
mod os {
    use std::sync::atomic::{AtomicBool, Ordering};

    static ON: AtomicBool = AtomicBool::new(false);

    pub(super) fn set(on: bool) {
        ON.store(on, Ordering::Release);
    }

    pub(super) fn get() -> bool {
        ON.load(Ordering::Acquire)
    }
}
