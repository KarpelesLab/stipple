//! The Linux tray icon: a `StatusNotifierItem` (the freedesktop / KDE protocol
//! that KDE Plasma, GNOME with the AppIndicator extension, XFCE, Cinnamon,
//! waybar, sway's swaybar and others host) with its menu exported over
//! `com.canonical.dbusmenu`, both served by hand over D-Bus.
//!
//! [`set_tray`] starts a thread that owns a session-bus connection: it claims
//! `org.kde.StatusNotifierItem-<pid>-<n>`, registers that name with
//! `org.kde.StatusNotifierWatcher`, and answers the host's calls on
//! `/StatusNotifierItem` and `/MenuBar`. Later updates change the shared model
//! and poke the thread through a socket pair, and it tells the host with the
//! `NewTitle` / `NewIcon` / `NewToolTip` and `LayoutUpdated` signals. A menu
//! pick (`Event(id, "clicked")`) or a click on the icon (`Activate`) is
//! [`post`](super::post)ed to the event loop. Removing the tray closes the
//! connection, which takes the item off the host.
//!
//! Without a watcher (a desktop with no tray), the item waits, and registers
//! as soon as one appears.

use super::linux::{W, app_name, debug};
use crate::platform::a11y::{DBus, Message, align_to, read_string, read_u32, skip_value};
use crate::platform::event::Event;
use crate::platform::menu::{NativeMenuItem, NativeTray};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";
const ITEM_IFACE: &str = "org.kde.StatusNotifierItem";
const MENU_IFACE: &str = "com.canonical.dbusmenu";
const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const PROPS: &str = "org.freedesktop.DBus.Properties";

/// dbusmenu ids for separators and submenus, which have no command id.
const STRUCTURAL_ID_BASE: i32 = 0x10000;

/// What the host is shown, shared with the service thread.
struct Model {
    tray: NativeTray,
    /// The dbusmenu layout revision, bumped on each change.
    revision: u32,
}

struct Service {
    model: Arc<Mutex<Model>>,
    /// Writing a byte tells the thread the model changed; dropping it ends
    /// the thread.
    poke: UnixStream,
    registered: Arc<std::sync::atomic::AtomicBool>,
}

static SERVICE: Mutex<Option<Service>> = Mutex::new(None);
static INSTANCE: AtomicU32 = AtomicU32::new(0);

/// Show `tray` (or remove it, `None`). Returns whether a tray host has the
/// item: `false` without a session bus or a `StatusNotifierWatcher`.
pub(crate) fn set_tray(tray: Option<&NativeTray>) -> bool {
    let Ok(mut service) = SERVICE.lock() else {
        return false;
    };
    let Some(tray) = tray else {
        // Ends the thread, which closes the connection.
        *service = None;
        return false;
    };
    if let Some(s) = service.as_mut() {
        if let Ok(mut m) = s.model.lock() {
            m.tray = tray.clone();
            m.revision = m.revision.wrapping_add(1);
        }
        let _ = s.poke.write_all(&[1]);
        return s.registered.load(Ordering::Acquire);
    }
    match start(tray.clone()) {
        Ok(s) => {
            let shown = s.registered.load(Ordering::Acquire);
            *service = Some(s);
            shown
        }
        Err(e) => {
            debug(format_args!("no tray: {e}"));
            false
        }
    }
}

