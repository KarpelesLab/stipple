use crate::geometry::{Insets, PhysicalSize, Point, Rect, ScaleFactor, Size};
use crate::platform::event::WindowId;
use crate::platform::screen::{Orientation, Orientations};
use crate::render::{Color, Pixmap, Surface};

/// A mouse cursor shape. Elements declare one (see
/// [`Element::cursor`](crate::core::Element::cursor)) and the app sets it on
/// the window as the pointer moves over them.
///
/// Each backend maps these to the system's own cursors. Where a platform has
/// no exact match the nearest one is used (see `docs/windows-and-cursors.md`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
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
    /// No cursor: the pointer is hidden while over the element (a terminal
    /// hiding it while the user types, a video playing). It shows again when
    /// the pointer leaves the window, or the element asks for another cursor.
    Hidden,
}

/// What the input method needs to know about the focused text field, handed
/// to the window while one has keyboard focus.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
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
/// [`AppHandle::window`](crate::core::AppHandle::window) (the primary window)
/// or [`AppHandle::window_to`](crate::core::AppHandle::window_to) (any window)
/// and applied on the UI thread.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
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
    /// Change the title.
    Title(String),
    /// Move the window's outer top-left corner (its frame, where the system
    /// draws one) to this point, in logical pixels of the desktop. Wayland
    /// lets no client place its windows, so it is a no-op there.
    Position(Point),
    /// Resize the content area to this size, in logical pixels.
    InnerSize(Size),
    /// Make the user's resizing snap to whole steps of this size (in logical
    /// pixels) above the minimum size, such as a terminal's character cell;
    /// `None` resizes freely. Not available on Wayland.
    ResizeIncrements(Option<Size>),
    /// Minimize (`true`) or restore (`false`) the window.
    Minimized(bool),
    /// Ask for the user's attention (`Some`), or withdraw the request
    /// (`None`).
    Attention(Option<UserAttention>),
    /// Sound the system beep.
    Beep,
    /// Show a short badge on the application's icon, or remove it.
    Badge(Option<String>),
    /// Lock the pointer (`true`): hide the cursor, hold it in place and send
    /// relative motion as [`Event::PointerDelta`](crate::platform::Event::PointerDelta),
    /// or release it (`false`).
    PointerLock(bool),
    /// Keep the screen from dimming and locking while the app shows
    /// (`true`), or let it again (`false`). Applies to the app, whichever
    /// window it is sent to. See
    /// [`AppHandle::set_keep_awake`](crate::core::AppHandle::set_keep_awake).
    KeepAwake(bool),
    /// Let a phone or tablet turn the app only these ways. Applies to the
    /// app, whichever window it is sent to; a no-op on desktops. See
    /// [`AppHandle::set_orientations`](crate::core::AppHandle::set_orientations).
    Orientations(Orientations),
    /// Bring the window to the front, restored if minimized, with the
    /// keyboard focus: for the window that shows a
    /// [`Launch`](crate::platform::Launch) the running instance heard, with
    /// its [`activation_token`](crate::platform::Launch::activation_token),
    /// which Wayland compositors ask for (and X11 window managers use) before
    /// they let a window take the focus. Without a token, Wayland only marks
    /// the window as wanting attention.
    Activate(Option<String>),
}

/// How insistently [`WindowRequest::Attention`] asks for the user's
/// attention.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UserAttention {
    /// Something happened the user may want to look at: the taskbar button
    /// flashes a few times, the Dock icon bounces once.
    #[default]
    Informational,
    /// Something needs the user: the taskbar button flashes and the Dock icon
    /// bounces until the window is activated.
    Critical,
}

impl WindowRequest {
    /// Apply this request to `window`.
    pub(crate) fn apply(&self, window: &dyn Window) {
        match self {
            WindowRequest::Maximized(on) => window.set_maximized(*on),
            WindowRequest::Fullscreen(on) => window.set_fullscreen(*on),
            WindowRequest::MinSize(size) => window.set_min_size(*size),
            WindowRequest::MaxSize(size) => window.set_max_size(*size),
            WindowRequest::Icon(icon) => window.set_icon(icon.as_ref()),
            WindowRequest::Attention(kind) => window.request_attention(*kind),
            WindowRequest::Beep => window.beep(),
            WindowRequest::Badge(label) => window.set_badge(label.as_deref()),
            WindowRequest::Title(title) => window.set_title(title),
            WindowRequest::Position(at) => window.set_position(*at),
            WindowRequest::InnerSize(size) => window.set_inner_size(*size),
            WindowRequest::ResizeIncrements(step) => window.set_resize_increments(*step),
            WindowRequest::Minimized(on) => window.set_minimized(*on),
            WindowRequest::PointerLock(on) => {
                window.set_pointer_lock(*on);
            }
            WindowRequest::KeepAwake(on) => window.set_keep_awake(*on),
            WindowRequest::Orientations(set) => window.set_orientations(*set),
            WindowRequest::Activate(token) => window.activate(token.as_deref()),
        }
    }
}

