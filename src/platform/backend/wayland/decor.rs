//! Window decorations on Wayland.
//!
//! Where the compositor advertises `zxdg_decoration_manager_v1` (KDE, sway
//! and the other wlroots compositors) we ask it to draw the title bar and
//! borders itself (`server_side`). Where it doesn't (GNOME's Mutter, Weston),
//! or it answers `client_side`, Stipple draws them: a title bar with the
//! window's title and minimize, maximize and close buttons above the app's
//! content, and an invisible margin around the window that resizes it.
//! Dragging the title bar moves the window (`xdg_toplevel.move`), a
//! double-click maximizes it, a right-click opens the compositor's window
//! menu, and dragging the margin resizes it (`xdg_toplevel.resize`).
//!
//! The decorations live in the same buffer as the content (an ARGB one, so the
//! margin can be transparent); `xdg_surface.set_window_geometry` tells the
//! compositor which part is the window proper. Pointer positions over the
//! content are moved into the content's coordinates before the app sees
//! them.

use super::{WlConn, arg_u32};
use crate::geometry::{PhysicalSize, Point, Rect, Size};
use crate::platform::window::CursorIcon;
use crate::render::{Color, Font, Pixmap, Scene, SoftwareRenderer};

/// The global's interface name.
pub(super) const MANAGER: &str = "zxdg_decoration_manager_v1";
// zxdg_decoration_manager_v1 / zxdg_toplevel_decoration_v1 requests, and the
// decoration's one event.
const MANAGER_GET_TOPLEVEL_DECORATION: u16 = 1;
const DECORATION_SET_MODE: u16 = 1;
const DECORATION_CONFIGURE: u16 = 0;
const MODE_CLIENT_SIDE: u32 = 1;
const MODE_SERVER_SIDE: u32 = 2;
// xdg_surface / xdg_toplevel requests.
const XDG_SURFACE_SET_WINDOW_GEOMETRY: u16 = 3;
const XDG_TOPLEVEL_SHOW_WINDOW_MENU: u16 = 4;
const XDG_TOPLEVEL_MOVE: u16 = 5;
const XDG_TOPLEVEL_RESIZE: u16 = 6;
const XDG_TOPLEVEL_SET_MAXIMIZED: u16 = 9;
const XDG_TOPLEVEL_UNSET_MAXIMIZED: u16 = 10;
const XDG_TOPLEVEL_SET_MINIMIZED: u16 = 13;
// xdg_toplevel.configure states.
const STATE_MAXIMIZED: u32 = 1;
const STATE_FULLSCREEN: u32 = 2;
const STATE_ACTIVATED: u32 = 4;
const STATE_TILED_LEFT: u32 = 5;
const STATE_TILED_BOTTOM: u32 = 8;
// xdg_toplevel.resize_edge bits.
pub(super) const EDGE_TOP: u32 = 1;
pub(super) const EDGE_BOTTOM: u32 = 2;
pub(super) const EDGE_LEFT: u32 = 4;
pub(super) const EDGE_RIGHT: u32 = 8;
/// `BTN_LEFT` and `BTN_RIGHT` (evdev).
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

/// The title bar's height, in logical pixels.
pub(super) const TITLE_HEIGHT: f64 = 32.0;
/// The invisible resize margin around the window, in logical pixels.
pub(super) const MARGIN: f64 = 8.0;
/// How far along an edge from a corner the corner's diagonal resize reaches.
const CORNER: f64 = 20.0;
/// The longest gap between two presses on the title bar that maximizes.
const DOUBLE_CLICK: std::time::Duration = std::time::Duration::from_millis(500);

/// A title bar button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Button {
    Minimize,
    Maximize,
    Close,
}

/// What is under the pointer, in surface coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Hit {
    /// The app's content, at this point in its coordinates.
    Content(Point),
    /// The title bar, away from its buttons.
    Title,
    Button(Button),
    /// The resize margin: these `EDGE_*` bits.
    Edge(u32),
}

/// What a pointer motion means for the app.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Motion {
    /// Send the app a move to this point (content coordinates).
    pub(super) forward: Option<Point>,
    /// The pointer went from the content onto the decorations.
    pub(super) left_content: bool,
    /// The decorations look different (a button's hover).
    pub(super) redraw: bool,
}

/// What a pointer button press or release means.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Press {
    /// It's the app's, at this point in content coordinates.
    Forward(Point),
    /// The close button was clicked.
    Close,
    /// The decorations look different (a button pressed or let go).
    Redraw,
    /// Handled by the decorations (or nothing to do).
    Done,
}

