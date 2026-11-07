//! Hand-written **D-Bus** client and the start of an **AT-SPI** accessibility
//! bridge — no `zbus`/`dbus`/`libdbus`, just a `UnixStream` and the D-Bus wire
//! protocol, the same "talk to the OS directly" approach as the X11 and Wayland
//! backends. Linux-only.
//!
//! AT-SPI (the Linux accessibility framework) is layered on D-Bus: an app
//! connects to a bus, claims a name, and exports a tree of objects implementing
//! the `org.a11y.atspi.*` interfaces that screen readers walk. This module
//! connects to the session bus, runs the SASL `EXTERNAL` handshake, calls
//! `org.freedesktop.DBus.Hello` for our unique connection name, and then
//! [`serves the whole accessibility tree`](DBus::serve_atspi_tree) over
//! `org.a11y.atspi.Accessible` — every node a navigable D-Bus object, the
//! Linux counterpart of the macOS `NSAccessibility` and Windows UIA bridges.

#![allow(unsafe_code)]

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

unsafe extern "C" {
    fn geteuid() -> u32;
}

/// A minimal D-Bus connection: an authenticated stream plus a method-call serial
/// counter. Built by [`DBus::connect_session`].
#[derive(Debug)]
pub struct DBus {
    stream: UnixStream,
    serial: u32,
    /// The unique name the bus assigned us (e.g. `":1.42"`).
    unique_name: String,
}

impl DBus {
    /// Connect to the session bus (`$DBUS_SESSION_BUS_ADDRESS`), authenticate
    /// with SASL `EXTERNAL`, and call `Hello` to obtain our unique name.
    pub fn connect_session() -> Result<DBus, String> {
        let addr = std::env::var("DBUS_SESSION_BUS_ADDRESS")
            .map_err(|_| "DBUS_SESSION_BUS_ADDRESS not set".to_string())?;
        let path =
            parse_unix_path(&addr).ok_or_else(|| format!("unsupported bus address: {addr}"))?;
        let stream = UnixStream::connect(&path).map_err(|e| format!("connect {path}: {e}"))?;
        Self::handshake(stream)
    }

    /// Connect to an explicit `unix:path=…` / `unix:abstract=…` address.
    pub fn connect_address(addr: &str) -> Result<DBus, String> {
        let path =
            parse_unix_path(addr).ok_or_else(|| format!("unsupported bus address: {addr}"))?;
        let stream = UnixStream::connect(&path).map_err(|e| format!("connect {path}: {e}"))?;
        Self::handshake(stream)
    }

    fn handshake(mut stream: UnixStream) -> Result<DBus, String> {
        // SASL EXTERNAL: the leading NUL byte, then authenticate as our uid
        // (sent as the hex of its ASCII-decimal form, per the D-Bus spec).
        let uid = unsafe { geteuid() };
        let uid_hex: String = uid
            .to_string()
            .bytes()
            .map(|b| format!("{b:02x}"))
            .collect();
        stream.write_all(&[0]).map_err(io)?;
        stream
            .write_all(format!("AUTH EXTERNAL {uid_hex}\r\n").as_bytes())
            .map_err(io)?;
        let line = read_line(&mut stream)?;
        if !line.starts_with("OK ") {
            return Err(format!("SASL auth failed: {line:?}"));
        }
        stream.write_all(b"BEGIN\r\n").map_err(io)?;

        let mut bus = DBus {
            stream,
            serial: 0,
            unique_name: String::new(),
        };
        // org.freedesktop.DBus.Hello → our unique name (a STRING body).
        let reply = bus.call(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "Hello",
        )?;
        bus.unique_name = read_string(&reply, &mut 0)?;
        Ok(bus)
    }

    /// Our unique bus name (e.g. `":1.42"`).
    pub fn unique_name(&self) -> &str {
        &self.unique_name
    }

    /// Ask the session bus's accessibility broker (`org.a11y.Bus`) for the
    /// address of the separate **AT-SPI** bus that screen readers listen on.
    /// Activates `at-spi-bus-launcher` on demand.
    pub fn a11y_bus_address(&mut self) -> Result<String, String> {
        let reply = self.call(
            "org.a11y.Bus",
            "/org/a11y/bus",
            "org.a11y.Bus",
            "GetAddress",
        )?;
        read_string(&reply, &mut 0)
    }

    /// Connect to the **AT-SPI** accessibility bus: a second D-Bus connection to
    /// the address the session bus's `org.a11y.Bus` broker hands out. This is the
    /// bus an app exports its accessibility tree on.
    pub fn connect_a11y() -> Result<DBus, String> {
        let mut session = Self::connect_session()?;
        let addr = session.a11y_bus_address()?;
        Self::connect_address(&addr)
    }

    /// Claim a well-known bus `name` via `org.freedesktop.DBus.RequestName`
    /// (flags 0). Returns the reply code (1 = primary owner). Apps that export an
    /// accessibility tree own a name so clients can address them.
    pub fn request_name(&mut self, name: &str) -> Result<u32, String> {
        self.serial += 1;
        let serial = self.serial;
        let mut fields = Vec::new();
        put_field(&mut fields, 1, b'o', "/org/freedesktop/DBus");
        put_field(&mut fields, 6, b's', "org.freedesktop.DBus");
        put_field(&mut fields, 2, b's', "org.freedesktop.DBus");
        put_field(&mut fields, 3, b's', "RequestName");
        put_field_sig(&mut fields, 8, "su");
        let mut body = Vec::new();
        put_string(&mut body, name);
        align(&mut body, 4);
        body.extend_from_slice(&0u32.to_le_bytes()); // flags
        let msg = build_message(1, serial, &fields, &body);
        self.stream.write_all(&msg).map_err(io)?;
        loop {
            let m = self.read_message()?;
            match m.mtype {
                2 if m.reply_serial == serial => return read_u32(&m.body, &mut 0),
                3 if m.reply_serial == serial => return Err("RequestName error".into()),
                _ => continue,
            }
        }
    }