/// Where a window is and what state it is in, as last reported by the system.
/// Read it from a view with
/// [`Cx::window_info`](crate::core::Cx::window_info) or from anywhere with
/// [`AppHandle::window_info`](crate::core::AppHandle::window_info).
///
/// Sizes and positions are in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct WindowInfo {
    /// The outer (frame) top-left corner on the desktop, or `None` where the
    /// system doesn't tell (Wayland, which hides window positions from
    /// clients).
    pub position: Option<Point>,
    /// The content area's size.
    pub inner_size: Size,
    /// The size with the frame (title bar and borders) the system draws, or
    /// the inner size where that frame is unknown.
    pub outer_size: Size,
    /// The scale factor (physical pixels per logical pixel).
    pub scale: ScaleFactor,
    /// Whether the window is maximized.
    pub maximized: bool,
    /// Whether the window is minimized (iconified, hidden in the Dock or
    /// taskbar).
    pub minimized: bool,
    /// Whether the window is fullscreen.
    pub fullscreen: bool,
    /// The work area (the monitor without its panels, docks and task bars)
    /// of the monitor the window is on, in desktop coordinates, or `None`
    /// where unknown.
    pub work_area: Option<Rect>,
    /// How far in from each edge of the content area the system's own
    /// things are drawn over it: the status bar, a notch or Dynamic Island,
    /// the home indicator or navigation bar, rounded corners. Zero unless
    /// the app draws [edge to edge](crate::App::edge_to_edge) on iOS or
    /// Android (or a web page that covers a phone's notch); zero on
    /// desktops. Read it from a view with
    /// [`Cx::safe_area`](crate::core::Cx::safe_area).
    pub safe_area: Insets,
    /// How far up from the bottom of the content area the on-screen
    /// keyboard covers it, in logical pixels: zero while it is hidden, and
    /// on desktops. The App lays the view out above it and keeps the
    /// focused text field in view; read it from a view with
    /// [`Cx::keyboard_inset`](crate::core::Cx::keyboard_inset).
    pub keyboard_inset: f64,
}

impl WindowInfo {
    /// What is known of a window from its size and scale alone.
    pub fn from_size(inner: PhysicalSize, scale: ScaleFactor) -> Self {
        let size = scale.to_logical(inner);
        Self {
            position: None,
            inner_size: size,
            outer_size: size,
            scale,
            maximized: false,
            minimized: false,
            fullscreen: false,
            work_area: None,
            safe_area: Insets::ZERO,
            keyboard_inset: 0.0,
        }
    }

    /// Which way the window is turned: [`Landscape`](Orientation::Landscape)
    /// when it is wider than tall. A view picks its portrait or landscape
    /// layout from it (or from the size itself, which says more).
    pub fn orientation(&self) -> Orientation {
        if self.inner_size.width > self.inner_size.height {
            Orientation::Landscape
        } else {
            Orientation::Portrait
        }
    }
}

impl Default for WindowInfo {
    fn default() -> Self {
        Self::from_size(PhysicalSize::new(0, 0), ScaleFactor::IDENTITY)
    }
}