/// Connect, claim a name, and start serving; waits briefly for the watcher
/// to accept the registration.
fn start(tray: NativeTray) -> Result<Service, String> {
    let mut bus = DBus::connect_session()?;
    bus.set_read_timeout(Some(Duration::from_secs(2)));
    let n = INSTANCE.fetch_add(1, Ordering::Relaxed) + 1;
    let name = format!("org.kde.StatusNotifierItem-{}-{n}", std::process::id());
    bus.request_name(&name)?;
    bus.add_match(&format!(
        "type='signal',sender='org.freedesktop.DBus',interface='org.freedesktop.DBus',\
         member='NameOwnerChanged',arg0='{WATCHER}'"
    ))?;
    bus.set_read_timeout(None);

    let model = Arc::new(Mutex::new(Model { tray, revision: 1 }));
    let registered = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (poke, wake) = UnixStream::pair().map_err(|e| e.to_string())?;
    let (tx, rx) = std::sync::mpsc::channel();
    let mut server = Server {
        bus,
        name,
        model: model.clone(),
        registered: registered.clone(),
        register_serial: 0,
        first: Some(tx),
    };
    std::thread::Builder::new()
        .name("stipple-tray".into())
        .spawn(move || {
            if let Err(e) = server.run(wake) {
                debug(format_args!("tray service ended: {e}"));
            }
        })
        .map_err(|e| e.to_string())?;
    // The registration's reply (or its failure) comes back to the thread.
    let _ = rx.recv_timeout(Duration::from_secs(2));
    Ok(Service {
        model,
        poke,
        registered,
    })
}

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

unsafe extern "C" {
    fn poll(fds: *mut PollFd, nfds: std::ffi::c_ulong, timeout: i32) -> i32;
}

const POLLIN: i16 = 1;

struct Server {
    bus: DBus,
    name: String,
    model: Arc<Mutex<Model>>,
    registered: Arc<std::sync::atomic::AtomicBool>,
    register_serial: u32,
    /// Told the outcome of the first registration attempt.
    first: Option<std::sync::mpsc::Sender<bool>>,
}