    /// Send a method call carrying a typed `body` (marshalled by the caller, its
    /// D-Bus `signature` declared in the header) and return the reply body.
    fn call_with_body(
        &mut self,
        destination: &str,
        path: &str,
        interface: &str,
        member: &str,
        signature: &str,
        body: &[u8],
    ) -> Result<Vec<u8>, String> {
        self.serial += 1;
        let serial = self.serial;
        let mut fields = Vec::new();
        put_field(&mut fields, 1, b'o', path);
        put_field(&mut fields, 6, b's', destination);
        put_field(&mut fields, 2, b's', interface);
        put_field(&mut fields, 3, b's', member);
        if !signature.is_empty() {
            put_field_sig(&mut fields, 8, signature);
        }
        let msg = build_message(1, serial, &fields, body);
        self.stream.write_all(&msg).map_err(io)?;
        loop {
            let m = self.read_message()?;
            match m.mtype {
                2 if m.reply_serial == serial => return Ok(m.body),
                3 if m.reply_serial == serial => return Err("D-Bus error reply".into()),
                _ => continue,
            }
        }
    }

    /// Install a match rule (`org.freedesktop.DBus.AddMatch`) so the bus routes
    /// matching broadcast signals to us.
    fn add_match(&mut self, rule: &str) -> Result<(), String> {
        let mut body = Vec::new();
        put_string(&mut body, rule);
        self.call_with_body(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "AddMatch",
            "s",
            &body,
        )?;
        Ok(())
    }

    /// Emit a D-Bus signal (message type 4) with a marshalled `body`.
    fn emit_signal(
        &mut self,
        path: &str,
        interface: &str,
        member: &str,
        signature: &str,
        body: &[u8],
    ) -> Result<(), String> {
        self.serial += 1;
        let mut fields = Vec::new();
        put_field(&mut fields, 1, b'o', path);
        put_field(&mut fields, 2, b's', interface);
        put_field(&mut fields, 3, b's', member);
        if !signature.is_empty() {
            put_field_sig(&mut fields, 8, signature);
        }
        let msg = build_message(4, self.serial, &fields, body);
        self.stream.write_all(&msg).map_err(io)
    }

    /// Invoke `org.freedesktop.portal.FileChooser.{member}` (`OpenFile` or
    /// `SaveFile`) on the desktop portal and block for the `Response` signal,
    /// returning the first chosen path (`None` if the user cancelled).
    ///
    /// `directory` selects a folder rather than a file (portal `directory`
    /// option). The portal returns a `Request` handle object path; the chosen
    /// URIs arrive later as an `org.freedesktop.portal.Request.Response` signal
    /// on that handle.
    pub fn portal_file_chooser(
        &mut self,
        member: &str,
        title: &str,
        directory: bool,
    ) -> Result<Option<PathBuf>, String> {
        // Subscribe before calling so we can't miss the Response signal.
        self.add_match(
            "type='signal',interface='org.freedesktop.portal.Request',member='Response'",
        )?;

        // Body: parent_window (s) "", title (s), options (a{sv}).
        let mut body = Vec::new();
        put_string(&mut body, ""); // no parent window
        put_string(&mut body, title);
        let mut opts: Vec<(&str, Dv)> = vec![("handle_token", Dv::Str("forma1"))];
        if directory {
            opts.push(("directory", Dv::Bool(true)));
        }
        put_dict_sv(&mut body, &opts);

        let reply = self.call_with_body(
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.FileChooser",
            member,
            "ssa{sv}",
            &body,
        )?;
        let handle = read_string(&reply, &mut 0)?;

        // Wait for the Response signal addressed to that request handle.
        loop {
            let m = self.read_message()?;
            if m.mtype == 4 && m.member == "Response" && m.path == handle {
                return parse_portal_response(&m.body);
            }
        }
    }

    /// Serve incoming method calls forever (until the connection drops),
    /// answering the standard `org.freedesktop.DBus.Peer` and
    /// `Introspectable.Introspect` interfaces — the minimal surface a D-Bus
    /// object must expose. `introspect_xml` is returned for `Introspect`. This is
    /// the bidirectional half the AT-SPI tree export builds on (the registry
    /// calls back into us). Unknown methods get an `UnknownMethod` error.
    pub fn serve(&mut self, introspect_xml: &str) -> Result<(), String> {
        loop {
            let m = self.read_message()?;
            if m.mtype != 1 {
                continue; // only method calls
            }
            match (m.interface.as_str(), m.member.as_str()) {
                ("org.freedesktop.DBus.Peer", "Ping") => self.send_return(&m, "", &[])?,
                ("org.freedesktop.DBus.Peer", "GetMachineId") => {
                    let mut body = Vec::new();
                    put_string(&mut body, "00000000000000000000000000000000");
                    self.send_return(&m, "s", &body)?;
                }
                ("org.freedesktop.DBus.Introspectable", "Introspect") => {
                    let mut body = Vec::new();
                    put_string(&mut body, introspect_xml);
                    self.send_return(&m, "s", &body)?;
                }
                _ => self.send_error(&m, "org.freedesktop.DBus.Error.UnknownMethod")?,
            }
        }
    }