/// The decoration state of the toplevel, kept on the connection.
#[derive(Debug, Default)]
pub(super) struct Decor {
    /// The `zxdg_toplevel_decoration_v1`, when the compositor has a manager.
    object: Option<u32>,
    /// Whether the app wants decorations at all.
    wanted: bool,
    /// Whether Stipple draws them (client-side).
    pub(super) csd: bool,
    resizable: bool,
    /// The objects requests go to, and the seat moves cite.
    xdg_surface: u32,
    toplevel: u32,
    seat: Option<u32>,
    pub(super) title: String,
    focused: bool,
    maximized: bool,
    fullscreen: bool,
    /// Tiled to a side (maximized, or snapped): no resize margin.
    tiled: bool,
    dark: bool,
    /// The button under the pointer, and the one pressed.
    hover: Option<Button>,
    pressed: Option<Button>,
    /// A press went to the app: its motion and release go there too, wherever
    /// the pointer is.
    app_grab: bool,
    /// The pointer is over the content.
    in_content: bool,
    /// The cursor the decorations show (over the resize margin), instead of
    /// the app's.
    cursor: Option<CursorIcon>,
    last_title_press: Option<std::time::Instant>,
    /// The size limits the app set, in content pixels (the compositor's are
    /// the window's, title bar included).
    limits: (Option<Size>, Option<Size>),
    /// The title font, loaded on first use (`Some(None)`: there is none).
    font: Option<Option<Font>>,
}

/// `zxdg_toplevel_decoration_v1.destroy`.
const DECORATION_DESTROY: u16 = 0;

impl Decor {
    /// The toplevel these decorations are for (each window has its own; the
    /// connection holds the one in use, see `toplevel::use_decor`).
    pub(super) fn toplevel(&self) -> u32 {
        self.toplevel
    }

    /// The `zxdg_toplevel_decoration_v1`, if any.
    pub(super) fn object(&self) -> Option<u32> {
        self.object
    }
}

/// Destroy the decorations in use (before their toplevel, as xdg-decoration
/// requires).
pub(super) fn destroy(conn: &mut WlConn) {
    if let Some(object) = conn.decor.object.take() {
        let _ = conn.send(object, DECORATION_DESTROY, &[]);
    }
}

/// Set up decorations for a new toplevel: ask a decoration manager for
/// server-side ones, or draw them ourselves if there is none.
pub(super) fn setup(
    conn: &mut WlConn,
    manager: Option<u32>,
    xdg_surface: u32,
    toplevel: u32,
    seat: Option<u32>,
    attrs: &crate::platform::WindowAttributes,
) {
    let d = &mut conn.decor;
    d.wanted = attrs.decorations;
    d.resizable = attrs.resizable;
    d.xdg_surface = xdg_surface;
    d.toplevel = toplevel;
    d.seat = seat;
    d.title = attrs.title.clone();
    d.dark = attrs.dark.unwrap_or(false);
    d.csd = attrs.decorations && manager.is_none();
    super::dbg(format_args!(
        "decorations: {}",
        match (attrs.decorations, manager) {
            (false, _) => "none",
            (true, Some(_)) => "asking for server-side",
            (true, None) => "client-side (no decoration manager)",
        }
    ));
    let Some(manager) = manager else {
        return;
    };
    let id = conn.new_id();
    let mut a = Vec::new();
    arg_u32(&mut a, id);
    arg_u32(&mut a, toplevel);
    if conn
        .send(manager, MANAGER_GET_TOPLEVEL_DECORATION, &a)
        .is_err()
    {
        conn.decor.csd = conn.decor.wanted;
        return;
    }
    conn.decor.object = Some(id);
    let mode = if attrs.decorations {
        MODE_SERVER_SIDE
    } else {
        MODE_CLIENT_SIDE // and draw none
    };
    let _ = conn.send(id, DECORATION_SET_MODE, &mode.to_le_bytes());
}

/// Handle a decoration event: the compositor's choice of mode, which takes
/// effect with the `xdg_surface.configure` that follows. Returns whether the
/// message was the decoration's.
pub(super) fn handle(conn: &mut WlConn, object: u32, opcode: u16, args: &[u8]) -> bool {
    if conn.decor.object != Some(object) {
        return false;
    }
    if opcode == DECORATION_CONFIGURE && args.len() >= 4 {
        let mode = u32::from_le_bytes([args[0], args[1], args[2], args[3]]);
        conn.decor.csd = conn.decor.wanted && mode == MODE_CLIENT_SIDE;
        super::dbg(format_args!(
            "decoration mode {mode} (client-side: {})",
            conn.decor.csd
        ));
        resend_limits(conn);
    }
    true
}

/// The `xdg_toplevel.configure` states (an array of `u32`), starting at
/// `args[8]`.
pub(super) fn configure_states(args: &[u8]) -> Vec<u32> {
    let Some(len) = args.get(8..12) else {
        return Vec::new();
    };
    let len = u32::from_le_bytes([len[0], len[1], len[2], len[3]]) as usize;
    args.get(12..12 + len)
        .unwrap_or_default()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|w| u32::from_le_bytes(*w))
        .collect()
}

impl Decor {
    /// Take the toplevel's states from a configure. Returns the window's new
    /// focus (`activated`) if it changed.
    pub(super) fn set_states(&mut self, states: &[u32]) -> Option<bool> {
        let focused = states.contains(&STATE_ACTIVATED);
        let changed = (focused != self.focused).then_some(focused);
        self.focused = focused;
        self.maximized = states.contains(&STATE_MAXIMIZED);
        self.fullscreen = states.contains(&STATE_FULLSCREEN);
        self.tiled = self.maximized
            || states
                .iter()
                .any(|s| (STATE_TILED_LEFT..=STATE_TILED_BOTTOM).contains(s));
        changed
    }

    /// Whether the title bar shows.
    fn titled(&self) -> bool {
        self.csd && !self.fullscreen
    }