/// Desired initial properties for a window. Consumed by the backend when
/// creating the window; later changes go through `Window` methods.
#[derive(Clone, Debug)]
#[non_exhaustive]
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
    /// Whether the window has a title bar and borders. Turned off, the app
    /// draws everything (and moves the window itself, if it wants). Honored
    /// on Wayland (server-side through xdg-decoration, else drawn by
    /// Stipple), X11 (`_MOTIF_WM_HINTS`) and Windows; macOS keeps its title bar.
    pub decorations: bool,
    /// Snap the user's resizing to whole steps of this size, in logical
    /// pixels (see [`WindowRequest::ResizeIncrements`]).
    pub resize_increments: Option<Size>,
    /// Whether a window opened while the app runs takes keyboard focus (and
    /// comes to the front). `true` by default; the window manager may still
    /// decide otherwise.
    pub focus: bool,
    /// Draw under the status bar, the notch and the home indicator or
    /// navigation bar, and read where they are from
    /// [`WindowInfo::safe_area`] (iOS and Android; see
    /// [`App::edge_to_edge`](crate::App::edge_to_edge)). `false` by default.
    pub edge_to_edge: bool,
    /// The ways a phone or tablet may turn the app, or `None` to leave it to
    /// the bundle (`Info.plist`, the manifest). See
    /// [`App::orientations`](crate::App::orientations).
    pub orientations: Option<Orientations>,
    /// The window this one belongs to (a dialog's terminal window, say): it
    /// stays above it, opens centered on it, and goes away with it.
    /// `WM_TRANSIENT_FOR` and the dialog window type on X11,
    /// `xdg_toplevel.set_parent` on Wayland, the owner window on Windows,
    /// and a child window (or, with [`sheet`](Self::sheet), a sheet) on
    /// macOS. The id is the app's (from
    /// [`App::open_window`](crate::App::open_window),
    /// [`AppHandle::open_window`](crate::core::AppHandle::open_window) or
    /// [`Cx::window_id`](crate::core::Cx::window_id)); the app hands the
    /// backend its own. `None` by default.
    pub parent: Option<WindowId>,
    /// While this window is open, its [`parent`](Self::parent) takes no
    /// input: the parent is disabled on Windows, and marked modal for the
    /// window manager on X11 (`_NET_WM_STATE_MODAL`); everywhere the app
    /// drops the parent's pointer and key input meanwhile. `false` by
    /// default; it needs a parent.
    pub modal: bool,
    /// On macOS, show the window as a sheet sliding out of its
    /// [`parent`](Self::parent)'s title bar, which blocks the parent (as
    /// [`modal`](Self::modal) does) until it closes. Elsewhere it's a
    /// modal window. `false` by default; it needs a parent.
    pub sheet: bool,
    /// Keep the frame's alpha all the way to the screen, so what is behind
    /// the window shows through wherever the app paints with less than full
    /// alpha (a theme whose `palette.background` is translucent, say).
    /// `false` by default. Honored on macOS, Windows, X11 (with a compositing
    /// manager running) and Wayland; see `docs/windows-and-cursors.md`.
    pub transparent: bool,
    /// Blur what shows through a [`transparent`](Self::transparent) window,
    /// where the system offers it: a vibrancy view on macOS, the acrylic
    /// backdrop on Windows 11, `ext-background-effect` or KWin's blur on
    /// Wayland, `_KDE_NET_WM_BLUR_BEHIND_REGION` on X11. Implies
    /// `transparent`.
    pub blur: bool,
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
            decorations: true,
            resize_increments: None,
            focus: true,
            edge_to_edge: false,
            orientations: None,
            parent: None,
            modal: false,
            sheet: false,
            transparent: false,
            blur: false,
        }
    }
}

impl WindowAttributes {
    /// The defaults: an 800×600 resizable, decorated window titled "Stipple".
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the title.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Set the initial inner (content) size, in logical pixels.
    pub fn logical_size(mut self, size: Size) -> Self {
        self.logical_size = size;
        self
    }