    /// Serve a whole [`AtspiTree`] over `org.a11y.atspi.Accessible`: every node is
    /// a D-Bus object (the root at [`ATSPI_ROOT_PATH`], the rest at
    /// `{root}/{index}`), and the tree is walkable exactly as a screen reader
    /// walks it — `GetChildAtIndex` / `GetChildren` return `(so)` object
    /// references, `GetRole` / `GetIndexInParent` the node's place, and the
    /// `Name` / `Description` / `ChildCount` / `Parent` properties its data.
    /// Loops until the connection drops. Unknown methods get an `UnknownMethod`
    /// error; calls to an unknown object path get `UnknownObject`.
    pub fn serve_atspi_tree(
        &mut self,
        tree: &AtspiTree,
        introspect_xml: &str,
    ) -> Result<(), String> {
        // Object references name us by our unique bus connection (AT-SPI
        // convention); a client follows the path, so the bus field is advisory.
        let bus = self.unique_name.clone();
        loop {
            let m = self.read_message()?;
            if m.mtype != 1 {
                continue;
            }
            let idx = node_index_for_path(&m.path);
            match (m.interface.as_str(), m.member.as_str()) {
                ("org.freedesktop.DBus.Peer", "Ping") => self.send_return(&m, "", &[])?,
                ("org.freedesktop.DBus.Introspectable", "Introspect") => {
                    let mut body = Vec::new();
                    put_string(&mut body, introspect_xml);
                    self.send_return(&m, "s", &body)?;
                }
                ("org.a11y.atspi.Accessible", member) => {
                    let Some(i) = idx.filter(|&i| i < tree.nodes.len()) else {
                        self.send_error(&m, "org.freedesktop.DBus.Error.UnknownObject")?;
                        continue;
                    };
                    let node = &tree.nodes[i];
                    match member {
                        "GetRole" => self.send_return(&m, "u", &node.role.to_le_bytes())?,
                        "GetRoleName" | "GetLocalizedRoleName" => {
                            let mut body = Vec::new();
                            put_string(&mut body, role_name(node.role));
                            self.send_return(&m, "s", &body)?;
                        }
                        "GetIndexInParent" => {
                            self.send_return(&m, "i", &node.index_in_parent.to_le_bytes())?
                        }
                        "GetChildAtIndex" => {
                            // Arg: one INT32 child index.
                            let mut off = 0;
                            let ci = read_u32(&m.body, &mut off).unwrap_or(0) as i32;
                            let mut body = Vec::new();
                            match usize::try_from(ci).ok().and_then(|c| node.children.get(c)) {
                                Some(&c) => put_object_ref(&mut body, &bus, &path_for_index(c)),
                                None => put_object_ref(&mut body, "", ATSPI_NULL_PATH),
                            }
                            self.send_return(&m, "(so)", &body)?;
                        }
                        "GetChildren" => {
                            let refs: Vec<(String, String)> = node
                                .children
                                .iter()
                                .map(|&c| (bus.clone(), path_for_index(c)))
                                .collect();
                            let mut body = Vec::new();
                            put_object_ref_array(&mut body, &refs);
                            self.send_return(&m, "a(so)", &body)?;
                        }
                        _ => self.send_error(&m, "org.freedesktop.DBus.Error.UnknownMethod")?,
                    }
                }
                ("org.freedesktop.DBus.Properties", "Get") => {
                    // Args: (ss) = interface name, property name.
                    let mut off = 0;
                    let _iface = read_string(&m.body, &mut off).unwrap_or_default();
                    let prop = read_string(&m.body, &mut off).unwrap_or_default();
                    let Some(i) = idx.filter(|&i| i < tree.nodes.len()) else {
                        self.send_error(&m, "org.freedesktop.DBus.Error.UnknownObject")?;
                        continue;
                    };
                    let node = &tree.nodes[i];
                    let mut body = Vec::new();
                    match prop.as_str() {
                        "Name" => put_variant_string(&mut body, &node.name),
                        "Description" => put_variant_string(&mut body, ""),
                        "ChildCount" => put_variant_i32(&mut body, node.children.len() as i32),
                        "Parent" => match node.parent {
                            Some(p) => put_variant_object_ref(&mut body, &bus, &path_for_index(p)),
                            None => put_variant_object_ref(&mut body, "", ATSPI_NULL_PATH),
                        },
                        _ => {
                            self.send_error(&m, "org.freedesktop.DBus.Error.InvalidArgs")?;
                            continue;
                        }
                    }
                    self.send_return(&m, "v", &body)?;
                }
                _ => self.send_error(&m, "org.freedesktop.DBus.Error.UnknownMethod")?,
            }
        }
    }

    /// Send a `METHOD_RETURN` for `req` with optional `signature`/`body`.
    fn send_return(&mut self, req: &Message, signature: &str, body: &[u8]) -> Result<(), String> {
        self.serial += 1;
        let mut fields = Vec::new();
        put_field_u32(&mut fields, 5, req.serial); // REPLY_SERIAL
        if !req.sender.is_empty() {
            put_field(&mut fields, 6, b's', &req.sender); // DESTINATION
        }
        if !signature.is_empty() {
            put_field_sig(&mut fields, 8, signature);
        }
        let msg = build_message(2, self.serial, &fields, body);
        self.stream.write_all(&msg).map_err(io)
    }

    /// Send an `ERROR` reply for `req` naming `error_name`.
    fn send_error(&mut self, req: &Message, error_name: &str) -> Result<(), String> {
        self.serial += 1;
        let mut fields = Vec::new();
        put_field_u32(&mut fields, 5, req.serial); // REPLY_SERIAL
        put_field(&mut fields, 4, b's', error_name); // ERROR_NAME
        if !req.sender.is_empty() {
            put_field(&mut fields, 6, b's', &req.sender); // DESTINATION
        }
        let msg = build_message(3, self.serial, &fields, &[]);
        self.stream.write_all(&msg).map_err(io)
    }

    /// Send a no-argument method call and return the reply's body bytes. Errors
    /// on a D-Bus `ERROR` reply or I/O failure.
    fn call(
        &mut self,
        destination: &str,
        path: &str,
        interface: &str,
        member: &str,
    ) -> Result<Vec<u8>, String> {
        self.serial += 1;
        let serial = self.serial;
        let msg = marshal_method_call(serial, destination, path, interface, member);
        self.stream.write_all(&msg).map_err(io)?;

        // Read replies until we see the method_return / error for our serial
        // (the bus may interleave signals).
        loop {
            let msg = self.read_message()?;
            match msg.mtype {
                2 if msg.reply_serial == serial => return Ok(msg.body), // METHOD_RETURN
                3 if msg.reply_serial == serial => return Err("D-Bus error reply".into()), // ERROR
                _ => continue,
            }
        }
    }

