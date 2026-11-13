//! The **AT-SPI** bridge for X11 and Wayland windows: what Orca and every
//! other Linux screen reader reads. Built on the hand-written D-Bus client in
//! [`crate::platform::a11y`]; no `libatspi`, `atk` or `zbus`.
//!
//! On the first [`Window::set_accessibility_tree`] a helper thread finds the
//! accessibility bus (`$AT_SPI_BUS_ADDRESS`, or `org.a11y.Bus.GetAddress` on
//! the session bus, which starts `at-spi-bus-launcher` when needed), connects
//! to it and registers the app with the registry (`org.a11y.atspi.Socket.Embed`
//! on `org.a11y.atspi.Registry`'s root). With no session bus or no
//! accessibility bus it gives up quietly and the app runs as before; it never
//! holds up startup. `NO_AT_BRIDGE=1` (GTK's switch) turns it off.
//!
//! The thread answers the screen reader's calls itself, from the latest tree
//! each window handed over: the app's root (`/org/a11y/atspi/accessible/root`,
//! role application, with the `Application` interface) has the windows'
//! frames as children, and every node of a window's tree is an object whose
//! path is its place in the tree (`…/w{window}_{i}_{j}…`), so a node keeps its
//! path across rebuilds while the tree's shape stays. A swap is a clone of the
//! tree into an `Arc` (skipped when nothing changed); the calls walk the tree
//! from the path. Nodes answer `Accessible`, `Component` (extents in screen,
//! window and parent coordinates, hit testing), `Action` (a click at the
//! node's middle, handed to the UI thread through the loop's wake and
//! delivered as pointer events, as UIA's Invoke does on Windows), `Text` and
//! `EditableText` (text fields), and `Value` (a spin button's number).
//!
//! The UI thread raises the events when it swaps a tree: names, roles,
//! text, checked/selected/expanded states and children that changed, and
//! the focus moving (`object:state-changed:focused` and `focus:`); a window's
//! activation raises `window:activate` / `window:deactivate`.

mod events;
mod marshal;
mod serve;
mod text;

use crate::platform::a11y::{DBus, put_field, put_field_sig, put_field_u32};
use crate::platform::access::{A11yNode, A11yRole};
use crate::platform::{ControlFlow, Event, Window, WindowId};
use events::Signal;
use marshal::{Body, Value};
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Once};

/// The app's root object, where the registry expects it.
pub(crate) const ROOT_PATH: &str = "/org/a11y/atspi/accessible/root";
/// Every window's nodes are under this prefix (`{prefix}{window}_{i}_{j}…`).
const NODE_PREFIX: &str = "/org/a11y/atspi/accessible/w";
/// The AT-SPI null object.
const NULL_PATH: &str = "/org/a11y/atspi/null";
const REGISTRY: &str = "org.a11y.atspi.Registry";

/// One window's part of the tree.
#[derive(Debug)]
struct Win {
    id: u64,
    title: String,
    /// The content's top-left on the screen, in physical pixels (0,0 on
    /// Wayland, which doesn't tell).
    origin: (i32, i32),
    /// Physical pixels per logical pixel.
    scale: f64,
    /// Whether it is the active (focused) window.
    active: bool,
    root: Option<Arc<A11yNode>>,
}

/// What the bridge serves, shared by the UI thread and the bus thread.
#[derive(Debug)]
struct Bridge {
    /// Our unique name on the accessibility bus; empty until connected.
    bus: String,
    /// The registry's desktop, our root's parent (from `Embed`'s reply).
    desktop: (String, String),
    /// The id the registry gave the application.
    app_id: i32,
    windows: Vec<Win>,
    /// Pointer and input-method events for the UI thread to deliver to a
    /// window (an action a screen reader asked for).
    actions: Vec<(u64, Vec<Event>)>,
}

impl Bridge {
    const fn new() -> Self {
        Self {
            bus: String::new(),
            desktop: (String::new(), String::new()),
            app_id: 0,
            windows: Vec::new(),
            actions: Vec::new(),
        }
    }