impl Server {
    fn run(&mut self, mut wake: UnixStream) -> Result<(), String> {
        self.register()?;
        loop {
            let mut fds = [
                PollFd {
                    fd: self.bus.raw_fd(),
                    events: POLLIN,
                    revents: 0,
                },
                PollFd {
                    fd: std::os::fd::AsRawFd::as_raw_fd(&wake),
                    events: POLLIN,
                    revents: 0,
                },
            ];
            let r = unsafe { poll(fds.as_mut_ptr(), 2, -1) };
            if r < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err("poll failed".into());
            }
            if fds[1].revents != 0 {
                let mut buf = [0u8; 64];
                match wake.read(&mut buf) {
                    // The tray was removed (or the app is going away).
                    Ok(0) | Err(_) => return Ok(()),
                    Ok(_) => self.changed()?,
                }
            }
            if fds[0].revents != 0 {
                let m = self.bus.read_message()?;
                self.handle(m)?;
            }
        }
    }

    /// `RegisterStatusNotifierItem(name)`, answered in [`handle`](Self::handle).
    fn register(&mut self) -> Result<(), String> {
        let mut w = W::default();
        w.str(&self.name);
        self.register_serial = self.bus.send_call(
            WATCHER,
            "/StatusNotifierWatcher",
            WATCHER,
            "RegisterStatusNotifierItem",
            "s",
            &w.buf,
        )?;
        Ok(())
    }

    fn registration(&mut self, ok: bool) {
        debug(format_args!("tray registered with the watcher: {ok}"));
        self.registered.store(ok, Ordering::Release);
        if let Some(tx) = self.first.take() {
            let _ = tx.send(ok);
        }
    }

    /// Tell the host the title, icon, tooltip and menu may have changed.
    fn changed(&mut self) -> Result<(), String> {
        for member in ["NewTitle", "NewIcon", "NewToolTip"] {
            self.bus
                .emit_signal(ITEM_PATH, ITEM_IFACE, member, "", &[])?;
        }
        let revision = self.model.lock().map(|m| m.revision).unwrap_or(0);
        let mut w = W::default();
        w.u32(revision);
        w.i32(0);
        self.bus
            .emit_signal(MENU_PATH, MENU_IFACE, "LayoutUpdated", "ui", &w.buf)
    }

    fn handle(&mut self, m: Message) -> Result<(), String> {
        match m.mtype {
            2 | 3 if m.reply_serial == self.register_serial && self.register_serial != 0 => {
                self.registration(m.mtype == 2);
                return Ok(());
            }
            4 => {
                if m.member == "NameOwnerChanged" {
                    let mut off = 0;
                    let name = read_string(&m.body, &mut off).unwrap_or_default();
                    let _old = read_string(&m.body, &mut off).unwrap_or_default();
                    let new = read_string(&m.body, &mut off).unwrap_or_default();
                    if name == WATCHER && !new.is_empty() {
                        self.register()?;
                    }
                }
                return Ok(());
            }
            1 => {}
            _ => return Ok(()),
        }
        let model = self.model.clone();
        let model = model.lock().map_err(|_| "tray model poisoned")?;
        let mut w = W::default();
        match (m.path.as_str(), m.interface.as_str(), m.member.as_str()) {
            (_, "org.freedesktop.DBus.Peer", "Ping") => self.bus.send_return(&m, "", &[]),
            (path, "org.freedesktop.DBus.Introspectable", "Introspect") => {
                w.str(introspection(path));
                self.bus.send_return(&m, "s", &w.buf)
            }
            (ITEM_PATH, PROPS, "Get") | (MENU_PATH, PROPS, "Get") => {
                let mut off = 0;
                let _iface = read_string(&m.body, &mut off).unwrap_or_default();
                let prop = read_string(&m.body, &mut off).unwrap_or_default();
                let found = if m.path == ITEM_PATH {
                    item_property(&model.tray, &prop, &mut w)
                } else {
                    menu_property(&prop, &mut w)
                };
                if found {
                    self.bus.send_return(&m, "v", &w.buf)
                } else {
                    self.bus
                        .send_error(&m, "org.freedesktop.DBus.Error.UnknownProperty")
                }
            }
            (ITEM_PATH, PROPS, "GetAll") | (MENU_PATH, PROPS, "GetAll") => {
                let names: &[&str] = if m.path == ITEM_PATH {
                    &ITEM_PROPERTIES
                } else {
                    &MENU_PROPERTIES
                };
                let item = m.path == ITEM_PATH;
                w.array(8, |w| {
                    for name in names {
                        w.strukt(|w| {
                            w.str(name);
                            if item {
                                item_property(&model.tray, name, w);
                            } else {
                                menu_property(name, w);
                            }
                        });
                    }
                });
                self.bus.send_return(&m, "a{sv}", &w.buf)
            }
            (ITEM_PATH, ITEM_IFACE, "Activate") => {
                super::post(Event::TrayActivated);
                self.bus.send_return(&m, "", &[])
            }
            (ITEM_PATH, ITEM_IFACE, "SecondaryActivate" | "ContextMenu" | "Scroll") => {
                // The host shows the menu itself from `Menu`.
                self.bus.send_return(&m, "", &[])
            }
            (MENU_PATH, MENU_IFACE, member) => {
                let nodes = flatten(&model.tray.menu);
                match member {
                    "GetLayout" => {
                        let mut off = 0;
                        let parent = read_u32(&m.body, &mut off).unwrap_or(0) as i32;
                        let depth = read_u32(&m.body, &mut off).unwrap_or(u32::MAX) as i32;
                        let Some(root) = nodes.iter().position(|n| n.id == parent) else {
                            return self
                                .bus
                                .send_error(&m, "org.freedesktop.DBus.Error.InvalidArgs");
                        };
                        w.u32(model.revision);
                        write_layout(&nodes, root, depth, &mut w);
                        self.bus.send_return(&m, "u(ia{sv}av)", &w.buf)
                    }
                    "GetGroupProperties" => {
                        let ids = read_ids(&m.body);
                        w.array(8, |w| {
                            for n in &nodes {
                                if ids.is_empty() || ids.contains(&n.id) {
                                    w.strukt(|w| {
                                        w.i32(n.id);
                                        write_props(n, w);
                                    });
                                }
                            }
                        });
                        self.bus.send_return(&m, "a(ia{sv})", &w.buf)
                    }
                    "GetProperty" => {
                        let mut off = 0;
                        let id = read_u32(&m.body, &mut off).unwrap_or(0) as i32;
                        let name = read_string(&m.body, &mut off).unwrap_or_default();
                        let value = nodes
                            .iter()
                            .find(|n| n.id == id)
                            .and_then(|n| n.props.iter().find(|(k, _)| *k == name));
                        match value {
                            Some((_, v)) => {
                                v.write(&mut w);
                                self.bus.send_return(&m, "v", &w.buf)
                            }
                            None => self
                                .bus
                                .send_error(&m, "org.freedesktop.DBus.Error.InvalidArgs"),
                        }
                    }
                    "Event" => {
                        let mut off = 0;
                        let id = read_u32(&m.body, &mut off).unwrap_or(0) as i32;
                        let event = read_string(&m.body, &mut off).unwrap_or_default();
                        menu_event(&nodes, id, &event);
                        self.bus.send_return(&m, "", &[])
                    }
                    "EventGroup" => {
                        for (id, event) in read_event_group(&m.body) {
                            menu_event(&nodes, id, &event);
                        }
                        w.array(4, |_| {});
                        self.bus.send_return(&m, "ai", &w.buf)
                    }
                    "AboutToShow" => {
                        w.bool(false);
                        self.bus.send_return(&m, "b", &w.buf)
                    }
                    "AboutToShowGroup" => {
                        w.array(4, |_| {});
                        w.array(4, |_| {});
                        self.bus.send_return(&m, "aiai", &w.buf)
                    }
                    _ => self
                        .bus
                        .send_error(&m, "org.freedesktop.DBus.Error.UnknownMethod"),
                }
            }
            (ITEM_PATH | MENU_PATH, _, _) => self
                .bus
                .send_error(&m, "org.freedesktop.DBus.Error.UnknownMethod"),
            _ => self
                .bus
                .send_error(&m, "org.freedesktop.DBus.Error.UnknownObject"),
        }
    }
}