    /// Read one full D-Bus message (header fields + body). Little-endian only.
    fn read_message(&mut self) -> Result<Message, String> {
        // Fixed part of the header is 12 bytes, then the header-fields array
        // length (another u32).
        let mut fixed = [0u8; 16];
        self.stream.read_exact(&mut fixed).map_err(io)?;
        if fixed[0] != b'l' {
            return Err("only little-endian D-Bus is supported".into());
        }
        let mtype = fixed[1];
        let body_len = u32::from_le_bytes([fixed[4], fixed[5], fixed[6], fixed[7]]) as usize;
        let serial = u32::from_le_bytes([fixed[8], fixed[9], fixed[10], fixed[11]]);
        let fields_len = u32::from_le_bytes([fixed[12], fixed[13], fixed[14], fixed[15]]) as usize;

        // The header (fixed 12 + array-len 4 + fields) is padded to 8 bytes
        // before the body.
        let mut fields = vec![0u8; fields_len];
        self.stream.read_exact(&mut fields).map_err(io)?;
        let header_len = 16 + fields_len;
        let pad = (8 - (header_len % 8)) % 8;
        if pad > 0 {
            let mut p = vec![0u8; pad];
            self.stream.read_exact(&mut p).map_err(io)?;
        }
        let mut body = vec![0u8; body_len];
        self.stream.read_exact(&mut body).map_err(io)?;

        let mut msg = parse_fields(&fields);
        msg.mtype = mtype;
        msg.serial = serial;
        msg.body = body;
        Ok(msg)
    }
}

/// A minimal mock of `org.freedesktop.portal.Desktop` for tests: it owns the
/// portal name, answers one `FileChooser.OpenFile`/`SaveFile` call with a
/// request-handle object path, then emits the `Request.Response` signal carrying
/// `canned_uri`. Returns after serving a single request. Used by the file-dialog
/// CI job under `dbus-run-session`, where no real portal backend exists.
pub fn run_mock_file_portal(canned_uri: &str) -> Result<(), String> {
    let mut bus = DBus::connect_session()?;
    bus.request_name("org.freedesktop.portal.Desktop")?;
    loop {
        let m = bus.read_message()?;
        if m.mtype != 1 {
            continue; // method calls only
        }
        match (m.interface.as_str(), m.member.as_str()) {
            ("org.freedesktop.portal.FileChooser", "OpenFile" | "SaveFile") => {
                let handle = "/org/freedesktop/portal/desktop/request/stipple/1";
                let mut rbody = Vec::new();
                put_string(&mut rbody, handle); // OUT o handle
                bus.send_return(&m, "o", &rbody)?;
                let mut sbody = Vec::new();
                put_response_body(&mut sbody, 0, canned_uri);
                bus.emit_signal(
                    handle,
                    "org.freedesktop.portal.Request",
                    "Response",
                    "ua{sv}",
                    &sbody,
                )?;
                return Ok(());
            }
            ("org.freedesktop.DBus.Peer", "Ping") => bus.send_return(&m, "", &[])?,
            ("org.freedesktop.DBus.Introspectable", "Introspect") => {
                let mut body = Vec::new();
                put_string(&mut body, "<node/>");
                bus.send_return(&m, "s", &body)?;
            }
            _ => bus.send_error(&m, "org.freedesktop.DBus.Error.UnknownMethod")?,
        }
    }
}

/// The object path of the accessibility tree root we serve.
pub const ATSPI_ROOT_PATH: &str = "/org/stippleui/a11y";
/// The AT-SPI "null" object reference path (returned where there is no parent /
/// child).
const ATSPI_NULL_PATH: &str = "/org/a11y/atspi/null";

/// One node of an [`AtspiTree`]: an `org.a11y.atspi` role number, an accessible
/// name, and indices linking it into the tree (so this crate needs no dependency
/// on the widget tree — higher layers flatten their `AccessNode` into this).
#[derive(Debug, Clone)]
pub struct AtspiTreeNode {
    pub role: u32,
    pub name: String,
    /// Index of the parent node, or `None` for the root.
    pub parent: Option<usize>,
    /// Indices of the child nodes, in order.
    pub children: Vec<usize>,
    /// This node's position among its parent's children (`-1` for the root).
    pub index_in_parent: i32,
}

/// A flattened accessibility tree to expose over AT-SPI. Node `0` is the root
/// (served at [`ATSPI_ROOT_PATH`]); node `i>0` is served at `{root}/{i}`.
/// Build it with [`AtspiTree::push`], parents before children.
#[derive(Debug, Clone, Default)]
pub struct AtspiTree {
    pub nodes: Vec<AtspiTreeNode>,
}

impl AtspiTree {
    pub fn new() -> AtspiTree {
        AtspiTree::default()
    }

    /// Add a node with `role` and `name` under `parent` (`None` for the root),
    /// returning its index. The root must be pushed first.
    pub fn push(&mut self, role: u32, name: &str, parent: Option<usize>) -> usize {
        let idx = self.nodes.len();
        let index_in_parent = match parent {
            Some(p) => {
                let n = self.nodes[p].children.len() as i32;
                self.nodes[p].children.push(idx);
                n
            }
            None => -1,
        };
        self.nodes.push(AtspiTreeNode {
            role,
            name: name.to_string(),
            parent,
            children: Vec::new(),
            index_in_parent,
        });
        idx
    }
}

