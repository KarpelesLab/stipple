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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PointerButton {
    /// The primary button (a finger on a touch screen).
    Left,
    /// The secondary button.
    Right,
    /// The middle button, or a click of the wheel.
    Middle,
    /// The "back" side button (X11 button 8, `XBUTTON1`, AppKit button 3).
    Back,
    /// The "forward" side button (X11 button 9, `XBUTTON2`, AppKit button 4).
    Forward,
    /// Any other button, numbered as the platform numbers it.
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

/// A scroll delta: whole lines (wheel notches) or logical pixels (trackpads
/// and other precise devices), as [`unit`](ScrollDelta::unit) says. Positive
/// `dy` scrolls down (towards the end of the content), positive `dx` to the
/// right. [`in_pixels`](ScrollDelta::in_pixels) converts lines at
/// [`SCROLL_LINE_PX`] per line, as Stipple's scroll containers do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollDelta {
    pub dx: f64,
    pub dy: f64,
    /// What `dx` and `dy` count.
    pub unit: ScrollUnit,
    /// Where the gesture this delta belongs to is, where the platform says.
    pub phase: ScrollPhase,
}

/// Logical pixels per line when a wheel's lines scroll pixel content (the
/// distance one notch scrolls a Stipple scroll container).
pub const SCROLL_LINE_PX: f64 = 40.0;

impl ScrollDelta {
    /// A wheel's delta, in lines (notches; fractions from high-resolution
    /// wheels).
    pub const fn lines(dx: f64, dy: f64) -> Self {
        Self {
            dx,
            dy,
            unit: ScrollUnit::Lines,
            phase: ScrollPhase::None,
        }
    }

    /// A precise delta in logical pixels (trackpads, smooth-scrolling mice).
    pub const fn pixels(dx: f64, dy: f64) -> Self {
        Self {
            dx,
            dy,
            unit: ScrollUnit::Pixels,
            phase: ScrollPhase::None,
        }
    }

    /// This delta, part of a gesture at `phase`.
    pub const fn with_phase(mut self, phase: ScrollPhase) -> Self {
        self.phase = phase;
        self
    }

    /// The delta in logical pixels: a line counts [`SCROLL_LINE_PX`].
    pub fn in_pixels(&self) -> (f64, f64) {
        match self.unit {
            ScrollUnit::Pixels => (self.dx, self.dy),
            ScrollUnit::Lines => (self.dx * SCROLL_LINE_PX, self.dy * SCROLL_LINE_PX),
        }
    }
}

/// What a [`ScrollDelta`] counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ScrollUnit {
    /// Logical pixels: a trackpad or another precise device. Accumulate them
    /// to scroll by lines.
    #[default]
    Pixels,
    /// Lines: a wheel's notches (fractional for high-resolution wheels, and
    /// for precision touchpads on Windows).
    Lines,
}

/// Where a scroll is in a gesture: trackpads (on macOS, and finger scrolling
/// on Wayland) start, move and end, then may coast with momentum. Wheels, and
/// platforms that don't say, report [`None`](ScrollPhase::None).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ScrollPhase {
    /// Not part of a gesture the platform tracks (a wheel notch).
    #[default]
    None,
    /// The fingers started scrolling.
    Started,
    /// The fingers scrolled on.
    Moved,
    /// The fingers lifted (usually with no delta). Momentum may follow.
    Ended,
    /// Coasting after the fingers lifted (macOS's momentum scrolling).
    Momentum,
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
    /// The window stays open until the handler acts on it (returns
    /// [`ControlFlow::Exit`](crate::platform::ControlFlow) or calls
    /// [`Window::close_window`](crate::platform::Window::close_window)), so
    /// the app can ask first.
    CloseRequested,
    /// The user asked the whole application to quit (⌘Q or the Dock's Quit on
    /// macOS, logging out on Windows and macOS). Returning
    /// [`ControlFlow::Exit`](crate::platform::ControlFlow) quits; anything
    /// else cancels.
    QuitRequested,
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

    /// A drag from another application entered the window at `position`
    /// (logical pixels), carrying the kinds of data in `offer`. Answer with
    /// [`Window::set_drop_accepted`](crate::platform::Window::set_drop_accepted)
    /// while handling it, and again on each [`DragOver`](Event::DragOver).
    /// See `docs/drag-and-drop.md`.
    DragEnter { position: Point, offer: DragOffer },
    /// The drag moved to `position` within the window.
    DragOver { position: Point },
    /// The drag left the window, or was cancelled, without dropping.
    DragLeave,
    /// The user dropped `data` at `position`. Ends the drag (no
    /// [`DragLeave`](Event::DragLeave) follows).
    Drop { position: Point, data: DropData },
}

