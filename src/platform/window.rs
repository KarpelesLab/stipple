use crate::geometry::{PhysicalSize, Rect, ScaleFactor, Size};
use crate::platform::event::WindowId;
use crate::render::{Color, Pixmap, Surface};

/// A mouse cursor shape. Elements declare one (see
/// [`Element::cursor`](crate::core::Element::cursor)) and the app sets it on
/// the window as the pointer moves over them, through [`Window::set_cursor`].
///
/// Each backend maps these to the system's own cursors. Where a platform has
/// no exact match the nearest one is used (see `docs/windows-and-cursors.md`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CursorIcon {
    /// The platform's normal arrow.
    #[default]
    Default,
    /// The text I-beam, over editable or selectable text.
    Text,
    /// A pointing hand, over links and buttons.
    Pointer,
    /// A crosshair, for picking a point.
    Crosshair,
    /// The action isn't allowed here (a slashed circle).
    NotAllowed,
    /// An open hand: something can be dragged.
    Grab,
    /// A closed hand: something is being dragged.
    Grabbing,
    /// Four arrows: something can be moved.
    Move,
    /// Resize left–right (a vertical splitter).
    EwResize,
    /// Resize up–down (a horizontal splitter).
    NsResize,
    /// Resize along the top-left to bottom-right diagonal.
    NwseResize,
    /// Resize along the top-right to bottom-left diagonal.
    NeswResize,
}

/// What the input method needs to know about the focused text field, passed to
/// [`Window::set_ime`] while one has keyboard focus.
#[derive(Clone, Debug, PartialEq)]
pub struct ImeState {
    /// The caret (or, while composing, the start of the composition), in
    /// window-relative logical pixels. The IME puts its candidate window next
    /// to it.
    pub cursor_area: Rect,
    /// The field's text around the caret, without the preedit.
    pub surrounding: String,
    /// The caret, as a byte index into `surrounding`.
    pub cursor: usize,
    /// The other end of the selection (equal to `cursor` when nothing is
    /// selected), as a byte index into `surrounding`.
    pub anchor: usize,
}

/// A change to a running window, queued from anywhere with
/// [`AppHandle::window`](crate::core::AppHandle::window) and applied to the
/// primary window on the UI thread.
#[derive(Clone, Debug, PartialEq)]
pub enum WindowRequest {
    /// Maximize (`true`) or restore (`false`) the window.
    Maximized(bool),
    /// Enter (`true`) or leave (`false`) fullscreen.
    Fullscreen(bool),
    /// Set or clear the minimum inner size, in logical pixels.
    MinSize(Option<Size>),
    /// Set or clear the maximum inner size, in logical pixels.
    MaxSize(Option<Size>),
    /// Set or clear the window icon.
    Icon(Option<Pixmap>),
}

impl WindowRequest {
    /// Apply this request to `window`.
    pub fn apply(&self, window: &dyn Window) {
        match self {
            WindowRequest::Maximized(on) => window.set_maximized(*on),
            WindowRequest::Fullscreen(on) => window.set_fullscreen(*on),
            WindowRequest::MinSize(size) => window.set_min_size(*size),
            WindowRequest::MaxSize(size) => window.set_max_size(*size),
            WindowRequest::Icon(icon) => window.set_icon(icon.as_ref()),
        }
    }
}

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
    /// Smallest inner size the user can resize to, in logical pixels.
    pub min_size: Option<Size>,
    /// Largest inner size the user can resize to, in logical pixels.
    pub max_size: Option<Size>,
    /// Open maximized.
    pub maximized: bool,
    /// Open fullscreen (borderless, covering the display).
    pub fullscreen: bool,
    /// The window (or, on macOS, application) icon, as a straight-alpha RGBA
    /// image. Square images of 32 to 256 pixels work best everywhere.
    pub icon: Option<Pixmap>,
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
            min_size: None,
            max_size: None,
            maximized: false,
            fullscreen: false,
            icon: None,
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

    /// Set the smallest inner size, in logical pixels, the user can resize to.
    pub fn with_min_size(mut self, size: Size) -> Self {
        self.min_size = Some(size);
        self
    }

    /// Set the largest inner size, in logical pixels, the user can resize to.
    pub fn with_max_size(mut self, size: Size) -> Self {
        self.max_size = Some(size);
        self
    }

    /// Open the window maximized.
    pub fn with_maximized(mut self, maximized: bool) -> Self {
        self.maximized = maximized;
        self
    }

    /// Open the window fullscreen.
    pub fn with_fullscreen(mut self, fullscreen: bool) -> Self {
        self.fullscreen = fullscreen;
        self
    }

    /// Set the window icon from a straight-alpha RGBA image.
    pub fn with_icon(mut self, icon: Pixmap) -> Self {
        self.icon = Some(icon);
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

    /// Make the title bar dark or light, to match content whose theme changed
    /// (see [`WindowAttributes::dark`]). Default: a no-op. Honored on Windows
    /// and macOS.
    fn set_dark(&self, _dark: bool) {}

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

    /// Show `cursor` while the pointer is over this window's content. Default:
    /// a no-op (the platform's arrow stays).
    fn set_cursor(&self, _cursor: CursorIcon) {}

    /// Set or clear the smallest inner size, in logical pixels. Default: a no-op.
    fn set_min_size(&self, _size: Option<Size>) {}

    /// Set or clear the largest inner size, in logical pixels. Default: a no-op.
    fn set_max_size(&self, _size: Option<Size>) {}

    /// Maximize or restore the window. Default: a no-op.
    fn set_maximized(&self, _maximized: bool) {}

    /// Enter or leave fullscreen. Default: a no-op.
    fn set_fullscreen(&self, _fullscreen: bool) {}

    /// Set or clear the window icon (on macOS, the application's Dock icon).
    /// Default: a no-op.
    fn set_icon(&self, _icon: Option<&Pixmap>) {}

    /// Enable the input method for a focused text field (`Some`), telling it
    /// where the caret is, or disable it (`None`) when focus leaves text entry.
    /// The App calls this after a frame when the state changed. Default: a
    /// no-op. Implemented with `text-input-v3` on Wayland, IMM32 on Windows and
    /// `NSTextInputClient` on macOS.
    fn set_ime(&self, _state: Option<&ImeState>) {}
    /// Show `menu` as the window's (on macOS, the application's) native menu
    /// bar, replacing any earlier one. Picking an item delivers
    /// [`Event::MenuCommand`](crate::platform::Event::MenuCommand) with its id.
    /// Returns whether the platform shows it; if not (the default, and on
    /// Linux, which has no native menu bar) the app draws one in the window.
    fn set_menu_bar(&self, _menu: &crate::platform::menu::NativeMenuBar) -> bool {
        false
    }
    /// Whether a text input has keyboard focus, so a touch platform can show
    /// its on-screen keyboard (and hide it again). The app calls this after
    /// each frame it presents. Default: a no-op. Honored on Android.
    fn set_text_input_active(&self, _active: bool) {}
}