/// The D-Bus object path for tree node `i` (the root has no index suffix).
fn path_for_index(i: usize) -> String {
    if i == 0 {
        ATSPI_ROOT_PATH.to_string()
    } else {
        format!("{ATSPI_ROOT_PATH}/{i}")
    }
}

/// Map a served object path back to its tree-node index (inverse of
/// [`path_for_index`]).
fn node_index_for_path(path: &str) -> Option<usize> {
    if path == ATSPI_ROOT_PATH {
        return Some(0);
    }
    path.strip_prefix(ATSPI_ROOT_PATH)?
        .strip_prefix('/')?
        .parse()
        .ok()
}

/// An `org.a11y.atspi` role number → its conventional role name.
fn role_name(role: u32) -> &'static str {
    match role {
        27 => "frame",
        54 => "panel",
        44 => "push button",
        80 => "entry",
        29 => "label",
        _ => "unknown",
    }
}

/// A received D-Bus message: header metadata plus the raw body bytes.
#[derive(Debug, Default)]
struct Message {
    mtype: u8,
    serial: u32,
    reply_serial: u32,
    path: String,
    interface: String,
    member: String,
    sender: String,
    body: Vec<u8>,
}

/// Map `unix:path=/x` or `unix:abstract=/x` (with optional trailing `,guid=…`)
/// to a connectable path. Abstract sockets are returned with a leading NUL.
fn parse_unix_path(addr: &str) -> Option<String> {
    let addr = addr.split(';').next().unwrap_or(addr);
    for kv in addr.trim_start_matches("unix:").split(',') {
        if let Some(p) = kv.strip_prefix("path=") {
            return Some(p.to_string());
        }
        if let Some(p) = kv.strip_prefix("abstract=") {
            return Some(format!("\0{p}"));
        }
    }
    None
}

fn io(e: std::io::Error) -> String {
    format!("dbus i/o: {e}")
}

/// Read a CRLF-terminated SASL line (without the CRLF).
fn read_line(stream: &mut UnixStream) -> Result<String, String> {
    let mut out = Vec::new();
    let mut b = [0u8; 1];
    loop {
        stream.read_exact(&mut b).map_err(io)?;
        if b[0] == b'\n' {
            if out.last() == Some(&b'\r') {
                out.pop();
            }
            break;
        }
        out.push(b[0]);
        if out.len() > 4096 {
            return Err("SASL line too long".into());
        }
    }
    String::from_utf8(out).map_err(|_| "SASL line not UTF-8".to_string())
}

// ---- marshalling -----------------------------------------------------------

fn align(buf: &mut Vec<u8>, n: usize) {
    while !buf.len().is_multiple_of(n) {
        buf.push(0);
    }
}

/// Append a D-Bus STRING/OBJECT_PATH value: a u32 length, the bytes, and a NUL.
fn put_string(buf: &mut Vec<u8>, s: &str) {
    align(buf, 4);
    buf.extend_from_slice(&(s.len() as u32).to_le_bytes());
    buf.extend_from_slice(s.as_bytes());
    buf.push(0);
}

/// Append a header field carrying a STRING/OBJECT_PATH value (sig `s` or `o`).
fn put_field(buf: &mut Vec<u8>, code: u8, sig: u8, value: &str) {
    align(buf, 8);
    buf.push(code);
    // VARIANT: a one-byte signature length, the signature, a NUL, then value.
    buf.push(1);
    buf.push(sig);
    buf.push(0);
    put_string(buf, value);
}

/// Append a header field carrying a UINT32 value (e.g. REPLY_SERIAL=5).
fn put_field_u32(buf: &mut Vec<u8>, code: u8, value: u32) {
    align(buf, 8);
    buf.push(code);
    buf.push(1);
    buf.push(b'u');
    buf.push(0);
    align(buf, 4);
    buf.extend_from_slice(&value.to_le_bytes());
}

/// Append a header field carrying a SIGNATURE value (e.g. SIGNATURE=8).
fn put_field_sig(buf: &mut Vec<u8>, code: u8, sig: &str) {
    align(buf, 8);
    buf.push(code);
    buf.push(1);
    buf.push(b'g');
    buf.push(0);
    buf.push(sig.len() as u8);
    buf.extend_from_slice(sig.as_bytes());
    buf.push(0);
}

/// Assemble a full message from a type, serial, marshalled header `fields`, and
/// `body` (the header is padded to 8 bytes before the body).
fn build_message(mtype: u8, serial: u32, fields: &[u8], body: &[u8]) -> Vec<u8> {
    // Fixed header: little-endian, type, no flags, protocol version 1.
    let mut msg = vec![b'l', mtype, 0, 1];
    msg.extend_from_slice(&(body.len() as u32).to_le_bytes());
    msg.extend_from_slice(&serial.to_le_bytes());
    msg.extend_from_slice(&(fields.len() as u32).to_le_bytes());
    msg.extend_from_slice(fields);
    align(&mut msg, 8);
    msg.extend_from_slice(body);
    msg
}

/// Marshal a no-argument `METHOD_CALL` (little-endian).
fn marshal_method_call(
    serial: u32,
    destination: &str,
    path: &str,
    interface: &str,
    member: &str,
) -> Vec<u8> {
    // Header fields (PATH=1 'o', INTERFACE=2 's', MEMBER=3 's', DESTINATION=6 's').
    let mut fields = Vec::new();
    put_field(&mut fields, 1, b'o', path);
    put_field(&mut fields, 6, b's', destination);
    put_field(&mut fields, 2, b's', interface);
    put_field(&mut fields, 3, b's', member);
    build_message(1, serial, &fields, &[])
}

/// Append a VARIANT holding a string: signature `s`, then the string value.
fn put_variant_string(buf: &mut Vec<u8>, s: &str) {
    buf.push(1);
    buf.push(b's');
    buf.push(0);
    put_string(buf, s);
}

