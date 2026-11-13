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
//! target; on Linux it tries Wayland, then X11, then headless. On the web
//! (`web`) a `wasm32` module renders frames that a page script blits to a
//! `<canvas>`, and the page drives the loop (see `docs/platforms/web.md`).

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
// The iOS backend's UIKit-free parts, unit-tested on every host.
#[cfg(all(test, not(target_os = "ios")))]
#[path = "ios/a11y_tree.rs"]
mod ios_a11y_tree;
#[cfg(all(test, not(target_os = "ios")))]
#[path = "ios/doc.rs"]
mod ios_doc;
#[cfg(all(test, not(target_os = "ios")))]
#[path = "ios/edge_swipe.rs"]
pub(crate) mod ios_edge_swipe;
#[cfg(all(test, not(target_os = "ios")))]
#[path = "ios/saved_state.rs"]
mod ios_saved_state;

#[cfg(target_os = "android")]
pub mod android;

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)]
pub mod web;

/// Deliver [`Event::ThemeChanged`] ahead of the wake that follows an appearance
/// change, which the OS reports through a notification, a window message or (on
/// Linux) a D-Bus signal heard on another thread, and the events
/// [`shell::post`](crate::platform::shell) queued from other threads (tray
/// picks on Linux).
fn with_theme_changes<H>(mut handler: H) -> impl FnMut(Event, &dyn Window) -> ControlFlow
where
    H: FnMut(Event, &dyn Window) -> ControlFlow,
{
    move |event, window| {
        if matches!(event, Event::Wake) {
            if crate::platform::native::take_changed()
                && handler(Event::ThemeChanged, window) == ControlFlow::Exit
            {
                return ControlFlow::Exit;
            }
            // Tray picks heard on another thread (Linux's D-Bus service).
            for posted in crate::platform::shell::take_posted() {
                if handler(posted, window) == ControlFlow::Exit {
                    return ControlFlow::Exit;
                }
            }
        }
        handler(event, window)
    }
}

/// Run `handler` against the best available native backend, falling back to a
/// one-shot [`headless`] present when no display is reachable.
///
/// The handler receives platform-neutral [`Event`]s and the live [`Window`];
/// returning [`ControlFlow::Exit`] tears the loop down.
///
/// On the web this returns at once: the page keeps calling the handler.
// `mut` is used by the native backends that can fail over (they take
// `&mut handler`); on targets with no such backend the handler is moved
// straight to its only backend, so the `mut` is unused there.
#[cfg_attr(
    not(any(
        target_os = "linux",
        target_os = "windows",
        target_os = "macos",
        target_os = "android"
    )),
    allow(unused_mut)
)]
pub fn run<H>(attrs: WindowAttributes, handler: H)
where
    H: FnMut(Event, &dyn Window) -> ControlFlow + 'static,
{
    let handler = with_theme_changes(handler);
    // Window activation and screen-reader actions for the AT-SPI bridge.
    #[cfg(target_os = "linux")]
    let handler = crate::platform::atspi::wrap(handler);
    let mut handler = handler;
    // The page drives the web's loop, so the handler outlives this call.
    #[cfg(target_arch = "wasm32")]
    {
        web::run(attrs, handler);
    }
    // iOS hands control to UIApplicationMain (which never returns), so it owns
    // the handler outright rather than falling through to headless.
    #[cfg(target_os = "ios")]
    {
        let _ = ios::run(attrs, handler);
    }
    #[cfg(not(any(target_os = "ios", target_arch = "wasm32")))]
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
        #[cfg(target_os = "android")]
        {
            match android::run(attrs.clone(), &mut handler) {
                Ok(()) => return,
                Err(err) => {
                    eprintln!(
                        "stipple: Android backend unavailable ({err}); falling back to headless"
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