/// The kinds of data a drag carries, known as soon as it enters a window
/// (before the data itself, which some platforms only hand over on drop). Also
/// the filter of what a drop target accepts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct DragOffer {
    /// One or more files (or folders), as paths.
    pub files: bool,
    /// Plain text.
    pub text: bool,
    /// An in-app drag's own payload ([`DropData::Internal`]); never from
    /// another application.
    pub internal: bool,
}

impl DragOffer {
    /// Files only.
    pub const FILES: DragOffer = DragOffer {
        files: true,
        text: false,
        internal: false,
    };
    /// Text only.
    pub const TEXT: DragOffer = DragOffer {
        files: false,
        text: true,
        internal: false,
    };
    /// In-app payloads only.
    pub const INTERNAL: DragOffer = DragOffer {
        files: false,
        text: false,
        internal: true,
    };
    /// Every kind.
    pub const ANY: DragOffer = DragOffer {
        files: true,
        text: true,
        internal: true,
    };

    /// Whether a target accepting `self` takes something `offer` carries.
    pub fn accepts(self, offer: DragOffer) -> bool {
        (self.files && offer.files)
            || (self.text && offer.text)
            || (self.internal && offer.internal)
    }

    /// Whether the drag carries nothing Stipple understands.
    pub fn is_empty(self) -> bool {
        !(self.files || self.text || self.internal)
    }

    /// The kinds in either.
    pub fn union(self, other: DragOffer) -> DragOffer {
        DragOffer {
            files: self.files || other.files,
            text: self.text || other.text,
            internal: self.internal || other.internal,
        }
    }
}

/// Data dropped onto a window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropData {
    /// Files or folders, as local paths.
    Files(Vec<std::path::PathBuf>),
    /// Plain text.
    Text(String),
    /// The payload of a drag started inside the app (see
    /// [`Element::drag_source`](crate::core::Element::drag_source)): a string
    /// the app chose, such as an item's id.
    Internal(String),
}

impl DropData {
    /// The kind of data this is, as a one-kind [`DragOffer`].
    pub fn kind(&self) -> DragOffer {
        match self {
            DropData::Files(_) => DragOffer::FILES,
            DropData::Text(_) => DragOffer::TEXT,
            DropData::Internal(_) => DragOffer::INTERNAL,
        }
    }

    /// Whether this can be dragged out of the app to other applications:
    /// files and text can, an [`Internal`](DropData::Internal) payload can't
    /// (it means nothing outside the app). See
    /// [`Window::start_drag`](crate::platform::Window::start_drag).
    pub fn can_leave_app(&self) -> bool {
        !matches!(self, DropData::Internal(_))
    }
}

/// `paths` as a `text/uri-list` (RFC 2483): one percent-encoded `file://`
/// URI per line, each ending in CRLF. What X11 and Wayland drags carry files
/// as; [`parse_uri_list`] reads it back.
pub fn uri_list(paths: &[std::path::PathBuf]) -> String {
    paths.iter().map(|p| path_to_file_uri(p) + "\r\n").collect()
}

/// The `file://` URI of an absolute local path, percent-encoding every byte
/// outside RFC 3986's unreserved set and `/` (Windows paths get forward
/// slashes and a leading `/`: `file:///C:/x`).
pub fn path_to_file_uri(path: &std::path::Path) -> String {
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    };
    #[cfg(not(unix))]
    let bytes = {
        let s = path.to_string_lossy().replace('\\', "/");
        let s = if s.starts_with('/') {
            s
        } else {
            format!("/{s}")
        };
        s.into_bytes()
    };
    let mut uri = String::from("file://");
    for b in bytes {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) || (cfg!(not(unix)) && b == b':') {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}

/// Parse a `text/uri-list` (RFC 2483), as X11 and Wayland drags carry files:
/// one URI per line, `#` comments skipped, `file:` URIs percent-decoded to
/// paths and other schemes skipped.
pub fn parse_uri_list(list: &str) -> Vec<std::path::PathBuf> {
    list.lines()
        .map(|l| l.trim_matches(|c: char| c.is_whitespace() || c == '\0'))
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(file_uri_to_path)
        .collect()
}