/// Append an AT-SPI object reference `(so)`: a bus name + an object path (both
/// marshalled as strings; a STRUCT aligns to 8).
fn put_object_ref(buf: &mut Vec<u8>, bus: &str, path: &str) {
    align(buf, 8);
    put_string(buf, bus);
    put_string(buf, path);
}

/// Append an array of object references `a(so)`.
fn put_object_ref_array(buf: &mut Vec<u8>, refs: &[(String, String)]) {
    align(buf, 4);
    let len_pos = buf.len();
    buf.extend_from_slice(&0u32.to_le_bytes()); // array byte-length placeholder
    align(buf, 8); // STRUCT element alignment
    let start = buf.len();
    for (bus, path) in refs {
        put_object_ref(buf, bus, path);
    }
    let len = (buf.len() - start) as u32;
    buf[len_pos..len_pos + 4].copy_from_slice(&len.to_le_bytes());
}

/// Append a VARIANT holding an object reference: signature `(so)`, then the
/// struct.
fn put_variant_object_ref(buf: &mut Vec<u8>, bus: &str, path: &str) {
    buf.push(4);
    buf.extend_from_slice(b"(so)");
    buf.push(0);
    put_object_ref(buf, bus, path);
}

// ---- portal FileChooser marshalling ----------------------------------------

/// A D-Bus variant value we know how to marshal into an `a{sv}` options dict.
enum Dv<'a> {
    Str(&'a str),
    Bool(bool),
}

/// Append an `a{sv}` dictionary of `(key, variant)` entries.
fn put_dict_sv(buf: &mut Vec<u8>, entries: &[(&str, Dv)]) {
    align(buf, 4);
    let len_pos = buf.len();
    buf.extend_from_slice(&0u32.to_le_bytes()); // array byte-length placeholder
    align(buf, 8); // DICT_ENTRY alignment
    let start = buf.len();
    for (key, val) in entries {
        align(buf, 8);
        put_string(buf, key);
        match val {
            Dv::Str(s) => put_variant_string(buf, s),
            Dv::Bool(b) => {
                buf.push(1);
                buf.push(b'b');
                buf.push(0);
                align(buf, 4);
                buf.extend_from_slice(&(*b as u32).to_le_bytes());
            }
        }
    }
    let len = (buf.len() - start) as u32;
    buf[len_pos..len_pos + 4].copy_from_slice(&len.to_le_bytes());
}

/// Append a portal `Response` body (`ua{sv}`): a response code and a results
/// dict whose single `uris` key is an array of one string.
fn put_response_body(buf: &mut Vec<u8>, response: u32, uri: &str) {
    buf.extend_from_slice(&response.to_le_bytes()); // u response (offset 0)
    align(buf, 4);
    let len_pos = buf.len();
    buf.extend_from_slice(&0u32.to_le_bytes()); // a{sv} byte-length placeholder
    align(buf, 8);
    let start = buf.len();
    align(buf, 8);
    put_string(buf, "uris");
    // VARIANT holding an array of strings: signature "as".
    buf.push(2);
    buf.push(b'a');
    buf.push(b's');
    buf.push(0);
    align(buf, 4);
    let as_len_pos = buf.len();
    buf.extend_from_slice(&0u32.to_le_bytes()); // array byte-length
    align(buf, 4);
    let as_start = buf.len();
    put_string(buf, uri);
    let as_len = (buf.len() - as_start) as u32;
    buf[as_len_pos..as_len_pos + 4].copy_from_slice(&as_len.to_le_bytes());
    let len = (buf.len() - start) as u32;
    buf[len_pos..len_pos + 4].copy_from_slice(&len.to_le_bytes());
}

/// Parse a portal `Response` body (`ua{sv}`): if the response code is 0
/// (success) and the results carry a non-empty `uris`, return the first as a
/// path. A non-zero code (cancelled / other) maps to `None`.
fn parse_portal_response(body: &[u8]) -> Result<Option<PathBuf>, String> {
    let mut off = 0;
    let response = read_u32(body, &mut off)?;
    if response != 0 {
        return Ok(None);
    }
    Ok(read_results_uris(body, &mut off)?.map(|u| uri_to_path(&u)))
}

/// Walk an `a{sv}` dict at `*off`, returning the first string of the `uris`
/// entry (skipping any other keys by their variant signature).
fn read_results_uris(buf: &[u8], off: &mut usize) -> Result<Option<String>, String> {
    align_to(off, 4);
    let arr_len = read_u32(buf, off)? as usize;
    align_to(off, 8);
    let end = (*off + arr_len).min(buf.len());
    let mut found = None;
    while *off < end {
        align_to(off, 8);
        if *off >= end {
            break;
        }
        let key = read_string(buf, off)?;
        // Each value is a VARIANT: a 1-byte signature length, the signature, NUL.
        let sig_len = *buf.get(*off).ok_or("truncated variant sig")? as usize;
        *off += 1;
        let sig = buf
            .get(*off..*off + sig_len)
            .ok_or("truncated variant sig")?
            .to_vec();
        *off += sig_len + 1; // signature + NUL
        if key == "uris" && sig == b"as" {
            align_to(off, 4);
            let as_len = read_u32(buf, off)? as usize;
            let as_end = (*off + as_len).min(buf.len());
            if found.is_none() && *off < as_end {
                found = Some(read_string(buf, off)?);
            }
            *off = as_end;
        } else {
            let mut si = 0;
            skip_value(buf, off, &sig, &mut si)?;
        }
    }
    Ok(found)
}

/// Advance `*off` to the next multiple of `n`.
fn align_to(off: &mut usize, n: usize) {
    while !off.is_multiple_of(n) {
        *off += 1;
    }
}

/// Natural alignment of a D-Bus type by its signature code.
fn type_align(c: u8) -> usize {
    match c {
        b'n' | b'q' => 2,
        b'b' | b'i' | b'u' | b'h' | b'a' | b's' | b'o' => 4,
        b'x' | b't' | b'd' | b'(' | b'{' => 8,
        _ => 1, // y, g, v
    }
}

