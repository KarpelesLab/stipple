//! Keeping the screen awake on desktops, for
//! [`WindowRequest::KeepAwake`](crate::platform::WindowRequest::KeepAwake):
//!
//! - **macOS**: an IOKit power assertion,
//!   `kIOPMAssertionTypePreventUserIdleDisplaySleep`, held until released.
//! - **Windows**: `SetThreadExecutionState(ES_CONTINUOUS |
//!   ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED)` on the UI thread, which the
//!   request is applied on (the state belongs to the thread that set it).
//! - **Linux**: the XDG desktop portal's `Inhibit` (idle), else
//!   `org.freedesktop.ScreenSaver.Inhibit`, over the hand-written D-Bus
//!   client. Both last while the connection does, so it is kept open until
//!   the app lets the screen sleep again.
//!
//! The mobile and web backends do it their own way (see their
//! `set_keep_awake`). Elsewhere it is a no-op.

/// Keep the screen awake (`true`) or let it sleep again. Idempotent.
pub(crate) fn set(on: bool) {
    imp::set(on);
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod imp {
    use std::ffi::{c_char, c_void};
    use std::sync::Mutex;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPMAssertionCreateWithName(
            kind: *const c_void,
            level: u32,
            name: *const c_void,
            id: *mut u32,
        ) -> i32;
        fn IOPMAssertionRelease(id: u32) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            s: *const c_char,
            encoding: u32,
        ) -> *const c_void;
        fn CFRelease(cf: *const c_void);
    }
    const UTF8: u32 = 0x0800_0100;
    const LEVEL_ON: u32 = 255;

    /// The assertion held, if any.
    static HELD: Mutex<Option<u32>> = Mutex::new(None);

    pub(super) fn set(on: bool) {
        let mut held = HELD.lock().unwrap_or_else(|e| e.into_inner());
        match (on, *held) {
            (true, None) => unsafe {
                let kind = CFStringCreateWithCString(
                    std::ptr::null(),
                    c"PreventUserIdleDisplaySleep".as_ptr(),
                    UTF8,
                );
                let name =
                    CFStringCreateWithCString(std::ptr::null(), c"Stipple app".as_ptr(), UTF8);
                let mut id = 0;
                if !kind.is_null()
                    && !name.is_null()
                    && IOPMAssertionCreateWithName(kind, LEVEL_ON, name, &mut id) == 0
                {
                    *held = Some(id);
                }
                for s in [kind, name] {
                    if !s.is_null() {
                        CFRelease(s);
                    }
                }
            },
            (false, Some(id)) => {
                unsafe { IOPMAssertionRelease(id) };
                *held = None;
            }
            _ => {}
        }
    }
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod imp {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetThreadExecutionState(flags: u32) -> u32;
    }
    const ES_SYSTEM_REQUIRED: u32 = 0x1;
    const ES_DISPLAY_REQUIRED: u32 = 0x2;
    const ES_CONTINUOUS: u32 = 0x8000_0000;

    pub(super) fn set(on: bool) {
        let flags = if on {
            ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED
        } else {
            ES_CONTINUOUS
        };
        unsafe { SetThreadExecutionState(flags) };
    }
}

#[cfg(any(target_os = "linux", target_os = "fullrust"))]
mod imp {
    use crate::platform::a11y::{DBus, read_string, read_u32};
    use crate::platform::shell::linux::{W, app_name, debug};
    use std::sync::Mutex;
    use std::time::Duration;

    /// What holds the screen awake: the connection the inhibition lives
    /// with, and how to end it.
    enum Held {
        /// The portal's request object.
        Portal(DBus, String),
        /// The screen saver's cookie.
        ScreenSaver(DBus, u32),
    }

    static HELD: Mutex<Option<Held>> = Mutex::new(None);
    /// The latest wish, which the worker threads catch up with.
    static WANT: Mutex<bool> = Mutex::new(false);

    const REASON: &str = "Keeping the screen awake";

    pub(super) fn set(on: bool) {
        *WANT.lock().unwrap_or_else(|e| e.into_inner()) = on;
        // The bus may take a moment (a service started on demand): off the
        // UI thread.
        let _ = std::thread::Builder::new()
            .name("stipple-keep-awake".into())
            .spawn(sync);
    }

    /// Bring the inhibition in line with the latest wish.
    fn sync() {
        let mut held = HELD.lock().unwrap_or_else(|e| e.into_inner());
        let want = *WANT.lock().unwrap_or_else(|e| e.into_inner());
        match (want, held.is_some()) {
            (true, false) => match inhibit() {
                Ok(h) => *held = Some(h),
                Err(e) => eprintln!("stipple: can't keep the screen awake: {e}"),
            },
            (false, true) => {
                if let Some(h) = held.take() {
                    release(h);
                }
            }
            _ => {}
        }
    }

    fn inhibit() -> Result<Held, String> {
        let mut bus = DBus::connect_session()?;
        bus.set_read_timeout(Some(Duration::from_secs(5)));
        // The portal: Inhibit(window, flags, options) with flags 8, idle.
        let mut w = W::default();
        w.str("");
        w.u32(8);
        w.array(8, |w| {
            w.strukt(|w| {
                w.str("reason");
                w.variant("s", |w| w.str(REASON));
            })
        });
        match bus.call_with_body(
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Inhibit",
            "Inhibit",
            "sua{sv}",
            &w.buf,
        ) {
            Ok(reply) => {
                let handle = read_string(&reply, &mut 0)?;
                debug(format_args!("portal Inhibit: {handle}"));
                return Ok(Held::Portal(bus, handle));
            }
            Err(e) => debug(format_args!(
                "portal Inhibit failed ({e}); the screen saver"
            )),
        }
        let mut w = W::default();
        w.str(&app_name());
        w.str(REASON);
        let reply = bus.call_with_body(
            "org.freedesktop.ScreenSaver",
            "/org/freedesktop/ScreenSaver",
            "org.freedesktop.ScreenSaver",
            "Inhibit",
            "ss",
            &w.buf,
        )?;
        let cookie = read_u32(&reply, &mut 0)?;
        debug(format_args!("ScreenSaver Inhibit: {cookie}"));
        Ok(Held::ScreenSaver(bus, cookie))
    }

    /// End the inhibition (closing the connection would too).
    fn release(held: Held) {
        match held {
            Held::Portal(mut bus, handle) => {
                let _ = bus.call_with_body(
                    "org.freedesktop.portal.Desktop",
                    &handle,
                    "org.freedesktop.portal.Request",
                    "Close",
                    "",
                    &[],
                );
            }
            Held::ScreenSaver(mut bus, cookie) => {
                let mut w = W::default();
                w.u32(cookie);
                let _ = bus.call_with_body(
                    "org.freedesktop.ScreenSaver",
                    "/org/freedesktop/ScreenSaver",
                    "org.freedesktop.ScreenSaver",
                    "UnInhibit",
                    "u",
                    &w.buf,
                );
            }
        }
    }
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    target_os = "linux",
    target_os = "fullrust"
)))]
mod imp {
    pub(super) fn set(_on: bool) {}
}