    /// Whether the user can resize the window (`true` by default).
    pub fn resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }

    /// Ask for a dark (`true`) or light title bar, to match the content.
    pub fn dark(mut self, dark: bool) -> Self {
        self.dark = Some(dark);
        self
    }

    /// Set the color behind the content, for areas the app doesn't draw.
    pub fn background(mut self, color: Color) -> Self {
        self.background = Some(color);
        self
    }

    /// Set the initial top-left position in logical pixels.
    pub fn position(mut self, x: i32, y: i32) -> Self {
        self.position = Some((x, y));
        self
    }

    /// Set the smallest inner size, in logical pixels, the user can resize to.
    pub fn min_size(mut self, size: Size) -> Self {
        self.min_size = Some(size);
        self
    }

    /// Set the largest inner size, in logical pixels, the user can resize to.
    pub fn max_size(mut self, size: Size) -> Self {
        self.max_size = Some(size);
        self
    }

    /// Open the window maximized.
    pub fn maximized(mut self, maximized: bool) -> Self {
        self.maximized = maximized;
        self
    }

    /// Open the window fullscreen.
    pub fn fullscreen(mut self, fullscreen: bool) -> Self {
        self.fullscreen = fullscreen;
        self
    }

    /// Set the window icon from a straight-alpha RGBA image.
    pub fn icon(mut self, icon: Pixmap) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Give the window a title bar and borders (the default), or not.
    pub fn decorations(mut self, decorations: bool) -> Self {
        self.decorations = decorations;
        self
    }

    /// Snap the user's resizing to whole steps of `step` logical pixels.
    pub fn resize_increments(mut self, step: Size) -> Self {
        self.resize_increments = Some(step);
        self
    }

    /// Whether the window takes keyboard focus when it opens while the app
    /// runs (`true` by default).
    pub fn focus(mut self, focus: bool) -> Self {
        self.focus = focus;
        self
    }

    /// Draw under the system's bars, the notch and the home indicator
    /// (see [`edge_to_edge`](Self::edge_to_edge)).
    pub fn edge_to_edge(mut self, on: bool) -> Self {
        self.edge_to_edge = on;
        self
    }

    /// Let a phone or tablet turn the app only these ways.
    pub fn orientations(mut self, set: Orientations) -> Self {
        self.orientations = Some(set);
        self
    }

    /// Make the window belong to window `parent` (see
    /// [`parent`](Self::parent)): above it, centered on it, gone with it.
    pub fn parent(mut self, parent: WindowId) -> Self {
        self.parent = Some(parent);
        self
    }

    /// Block the [`parent`](Self::parent) while this window is open (see
    /// [`modal`](Self::modal)).
    pub fn modal(mut self, modal: bool) -> Self {
        self.modal = modal;
        self
    }

    /// On macOS, a sheet on the [`parent`](Self::parent) (see
    /// [`sheet`](Self::sheet)); a modal window elsewhere.
    pub fn sheet(mut self, sheet: bool) -> Self {
        self.sheet = sheet;
        self
    }

    /// Whether the window blocks its parent: [`modal`](Self::modal) or a
    /// [`sheet`](Self::sheet), with a parent.
    pub(crate) fn blocks_parent(&self) -> bool {
        self.parent.is_some() && (self.modal || self.sheet)
    }

    /// Let what is behind the window show through where the app paints
    /// translucently (see [`transparent`](Self::transparent)).
    pub fn transparent(mut self, on: bool) -> Self {
        self.transparent = on;
        self
    }

    /// Blur what shows through the window, where the system can (see
    /// [`blur`](Self::blur)); turns on [`transparent`](Self::transparent).
    pub fn blur(mut self, on: bool) -> Self {
        self.blur = on;
        if on {
            self.transparent = true;
        }
        self
    }

    /// Whether the window keeps its frame's alpha: asked for transparency,
    /// or for blur, which implies it. (The phones and the web have no
    /// transparent windows.)
    #[cfg_attr(
        not(any(target_os = "macos", target_os = "windows", target_os = "linux")),
        allow(dead_code)
    )]
    pub(crate) fn is_transparent(&self) -> bool {
        self.transparent || self.blur
    }
}

/// A window's GPU layer, which a GPU renderer draws frames into and presents
/// (see [`Window::gpu_layer`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(feature = "gpu"), allow(dead_code))]
pub(crate) enum GpuLayer {
    /// A `CAMetalLayer` over the content view (macOS, iOS), sized to it.
    #[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
    Metal(*mut std::ffi::c_void),
    /// A window (`HWND`) the renderer gives a DXGI swap chain (Windows); the
    /// window stops painting its bitmap over it.
    #[cfg_attr(not(all(target_os = "windows", feature = "d3d")), allow(dead_code))]
    Hwnd(*mut std::ffi::c_void),
    /// The page's canvas, drawn into through `web/gpu.js` (the web).
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    Web,
    /// An X11 window, by its id. The backend speaks the protocol over its
    /// own socket, so a GPU renderer presents to the window over a libxcb
    /// connection of its own (`gpu::xcb`): a window id is the same on every
    /// connection to the server.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    X11 { window: u32 },
    /// A Wayland window, by its `wl_surface` id, and where the renderer
    /// hands its frames as `dma-buf`s (see [`crate::platform::dmabuf`]): the
    /// backend speaks the protocol itself, so there is no libwayland surface
    /// for Vulkan or EGL to present to. The sink lives as long as the window;
    /// the backend releases the layer through the renderer before it goes.
    #[cfg(target_os = "linux")]
    Wayland {
        surface: u32,
        sink: *const crate::platform::dmabuf::Sink,
    },
    /// The activity's `ANativeWindow` (Android): the window is `size`
    /// pixels, and the app draws in its `content` rectangle (`[x0, y0, x1,
    /// y1]`, clear of the system bars unless edge to edge); the rest shows
    /// the background.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    Android {
        window: *mut std::ffi::c_void,
        size: (i32, i32),
        content: [i32; 4],
    },
}

/// A live native window.
///
/// Backends implement this; the rest of Stipple drives windows only through this
/// trait. A window can hand out a [`Surface`] (via `Window::create_surface`)
/// that the renderer presents into.
pub trait Window {
    fn id(&self) -> WindowId;

    /// Current drawable size in physical pixels.
    fn inner_size(&self) -> PhysicalSize;

    /// Current DPI scale factor (physical ÷ logical).
    fn scale_factor(&self) -> ScaleFactor;

    /// Ask the system to deliver a [`RedrawRequested`](crate::platform::Event::RedrawRequested).
    // Every backend implements it; the app paces redraws through `frame`.
    #[allow(dead_code)]
    fn request_redraw(&self);