/// Skip one complete value of the type at `sig[*si]`, advancing both the
/// signature index `*si` and the data offset `*off`. Handles the full D-Bus
/// type grammar (basics, strings, variants, arrays, structs, dict entries) so
/// unknown result keys can be stepped over.
fn skip_value(buf: &[u8], off: &mut usize, sig: &[u8], si: &mut usize) -> Result<(), String> {
    let t = *sig.get(*si).ok_or("signature underrun")?;
    *si += 1;
    match t {
        b'y' => *off += 1,
        b'n' | b'q' => {
            align_to(off, 2);
            *off += 2;
        }
        b'b' | b'i' | b'u' | b'h' => {
            align_to(off, 4);
            *off += 4;
        }
        b'x' | b't' | b'd' => {
            align_to(off, 8);
            *off += 8;
        }
        b's' | b'o' => {
            read_string(buf, off)?;
        }
        b'g' => {
            let l = *buf.get(*off).ok_or("truncated signature")? as usize;
            *off += 1 + l + 1;
        }
        b'v' => {
            let l = *buf.get(*off).ok_or("truncated variant")? as usize;
            *off += 1;
            let vsig = buf.get(*off..*off + l).ok_or("truncated variant")?.to_vec();
            *off += l + 1;
            let mut vi = 0;
            skip_value(buf, off, &vsig, &mut vi)?;
        }
        b'a' => {
            align_to(off, 4);
            let len = read_u32(buf, off)? as usize;
            let elem = *sig.get(*si).ok_or("array element type")?;
            align_to(off, type_align(elem));
            let target = (*off + len).min(buf.len());
            skip_type_sig(sig, si)?;
            *off = target;
        }
        b'(' => {
            align_to(off, 8);
            while sig.get(*si).copied() != Some(b')') {
                skip_value(buf, off, sig, si)?;
            }
            *si += 1; // consume ')'
        }
        b'{' => {
            align_to(off, 8);
            skip_value(buf, off, sig, si)?; // key
            skip_value(buf, off, sig, si)?; // value
            *si += 1; // consume '}'
        }
        other => return Err(format!("unsupported D-Bus type '{}'", other as char)),
    }
    Ok(())
}

/// Advance `*si` past one complete type in a signature (no data consumed).
fn skip_type_sig(sig: &[u8], si: &mut usize) -> Result<(), String> {
    let t = *sig.get(*si).ok_or("signature underrun")?;
    *si += 1;
    match t {
        b'a' => skip_type_sig(sig, si)?,
        b'(' => {
            while sig.get(*si).copied() != Some(b')') {
                skip_type_sig(sig, si)?;
            }
            *si += 1;
        }
        b'{' => {
            skip_type_sig(sig, si)?;
            skip_type_sig(sig, si)?;
            *si += 1;
        }
        _ => {}
    }
    Ok(())
}

/// Convert a `file://` URI to a filesystem path (percent-decoding the path).
fn uri_to_path(uri: &str) -> PathBuf {
    let rest = uri.strip_prefix("file://").unwrap_or(uri);
    // A local file URI has an empty host, so `rest` begins with the path's '/'.
    PathBuf::from(percent_decode(rest))
}

/// Decode `%XX` escapes in a URI path into raw bytes, then UTF-8.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex_digit(b[i + 1]), hex_digit(b[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Append a VARIANT holding an int32: signature `i`, then the 4-aligned value.
fn put_variant_i32(buf: &mut Vec<u8>, v: i32) {
    buf.push(1);
    buf.push(b'i');
    buf.push(0);
    align(buf, 4);
    buf.extend_from_slice(&v.to_le_bytes());
}

/// Read a u32 at `*off` in `buf` (4-aligned), advancing `*off`.
fn read_u32(buf: &[u8], off: &mut usize) -> Result<u32, String> {
    while !off.is_multiple_of(4) {
        *off += 1;
    }
    if *off + 4 > buf.len() {
        return Err("truncated u32".into());
    }
    let v = u32::from_le_bytes([buf[*off], buf[*off + 1], buf[*off + 2], buf[*off + 3]]);
    *off += 4;
    Ok(v)
}

// ---- demarshalling ---------------------------------------------------------

/// Read a D-Bus STRING at `*off` in `buf` (u32 length + bytes + NUL), advancing.
fn read_string(buf: &[u8], off: &mut usize) -> Result<String, String> {
    while !off.is_multiple_of(4) {
        *off += 1;
    }
    if *off + 4 > buf.len() {
        return Err("truncated string length".into());
    }
    let len = u32::from_le_bytes([buf[*off], buf[*off + 1], buf[*off + 2], buf[*off + 3]]) as usize;
    *off += 4;
    if *off + len > buf.len() {
        return Err("truncated string body".into());
    }
    let s = String::from_utf8_lossy(&buf[*off..*off + len]).into_owned();
    *off += len + 1; // skip the trailing NUL
    Ok(s)
}