    /// The title bar's height (0 without one), in logical pixels.
    pub(super) fn title_height(&self) -> f64 {
        if self.titled() { TITLE_HEIGHT } else { 0.0 }
    }

    /// The resize margin (0 where the window can't be resized from its edges).
    fn margin(&self) -> f64 {
        if self.titled() && !self.tiled && self.resizable {
            MARGIN
        } else {
            0.0
        }
    }

    /// The space around the content: left, top, right, bottom, in logical
    /// pixels.
    pub(super) fn insets(&self) -> (f64, f64, f64, f64) {
        let m = self.margin();
        (m, m + self.title_height(), m, m)
    }

    /// The content size for a window (title bar included) of `window`
    /// logical pixels.
    pub(super) fn content_size(&self, window: PhysicalSize) -> PhysicalSize {
        let title = self.title_height() as u32;
        PhysicalSize::new(window.width, window.height.saturating_sub(title).max(1))
    }

    /// The window geometry around content of `content` logical pixels: the
    /// title bar and content, without the margin. `(x, y, width, height)`.
    pub(super) fn geometry(&self, content: PhysicalSize) -> (u32, u32, u32, u32) {
        let m = self.margin() as u32;
        let title = self.title_height() as u32;
        (m, m, content.width, content.height + title)
    }

    /// A content size limit as the window's (title bar included).
    pub(super) fn outer_limit(&self, size: Option<Size>) -> Option<Size> {
        size.map(|s| Size::new(s.width, s.height + self.title_height()))
    }

    /// Remember the app's size limits, returning them as the window's.
    pub(super) fn set_limits(
        &mut self,
        min: Option<Option<Size>>,
        max: Option<Option<Size>>,
    ) -> (Option<Size>, Option<Size>) {
        if let Some(min) = min {
            self.limits.0 = min;
        }
        if let Some(max) = max {
            self.limits.1 = max;
        }
        (
            self.outer_limit(self.limits.0),
            self.outer_limit(self.limits.1),
        )
    }

    /// What is at `p` (surface coordinates, logical pixels), over content of
    /// `content` logical pixels.
    pub(super) fn hit(&self, p: Point, content: PhysicalSize) -> Hit {
        let (l, t, _, _) = self.insets();
        let (cw, ch) = (content.width as f64, content.height as f64);
        let inner = Point::new(p.x - l, p.y - t);
        if !self.csd || (inner.x >= 0.0 && inner.y >= 0.0 && inner.x < cw && inner.y < ch) {
            return Hit::Content(inner);
        }
        // The window proper: title bar and content.
        let m = self.margin();
        let (x0, y0) = (m, m);
        let (x1, y1) = (m + cw, t + ch);
        let inside = p.x >= x0 && p.x < x1 && p.y >= y0 && p.y < y1;
        if inside {
            // The title bar; its buttons sit at the right end, close last.
            let from_right = ((x1 - p.x) / TITLE_HEIGHT).floor() as u32;
            return match from_right {
                0 => Hit::Button(Button::Close),
                1 if self.resizable => Hit::Button(Button::Maximize),
                1 => Hit::Button(Button::Minimize),
                2 if self.resizable => Hit::Button(Button::Minimize),
                _ => Hit::Title,
            };
        }
        // The margin: the edges it's outside of, and the corners' diagonals
        // a little way along each edge.
        let mut edges = 0;
        if p.x
            < x0 + if p.y < y0 + CORNER || p.y >= y1 - CORNER {
                CORNER
            } else {
                0.0
            }
        {
            edges |= EDGE_LEFT;
        }
        if p.x
            >= x1
                - if p.y < y0 + CORNER || p.y >= y1 - CORNER {
                    CORNER
                } else {
                    0.0
                }
        {
            edges |= EDGE_RIGHT;
        }
        if p.y
            < y0 + if p.x < x0 + CORNER || p.x >= x1 - CORNER {
                CORNER
            } else {
                0.0
            }
        {
            edges |= EDGE_TOP;
        }
        if p.y
            >= y1
                - if p.x < x0 + CORNER || p.x >= x1 - CORNER {
                    CORNER
                } else {
                    0.0
                }
        {
            edges |= EDGE_BOTTOM;
        }
        Hit::Edge(edges)
    }

    /// The pointer moved to `p` (surface coordinates).
    pub(super) fn motion(&mut self, p: Point, content: PhysicalSize) -> Motion {
        let hit = self.hit(p, content);
        if self.app_grab {
            let (l, t, _, _) = self.insets();
            return Motion {
                forward: Some(Point::new(p.x - l, p.y - t)),
                ..Default::default()
            };
        }
        let hover = match hit {
            Hit::Button(b) => Some(b),
            _ => None,
        };
        let redraw = hover != self.hover;
        self.hover = hover;
        self.cursor = match hit {
            Hit::Edge(edges) => Some(edge_cursor(edges)),
            Hit::Content(_) => None,
            _ => Some(CursorIcon::Default),
        };
        let was_in = self.in_content;
        self.in_content = matches!(hit, Hit::Content(_));
        Motion {
            forward: match hit {
                Hit::Content(q) => Some(q),
                _ => None,
            },
            left_content: was_in && !self.in_content,
            redraw,
        }
    }

