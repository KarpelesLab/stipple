use crate::geometry::{PhysicalSize, ScaleFactor, Size};
use crate::platform::event::WindowId;
use crate::render::{Color, Surface};

/// Desired initial properties for a window. Consumed by the backend when
/// creating the window; later changes go through [`Window`] methods.
#[derive(Clone, Debug)]
pub struct WindowAttributes {
    pub title: String,
    /// Initial inner (content) size in logical pixels.
    pub logical_size: Size,
    pub resizable: bool,
    /// Optional initial top-left position in logical pixels. `None` lets the
    /// window manager place the window (and, with no WM, the server uses 0,0).
    /// Used to lay multiple windows out side by side.
    pub position: Option<(i32, i32)>,
    /// Whether the window's title bar should be dark, to match its content.
    /// `None` leaves it to the OS. Honored on Windows and macOS.
    pub dark: Option<bool>,
    /// The color behind the content, for any window area the app doesn't draw.
    /// Honored on iOS, where the safe area leaves such areas around the content.
    pub background: Option<Color>,
}

impl Default for WindowAttributes {
    fn default() -> Self {
        Self {
            title: "Stipple".to_string(),
            logical_size: Size::new(800.0, 600.0),
            resizable: true,
            position: None,
            dark: None,
            background: None,
        }
    }
}

impl WindowAttributes {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn with_logical_size(mut self, size: Size) -> Self {
        self.logical_size = size;
        self
    }

    pub fn resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }

    /// Set the initial top-left position in logical pixels.
    pub fn with_position(mut self, x: i32, y: i32) -> Self {
        self.position = Some((x, y));
        self
    }
}

/// A live native window.
///
/// Backends implement this; the rest of Stipple drives windows only through this
/// trait. A window can hand out a [`Surface`] (via [`Window::create_surface`])
/// that the renderer presents into.
pub trait Window {
    fn id(&self) -> WindowId;

    /// Current drawable size in physical pixels.
    fn inner_size(&self) -> PhysicalSize;

    /// Current DPI scale factor (physical ÷ logical).
    fn scale_factor(&self) -> ScaleFactor;

    /// Ask the system to deliver a [`RedrawRequested`](crate::platform::Event::RedrawRequested).
    fn request_redraw(&self);

    /// Update the window title.
    fn set_title(&self, title: &str);

    /// Push the current accessibility tree so the OS assistive-technology API can
    /// read the full element hierarchy (not just the window root). Default: a
    /// no-op — backends without an AT bridge ignore it. The macOS (`NSAccessibility`)
    /// and Windows (UI Automation) backends override this to vend `root`'s
    /// descendants as native accessibility elements.
    fn set_accessibility_tree(&self, _root: &crate::platform::access::A11yNode) {}

    /// Create a [`Surface`] that presents into this window's drawable.
    fn create_surface(&self) -> Box<dyn Surface>;

    /// Read the system clipboard's text, if any. Default: `None` (backends that
    /// don't implement the clipboard fall back to the in-process mirror).
    fn clipboard(&self) -> Option<String> {
        None
    }

    /// Set the system clipboard's text. Default: a no-op.
    fn set_clipboard(&self, _text: &str) {}

    /// Open a new top-level sibling window sharing this window's event loop /
    /// connection, returning its [`WindowId`]. Default: `None` — the backend is
    /// single-window. Backends that support multiple native windows (X11)
    /// override this; the new window's events arrive through the same handler,
    /// distinguished by [`Window::id`].
    fn open_window(&self, _attrs: WindowAttributes) -> Option<WindowId> {
        None
    }

    /// Request that *this* window be closed and removed from the event loop.
    /// Default: a no-op (single-window backends instead end the loop when the
    /// handler returns [`ControlFlow::Exit`](crate::platform::ControlFlow)). Multi-window
    /// backends destroy just this window and keep running while others remain.
    fn close_window(&self) {}
}