/// A dbusmenu item clicked: run the command (if it is one, and enabled).
fn menu_event(nodes: &[Node], id: i32, event: &str) {
    debug(format_args!("tray menu event {event} on {id}"));
    if event != "clicked" {
        return;
    }
    if let Some(n) = nodes.iter().find(|n| n.id == id)
        && n.command
        && n.enabled
    {
        super::post(Event::MenuCommand(id as u32));
    }
}

/// The ids of a `GetGroupProperties` call (`aias`).
fn read_ids(body: &[u8]) -> Vec<i32> {
    let mut off = 0;
    let Ok(len) = read_u32(body, &mut off) else {
        return Vec::new();
    };
    let end = (off + len as usize).min(body.len());
    let mut ids = Vec::new();
    while off < end {
        match read_u32(body, &mut off) {
            Ok(id) => ids.push(id as i32),
            Err(_) => break,
        }
    }
    ids
}

/// The `(id, eventId)` pairs of an `EventGroup` call (`a(isvu)`).
fn read_event_group(body: &[u8]) -> Vec<(i32, String)> {
    let mut out = Vec::new();
    let mut off = 0;
    let Ok(len) = read_u32(body, &mut off) else {
        return out;
    };
    align_to(&mut off, 8);
    let end = (off + len as usize).min(body.len());
    while off < end {
        align_to(&mut off, 8);
        let (Ok(id), Ok(event)) = (read_u32(body, &mut off), read_string(body, &mut off)) else {
            break;
        };
        let mut si = 0;
        if skip_value(body, &mut off, b"v", &mut si).is_err() || read_u32(body, &mut off).is_err() {
            break;
        }
        out.push((id as i32, event));
    }
    out
}

// ---- The item's properties ----------------------------------------------------

const ITEM_PROPERTIES: [&str; 14] = [
    "Category",
    "Id",
    "Title",
    "Status",
    "WindowId",
    "IconName",
    "IconPixmap",
    "OverlayIconName",
    "OverlayIconPixmap",
    "AttentionIconName",
    "AttentionIconPixmap",
    "ToolTip",
    "ItemIsMenu",
    "Menu",
];

const MENU_PROPERTIES: [&str; 4] = ["Version", "TextDirection", "Status", "IconThemePath"];