    /// The pointer left the surface.
    pub(super) fn leave(&mut self) -> bool {
        self.in_content = false;
        self.cursor = None;
        let redraw = self.hover.is_some() || self.pressed.is_some();
        self.hover = None;
        self.pressed = None;
        redraw
    }

    /// Whether the pointer is over the decorations (not the content, and no
    /// press on the content holds it): its wheel isn't the app's.
    pub(super) fn pointer_on_decorations(&self) -> bool {
        self.csd && !self.in_content && !self.app_grab
    }

    /// The cursor the decorations show instead of the app's, if any.
    pub(super) fn cursor(&self) -> Option<CursorIcon> {
        if self.csd { self.cursor } else { None }
    }
}

/// The resize cursor for these edges.
fn edge_cursor(edges: u32) -> CursorIcon {
    match edges {
        e if e == EDGE_TOP | EDGE_LEFT || e == EDGE_BOTTOM | EDGE_RIGHT => CursorIcon::NwseResize,
        e if e == EDGE_TOP | EDGE_RIGHT || e == EDGE_BOTTOM | EDGE_LEFT => CursorIcon::NeswResize,
        EDGE_LEFT | EDGE_RIGHT => CursorIcon::EwResize,
        _ => CursorIcon::NsResize,
    }
}

/// A pointer button changed state at `p` (surface coordinates): run the
/// decorations' part (moving, resizing, the buttons), or say it's the app's.
pub(super) fn button(
    conn: &mut WlConn,
    p: Point,
    content: PhysicalSize,
    code: u32,
    pressed: bool,
    serial: u32,
) -> Press {
    let d = &mut conn.decor;
    let hit = d.hit(p, content);
    let (l, t, _, _) = d.insets();
    let local = Point::new(p.x - l, p.y - t);
    if !pressed {
        if d.app_grab {
            d.app_grab = false;
            return Press::Forward(local);
        }
        let Some(button) = d.pressed.take() else {
            return match hit {
                Hit::Content(q) => Press::Forward(q),
                _ => Press::Done,
            };
        };
        if hit != Hit::Button(button) {
            return Press::Redraw; // let go elsewhere: cancelled
        }
        return match button {
            Button::Close => Press::Close,
            Button::Maximize => {
                toggle_maximized(conn);
                Press::Redraw
            }
            Button::Minimize => {
                let toplevel = conn.decor.toplevel;
                let _ = conn.send(toplevel, XDG_TOPLEVEL_SET_MINIMIZED, &[]);
                Press::Redraw
            }
        };
    }
    match hit {
        Hit::Content(q) => {
            d.app_grab = true;
            Press::Forward(q)
        }
        Hit::Button(b) if code == BTN_LEFT => {
            d.pressed = Some(b);
            Press::Redraw
        }
        Hit::Title if code == BTN_LEFT => {
            let now = std::time::Instant::now();
            let double = d
                .last_title_press
                .is_some_and(|t| now.duration_since(t) <= DOUBLE_CLICK);
            if double {
                d.last_title_press = None;
                if d.resizable {
                    toggle_maximized(conn);
                }
            } else {
                d.last_title_press = Some(now);
                send_seat_request(conn, XDG_TOPLEVEL_MOVE, serial, &[]);
            }
            Press::Done
        }
        Hit::Title | Hit::Button(_) if code == BTN_RIGHT => {
            // The compositor's window menu, at the pointer in window
            // geometry coordinates.
            let m = d.margin();
            let mut a = Vec::new();
            arg_u32(&mut a, (p.x - m) as i32 as u32);
            arg_u32(&mut a, (p.y - m) as i32 as u32);
            send_seat_request(conn, XDG_TOPLEVEL_SHOW_WINDOW_MENU, serial, &a);
            Press::Done
        }
        Hit::Edge(edges) if code == BTN_LEFT && edges != 0 => {
            send_seat_request(conn, XDG_TOPLEVEL_RESIZE, serial, &edges.to_le_bytes());
            Press::Done
        }
        _ => Press::Done,
    }
}

/// Send a toplevel request that cites the seat and the press's serial
/// (`move`, `resize`, `show_window_menu`), followed by `extra` arguments.
fn send_seat_request(conn: &mut WlConn, opcode: u16, serial: u32, extra: &[u8]) {
    let Some(seat) = conn.decor.seat else {
        return;
    };
    let mut a = Vec::new();
    arg_u32(&mut a, seat);
    arg_u32(&mut a, serial);
    a.extend_from_slice(extra);
    let toplevel = conn.decor.toplevel;
    let _ = conn.send(toplevel, opcode, &a);
}

fn toggle_maximized(conn: &mut WlConn) {
    let opcode = if conn.decor.maximized {
        XDG_TOPLEVEL_UNSET_MAXIMIZED
    } else {
        XDG_TOPLEVEL_SET_MAXIMIZED
    };
    let toplevel = conn.decor.toplevel;
    let _ = conn.send(toplevel, opcode, &[]);
}

/// Send the size limits again after the title bar came or went.
fn resend_limits(conn: &mut WlConn) {
    let (min, max) = conn.decor.set_limits(None, None);
    let toplevel = conn.decor.toplevel;
    let _ = conn.send(
        toplevel,
        super::XDG_TOPLEVEL_SET_MIN_SIZE,
        &super::size_args(min),
    );
    let _ = conn.send(
        toplevel,
        super::XDG_TOPLEVEL_SET_MAX_SIZE,
        &super::size_args(max),
    );
}