    /// Update the window title.
    fn set_title(&self, title: &str);

    /// Push the current accessibility tree so the OS assistive-technology API can
    /// read the full element hierarchy (not just the window root). Default: a
    /// no-op — backends without an AT bridge ignore it. The macOS (`NSAccessibility`)
    /// and Windows (UI Automation) backends override this to vend `root`'s
    /// descendants as native accessibility elements.
    fn set_accessibility_tree(&self, _root: &crate::platform::access::A11yNode) {}

    /// Whether this window can show real OS controls over its content
    /// (UIKit, AppKit and Win32 windows): under
    /// [`Theme::native`](crate::style::Theme::native) the App then hands it
    /// the live ones with [`set_native_views`](Self::set_native_views).
    fn hosts_native_views(&self) -> bool {
        false
    }

    /// Show exactly these live OS controls over the content, in paint order
    /// (see `docs/native-theme.md`, *Real controls*). What the user does to
    /// them comes back as [`Event::NativeView`](crate::platform::Event::NativeView).
    fn set_native_views(&self, _views: &[crate::platform::native::NativeView]) {}

    /// Create a [`Surface`] that presents into this window's drawable.
    fn create_surface(&self) -> Box<dyn Surface>;

    /// The GPU presentation seam: put a GPU layer over the window's content
    /// (once), keep it at the window's size and scale, and hand it out for a
    /// GPU renderer to present frames into instead of a [`Surface`]. `None`
    /// (the default): this backend only takes `Pixmap`s.
    #[cfg_attr(not(feature = "gpu"), allow(dead_code))]
    fn gpu_layer(&self) -> Option<GpuLayer> {
        None
    }

    /// Take the GPU layer away again (the renderer fell back to the CPU):
    /// the [`Surface`]'s frames show from the next present on.
    fn drop_gpu_layer(&self) {}

    /// Read the system clipboard's text, if any. Default: `None` (backends that
    /// don't implement the clipboard fall back to the in-process mirror).
    fn clipboard(&self) -> Option<String> {
        None
    }

    /// Set the system clipboard's text. Default: a no-op.
    fn set_clipboard(&self, _text: &str) {}

    /// Read the clipboard or the primary selection in the representations
    /// `formats` asks for. A backend whose owner answers later (X11, when
    /// another client owns the selection) returns
    /// [`ClipboardRead::Pending`] and delivers the content as
    /// [`Event::Clipboard`](crate::platform::Event::Clipboard), to this window,
    /// within a couple of seconds (with `None` when it timed out). Default: the
    /// clipboard's text through `Window::clipboard`, and no primary
    /// selection.
    fn read_clipboard(
        &self,
        kind: crate::platform::clipboard::ClipboardKind,
        formats: crate::platform::clipboard::ClipboardFormats,
    ) -> crate::platform::clipboard::ClipboardRead {
        use crate::platform::clipboard::{ClipboardContent, ClipboardKind, ClipboardRead};
        let text = (kind == ClipboardKind::Clipboard && formats.text)
            .then(|| self.clipboard())
            .flatten();
        ClipboardRead::Ready(text.map(ClipboardContent::from_text))
    }

    /// Put `content` on the clipboard or the primary selection, every
    /// representation it has. Default: the clipboard's text through
    /// `Window::set_clipboard`, and no primary selection.
    fn write_clipboard(
        &self,
        kind: crate::platform::clipboard::ClipboardKind,
        content: &crate::platform::clipboard::ClipboardContent,
    ) {
        if kind == crate::platform::clipboard::ClipboardKind::Clipboard
            && let Some(text) = &content.text
        {
            self.set_clipboard(text);
        }
    }

    /// Whether [`ClipboardKind::Primary`](crate::platform::clipboard::ClipboardKind::Primary)
    /// reaches a system-wide selection. Default: `false` (the `App` then keeps
    /// it in process).
    fn has_primary_selection(&self) -> bool {
        false
    }

    /// Make the title bar dark or light, to match content whose theme changed
    /// (see [`WindowAttributes::dark`]). Default: a no-op. Honored on Windows
    /// and macOS.
    fn set_dark(&self, _dark: bool) {}

    /// Open a new top-level sibling window sharing this window's event loop /
    /// connection, returning its [`WindowId`]. Default: `None` — the backend is
    /// single-window. Multi-window backends (X11, Wayland, Windows, macOS)
    /// override this; the new window's events arrive through the same handler,
    /// distinguished by `Window::id`. It takes focus if
    /// [`WindowAttributes::focus`] says so.
    ///
    /// A multi-window backend delivers [`Event::Wake`](crate::platform::Event::Wake)
    /// to every open window in turn, so the app can bring each one up to date.
    fn open_window(&self, _attrs: WindowAttributes) -> Option<WindowId> {
        None
    }