/// Parse a header-fields array into the fields we care about (path, interface,
/// member, sender, reply_serial). Walks the `a(yv)` grammar, skipping values by
/// their variant signature; stops on anything it can't decode.
fn parse_fields(fields: &[u8]) -> Message {
    let mut msg = Message::default();
    let mut off = 0usize;
    while off < fields.len() {
        // Each field struct is 8-aligned.
        while !off.is_multiple_of(8) && off < fields.len() {
            off += 1;
        }
        if off + 2 > fields.len() {
            break;
        }
        let code = fields[off];
        off += 1;
        // VARIANT: signature length, signature bytes, NUL.
        let sig_len = fields[off] as usize;
        off += 1;
        let sig = fields.get(off..off + sig_len).unwrap_or(&[]).to_vec();
        off += sig_len + 1; // signature + NUL
        match sig.first().copied() {
            Some(b'u') | Some(b'i') => {
                while !off.is_multiple_of(4) && off < fields.len() {
                    off += 1;
                }
                if off + 4 > fields.len() {
                    break;
                }
                let v = u32::from_le_bytes([
                    fields[off],
                    fields[off + 1],
                    fields[off + 2],
                    fields[off + 3],
                ]);
                off += 4;
                if code == 5 {
                    msg.reply_serial = v;
                }
            }
            Some(b's') | Some(b'o') | Some(b'g') => {
                // 'g' (signature) is length-prefixed by one byte, not a u32.
                let value = if sig.first() == Some(&b'g') {
                    let l = *fields.get(off).unwrap_or(&0) as usize;
                    let v =
                        String::from_utf8_lossy(fields.get(off + 1..off + 1 + l).unwrap_or(&[]))
                            .into_owned();
                    off += 1 + l + 1;
                    v
                } else {
                    match read_string(fields, &mut off) {
                        Ok(v) => v,
                        Err(_) => break,
                    }
                };
                match code {
                    1 => msg.path = value,
                    2 => msg.interface = value,
                    3 => msg.member = value,
                    7 => msg.sender = value,
                    _ => {}
                }
            }
            _ => break, // unknown — stop scanning
        }
    }
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atspi_path_index_round_trip() {
        for i in [0usize, 1, 5, 42] {
            assert_eq!(node_index_for_path(&path_for_index(i)), Some(i));
        }
        assert_eq!(node_index_for_path(ATSPI_ROOT_PATH), Some(0));
        assert_eq!(node_index_for_path("/some/other/path"), None);
    }

    #[test]
    fn atspi_tree_links_parent_and_index() {
        // Window > [Hello(label), Group > OK(label)].
        let mut t = AtspiTree::new();
        let root = t.push(27, "Win", None);
        let a = t.push(29, "Hello", Some(root));
        let g = t.push(54, "", Some(root));
        let b = t.push(29, "OK", Some(g));
        assert_eq!(t.nodes[root].children, vec![a, g]);
        assert_eq!(t.nodes[root].index_in_parent, -1);
        assert_eq!(t.nodes[g].index_in_parent, 1); // 2nd child of root
        assert_eq!(t.nodes[b].parent, Some(g));
        assert_eq!(t.nodes[b].index_in_parent, 0);
    }

    #[test]
    fn object_ref_array_round_trips() {
        // Marshal an a(so) and read it back, exercising the struct/array
        // alignment the AT-SPI GetChildren reply depends on.
        let refs = vec![
            (":1.5".to_string(), "/org/stippleui/a11y/1".to_string()),
            (String::new(), "/org/stippleui/a11y/2".to_string()),
        ];
        let mut buf = Vec::new();
        put_object_ref_array(&mut buf, &refs);
        let mut off = 0;
        let alen = read_u32(&buf, &mut off).unwrap() as usize;
        align_to(&mut off, 8);
        let end = off + alen;
        let mut got = Vec::new();
        while off < end {
            align_to(&mut off, 8);
            let bus = read_string(&buf, &mut off).unwrap();
            let path = read_string(&buf, &mut off).unwrap();
            got.push((bus, path));
        }
        assert_eq!(got, refs);
        assert_eq!(off, end);
    }

    #[test]
    fn portal_response_round_trips_a_uri_to_a_path() {
        // Marshal a success Response (ua{sv} with uris=[file://…]) exactly as the
        // portal would, then parse it back — exercising the dict + array codec
        // and the percent-decoding, without needing a live bus.
        let mut body = Vec::new();
        put_response_body(&mut body, 0, "file:///tmp/stipple%20pick.txt");
        let path = parse_portal_response(&body).unwrap();
        assert_eq!(path, Some(PathBuf::from("/tmp/stipple pick.txt")));
    }

    #[test]
    fn portal_response_cancelled_is_none() {
        // A non-zero response code (1 = user cancelled) yields no path.
        let mut body = Vec::new();
        put_response_body(&mut body, 1, "file:///tmp/x.txt");
        assert_eq!(parse_portal_response(&body).unwrap(), None);
    }

    #[test]
    fn skips_unknown_result_keys_before_uris() {
        // A results dict whose first entry is an unrelated key (boolean
        // "writable") must be stepped over so "uris" is still found.
        let mut body = Vec::new();
        body.extend_from_slice(&0u32.to_le_bytes()); // response = 0
        align(&mut body, 4);
        let len_pos = body.len();
        body.extend_from_slice(&0u32.to_le_bytes());
        align(&mut body, 8);
        let start = body.len();
        // entry 1: "writable" -> variant b true
        align(&mut body, 8);
        put_string(&mut body, "writable");
        body.push(1);
        body.push(b'b');
        body.push(0);
        align(&mut body, 4);
        body.extend_from_slice(&1u32.to_le_bytes());
        // entry 2: "uris" -> variant as ["file:///tmp/y.txt"]
        align(&mut body, 8);
        put_string(&mut body, "uris");
        body.push(2);
        body.push(b'a');
        body.push(b's');
        body.push(0);
        align(&mut body, 4);
        let as_len_pos = body.len();
        body.extend_from_slice(&0u32.to_le_bytes());
        align(&mut body, 4);
        let as_start = body.len();
        put_string(&mut body, "file:///tmp/y.txt");
        let as_len = (body.len() - as_start) as u32;
        body[as_len_pos..as_len_pos + 4].copy_from_slice(&as_len.to_le_bytes());
        let len = (body.len() - start) as u32;
        body[len_pos..len_pos + 4].copy_from_slice(&len.to_le_bytes());

        let path = parse_portal_response(&body).unwrap();
        assert_eq!(path, Some(PathBuf::from("/tmp/y.txt")));
    }
}