/// Write `org.kde.StatusNotifierItem` property `name` as a variant; `false`
/// if there's no such property.
fn item_property(tray: &NativeTray, name: &str, w: &mut W) -> bool {
    let empty_pixmaps = |w: &mut W| w.variant("a(iiay)", |w| w.array(8, |_| {}));
    match name {
        "Category" => w.variant("s", |w| w.str("ApplicationStatus")),
        "Id" => w.variant("s", |w| w.str(&app_name())),
        "Title" => w.variant("s", |w| w.str(&tray.tooltip)),
        "Status" => w.variant("s", |w| w.str("Active")),
        "WindowId" => w.variant("i", |w| w.i32(0)),
        "IconName" | "OverlayIconName" | "AttentionIconName" => w.variant("s", |w| w.str("")),
        "IconPixmap" => w.variant("a(iiay)", |w| write_pixmaps(tray, w)),
        "OverlayIconPixmap" | "AttentionIconPixmap" => empty_pixmaps(w),
        "ToolTip" => w.variant("(sa(iiay)ss)", |w| {
            w.strukt(|w| {
                w.str("");
                w.array(8, |_| {});
                w.str(&tray.tooltip);
                w.str("");
            })
        }),
        "ItemIsMenu" => w.variant("b", |w| w.bool(false)),
        "Menu" => w.variant("o", |w| w.str(MENU_PATH)),
        _ => return false,
    }
    true
}

/// Write `com.canonical.dbusmenu` property `name` as a variant.
fn menu_property(name: &str, w: &mut W) -> bool {
    match name {
        "Version" => w.variant("u", |w| w.u32(3)),
        "TextDirection" => w.variant("s", |w| w.str("ltr")),
        "Status" => w.variant("s", |w| w.str("normal")),
        "IconThemePath" => w.variant("as", |w| w.array(4, |_| {})),
        _ => return false,
    }
    true
}

/// The icon at a few sizes as `a(iiay)`: width, height, and ARGB32 pixels in
/// network byte order.
fn write_pixmaps(tray: &NativeTray, w: &mut W) {
    let icon = tray.icon.clone().unwrap_or_else(placeholder_icon);
    let mut sizes: Vec<crate::render::Pixmap> = Vec::new();
    for max in [22u32, 32, 64] {
        let p = crate::platform::window::fit_icon(&icon, max);
        if !sizes.iter().any(|s| s.size() == p.size()) {
            sizes.push(p);
        }
    }
    w.array(8, |w| {
        for p in &sizes {
            w.strukt(|w| {
                let size = p.size();
                w.i32(size.width as i32);
                w.i32(size.height as i32);
                w.array(1, |w| {
                    let stride = p.stride();
                    let bytes = p.as_bytes();
                    for y in 0..size.height as usize {
                        for px in bytes[y * stride..y * stride + size.width as usize * 4]
                            .as_chunks::<4>()
                            .0
                        {
                            w.buf.extend_from_slice(&[px[3], px[0], px[1], px[2]]);
                        }
                    }
                });
            });
        }
    });
}

/// A plain round icon for a tray declared without one.
fn placeholder_icon() -> crate::render::Pixmap {
    let n = 32u32;
    let mut p = crate::render::Pixmap::new(crate::geometry::PhysicalSize::new(n, n));
    let c = (n as f64 - 1.0) / 2.0;
    for (i, px) in p
        .as_bytes_mut()
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .enumerate()
    {
        let (x, y) = ((i as u32 % n) as f64, (i as u32 / n) as f64);
        let d = ((x - c).powi(2) + (y - c).powi(2)).sqrt();
        let a = (c - d + 0.5).clamp(0.0, 1.0);
        px.copy_from_slice(&[0x3b, 0x82, 0xf6, (a * 255.0) as u8]);
    }
    p
}

// ---- The menu as dbusmenu nodes ------------------------------------------------

/// A dbusmenu property value.
#[derive(Clone, Debug, PartialEq)]
enum Value {
    Str(String),
    Bool(bool),
    Int(i32),
}