    fn win_mut(&mut self, id: u64) -> &mut Win {
        if let Some(i) = self.windows.iter().position(|w| w.id == id) {
            return &mut self.windows[i];
        }
        self.windows.push(Win {
            id,
            title: String::new(),
            origin: (0, 0),
            scale: 1.0,
            active: false,
            root: None,
        });
        self.windows.last_mut().unwrap()
    }

    /// The windows that have a tree: the root's children, in order.
    fn frames(&self) -> impl Iterator<Item = &Win> {
        self.windows.iter().filter(|w| w.root.is_some())
    }
}

static BRIDGE: Mutex<Bridge> = Mutex::new(Bridge::new());
/// Sends on the accessibility bus, from either thread. `None` until connected.
static WRITER: Mutex<Option<Writer>> = Mutex::new(None);

fn debug(args: std::fmt::Arguments<'_>) {
    if std::env::var_os("STIPPLE_ATSPI_DEBUG").is_some() {
        eprintln!("stipple atspi: {args}");
    }
}

/// Set once the bus thread has given up (no bus, or it went away): the
/// windows stop handing their trees over.
static STOPPED: AtomicBool = AtomicBool::new(false);

fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    !STOPPED.load(Ordering::Relaxed)
        && *ON.get_or_init(|| std::env::var("NO_AT_BRIDGE").as_deref() != Ok("1"))
}

fn lock() -> std::sync::MutexGuard<'static, Bridge> {
    BRIDGE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The object path of the node at `chain` (child indices from the root) in
/// window `win`; the empty chain is the window's frame.
pub(super) fn node_path(win: u64, chain: &[usize]) -> String {
    use std::fmt::Write;
    let mut p = format!("{NODE_PREFIX}{win}");
    for i in chain {
        let _ = write!(p, "_{i}");
    }
    p
}

/// The window and child-index chain an object path names.
fn parse_path(path: &str) -> Option<(u64, Vec<usize>)> {
    let mut parts = path.strip_prefix(NODE_PREFIX)?.split('_');
    let win = parts.next()?.parse().ok()?;
    let chain = parts.map(|p| p.parse().ok()).collect::<Option<Vec<_>>>()?;
    Some((win, chain))
}

/// The `AtspiRole` for a role.
pub(super) fn role(role: A11yRole) -> u32 {
    use A11yRole::*;
    match role {
        Window => 23,        // FRAME
        Group => 39,         // PANEL
        Button => 43,        // PUSH_BUTTON
        TextField => 79,     // ENTRY
        Text => 29,          // LABEL
        List => 31,          // LIST
        ListItem => 32,      // LIST_ITEM
        Table => 55,         // TABLE
        Row => 90,           // TABLE_ROW
        Cell => 56,          // TABLE_CELL
        ColumnHeader => 57,  // TABLE_COLUMN_HEADER
        Tree => 65,          // TREE
        TreeItem => 91,      // TREE_ITEM
        MenuBar => 34,       // MENU_BAR
        Menu => 33,          // MENU
        MenuItem => 35,      // MENU_ITEM
        ComboBox => 11,      // COMBO_BOX
        SpinButton => 52,    // SPIN_BUTTON
        CheckBox => 7,       // CHECK_BOX
        Switch => 62,        // TOGGLE_BUTTON
        Splitter => 53,      // SPLIT_PANE
        Toolbar => 63,       // TOOL_BAR
        Separator => 50,     // SEPARATOR
        Alert => 101,        // NOTIFICATION
        TabList => 38,       // PAGE_TAB_LIST
        Tab => 37,           // PAGE_TAB
        PasswordField => 40, // PASSWORD_TEXT
        Dialog => 16,        // DIALOG
    }
}

/// The roles whose name is their text (the `Text` interface).
pub(super) fn text_role(role: A11yRole) -> bool {
    use A11yRole::*;
    matches!(role, Text | TextField | PasswordField | SpinButton)
}

/// The roles a screen reader can activate (the `Action` interface's
/// "click"), as UIA's Invoke pattern on Windows.
pub(super) fn clickable(role: A11yRole) -> bool {
    use A11yRole::*;
    matches!(
        role,
        Button | MenuItem | ComboBox | CheckBox | Switch | Tab | ColumnHeader | ListItem | TreeItem
    )
}