/// The path of a `file:` URI (`file:///p`, `file://localhost/p` or `file:/p`),
/// percent-decoded. `None` for other schemes and remote hosts.
pub fn file_uri_to_path(uri: &str) -> Option<std::path::PathBuf> {
    let rest = uri.strip_prefix("file:")?;
    let path = match rest.strip_prefix("//") {
        Some(after) => {
            // An authority: empty or localhost means this machine.
            let slash = after.find('/')?;
            let host = &after[..slash];
            if !(host.is_empty() || host.eq_ignore_ascii_case("localhost")) {
                return None;
            }
            &after[slash..]
        }
        None => rest,
    };
    let bytes = path.as_bytes();
    let hex = |b: Option<&u8>| b.and_then(|&b| (b as char).to_digit(16)).map(|d| d as u8);
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match (bytes[i], hex(bytes.get(i + 1)), hex(bytes.get(i + 2))) {
            (b'%', Some(h), Some(l)) => {
                out.push(h << 4 | l);
                i += 3;
            }
            (b, _, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok().map(std::path::PathBuf::from)
}

/// A `CF_HDROP` block as Windows drags carry files: a `DROPFILES` header
/// (20 bytes: the offset of the list, a point, the non-client flag and
/// "wide"), then each path in UTF-16 with a terminating NUL, then one more
/// NUL. `paths` are UTF-16 code units, without NULs.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn hdrop_bytes(paths: &[Vec<u16>]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&20u32.to_le_bytes()); // pFiles
    out.extend_from_slice(&[0; 12]); // pt, fNC
    out.extend_from_slice(&1u32.to_le_bytes()); // fWide
    for unit in paths
        .iter()
        .flat_map(|p| p.iter().copied().chain([0]))
        .chain([0])
    {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

/// `text` as `CF_UNICODETEXT`: UTF-16 with a terminating NUL.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn unicode_text_bytes(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(test)]
mod dnd_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn uri_lists_encode_and_round_trip() {
        let paths = vec![
            PathBuf::from("/tmp/a b.txt"),
            PathBuf::from("/tmp/h\u{e9}llo%#?.txt"),
        ];
        let list = uri_list(&paths);
        assert_eq!(
            list,
            "file:///tmp/a%20b.txt\r\nfile:///tmp/h%C3%A9llo%25%23%3F.txt\r\n"
        );
        assert_eq!(parse_uri_list(&list), paths);
        assert!(DropData::Files(paths).can_leave_app());
        assert!(DropData::Text("x".into()).can_leave_app());
        assert!(!DropData::Internal("x".into()).can_leave_app());
    }

    #[test]
    fn windows_formats_are_laid_out() {
        let wide = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        let block = hdrop_bytes(&[wide("C:\\a"), wide("D:\\\u{e9}")]);
        assert_eq!(&block[..4], &20u32.to_le_bytes());
        assert_eq!(&block[16..20], &1u32.to_le_bytes());
        let units: Vec<u16> = block[20..]
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let mut want = wide("C:\\a");
        want.push(0);
        want.extend(wide("D:\\\u{e9}"));
        want.extend([0, 0]);
        assert_eq!(units, want);
        assert_eq!(unicode_text_bytes("h\u{e9}"), vec![b'h', 0, 0xe9, 0, 0, 0]);
    }

    #[test]
    fn uri_list_parses_file_uris() {
        let list = "# comment\r\nfile:///tmp/a%20b.txt\r\nhttp://example.com/x\r\n\
                    file://localhost/etc/hosts\nfile:/x/y\nfile://remote/z\n\0";
        assert_eq!(
            parse_uri_list(list),
            vec![
                PathBuf::from("/tmp/a b.txt"),
                PathBuf::from("/etc/hosts"),
                PathBuf::from("/x/y"),
            ]
        );
    }

    #[test]
    fn offers_filter_kinds() {
        assert!(DragOffer::ANY.accepts(DragOffer::FILES));
        assert!(!DragOffer::TEXT.accepts(DragOffer::FILES));
        assert!(
            DragOffer::FILES
                .union(DragOffer::TEXT)
                .accepts(DragOffer::TEXT)
        );
        assert!(DragOffer::default().is_empty());
        assert_eq!(DropData::Text("x".into()).kind(), DragOffer::TEXT);
    }
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