/// Scale `icon` down (by area averaging) so neither side exceeds `max` pixels,
/// keeping its aspect ratio. Returns a copy when it already fits. Backends
/// whose icon property has a size limit (X11's request length) use this.
#[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))]
pub(crate) fn fit_icon(icon: &Pixmap, max: u32) -> Pixmap {
    let src = icon.size();
    if src.width <= max && src.height <= max {
        return icon.clone();
    }
    let k = max as f64 / src.width.max(src.height) as f64;
    let dw = ((src.width as f64 * k).round() as u32).max(1);
    let dh = ((src.height as f64 * k).round() as u32).max(1);
    let mut out = Pixmap::new(PhysicalSize::new(dw, dh));
    let bytes = icon.as_bytes();
    let stride = icon.stride();
    let dst = out.as_bytes_mut();
    for y in 0..dh {
        let y0 = (y as u64 * src.height as u64 / dh as u64) as u32;
        let y1 = (((y + 1) as u64 * src.height as u64).div_ceil(dh as u64) as u32).max(y0 + 1);
        for x in 0..dw {
            let x0 = (x as u64 * src.width as u64 / dw as u64) as u32;
            let x1 = (((x + 1) as u64 * src.width as u64).div_ceil(dw as u64) as u32).max(x0 + 1);
            // Average with alpha weighting, so transparent pixels don't darken
            // the edges.
            let (mut r, mut g, mut b, mut a, mut n) = (0u64, 0u64, 0u64, 0u64, 0u64);
            for sy in y0..y1.min(src.height) {
                for sx in x0..x1.min(src.width) {
                    let i = sy as usize * stride + sx as usize * 4;
                    let pa = bytes[i + 3] as u64;
                    r += bytes[i] as u64 * pa;
                    g += bytes[i + 1] as u64 * pa;
                    b += bytes[i + 2] as u64 * pa;
                    a += pa;
                    n += 1;
                }
            }
            let o = (y as usize * dw as usize + x as usize) * 4;
            if let (Some(r), Some(g), Some(b)) =
                (r.checked_div(a), g.checked_div(a), b.checked_div(a))
            {
                dst[o..o + 3].copy_from_slice(&[r as u8, g as u8, b as u8]);
            }
            dst[o + 3] = (a / n.max(1)) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_icon_downscales_keeping_aspect_and_color() {
        let mut icon = Pixmap::new(PhysicalSize::new(200, 100));
        for px in icon.as_bytes_mut().as_chunks_mut::<4>().0 {
            px.copy_from_slice(&[10, 200, 30, 255]);
        }
        let small = fit_icon(&icon, 64);
        assert_eq!(small.size(), PhysicalSize::new(64, 32));
        assert_eq!(small.pixel(10, 10), Some([10, 200, 30, 255]));
        // Already small enough: unchanged.
        assert_eq!(fit_icon(&small, 64), small);
    }

    #[test]
    fn window_attributes_builders() {
        let a = WindowAttributes::new()
            .with_min_size(Size::new(100.0, 50.0))
            .with_max_size(Size::new(800.0, 600.0))
            .with_maximized(true)
            .with_fullscreen(true)
            .with_icon(Pixmap::new(PhysicalSize::new(16, 16)));
        assert_eq!(a.min_size, Some(Size::new(100.0, 50.0)));
        assert_eq!(a.max_size, Some(Size::new(800.0, 600.0)));
        assert!(a.maximized && a.fullscreen);
        assert_eq!(a.icon.map(|i| i.size()), Some(PhysicalSize::new(16, 16)));
    }
}