/// The roles that take keyboard focus.
pub(super) fn focusable(role: A11yRole) -> bool {
    use A11yRole::*;
    clickable(role)
        || matches!(
            role,
            TextField | PasswordField | SpinButton | Row | Splitter
        )
}

/// The roles a screen reader may type into.
pub(super) fn editable(role: A11yRole) -> bool {
    use A11yRole::*;
    matches!(role, TextField | PasswordField | SpinButton)
}

// ---- the UI thread's side ---------------------------------------------------

/// Hand a window's new tree to the bridge. `origin` is the content's top-left
/// on the screen in physical pixels, where known. Cheap when nothing changed.
pub(crate) fn set_tree(id: WindowId, root: &A11yNode, origin: Option<(i32, i32)>, scale: f64) {
    if !enabled() {
        return;
    }
    start();
    let signals = {
        let mut b = lock();
        let index = b.frames().count();
        let w = b.win_mut(id.0);
        if let Some(o) = origin {
            w.origin = o;
        }
        w.scale = scale;
        if w.root.as_deref() == Some(root) {
            return;
        }
        let old = w.root.replace(Arc::new(root.clone()));
        match old {
            Some(old) => events::diff(id.0, &old, root),
            None => {
                // A new frame under the app's root.
                let mut out = vec![events::children_changed(
                    ROOT_PATH.to_string(),
                    "add",
                    index,
                    node_path(id.0, &[]),
                )];
                if w.active {
                    out.extend(activation(w, true));
                }
                out
            }
        }
    };
    send(&signals);
}

/// The window's content moved on the screen.
pub(crate) fn set_origin(id: WindowId, origin: (i32, i32)) {
    if let Some(w) = lock().windows.iter_mut().find(|w| w.id == id.0) {
        w.origin = origin;
    }
}

/// The window's title, the frame's name when its tree names none.
pub(crate) fn set_title(id: WindowId, title: &str) {
    if !enabled() {
        return;
    }
    let signal = {
        let mut b = lock();
        let w = b.win_mut(id.0);
        if w.title == title {
            return;
        }
        w.title = title.to_string();
        let shown = w.root.as_ref().is_some_and(|r| r.name.is_empty());
        shown.then(|| {
            Signal::new(node_path(id.0, &[]), events::OBJECT, "PropertyChange")
                .kind("accessible-name")
                .any(Value::Str(title.to_string()))
        })
    };
    send(signal.as_slice());
}

/// The window gained or lost the keyboard focus.
fn set_active(id: WindowId, active: bool) {
    let signals = {
        let mut b = lock();
        let w = b.win_mut(id.0);
        if w.active == active {
            return;
        }
        w.active = active;
        if w.root.is_none() {
            return;
        }
        activation(w, active)
    };
    send(&signals);
}

/// The events of `w` becoming active or inactive: its frame's state and
/// `window:activate`, then the focus inside it.
fn activation(w: &Win, active: bool) -> Vec<Signal> {
    let Some(root) = &w.root else {
        return Vec::new();
    };
    let frame = node_path(w.id, &[]);
    let name = if root.name.is_empty() {
        w.title.clone()
    } else {
        root.name.clone()
    };
    let mut out = vec![
        events::state(frame.clone(), "active", active),
        Signal::new(
            frame,
            events::WINDOW,
            if active { "Activate" } else { "Deactivate" },
        )
        .any(Value::Str(name)),
    ];
    if active && let Some(chain) = events::focused(root) {
        out.extend(events::focus(node_path(w.id, &chain)));
    }
    out
}

/// The window closed: its frame leaves the app's root.
pub(crate) fn closed(id: WindowId) {
    let signal = {
        let mut b = lock();
        let Some(i) = b.windows.iter().position(|w| w.id == id.0) else {
            return;
        };
        let index = b.windows[..i].iter().filter(|w| w.root.is_some()).count();
        let had_tree = b.windows.remove(i).root.is_some();
        had_tree.then(|| {
            events::children_changed(ROOT_PATH.to_string(), "remove", index, node_path(id.0, &[]))
        })
    };
    send(signal.as_slice());
}

