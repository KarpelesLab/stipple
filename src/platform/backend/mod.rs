//! Backend selection.
//!
//! Each supported OS gets a module here that implements [`Window`](crate::platform::Window)
//! and an event loop against that platform's lowest idiomatic interface, written
//! by hand with no windowing crates:
//!
//! - **Linux**: [`wayland`] (`wl_shm`), then [`x11`] (MIT-SHM), over their wire
//!   protocols.
//! - **Windows**: Win32 (`HWND`, GDI blit).
//! - **macOS**: AppKit (`NSWindow`/`NSView`).
//! - **iOS**: UIKit (`UIApplicationMain`, `UIView`).
//! - **Android**: NDK `NativeActivity` / `ANativeWindow`.
//!
//! [`headless`] is always available, so the rest of the stack builds, runs and is
//! golden-image testable on any target. [`run`] picks the native backend for the
//! target; on Linux it tries Wayland, then X11, then headless. The web has no
//! backend here: a `wasm32` module renders frames that a page blits to a
//! `<canvas>` (see `docs/platforms/web.md`).

use crate::platform::ControlFlow;
use crate::platform::event::Event;
use crate::platform::window::{Window, WindowAttributes};

pub mod headless;

#[cfg(target_os = "linux")]
pub mod wayland;

#[cfg(target_os = "linux")]
pub mod x11;

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "ios")]
pub mod ios;

#[cfg(target_os = "android")]
pub mod android;

/// Run `handler` against the best available native backend, falling back to a
/// one-shot [`headless`] present when no display is reachable.
///
/// The handler receives platform-neutral [`Event`]s and the live [`Window`];
/// returning [`ControlFlow::Exit`] tears the loop down.
// `mut` is used by the native desktop backends (they take `&mut handler`); on
// targets with no native backend (e.g. Android/web) the handler is moved
// straight into the headless fallback, so the `mut` is unused there.
#[cfg_attr(
    not(any(target_os = "linux", target_os = "windows", target_os = "macos")),
    allow(unused_mut)
)]
pub fn run<H>(attrs: WindowAttributes, mut handler: H)
where
    H: FnMut(Event, &dyn Window) -> ControlFlow,
{
    // iOS hands control to UIApplicationMain (which never returns), so it owns
    // the handler outright rather than falling through to headless.
    #[cfg(target_os = "ios")]
    {
        let _ = ios::run(attrs, handler);
    }
    #[cfg(not(target_os = "ios"))]
    {
        #[cfg(target_os = "linux")]
        {
            // Wayland is preferred; fall back to X11, then headless.
            if wayland::available() {
                match wayland::run(attrs.clone(), &mut handler) {
                    Ok(()) => return,
                    Err(err) => {
                        eprintln!(
                            "stipple: Wayland backend unavailable ({err}); falling back to X11/headless"
                        );
                    }
                }
            }
            if x11::available() {
                match x11::run(attrs.clone(), &mut handler) {
                    Ok(()) => return,
                    Err(err) => {
                        eprintln!(
                            "stipple: X11 backend unavailable ({err}); falling back to headless"
                        );
                    }
                }
            }
        }
        #[cfg(target_os = "windows")]
        {
            match windows::run(attrs.clone(), &mut handler) {
                Ok(()) => return,
                Err(err) => {
                    eprintln!(
                        "stipple: Windows backend unavailable ({err}); falling back to headless"
                    );
                }
            }
        }
        #[cfg(target_os = "macos")]
        {
            match macos::run(attrs.clone(), &mut handler) {
                Ok(()) => return,
                Err(err) => {
                    eprintln!(
                        "stipple: macOS backend unavailable ({err}); falling back to headless"
                    );
                }
            }
        }
        // Headless fallback: present one frame, then close.
        let _ = headless::run(
            attrs,
            [Event::RedrawRequested, Event::CloseRequested],
            handler,
        );
    }
}