impl Value {
    fn write(&self, w: &mut W) {
        match self {
            Value::Str(s) => w.variant("s", |w| w.str(s)),
            Value::Bool(b) => w.variant("b", |w| w.bool(*b)),
            Value::Int(i) => w.variant("i", |w| w.i32(*i)),
        }
    }
}

/// One dbusmenu item: the root (id 0), a command (its command id), or a
/// separator or submenu (ids from [`STRUCTURAL_ID_BASE`]).
#[derive(Debug)]
struct Node {
    id: i32,
    props: Vec<(&'static str, Value)>,
    children: Vec<usize>,
    command: bool,
    enabled: bool,
}

/// The menu as a flat list of nodes, the root first.
fn flatten(items: &[NativeMenuItem]) -> Vec<Node> {
    fn add(items: &[NativeMenuItem], parent: usize, nodes: &mut Vec<Node>, next: &mut i32) {
        for item in items {
            let index = nodes.len();
            match item {
                NativeMenuItem::Action {
                    id,
                    label,
                    enabled,
                    checked,
                    ..
                } => {
                    let mut props = vec![("label", Value::Str(dbusmenu_label(label)))];
                    if !enabled {
                        props.push(("enabled", Value::Bool(false)));
                    }
                    if let Some(on) = checked {
                        props.push(("toggle-type", Value::Str("checkmark".into())));
                        props.push(("toggle-state", Value::Int(i32::from(*on))));
                    }
                    nodes.push(Node {
                        id: *id as i32,
                        props,
                        children: Vec::new(),
                        command: true,
                        enabled: *enabled,
                    });
                }
                NativeMenuItem::Separator => {
                    nodes.push(Node {
                        id: *next,
                        props: vec![("type", Value::Str("separator".into()))],
                        children: Vec::new(),
                        command: false,
                        enabled: true,
                    });
                    *next += 1;
                }
                NativeMenuItem::Submenu(menu) => {
                    nodes.push(Node {
                        id: *next,
                        props: vec![
                            ("label", Value::Str(dbusmenu_label(&menu.label))),
                            ("children-display", Value::Str("submenu".into())),
                        ],
                        children: Vec::new(),
                        command: false,
                        enabled: true,
                    });
                    *next += 1;
                    add(&menu.items, index, nodes, next);
                }
            }
            nodes[parent].children.push(index);
        }
    }
    let mut nodes = vec![Node {
        id: 0,
        props: vec![("children-display", Value::Str("submenu".into()))],
        children: Vec::new(),
        command: false,
        enabled: true,
    }];
    let mut next = STRUCTURAL_ID_BASE;
    add(items, 0, &mut nodes, &mut next);
    nodes
}

/// A Stipple label (`&` mnemonics) as a dbusmenu one (`_` mnemonics).
fn dbusmenu_label(label: &str) -> String {
    let mut out = String::with_capacity(label.len() + 1);
    let mut marked = false;
    let mut chars = label.chars();
    while let Some(c) = chars.next() {
        match c {
            '_' => out.push_str("__"),
            '&' => match chars.next() {
                Some('&') => out.push('&'),
                Some('_') => out.push_str("__"),
                Some(next) => {
                    if !marked {
                        out.push('_');
                        marked = true;
                    }
                    out.push(next);
                }
                None => {}
            },
            c => out.push(c),
        }
    }
    out
}

fn write_props(node: &Node, w: &mut W) {
    w.array(8, |w| {
        for (k, v) in &node.props {
            w.strukt(|w| {
                w.str(k);
                v.write(w);
            });
        }
    });
}

/// Write node `i` as `(ia{sv}av)`, with children down to `depth` levels (-1:
/// all of them).
fn write_layout(nodes: &[Node], i: usize, depth: i32, w: &mut W) {
    let node = &nodes[i];
    w.strukt(|w| {
        w.i32(node.id);
        write_props(node, w);
        w.array(1, |w| {
            if depth != 0 {
                for &c in &node.children {
                    w.variant("(ia{sv}av)", |w| write_layout(nodes, c, depth - 1, w));
                }
            }
        });
    });
}

fn introspection(path: &str) -> &'static str {
    match path {
        ITEM_PATH => {
            r#"<node>
 <interface name="org.kde.StatusNotifierItem">
  <property name="Category" type="s" access="read"/>
  <property name="Id" type="s" access="read"/>
  <property name="Title" type="s" access="read"/>
  <property name="Status" type="s" access="read"/>
  <property name="WindowId" type="i" access="read"/>
  <property name="IconName" type="s" access="read"/>
  <property name="IconPixmap" type="a(iiay)" access="read"/>
  <property name="OverlayIconName" type="s" access="read"/>
  <property name="OverlayIconPixmap" type="a(iiay)" access="read"/>
  <property name="AttentionIconName" type="s" access="read"/>
  <property name="AttentionIconPixmap" type="a(iiay)" access="read"/>
  <property name="ToolTip" type="(sa(iiay)ss)" access="read"/>
  <property name="ItemIsMenu" type="b" access="read"/>
  <property name="Menu" type="o" access="read"/>
  <method name="Activate"><arg name="x" type="i" direction="in"/><arg name="y" type="i" direction="in"/></method>
  <method name="SecondaryActivate"><arg name="x" type="i" direction="in"/><arg name="y" type="i" direction="in"/></method>
  <method name="ContextMenu"><arg name="x" type="i" direction="in"/><arg name="y" type="i" direction="in"/></method>
  <method name="Scroll"><arg name="delta" type="i" direction="in"/><arg name="orientation" type="s" direction="in"/></method>
  <signal name="NewTitle"/>
  <signal name="NewIcon"/>
  <signal name="NewToolTip"/>
 </interface>
 <interface name="org.freedesktop.DBus.Properties">
  <method name="Get"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="v" direction="out"/></method>
  <method name="GetAll"><arg type="s" direction="in"/><arg type="a{sv}" direction="out"/></method>
 </interface>
</node>"#
        }
        MENU_PATH => {
            r#"<node>
 <interface name="com.canonical.dbusmenu">
  <property name="Version" type="u" access="read"/>
  <property name="TextDirection" type="s" access="read"/>
  <property name="Status" type="s" access="read"/>
  <property name="IconThemePath" type="as" access="read"/>
  <method name="GetLayout"><arg type="i" name="parentId" direction="in"/><arg type="i" name="recursionDepth" direction="in"/><arg type="as" name="propertyNames" direction="in"/><arg type="u" name="revision" direction="out"/><arg type="(ia{sv}av)" name="layout" direction="out"/></method>
  <method name="GetGroupProperties"><arg type="ai" name="ids" direction="in"/><arg type="as" name="propertyNames" direction="in"/><arg type="a(ia{sv})" name="properties" direction="out"/></method>
  <method name="GetProperty"><arg type="i" name="id" direction="in"/><arg type="s" name="name" direction="in"/><arg type="v" name="value" direction="out"/></method>
  <method name="Event"><arg type="i" name="id" direction="in"/><arg type="s" name="eventId" direction="in"/><arg type="v" name="data" direction="in"/><arg type="u" name="timestamp" direction="in"/></method>
  <method name="EventGroup"><arg type="a(isvu)" name="events" direction="in"/><arg type="ai" name="idErrors" direction="out"/></method>
  <method name="AboutToShow"><arg type="i" name="id" direction="in"/><arg type="b" name="needUpdate" direction="out"/></method>
  <method name="AboutToShowGroup"><arg type="ai" name="ids" direction="in"/><arg type="ai" name="updatesNeeded" direction="out"/><arg type="ai" name="idErrors" direction="out"/></method>
  <signal name="ItemsPropertiesUpdated"><arg type="a(ia{sv})" name="updatedProps"/><arg type="a(ias)" name="removedProps"/></signal>
  <signal name="LayoutUpdated"><arg type="u" name="revision"/><arg type="i" name="parent"/></signal>
  <signal name="ItemActivationRequested"><arg type="i" name="id"/><arg type="u" name="timestamp"/></signal>
 </interface>
</node>"#
        }
        "/" => r#"<node><node name="StatusNotifierItem"/><node name="MenuBar"/></node>"#,
        _ => "<node/>",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::menu::{NativeMenu, TRAY_ID_BASE};

    fn tray() -> NativeTray {
        NativeTray {
            tooltip: "Demo".into(),
            icon: None,
            menu: vec![
                NativeMenuItem::Action {
                    id: TRAY_ID_BASE,
                    label: "&Ping".into(),
                    shortcut: None,
                    enabled: true,
                    checked: None,
                },
                NativeMenuItem::Separator,
                NativeMenuItem::Submenu(NativeMenu {
                    label: "More".into(),
                    items: vec![NativeMenuItem::Action {
                        id: TRAY_ID_BASE + 1,
                        label: "Muted".into(),
                        shortcut: None,
                        enabled: false,
                        checked: Some(true),
                    }],
                }),
            ],
        }
    }

    #[test]
    fn menu_flattens_into_dbusmenu_nodes() {
        let nodes = flatten(&tray().menu);
        let ids: Vec<i32> = nodes.iter().map(|n| n.id).collect();
        let base = TRAY_ID_BASE as i32;
        assert_eq!(
            ids,
            vec![
                0,
                base,
                STRUCTURAL_ID_BASE,
                STRUCTURAL_ID_BASE + 1,
                base + 1
            ]
        );
        assert_eq!(nodes[0].children, vec![1, 2, 3]);
        assert_eq!(nodes[3].children, vec![4]);
        assert_eq!(nodes[1].props[0], ("label", Value::Str("_Ping".into())));
        assert!(nodes[4].props.contains(&("toggle-state", Value::Int(1))));
        assert!(!nodes[4].enabled);
    }

    #[test]
    fn labels_use_underscore_mnemonics() {
        assert_eq!(dbusmenu_label("&File"), "_File");
        assert_eq!(dbusmenu_label("Save &As"), "Save _As");
        assert_eq!(dbusmenu_label("snake_case"), "snake__case");
        assert_eq!(dbusmenu_label("Fish && Chips"), "Fish & Chips");
    }

    #[test]
    fn layout_marshals_and_events_read_back() {
        let nodes = flatten(&tray().menu);
        let mut w = W::default();
        w.u32(3);
        write_layout(&nodes, 0, -1, &mut w);
        // revision, then the root's id 0, then its property dict.
        let mut off = 0;
        assert_eq!(read_u32(&w.buf, &mut off).unwrap(), 3);
        align_to(&mut off, 8);
        assert_eq!(read_u32(&w.buf, &mut off).unwrap(), 0);
        let mut si = 0;
        skip_value(&w.buf, &mut off, b"a{sv}", &mut si).unwrap();
        let mut si = 0;
        skip_value(&w.buf, &mut off, b"av", &mut si).unwrap();
        assert_eq!(off, w.buf.len());

        // An EventGroup body with two clicks.
        let mut g = W::default();
        g.array(8, |w| {
            for id in [5, 7] {
                w.strukt(|w| {
                    w.i32(id);
                    w.str("clicked");
                    w.variant("i", |w| w.i32(0));
                    w.u32(0);
                });
            }
        });
        assert_eq!(
            read_event_group(&g.buf),
            vec![(5, "clicked".into()), (7, "clicked".into())]
        );
        let mut ids = W::default();
        ids.array(4, |w| {
            w.i32(1);
            w.i32(0x8000);
        });
        assert_eq!(read_ids(&ids.buf), vec![1, 0x8000]);
    }

    #[test]
    fn item_properties_cover_the_list() {
        let t = tray();
        for name in ITEM_PROPERTIES {
            let mut w = W::default();
            assert!(item_property(&t, name, &mut w), "{name}");
            let mut off = 0;
            let mut si = 0;
            skip_value(&w.buf, &mut off, b"v", &mut si).unwrap();
            assert_eq!(off, w.buf.len(), "{name}");
        }
        for name in MENU_PROPERTIES {
            assert!(menu_property(name, &mut W::default()));
        }
    }
}