/// The events a screen reader's actions left for window `id`.
fn take_actions(id: u64) -> Vec<Event> {
    let mut b = lock();
    if b.actions.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    b.actions.retain_mut(|(w, events)| {
        if *w == id {
            out.append(events);
            false
        } else {
            true
        }
    });
    out
}

/// Wrap the event loop's handler: a window's activation reaches the bridge,
/// and on a wake each window takes the actions a screen reader asked of it.
pub(crate) fn wrap<H>(mut inner: H) -> impl FnMut(Event, &dyn Window) -> ControlFlow
where
    H: FnMut(Event, &dyn Window) -> ControlFlow,
{
    move |event, window| {
        match event {
            Event::FocusChanged(focused) if enabled() => set_active(window.id(), focused),
            Event::Wake => {
                for e in take_actions(window.id().0) {
                    debug(format_args!("action for {:#x}: {e:?}", window.id().0));
                    if inner(e, window) == ControlFlow::Exit {
                        return ControlFlow::Exit;
                    }
                }
            }
            _ => {}
        }
        inner(event, window)
    }
}

// ---- the bus -----------------------------------------------------------------

/// Start the bus thread, once.
fn start() {
    static START: Once = Once::new();
    START.call_once(|| {
        let spawned = std::thread::Builder::new()
            .name("stipple-atspi".into())
            .spawn(|| {
                if let Err(e) = run() {
                    debug(format_args!("bridge stopped: {e}"));
                }
                STOPPED.store(true, Ordering::Relaxed);
                *WRITER.lock().unwrap_or_else(|e| e.into_inner()) = None;
                let mut b = lock();
                b.bus.clear();
                b.windows.clear();
                b.actions.clear();
            });
        if let Err(e) = spawned {
            debug(format_args!("no bridge thread: {e}"));
        }
    });
}

/// The accessibility bus's address.
fn bus_address() -> Result<String, String> {
    if let Ok(addr) = std::env::var("AT_SPI_BUS_ADDRESS")
        && !addr.is_empty()
    {
        return Ok(addr);
    }
    let session = match std::env::var("DBUS_SESSION_BUS_ADDRESS") {
        Ok(a) if !a.is_empty() => a,
        _ => {
            // The systemd user bus, where the variable isn't set.
            let dir = std::env::var("XDG_RUNTIME_DIR").map_err(|_| "no session bus")?;
            let path = format!("{dir}/bus");
            if !std::path::Path::new(&path).exists() {
                return Err("no session bus".into());
            }
            format!("unix:path={path}")
        }
    };
    let mut bus = DBus::connect_address(&session)?;
    bus.set_read_timeout(Some(std::time::Duration::from_secs(10)));
    bus.a11y_bus_address()
}

/// Connect, register with the registry, then answer calls until the
/// connection drops.
fn run() -> Result<(), String> {
    let addr = bus_address()?;
    debug(format_args!("accessibility bus at {addr}"));
    let mut bus = DBus::connect_address(&addr)?;
    let name = bus.unique_name().to_string();
    let stream = bus.clone_stream().map_err(|e| format!("socket: {e}"))?;
    // A stuck bus mustn't stall the UI thread raising an event.
    let _ = stream.set_write_timeout(Some(std::time::Duration::from_millis(500)));
    let mut writer = Writer {
        stream,
        serial: bus.serial() + 1,
    };
    let mut body = Body::new();
    body.obj(&name, ROOT_PATH);
    let embed = writer.call(
        REGISTRY,
        ROOT_PATH,
        "org.a11y.atspi.Socket",
        "Embed",
        "(so)",
        &body.0,
    )?;
    lock().bus = name.clone();
    *WRITER.lock().unwrap_or_else(|e| e.into_inner()) = Some(writer);
    debug(format_args!("connected as {name}; embedding"));
    loop {
        let m = bus.read_message()?;
        match m.mtype {
            1 => {
                let (reply, wake) = {
                    let mut b = lock();
                    let reply = serve::answer(&mut b, &m);
                    (reply, !b.actions.is_empty())
                };
                if let Some(w) = WRITER.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                    w.answer(&m, &reply)?;
                }
                if wake {
                    crate::platform::wake::wake();
                }
            }
            2 if m.reply_serial == embed => {
                let mut args = marshal::Args::new(&m.body);
                if let (Some(bus), Some(path)) = (args.str(), args.str()) {
                    debug(format_args!("embedded; the desktop is {bus} {path}"));
                    lock().desktop = (bus, path);
                }
            }
            3 if m.reply_serial == embed => {
                debug(format_args!("the registry refused Embed"));
            }
            _ => {}
        }
    }
}