/// Tell the compositor which part of the buffer is the window (without the
/// resize margin), for content of `content` logical pixels. Applied by the
/// next commit.
pub(super) fn send_geometry(conn: &mut WlConn, content: PhysicalSize) {
    let (x, y, w, h) = conn.decor.geometry(content);
    let mut a = Vec::new();
    for v in [x, y, w, h] {
        arg_u32(&mut a, v);
    }
    let xdg_surface = conn.decor.xdg_surface;
    if xdg_surface != 0 {
        let _ = conn.send(xdg_surface, XDG_SURFACE_SET_WINDOW_GEOMETRY, &a);
    }
}

/// The colors of the title bar: background, title, button glyphs.
fn colors(dark: bool, focused: bool) -> (Color, Color) {
    match (dark, focused) {
        (false, true) => (Color::rgb(0xe8, 0xe8, 0xe8), Color::rgb(0x20, 0x20, 0x20)),
        (false, false) => (Color::rgb(0xf4, 0xf4, 0xf4), Color::rgb(0x90, 0x90, 0x90)),
        (true, true) => (Color::rgb(0x2b, 0x2b, 0x2b), Color::rgb(0xf0, 0xf0, 0xf0)),
        (true, false) => (Color::rgb(0x24, 0x24, 0x24), Color::rgb(0x88, 0x88, 0x88)),
    }
}

impl Decor {
    /// Draw the title bar for content `width` logical pixels wide, at `scale`.
    fn title_bar(&mut self, width: f64, scale: u32) -> Pixmap {
        // Out of `self` while drawing, back at the end.
        let font = self.font.take().unwrap_or_else(Font::system_default);
        let (bg, fg) = colors(self.dark, self.focused);
        let mut scene = Scene::new(Size::new(width, TITLE_HEIGHT));
        scene.fill_rect(Rect::from_xywh(0.0, 0.0, width, TITLE_HEIGHT), bg);
        let buttons: &[Button] = if self.resizable {
            &[Button::Minimize, Button::Maximize, Button::Close]
        } else {
            &[Button::Minimize, Button::Close]
        };
        let left = width - TITLE_HEIGHT * buttons.len() as f64;
        for (i, &b) in buttons.iter().enumerate() {
            let x = left + TITLE_HEIGHT * i as f64;
            let cell = Rect::from_xywh(x, 0.0, TITLE_HEIGHT, TITLE_HEIGHT);
            let active = self.hover == Some(b) || self.pressed == Some(b);
            let mut glyph = fg;
            if active {
                let fill = match b {
                    Button::Close => {
                        glyph = Color::WHITE;
                        Color::rgb(0xd9, 0x3b, 0x34)
                    }
                    _ => bg.mix(fg, 0.15),
                };
                scene.fill_rect(cell, fill);
            }
            let (cx, cy) = (x + TITLE_HEIGHT / 2.0, TITLE_HEIGHT / 2.0);
            match b {
                Button::Minimize => {
                    scene.fill_rect(Rect::from_xywh(cx - 5.0, cy + 4.0, 10.0, 1.5), glyph)
                }
                Button::Maximize if self.maximized => {
                    scene.stroke_rect(Rect::from_xywh(cx - 3.0, cy - 5.0, 8.0, 8.0), glyph, 1.2);
                    scene.fill_rect(Rect::from_xywh(cx - 5.0, cy - 3.0, 8.0, 8.0), bg);
                    scene.stroke_rect(Rect::from_xywh(cx - 5.0, cy - 3.0, 8.0, 8.0), glyph, 1.2);
                }
                Button::Maximize => {
                    scene.stroke_rect(Rect::from_xywh(cx - 5.0, cy - 5.0, 10.0, 10.0), glyph, 1.2)
                }
                Button::Close => match font.as_ref() {
                    Some(font) => {
                        let size = 18.0;
                        let m = font.measure("\u{d7}", size);
                        scene.fill_text(
                            font,
                            "\u{d7}",
                            Point::new(cx - m.width / 2.0, cy - m.height / 2.0),
                            size,
                            glyph,
                        );
                    }
                    None => scene.fill_rect(Rect::from_xywh(cx - 4.0, cy - 4.0, 8.0, 8.0), glyph),
                },
            }
        }
        if let Some(font) = font.as_ref() {
            let size = 13.0;
            let room = (left - 24.0).max(0.0);
            let mut title = self.title.clone();
            // Elide what doesn't fit.
            if font.measure(&title, size).width > room {
                while !title.is_empty()
                    && font.measure(&format!("{title}\u{2026}"), size).width > room
                {
                    title.pop();
                }
                title.push('\u{2026}');
            }
            let m = font.measure(&title, size);
            scene.push_clip(Rect::from_xywh(0.0, 0.0, left, TITLE_HEIGHT));
            scene.fill_text(
                font,
                &title,
                Point::new(12.0, (TITLE_HEIGHT - m.height) / 2.0),
                size,
                fg,
            );
            scene.pop_clip();
        }
        self.font = Some(font);
        let s = scale.max(1);
        SoftwareRenderer::new().with_background(bg).render_at(
            scene,
            PhysicalSize::new((width as u32) * s, TITLE_HEIGHT as u32 * s),
        )
    }

