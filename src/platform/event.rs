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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
    PageUp,
    PageDown,
    Insert,
    Delete,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    /// Either Alt key (Option on macOS), pressed or released on its own. A
    /// press and release with no other key in between focuses an in-window
    /// menu bar.
    Alt,
    /// The context-menu ("Menu" / "Apps") key.
    ContextMenu,
    /// A printable character key, identified by its base character.
    Char(char),
    /// Anything not yet mapped, carrying the backend's raw scancode.
    Unidentified(u32),
}

/// A scroll delta in logical pixels (precise/trackpad) — line-based wheels are
/// normalized to an approximate pixel delta by the backend (40 px per notch).
/// Positive `dy` scrolls down (towards the end of the content), positive `dx`
/// to the right.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollDelta {
    pub dx: f64,
    pub dy: f64,
}

/// Where a touch is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchPhase {
    /// The finger went down.
    Started,
    /// The finger moved (or another finger did: backends may report every
    /// finger still down on each move).
    Moved,
    /// The finger lifted.
    Ended,
    /// The system took the touch away (a gesture of its own, an incoming call):
    /// whatever it started should be undone rather than completed.
    Cancelled,
}

/// One finger on a touch screen.
///
/// Every finger is reported, each with an `id` that stays the same from
/// [`Started`](TouchPhase::Started) until it ends. The first finger down is
/// also the pointer: backends follow each of its touch events with the
/// matching [`Event::PointerMoved`] / [`Event::PointerButton`], so widgets
/// that only know the pointer keep working on touch screens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Touch {
    /// Identifies the finger while it's down; ids may be reused afterwards.
    pub id: u64,
    pub phase: TouchPhase,
    /// Logical pixels, window-relative.
    pub position: Point,
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
    /// The loop was woken by [`wake::wake`](crate::platform::wake::wake) or by
    /// the deadline set with [`wake::set_deadline`](crate::platform::wake::set_deadline).
    Wake,
    /// Keyboard focus entered (`true`) or left (`false`) the window.
    FocusChanged(bool),
    /// The system's light/dark mode or accent color changed. Read the new look
    /// with [`Theme::native`](crate::style::Theme::native).
    ThemeChanged,

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
    /// A finger on a touch screen started, moved, lifted or was cancelled.
    /// Sent before the pointer events the first finger also produces.
    Touch(Touch),

    /// A key changed state.
    Key {
        code: KeyCode,
        state: ButtonState,
        modifiers: Modifiers,
    },
    /// Committed text from the keyboard or IME (already composed).
    Text(String),
    /// An input method (IME) or compose-sequence update: composition (preedit)
    /// text to show at the caret, text to commit, or text to delete around the
    /// caret. See `docs/ime.md`.
    Ime(Ime),
    /// The user picked the item with this id from the native menu bar (see
    /// [`Window::set_menu_bar`](crate::platform::Window::set_menu_bar)).
    MenuCommand(u32),
    /// The user clicked the tray icon (see
    /// [`Window::set_tray`](crate::platform::Window::set_tray)); its menu
    /// items arrive as [`MenuCommand`](Event::MenuCommand)s instead.
    TrayActivated,
}

/// An update from an input method (Chinese, Japanese and Korean IMEs, and dead
/// keys and compose sequences, which Stipple runs as a tiny input method on
/// X11 and Wayland).
///
/// While the user composes, the IME sends [`Preedit`](Ime::Preedit) text, which
/// the focused text field shows underlined at the caret without making it part
/// of its text; [`Commit`](Ime::Commit) replaces the preedit with final text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ime {
    /// The composition in progress. An empty `text` ends it. `cursor` is the
    /// byte range within `text` the IME highlights (equal ends for a plain
    /// caret); `None` hides the caret.
    Preedit {
        text: String,
        cursor: Option<(usize, usize)>,
    },
    /// Final text: clears the preedit and inserts `text` at the caret.
    Commit(String),
    /// Delete `before` bytes before the caret and `after` bytes after it (the
    /// preedit not counted), as Wayland's `text-input-v3` asks some IMEs to.
    DeleteSurrounding { before: usize, after: usize },
}