/// Send `signals` (once connected).
fn send(signals: &[Signal]) {
    if signals.is_empty() {
        return;
    }
    let bus = lock().bus.clone();
    let mut writer = WRITER.lock().unwrap_or_else(|e| e.into_inner());
    let Some(w) = writer.as_mut() else {
        return;
    };
    for s in signals {
        debug(format_args!(
            "event {}.{}:{} {} {} on {}",
            s.interface, s.member, s.kind, s.detail1, s.detail2, s.path
        ));
        let mut body = Body::new();
        body.str(&s.kind).i32(s.detail1).i32(s.detail2);
        match &s.any {
            Value::Obj(_, path) => body.variant(&Value::Obj(bus.clone(), path.clone())),
            any => body.variant(any),
        };
        body.empty_dict();
        if w.signal(&s.path, s.interface, s.member, "siiva{sv}", &body.0)
            .is_err()
        {
            return;
        }
    }
}

/// A method call's answer.
#[derive(Debug, PartialEq)]
pub(super) enum Reply {
    Return(&'static str, Vec<u8>),
    Error(&'static str),
}

/// The sending half of the connection.
struct Writer {
    stream: UnixStream,
    serial: u32,
}

impl Writer {
    fn next(&mut self) -> u32 {
        self.serial = self.serial.wrapping_add(1).max(1);
        self.serial
    }

    fn write(&mut self, mtype: u8, fields: &[u8], body: &[u8]) -> Result<u32, String> {
        let serial = self.next();
        let msg = crate::platform::a11y::build_message(mtype, serial, fields, body);
        self.stream
            .write_all(&msg)
            .map_err(|e| format!("write: {e}"))?;
        Ok(serial)
    }

    fn call(
        &mut self,
        dest: &str,
        path: &str,
        iface: &str,
        member: &str,
        sig: &str,
        body: &[u8],
    ) -> Result<u32, String> {
        let mut f = Vec::new();
        put_field(&mut f, 1, b'o', path);
        put_field(&mut f, 6, b's', dest);
        put_field(&mut f, 2, b's', iface);
        put_field(&mut f, 3, b's', member);
        put_field_sig(&mut f, 8, sig);
        self.write(1, &f, body)
    }

    fn signal(
        &mut self,
        path: &str,
        iface: &str,
        member: &str,
        sig: &str,
        body: &[u8],
    ) -> Result<u32, String> {
        let mut f = Vec::new();
        put_field(&mut f, 1, b'o', path);
        put_field(&mut f, 2, b's', iface);
        put_field(&mut f, 3, b's', member);
        put_field_sig(&mut f, 8, sig);
        self.write(4, &f, body)
    }

    fn answer(
        &mut self,
        req: &crate::platform::a11y::Message,
        reply: &Reply,
    ) -> Result<(), String> {
        let mut f = Vec::new();
        put_field_u32(&mut f, 5, req.serial);
        if !req.sender.is_empty() {
            put_field(&mut f, 6, b's', &req.sender);
        }
        match reply {
            Reply::Return(sig, body) => {
                if !sig.is_empty() {
                    put_field_sig(&mut f, 8, sig);
                }
                self.write(2, &f, body)?;
            }
            Reply::Error(name) => {
                put_field(&mut f, 4, b's', name);
                self.write(3, &f, &[])?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