    /// Request that *this* window be closed and removed from the event loop.
    /// Default: a no-op (single-window backends instead end the loop when the
    /// handler returns `ControlFlow::Exit`). Multi-window
    /// backends destroy just this window and keep running while others remain.
    fn close_window(&self) {}

    /// Ask for the user's attention without stealing focus (`Some`), or
    /// withdraw the request (`None`): `_NET_WM_STATE_DEMANDS_ATTENTION` and
    /// the urgency hint on X11, an `xdg_activation_v1` request on Wayland,
    /// `FlashWindowEx` on Windows and `requestUserAttention:` on macOS. The
    /// platform drops the request once the window is activated. Default: a
    /// no-op.
    fn request_attention(&self, _kind: Option<UserAttention>) {}

    /// Sound the system beep (an audible bell): the X11 `Bell` request,
    /// `xdg_system_bell_v1` on Wayland where the compositor has it,
    /// `MessageBeep` on Windows and `NSBeep` on macOS. Default: a no-op.
    fn beep(&self) {}

    /// Show `label` (a count, say) as a badge on the application's icon, or
    /// remove it (`None`): the Dock tile's badge on macOS. Default: a no-op
    /// (the other platforms have no badge Stipple sets yet).
    fn set_badge(&self, _label: Option<&str>) {}
    /// Show (`true`) or hide (`false`) the window without closing it.
    /// Default: a no-op. The app hides its last window instead of closing it
    /// when it should keep running with none (see
    /// [`App::exit_on_last_window_closed`](crate::App::exit_on_last_window_closed)).
    fn set_visible(&self, _visible: bool) {}

    /// Where the window is and what state it is in (see [`WindowInfo`]).
    /// Must be cheap: the app reads it after every event, and backends answer
    /// from what the system last told them. Backends deliver
    /// [`Event::WindowChanged`](crate::platform::Event::WindowChanged) when it
    /// changes for any reason other than a resize. Default: the size and scale
    /// only.
    fn info(&self) -> WindowInfo {
        WindowInfo::from_size(self.inner_size(), self.scale_factor())
    }

    /// Move the window's outer top-left corner to `position` (logical pixels
    /// on the desktop). Default: a no-op, as on Wayland.
    fn set_position(&self, _position: Point) {}

    /// Resize the content area to `size` logical pixels. Default: a no-op.
    fn set_inner_size(&self, _size: Size) {}

    /// Snap the user's resizing to whole steps of `step` logical pixels, or
    /// resize freely (`None`). Default: a no-op, as on Wayland.
    fn set_resize_increments(&self, _step: Option<Size>) {}

    /// Minimize or restore the window. Default: a no-op.
    fn set_minimized(&self, _minimized: bool) {}

    /// Show `cursor` while the pointer is over this window's content. Default:
    /// a no-op (the platform's arrow stays).
    fn set_cursor(&self, _cursor: CursorIcon) {}