    /// The whole buffer for `content` (physical pixels, `scale` per logical
    /// one) with the decorations around it, as `wl_shm` ARGB8888 bytes
    /// (premultiplied `[B, G, R, A]`), and its size. `None` when there are no
    /// decorations to draw: the content is the buffer.
    pub(super) fn frame(
        &mut self,
        content: &Pixmap,
        scale: u32,
    ) -> Option<(PhysicalSize, Vec<u8>)> {
        if !self.titled() {
            return None;
        }
        let s = scale.max(1);
        let size = content.size();
        let (l, t, r, b) = self.insets();
        let (l, t, r, b) = (l as u32 * s, t as u32 * s, r as u32 * s, b as u32 * s);
        let full = PhysicalSize::new(size.width + l + r, size.height + t + b);
        let mut out = vec![0u8; full.width as usize * full.height as usize * 4];
        let stride = full.width as usize * 4;
        let title = self.title_bar(size.width as f64 / s as f64, s);
        let title_top = t - TITLE_HEIGHT as u32 * s;
        let mut copy = |src: &Pixmap, x: u32, y: u32| {
            let sw = src.size().width.min(full.width.saturating_sub(x)) as usize;
            for row in 0..src.size().height.min(full.height.saturating_sub(y)) {
                let from = &src.as_bytes()[row as usize * src.stride()..][..sw * 4];
                let o = (y + row) as usize * stride + x as usize * 4;
                for (d, p) in out[o..o + sw * 4]
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(from.as_chunks::<4>().0)
                {
                    // Opaque: premultiplied is the color itself.
                    *d = [p[2], p[1], p[0], 255];
                }
            }
        };
        copy(&title, l, title_top);
        copy(content, l, t);
        Some((full, out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn csd() -> Decor {
        Decor {
            wanted: true,
            csd: true,
            resizable: true,
            focused: true,
            title: "Hello".into(),
            ..Default::default()
        }
    }

    const CONTENT: PhysicalSize = PhysicalSize::new(300, 200);

    #[test]
    fn insets_follow_the_window_state() {
        let mut d = csd();
        assert_eq!(d.insets(), (MARGIN, MARGIN + TITLE_HEIGHT, MARGIN, MARGIN));
        assert_eq!(d.set_states(&[STATE_MAXIMIZED]), Some(false), "not active");
        assert_eq!(d.insets(), (0.0, TITLE_HEIGHT, 0.0, 0.0), "no margin");
        assert_eq!(d.set_states(&[STATE_ACTIVATED]), Some(true));
        assert_eq!(d.set_states(&[STATE_ACTIVATED]), None, "no change");
        d.set_states(&[STATE_FULLSCREEN]);
        assert_eq!(d.insets(), (0.0, 0.0, 0.0, 0.0), "no title bar either");
        d.set_states(&[]);
        d.csd = false;
        assert_eq!(d.insets(), (0.0, 0.0, 0.0, 0.0), "server-side");
    }

    #[test]
    fn window_and_content_sizes() {
        let d = csd();
        // The compositor configures the window: title bar plus content.
        assert_eq!(
            d.content_size(PhysicalSize::new(300, 232)),
            PhysicalSize::new(300, 200)
        );
        assert_eq!(d.geometry(CONTENT), (8, 8, 300, 232));
        assert_eq!(
            d.outer_limit(Some(Size::new(100.0, 50.0))),
            Some(Size::new(100.0, 82.0))
        );
        let mut ssd = csd();
        ssd.csd = false;
        assert_eq!(ssd.content_size(PhysicalSize::new(300, 232)).height, 232);
        assert_eq!(ssd.geometry(CONTENT), (0, 0, 300, 200));
    }

    #[test]
    fn hit_testing() {
        let d = csd();
        let (l, t) = (MARGIN, MARGIN + TITLE_HEIGHT);
        assert_eq!(
            d.hit(Point::new(l + 10.0, t + 5.0), CONTENT),
            Hit::Content(Point::new(10.0, 5.0))
        );
        // The title bar, and its buttons from the right.
        assert_eq!(
            d.hit(Point::new(l + 20.0, MARGIN + 10.0), CONTENT),
            Hit::Title
        );
        let right = l + 300.0;
        let y = MARGIN + 10.0;
        assert_eq!(
            d.hit(Point::new(right - 5.0, y), CONTENT),
            Hit::Button(Button::Close)
        );
        assert_eq!(
            d.hit(Point::new(right - 40.0, y), CONTENT),
            Hit::Button(Button::Maximize)
        );
        assert_eq!(
            d.hit(Point::new(right - 70.0, y), CONTENT),
            Hit::Button(Button::Minimize)
        );
        // The margin: edges, and corners a little way along them.
        assert_eq!(d.hit(Point::new(2.0, 150.0), CONTENT), Hit::Edge(EDGE_LEFT));
        assert_eq!(
            d.hit(Point::new(150.0, t + 200.0 + 3.0), CONTENT),
            Hit::Edge(EDGE_BOTTOM)
        );
        assert_eq!(
            d.hit(Point::new(2.0, 2.0), CONTENT),
            Hit::Edge(EDGE_TOP | EDGE_LEFT)
        );
        assert_eq!(
            d.hit(Point::new(right + 3.0, MARGIN + 10.0), CONTENT),
            Hit::Edge(EDGE_TOP | EDGE_RIGHT),
            "near the top-right corner"
        );
        // Server-side: everything is content.
        let mut ssd = csd();
        ssd.csd = false;
        assert_eq!(
            ssd.hit(Point::new(1.0, 1.0), CONTENT),
            Hit::Content(Point::new(1.0, 1.0))
        );
    }

    #[test]
    fn motion_tracks_hover_and_leaving_the_content() {
        let mut d = csd();
        let inside = Point::new(MARGIN + 50.0, MARGIN + TITLE_HEIGHT + 50.0);
        let m = d.motion(inside, CONTENT);
        assert_eq!(m.forward, Some(Point::new(50.0, 50.0)));
        assert!(!m.left_content && !m.redraw);
        let close = Point::new(MARGIN + 295.0, MARGIN + 10.0);
        let m = d.motion(close, CONTENT);
        assert_eq!(m.forward, None);
        assert!(m.left_content && m.redraw, "hovering the close button");
        assert_eq!(d.cursor(), Some(CursorIcon::Default));
        d.motion(Point::new(1.0, 150.0), CONTENT);
        assert_eq!(d.cursor(), Some(CursorIcon::EwResize));
        d.motion(close, CONTENT);
        assert!(d.leave(), "nothing hovered any more");
        assert!(d.pointer_on_decorations());
        d.motion(inside, CONTENT);
        assert!(!d.pointer_on_decorations());
        assert_eq!(d.cursor(), None);
    }

    #[test]
    fn configure_states_parse() {
        let mut args = Vec::new();
        args.extend_from_slice(&300u32.to_le_bytes());
        args.extend_from_slice(&200u32.to_le_bytes());
        args.extend_from_slice(&8u32.to_le_bytes());
        args.extend_from_slice(&STATE_MAXIMIZED.to_le_bytes());
        args.extend_from_slice(&4u32.to_le_bytes());
        assert_eq!(configure_states(&args), [STATE_MAXIMIZED, 4]);
        assert!(configure_states(&args[..8]).is_empty());
    }

    #[test]
    fn frame_puts_the_title_bar_above_the_content() {
        let mut d = csd();
        d.font = Some(None); // no font needed for the layout
        let mut content = Pixmap::new(CONTENT);
        for px in content.as_bytes_mut().as_chunks_mut::<4>().0 {
            *px = [10, 20, 30, 255];
        }
        let (size, bytes) = d.frame(&content, 1).expect("decorated");
        assert_eq!(size, PhysicalSize::new(316, 248));
        let px = |x: usize, y: usize| {
            let o = (y * size.width as usize + x) * 4;
            [bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]
        };
        assert_eq!(px(0, 0), [0, 0, 0, 0], "the margin is transparent");
        assert_eq!(px(8, 40), [30, 20, 10, 255], "content, as BGRA");
        let (bg, _) = colors(false, true);
        assert_eq!(px(20, 20), [bg.b, bg.g, bg.r, 255], "title bar");
        // At 2x, everything doubles.
        let mut content = Pixmap::new(PhysicalSize::new(600, 400));
        content.as_bytes_mut().fill(255);
        let (size, _) = d.frame(&content, 2).unwrap();
        assert_eq!(size, PhysicalSize::new(632, 496));
        // Fullscreen: no decorations.
        d.set_states(&[STATE_FULLSCREEN]);
        assert!(d.frame(&content, 1).is_none());
    }

    mod protocol {
        use super::super::super::XDG_TOPLEVEL_SET_MIN_SIZE;
        use super::super::super::clipboard::tests::{conn_pair, message, read_request};
        use super::super::*;
        use crate::platform::WindowAttributes;

        const MANAGER_ID: u32 = 3;
        const XDG_SURFACE: u32 = 4;
        const TOPLEVEL: u32 = 5;
        const SEAT: u32 = 6;

        fn word(args: &[u8], i: usize) -> u32 {
            u32::from_le_bytes(args[i * 4..i * 4 + 4].try_into().unwrap())
        }

        fn csd_conn() -> (WlConn, std::os::unix::net::UnixStream) {
            let (mut conn, compositor) = conn_pair();
            let attrs = WindowAttributes::new();
            setup(&mut conn, None, XDG_SURFACE, TOPLEVEL, Some(SEAT), &attrs);
            (conn, compositor)
        }

        #[test]
        fn asks_for_server_side_and_switches_on_client_side() {
            let (mut conn, compositor) = conn_pair();
            let attrs = WindowAttributes::new().with_min_size(Size::new(100.0, 50.0));
            setup(
                &mut conn,
                Some(MANAGER_ID),
                XDG_SURFACE,
                TOPLEVEL,
                Some(SEAT),
                &attrs,
            );
            conn.decor.set_limits(Some(attrs.min_size), None);
            assert!(!conn.decor.csd, "server-side asked for");
            let (object, opcode, args, _) = read_request(&compositor);
            assert_eq!(
                (object, opcode),
                (MANAGER_ID, MANAGER_GET_TOPLEVEL_DECORATION)
            );
            let decoration = word(&args, 0);
            assert_eq!(word(&args, 1), TOPLEVEL);
            let (object, opcode, args, _) = read_request(&compositor);
            assert_eq!(
                (object, opcode, word(&args, 0)),
                (decoration, DECORATION_SET_MODE, MODE_SERVER_SIDE)
            );
            // The compositor would rather we drew them.
            let event = message(
                decoration,
                DECORATION_CONFIGURE,
                &MODE_CLIENT_SIDE.to_le_bytes(),
            );
            assert!(handle(
                &mut conn,
                decoration,
                DECORATION_CONFIGURE,
                &event[8..]
            ));
            assert!(conn.decor.csd);
            // The window's minimum grows by the title bar.
            let (object, opcode, args, _) = read_request(&compositor);
            assert_eq!((object, opcode), (TOPLEVEL, XDG_TOPLEVEL_SET_MIN_SIZE));
            assert_eq!(
                (word(&args, 0), word(&args, 1)),
                (100, 50 + TITLE_HEIGHT as u32)
            );
            assert!(!handle(&mut conn, 99, 0, &[]), "not the decoration's");
        }

        #[test]
        fn no_manager_means_client_side_unless_turned_off() {
            let (conn, _compositor) = csd_conn();
            assert!(conn.decor.csd);
            let (mut conn, _compositor) = conn_pair();
            let attrs = WindowAttributes::new().with_decorations(false);
            setup(&mut conn, None, XDG_SURFACE, TOPLEVEL, Some(SEAT), &attrs);
            assert!(!conn.decor.csd);
        }

        #[test]
        fn title_bar_moves_edges_resize_and_buttons_click() {
            let (mut conn, compositor) = csd_conn();
            let content = PhysicalSize::new(300, 200);
            let title = Point::new(MARGIN + 40.0, MARGIN + 10.0);
            // Press on the title bar: move, citing the seat and the serial.
            assert_eq!(
                button(&mut conn, title, content, BTN_LEFT, true, 41),
                Press::Done
            );
            let (object, opcode, args, _) = read_request(&compositor);
            assert_eq!((object, opcode), (TOPLEVEL, XDG_TOPLEVEL_MOVE));
            assert_eq!((word(&args, 0), word(&args, 1)), (SEAT, 41));
            assert_eq!(
                button(&mut conn, title, content, BTN_LEFT, false, 42),
                Press::Done
            );
            // A second press soon after: maximize.
            button(&mut conn, title, content, BTN_LEFT, true, 43);
            let (_, opcode, _, _) = read_request(&compositor);
            assert_eq!(opcode, XDG_TOPLEVEL_SET_MAXIMIZED);
            // The bottom-right corner: resize both ways.
            let corner = Point::new(MARGIN + 304.0, MARGIN + TITLE_HEIGHT + 204.0);
            button(&mut conn, corner, content, BTN_LEFT, true, 44);
            let (_, opcode, args, _) = read_request(&compositor);
            assert_eq!(opcode, XDG_TOPLEVEL_RESIZE);
            assert_eq!(word(&args, 2), EDGE_BOTTOM | EDGE_RIGHT);
            // Right-click on the title: the window menu, in window coordinates.
            button(&mut conn, title, content, BTN_RIGHT, true, 45);
            let (_, opcode, args, _) = read_request(&compositor);
            assert_eq!(opcode, XDG_TOPLEVEL_SHOW_WINDOW_MENU);
            assert_eq!((word(&args, 2), word(&args, 3)), (40, 10));
            // The close button closes on release over it, not on press.
            let close = Point::new(MARGIN + 295.0, MARGIN + 10.0);
            assert_eq!(
                button(&mut conn, close, content, BTN_LEFT, true, 46),
                Press::Redraw
            );
            assert_eq!(
                button(&mut conn, close, content, BTN_LEFT, false, 47),
                Press::Close
            );
            // Pressed, then let go elsewhere: cancelled.
            button(&mut conn, close, content, BTN_LEFT, true, 48);
            assert_eq!(
                button(&mut conn, title, content, BTN_LEFT, false, 49),
                Press::Redraw
            );
            // The content's presses are the app's, in its coordinates, and so
            // is the release wherever it lands.
            let inside = Point::new(MARGIN + 10.0, MARGIN + TITLE_HEIGHT + 20.0);
            assert_eq!(
                button(&mut conn, inside, content, BTN_LEFT, true, 50),
                Press::Forward(Point::new(10.0, 20.0))
            );
            assert_eq!(
                button(&mut conn, title, content, BTN_LEFT, false, 51),
                Press::Forward(Point::new(40.0, 10.0 - TITLE_HEIGHT))
            );
        }

        #[test]
        fn geometry_excludes_the_margin() {
            let (mut conn, compositor) = csd_conn();
            send_geometry(&mut conn, PhysicalSize::new(300, 200));
            let (object, opcode, args, _) = read_request(&compositor);
            assert_eq!(
                (object, opcode),
                (XDG_SURFACE, XDG_SURFACE_SET_WINDOW_GEOMETRY)
            );
            let g: Vec<u32> = (0..4).map(|i| word(&args, i)).collect();
            assert_eq!(g, [8, 8, 300, 232]);
        }
    }
}
