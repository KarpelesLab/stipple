//! Platform-neutral input and window events.
//!
//! Each backend translates its native event stream into these types so the
//! rest of Stipple never sees an X11 `XEvent`, an AppKit `NSEvent`, or a Win32
//! `WM_*` message.

use crate::geometry::{PhysicalSize, Point, ScaleFactor};

/// Opaque, process-unique identifier for a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WindowId(pub(crate) u64);

/// Whether a button or key is being pressed or released.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonState {
    Pressed,
    Released,
}

/// A pointer (mouse / pen / single touch) button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Left,
    Right,
    Middle,
    Other(u16),
}

/// Active keyboard modifier keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// The "logo"/Command/Super/Windows key.
    pub meta: bool,
}

/// A small, platform-neutral key identity.
///
/// This is deliberately a coarse subset for the scaffold; full physical/logical
/// key mapping (à la the W3C UI Events `code`/`key` split) lands with real
/// keyboard input in Phase 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum KeyCode {
    Enter,
    Escape,
    Backspace,
    Tab,
    Space,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    Home,
    End,
    Delete,
    /// A printable character key, identified by its base character.
    Char(char),
    /// Anything not yet mapped, carrying the backend's raw scancode.
    Unidentified(u32),
}

/// A scroll delta in logical pixels (precise/trackpad) — line-based wheels are
/// normalized to an approximate pixel delta by the backend.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollDelta {
    pub dx: f64,
    pub dy: f64,
}

/// An event delivered to the application for a specific window.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Event {
    /// The window should repaint. Emitted after `request_redraw` or when the
    /// system invalidates the surface.
    RedrawRequested,
    /// The drawable was resized to the given physical-pixel size.
    Resized(PhysicalSize),
    /// The DPI scale factor changed (e.g. the window moved to another monitor).
    ScaleFactorChanged(ScaleFactor),
    /// The user requested the window be closed (title-bar button, ⌘W, …).
    CloseRequested,
    /// Keyboard focus entered (`true`) or left (`false`) the window.
    FocusChanged(bool),

    /// The pointer moved to `position` (logical pixels, window-relative).
    PointerMoved { position: Point },
    /// The pointer left the window surface.
    PointerLeft,
    /// A pointer button changed state at `position`.
    PointerButton {
        button: PointerButton,
        state: ButtonState,
        position: Point,
    },
    /// A scroll/wheel gesture.
    Scroll { delta: ScrollDelta },

    /// A key changed state.
    Key {
        code: KeyCode,
        state: ButtonState,
        modifiers: Modifiers,
    },
    /// Committed text from the keyboard or IME (already composed).
    Text(String),
}