    /// Lock the pointer to the window (`true`), or release it (`false`).
    /// While locked the cursor is hidden and held in place, and mouse motion
    /// arrives as [`Event::PointerDelta`](crate::platform::Event::PointerDelta)
    /// instead of [`Event::PointerMoved`](crate::platform::Event::PointerMoved);
    /// buttons and the wheel still come, at the position the pointer is
    /// held at (the window's center on desktops). Unlocking shows the cursor
    /// again there.
    ///
    /// Returns whether the pointer is (or is being) locked as asked: `false`
    /// where the platform has no pointer lock (the default, iOS, Android, a
    /// Wayland compositor without `zwp_pointer_constraints_v1`), or refused
    /// it. A lock the system ends on its own, or a pending one it refuses
    /// (the web's), comes as
    /// [`Event::PointerLockLost`](crate::platform::Event::PointerLockLost).
    /// The app releases the lock when the window loses focus. Releasing
    /// always returns `true`. See `docs/pointer-events.md`.
    fn set_pointer_lock(&self, locked: bool) -> bool {
        !locked
    }

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
    /// Which Option keys act as Meta for the focused element (see
    /// [`OptionAsMeta`](crate::platform::OptionAsMeta)): with one held, a
    /// key's text isn't composed by the layout and the key skips the input
    /// method. The App calls this after a frame when it changed. Default: a
    /// no-op (only macOS composes with Option).
    fn set_option_as_meta(&self, _option: crate::platform::OptionAsMeta) {}
    /// Show `menu` as the window's (on macOS, the application's) native menu
    /// bar, replacing any earlier one. Picking an item delivers
    /// [`Event::MenuCommand`](crate::platform::Event::MenuCommand) with its id.
    /// Returns whether the platform shows it; if not (the default, and on
    /// Linux, which has no native menu bar) the app draws one in the window.
    fn set_menu_bar(&self, _menu: &crate::platform::menu::NativeMenuBar) -> bool {
        false
    }
    /// Show `menu` as a native context menu at `at` (logical pixels in the
    /// window's content) and wait until it closes. Returns what was picked,
    /// or [`MenuPopup::Unsupported`](crate::platform::menu::MenuPopup) (the
    /// default) where the platform has none, and the app draws the menu
    /// itself. Implemented with `TrackPopupMenuEx` on Windows and `NSMenu`
    /// on macOS.
    fn popup_menu(
        &self,
        _menu: &crate::platform::menu::NativeMenu,
        _at: crate::geometry::Point,
    ) -> crate::platform::menu::MenuPopup {
        crate::platform::menu::MenuPopup::Unsupported
    }
    /// Show `tray` as the application's tray icon, replacing any earlier one,
    /// or remove it (`None`). Picking one of its menu items delivers
    /// [`Event::MenuCommand`](crate::platform::Event::MenuCommand) and clicking
    /// the icon [`Event::TrayActivated`](crate::platform::Event::TrayActivated).
    /// Returns whether the platform shows it. Default: `false`.
    fn set_tray(&self, _tray: Option<&crate::platform::menu::NativeTray>) -> bool {
        false
    }
    /// Set the application's Dock menu (macOS), or remove it (`None`).
    /// Command ids start at
    /// [`DOCK_ID_BASE`](crate::platform::menu::DOCK_ID_BASE) and come back
    /// as [`Event::MenuCommand`](crate::platform::Event::MenuCommand)s.
    /// Returns whether the platform has one; default: `false`.
    fn set_dock_menu(&self, _menu: Option<&crate::platform::menu::NativeMenu>) -> bool {
        false
    }
    /// Bring the window to the front and give it the keyboard focus, with
    /// the activation token a launcher handed the app where the platform
    /// asks for one (Wayland's `xdg_activation_v1`, X11's startup id).
    /// Default: a no-op.
    fn activate(&self, _token: Option<&str>) {}
    /// Whether a text input has keyboard focus, so a touch platform can show
    /// its on-screen keyboard (and hide it again). The app calls this after
    /// each frame it presents. Default: a no-op. Honored on Android.
    fn set_text_input_active(&self, _active: bool) {}
    /// The focused text field's hints for the on-screen keyboard (its
    /// kind, capitalization, corrections, action key and autofill). The App
    /// calls this when they change, before [`set_ime`](Self::set_ime) and
    /// [`set_text_input_active`](Self::set_text_input_active). Default: a
    /// no-op (desktops); honored on Android, iOS and the web.
    fn set_input_hints(&self, _hints: &crate::platform::InputHints) {}
    /// Whether the platform shows the edit menu itself (see
    /// [`show_edit_menu`](Self::show_edit_menu)); if not (the default), the
    /// App draws one in the window. `UIEditMenuInteraction` on iOS 16 and
    /// later, a floating `ActionMode` on Android 8 and later.
    fn has_edit_menu(&self) -> bool {
        false
    }
    /// Show the system's edit menu over a text field's selection (`Some`),
    /// replacing one already shown, or close it (`None`). What the user picks
    /// comes back as [`Event::EditAction`](crate::platform::Event::EditAction);
    /// the menu closes itself then, and when the user taps elsewhere. Only
    /// called where [`has_edit_menu`](Self::has_edit_menu) says so. Default:
    /// a no-op.
    fn show_edit_menu(&self, _menu: Option<&crate::platform::EditMenu>) {}
    /// Whether the window takes the drag that is over it, at the position of
    /// the last [`Event::DragEnter`](crate::platform::Event::DragEnter) or
    /// [`Event::DragOver`](crate::platform::Event::DragOver). Call it while
    /// handling those; the backend reports the answer to the drag source (the
    /// cursor shows a copy or a "no" sign) and refuses a drop it declined.
    /// Default: a no-op (backends without drops).
    fn set_drop_accepted(&self, _accepted: bool) {}
    /// Keep the screen from dimming and locking while the app shows (`true`),
    /// or let it again. Default: the desktop's own way, through
    /// `platform::awake` (a power assertion on macOS,
    /// `SetThreadExecutionState` on Windows, the screen saver's `Inhibit`
    /// over D-Bus on Linux); the mobile and web backends override it.
    fn set_keep_awake(&self, on: bool) {
        crate::platform::awake::set(on);
    }
    /// Let the device turn the app only the ways in `set`. Default: a
    /// no-op (desktops); honored on iOS and Android.
    fn set_orientations(&self, _set: Orientations) {}
    /// Carry `data` (files or text) out of the window as a drag other
    /// applications can drop: the App calls this when an in-app drag from an
    /// [`Element::drag_source`](crate::core::Element::drag_source) leaves the
    /// window with the button still down. From then on the platform runs the
    /// drag (the pointer, the cursor, Escape) until it drops or is cancelled;
    /// if it comes back over the window it arrives like any drag from another
    /// application. Returns whether the platform took it; if not (the
    /// default), the drag stays in the app. Implemented with XDND on X11,
    /// `wl_data_source` on Wayland, OLE `DoDragDrop` on Windows and an
    /// `NSDraggingSession` on macOS. See `docs/drag-and-drop.md`.
    fn start_drag(&self, _data: &crate::platform::DropData) -> bool {
        false
    }
    /// Lift `lift.data` (files or text) out of the app on a touch screen: the
    /// App calls this when a long press lands on an
    /// [`Element::drag_source`](crate::core::Element::drag_source) whose
    /// payload can leave the app, and when the platform's own lift gesture
    /// asks what is under the finger ([`Event::DragLift`](crate::platform::Event::DragLift)).
    /// The platform runs the drag from there, with `lift.image` as its
    /// picture; dropped on the app's own window it arrives like any drag
    /// from another application. Returns whether the platform takes it; if
    /// not (the default), the long press goes on as usual (a context menu).
    /// Android starts a system drag (`View.startDragAndDrop`, files through
    /// the app's `FileProvider`); iOS answers its `UIDragInteraction`. See
    /// `docs/drag-and-drop.md`.
    fn lift_drag(&self, _lift: &DragLift) -> bool {
        false
    }
}

/// A drag lifted out of the app by a touch (see [`Window::lift_drag`]).
#[derive(Clone, Debug)]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(crate) struct DragLift {
    /// What is dragged: [`Files`](crate::platform::DropData::Files) or
    /// [`Text`](crate::platform::DropData::Text).
    pub data: crate::platform::DropData,
    /// The element lifted, in window coordinates (logical pixels).
    pub source: Rect,
    /// A picture of the element (at the window's scale, `source`'s size in
    /// logical pixels), for the platform's drag preview.
    pub image: Option<std::sync::Arc<Pixmap>>,
}

/// Whether the app asks before quitting (it set
/// [`App::on_quit_requested`](crate::App::on_quit_requested)). Backends that
/// must answer the OS on the spot (logout on Windows, `terminate:` on macOS)
/// let the session end at once when nobody asks.
static QUIT_CONFIRMED_BY_APP: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Set whether the app answers [`Event::QuitRequested`](crate::platform::Event)
/// itself (see [`QUIT_CONFIRMED_BY_APP`]).
pub(crate) fn set_app_confirms_quit(on: bool) {
    QUIT_CONFIRMED_BY_APP.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Whether the app answers quit requests itself.
#[cfg_attr(not(any(target_os = "windows", target_os = "macos")), allow(dead_code))]
pub(crate) fn app_confirms_quit() -> bool {
    QUIT_CONFIRMED_BY_APP.load(std::sync::atomic::Ordering::Relaxed)
}

/// With `STIPPLE_DEBUG_WINDOWS` set, print a line about a window change as the
/// system reports it back (`stipple-window: title 0x1234 "Hello"`), for tests
/// that check titles, geometry and windows opening and closing on each
/// platform. Backends call it after reading the value back from the OS.
#[allow(dead_code)]
pub(crate) fn debug_window(args: std::fmt::Arguments<'_>) {
    if std::env::var_os("STIPPLE_DEBUG_WINDOWS").is_some() {
        eprintln!("stipple-window: {args}");
    }
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
            .min_size(Size::new(100.0, 50.0))
            .max_size(Size::new(800.0, 600.0))
            .maximized(true)
            .fullscreen(true)
            .icon(Pixmap::new(PhysicalSize::new(16, 16)));
        assert_eq!(a.min_size, Some(Size::new(100.0, 50.0)));
        assert_eq!(a.max_size, Some(Size::new(800.0, 600.0)));
        assert!(a.maximized && a.fullscreen);
        assert_eq!(a.icon.map(|i| i.size()), Some(PhysicalSize::new(16, 16)));
        assert!(!WindowAttributes::new().is_transparent());
        let t = WindowAttributes::new().transparent(true);
        assert!(t.is_transparent() && !t.blur);
        let b = WindowAttributes::new().blur(true);
        assert!(b.transparent && b.blur && b.is_transparent());
    }
}
