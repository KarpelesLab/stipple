//! Answering a screen reader's calls: `org.a11y.atspi.Accessible`,
//! `Application`, `Component`, `Action`, `Text`, `EditableText` and `Value`
//! on the app's root and every node of every window's tree, plus the
//! standard `Properties`, `Introspectable` and `Peer` interfaces.

use super::marshal::{Args, Body, Value};
use super::text::{self, Which};
use super::{
    Bridge, NULL_PATH, ROOT_PATH, Reply, Win, clickable, editable, focusable, node_path,
    parse_path, role, text_role,
};
use crate::geometry::Point;
use crate::platform::a11y::Message;
use crate::platform::access::{A11yNode, A11yRole};
use crate::platform::{ButtonState, Event, Ime, PointerButton};

const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const APPLICATION: &str = "org.a11y.atspi.Application";
const COMPONENT: &str = "org.a11y.atspi.Component";
const ACTION: &str = "org.a11y.atspi.Action";
const TEXT: &str = "org.a11y.atspi.Text";
const EDITABLE_TEXT: &str = "org.a11y.atspi.EditableText";
const VALUE: &str = "org.a11y.atspi.Value";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

const UNKNOWN_METHOD: &str = "org.freedesktop.DBus.Error.UnknownMethod";
const UNKNOWN_OBJECT: &str = "org.freedesktop.DBus.Error.UnknownObject";
const UNKNOWN_PROPERTY: &str = "org.freedesktop.DBus.Error.UnknownProperty";
const INVALID_ARGS: &str = "org.freedesktop.DBus.Error.InvalidArgs";

/// `AtspiCoordType`.
const COORD_SCREEN: u32 = 0;
const COORD_WINDOW: u32 = 1;

/// `AtspiStateType` bits.
mod state {
    pub const ACTIVE: u32 = 1;
    pub const CHECKED: u32 = 4;
    pub const COLLAPSED: u32 = 5;
    pub const EDITABLE: u32 = 7;
    pub const ENABLED: u32 = 8;
    pub const EXPANDABLE: u32 = 9;
    pub const EXPANDED: u32 = 10;
    pub const FOCUSABLE: u32 = 11;
    pub const FOCUSED: u32 = 12;
    pub const MODAL: u32 = 16;
    pub const RESIZABLE: u32 = 21;
    pub const SELECTABLE: u32 = 22;
    pub const SELECTED: u32 = 23;
    pub const SENSITIVE: u32 = 24;
    pub const SHOWING: u32 = 25;
    pub const SINGLE_LINE: u32 = 26;
    pub const VISIBLE: u32 = 30;
    pub const CHECKABLE: u32 = 41;
    pub const HAS_POPUP: u32 = 42;
}

/// What a path names.
enum Target<'a> {
    /// The application's root.
    App,
    /// A node of a window's tree (the frame for the empty chain).
    Node(Node<'a>),
}

struct Node<'a> {
    win: &'a Win,
    /// Where the window's frame is among the root's children.
    frame_index: usize,
    chain: Vec<usize>,
    node: &'a A11yNode,
    /// The nodes from the frame down to the node's parent.
    ancestors: Vec<&'a A11yNode>,
}

impl Node<'_> {
    fn child_path(&self, i: usize) -> String {
        let mut chain = self.chain.clone();
        chain.push(i);
        node_path(self.win.id, &chain)
    }

    fn name(&self) -> &str {
        if self.chain.is_empty() && self.node.name.is_empty() {
            &self.win.title
        } else {
            &self.node.name
        }
    }

    fn role(&self) -> A11yRole {
        self.node.role
    }

    /// The node's rectangle in window pixels.
    fn window_rect(&self) -> (i32, i32, i32, i32) {
        rect(self.node, self.win.scale)
    }

    /// The node's rectangle in `coord` coordinates.
    fn extents(&self, coord: u32) -> (i32, i32, i32, i32) {
        let (x, y, w, h) = self.window_rect();
        let (ox, oy) = self.offset(coord);
        (x + ox, y + oy, w, h)
    }

    /// What to add to window pixels for `coord` coordinates.
    fn offset(&self, coord: u32) -> (i32, i32) {
        let (sx, sy) = self.win.origin;
        match coord {
            COORD_SCREEN => (sx, sy),
            COORD_WINDOW => (0, 0),
            // Relative to the parent (the frame's parent is the desktop).
            _ => match self.ancestors.last() {
                Some(p) => {
                    let (px, py, _, _) = rect(p, self.win.scale);
                    (-px, -py)
                }
                None => (sx, sy),
            },
        }
    }

    /// The text of a text node.
    fn text(&self) -> Option<&str> {
        text_role(self.role()).then_some(self.node.name.as_str())
    }

    /// A spin button's number.
    fn value(&self) -> Option<f64> {
        (self.role() == A11yRole::SpinButton)
            .then(|| self.node.name.trim().parse::<f64>().ok())
            .flatten()
            .filter(|v| v.is_finite())
    }

    fn interfaces(&self) -> Vec<&'static str> {
        let mut out = vec![ACCESSIBLE, COMPONENT];
        if clickable(self.role()) {
            out.push(ACTION);
        }
        if text_role(self.role()) {
            out.push(TEXT);
        }
        if editable(self.role()) {
            out.push(EDITABLE_TEXT);
        }
        if self.value().is_some() {
            out.push(VALUE);
        }
        out
    }

    fn states(&self) -> [u32; 2] {
        let n = self.node;
        let mut bits = vec![state::ENABLED, state::SENSITIVE, state::VISIBLE];
        let (_, _, w, h) = n.bounds;
        if self.chain.is_empty() || (w > 0.0 && h > 0.0) {
            bits.push(state::SHOWING);
        }
        if self.chain.is_empty() {
            bits.push(state::RESIZABLE);
            if self.win.active {
                bits.push(state::ACTIVE);
            }
        }
        if focusable(n.role) {
            bits.push(state::FOCUSABLE);
        }
        if n.focused {
            bits.extend([state::FOCUSABLE, state::FOCUSED]);
        }
        use A11yRole::*;
        if matches!(n.role, ListItem | Row | Cell | TreeItem | Tab) {
            bits.push(state::SELECTABLE);
        }
        if n.selected {
            bits.push(state::SELECTED);
        }
        match n.expanded {
            Some(true) => bits.extend([state::EXPANDABLE, state::EXPANDED]),
            Some(false) => bits.extend([state::EXPANDABLE, state::COLLAPSED]),
            None => {}
        }
        match n.checked {
            Some(true) => bits.extend([state::CHECKABLE, state::CHECKED]),
            Some(false) => bits.push(state::CHECKABLE),
            None => {}
        }
        if editable(n.role) {
            bits.extend([state::EDITABLE, state::SINGLE_LINE]);
        }
        if n.role == Dialog {
            bits.push(state::MODAL);
        }
        if n.role == ComboBox {
            bits.push(state::HAS_POPUP);
        }
        let mut set = [0u32; 2];
        for b in bits {
            set[(b / 32) as usize] |= 1 << (b % 32);
        }
        set
    }

    /// The pointer events of a click at the node's middle.
    fn click(&self) -> Vec<Event> {
        let (x, y, w, h) = self.node.bounds;
        let position = Point::new(x + w / 2.0, y + h / 2.0);
        vec![
            Event::PointerMoved { position },
            Event::PointerButton {
                button: PointerButton::Left,
                state: ButtonState::Pressed,
                position,
            },
            Event::PointerButton {
                button: PointerButton::Left,
                state: ButtonState::Released,
                position,
            },
        ]
    }

    /// Replace a text field's text: focus it (a click) unless it has the
    /// focus, then delete all of its text and type the new one.
    fn set_text(&self, s: &str) -> Vec<Event> {
        let mut out = if self.node.focused {
            Vec::new()
        } else {
            self.click()
        };
        out.push(Event::Ime(Ime::DeleteSurrounding {
            before: 1 << 30,
            after: 1 << 30,
        }));
        if !s.is_empty() {
            out.push(Event::Ime(Ime::Commit(s.to_string())));
        }
        out
    }
}

/// A node's rectangle in window pixels.
fn rect(n: &A11yNode, scale: f64) -> (i32, i32, i32, i32) {
    let (x, y, w, h) = n.bounds;
    let px = |v: f64| (v * scale).round() as i32;
    (px(x), px(y), px(w), px(h))
}

fn resolve<'a>(b: &'a Bridge, path: &str) -> Option<Target<'a>> {
    if path == ROOT_PATH {
        return Some(Target::App);
    }
    let (id, chain) = parse_path(path)?;
    let frame_index = b.frames().position(|w| w.id == id)?;
    let win = b.frames().nth(frame_index)?;
    let mut node: &A11yNode = win.root.as_deref()?;
    let mut ancestors = Vec::new();
    for &i in &chain {
        let child = node.children.get(i)?;
        ancestors.push(node);
        node = child;
    }
    Some(Target::Node(Node {
        win,
        frame_index,
        chain,
        node,
        ancestors,
    }))
}

fn locale() -> String {
    let lang = std::env::var("LC_ALL")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("LC_MESSAGES").ok().filter(|s| !s.is_empty()))
        .or_else(|| std::env::var("LANG").ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "C".into());
    lang.split('.').next().unwrap_or("C").to_string()
}

/// The application's name: the program's file name.
fn app_name() -> String {
    std::env::args_os()
        .next()
        .and_then(|a| {
            std::path::Path::new(&a)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "stipple".into())
}

fn ret(sig: &'static str, f: impl FnOnce(&mut Body)) -> Reply {
    let mut b = Body::new();
    f(&mut b);
    Reply::Return(sig, b.0)
}

/// The answer to `m`, a method call. Actions it asks for go to `b.actions`.
pub(super) fn answer(b: &mut Bridge, m: &Message) -> Reply {
    let iface = m.interface.as_str();
    let member = m.member.as_str();
    match (iface, member) {
        ("org.freedesktop.DBus.Peer", "Ping") => return Reply::Return("", Vec::new()),
        ("org.freedesktop.DBus.Peer", "GetMachineId") => {
            return ret("s", |r| {
                r.str("00000000000000000000000000000000");
            });
        }
        _ => {}
    }
    if m.path == "/org/a11y/atspi/cache" {
        return match member {
            // No cache: clients ask each object.
            "GetItems" => ret("a((so)(so)(so)iiassusau)", |r| {
                r.array(8, |_| {});
            }),
            _ if iface == "org.freedesktop.DBus.Introspectable" => ret("s", |r| {
                r.str("<node/>");
            }),
            _ => Reply::Error(UNKNOWN_METHOD),
        };
    }
    // The registry hands the application its id.
    if (iface, member) == (PROPERTIES, "Set") && m.path == ROOT_PATH {
        let mut a = Args::new(&m.body);
        let (i, p) = (a.str(), a.str());
        if i.as_deref() == Some(APPLICATION) && p.as_deref() == Some("Id") {
            return match a.variant_i32() {
                Some(id) => {
                    b.app_id = id;
                    Reply::Return("", Vec::new())
                }
                None => Reply::Error(INVALID_ARGS),
            };
        }
        return Reply::Error(UNKNOWN_PROPERTY);
    }
    let mut action = None;
    let reply = {
        let b: &Bridge = b;
        match resolve(b, &m.path) {
            None => Reply::Error(UNKNOWN_OBJECT),
            Some(Target::App) => app(b, m),
            Some(Target::Node(n)) => {
                let (reply, events) = node(b, &n, m);
                if let Some(events) = events {
                    action = Some((n.win.id, events));
                }
                reply
            }
        }
    };
    if let Some(a) = action {
        b.actions.push(a);
    }
    if let Reply::Error(e) = &reply {
        super::debug(format_args!("{iface}.{member} on {}: {e}", m.path));
    }
    reply
}

fn introspect(interfaces: &[&str]) -> Reply {
    let mut xml = String::from(
        "<!DOCTYPE node PUBLIC \"-//freedesktop//DTD D-BUS Object Introspection 1.0//EN\" \
         \"http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd\">\n<node>\n\
         <interface name=\"org.freedesktop.DBus.Introspectable\"/>\n\
         <interface name=\"org.freedesktop.DBus.Properties\"/>\n\
         <interface name=\"org.freedesktop.DBus.Peer\"/>\n",
    );
    for i in interfaces {
        xml.push_str(&format!("<interface name=\"{i}\"/>\n"));
    }
    xml.push_str("</node>\n");
    ret("s", |r| {
        r.str(&xml);
    })
}

/// A `Properties.Get`/`GetAll` answer from a property lookup.
fn properties(m: &Message, names: &[&str], get: impl Fn(&str, &str) -> Option<Value>) -> Reply {
    let mut a = Args::new(&m.body);
    let iface = a.str().unwrap_or_default();
    match m.member.as_str() {
        "Get" => {
            let prop = a.str().unwrap_or_default();
            match get(&iface, &prop) {
                Some(v) => ret("v", |r| {
                    r.variant(&v);
                }),
                None => Reply::Error(UNKNOWN_PROPERTY),
            }
        }
        "GetAll" => ret("a{sv}", |r| {
            r.array(8, |r| {
                for name in names {
                    if let Some(v) = get(&iface, name) {
                        crate::platform::a11y::align(&mut r.0, 8);
                        r.str(name).variant(&v);
                    }
                }
            });
        }),
        _ => Reply::Error(UNKNOWN_METHOD),
    }
}

const PROPERTY_NAMES: &[&str] = &[
    "Name",
    "Description",
    "Parent",
    "ChildCount",
    "Locale",
    "AccessibleId",
    "HelpText",
    "ToolkitName",
    "Version",
    "AtspiVersion",
    "Id",
    "NActions",
    "CharacterCount",
    "CaretOffset",
    "MinimumValue",
    "MaximumValue",
    "MinimumIncrement",
    "CurrentValue",
    "Text",
];

fn app(b: &Bridge, m: &Message) -> Reply {
    let bus = b.bus.as_str();
    let frames: Vec<&Win> = b.frames().collect();
    match (m.interface.as_str(), m.member.as_str()) {
        ("org.freedesktop.DBus.Introspectable", "Introspect") => {
            introspect(&[ACCESSIBLE, APPLICATION])
        }
        (PROPERTIES, _) => properties(m, PROPERTY_NAMES, |iface, prop| match (iface, prop) {
            (ACCESSIBLE, "Name") => Some(Value::Str(app_name())),
            (ACCESSIBLE, "Description" | "AccessibleId" | "HelpText") => {
                Some(Value::Str(String::new()))
            }
            (ACCESSIBLE, "Locale") => Some(Value::Str(locale())),
            (ACCESSIBLE, "Parent") => {
                let (dbus, path) = if b.desktop.1.is_empty() {
                    (super::REGISTRY.to_string(), ROOT_PATH.to_string())
                } else {
                    b.desktop.clone()
                };
                Some(Value::Obj(dbus, path))
            }
            (ACCESSIBLE, "ChildCount") => Some(Value::I32(frames.len() as i32)),
            (APPLICATION, "ToolkitName") => Some(Value::Str("Stipple".into())),
            (APPLICATION, "Version") => Some(Value::Str(env!("CARGO_PKG_VERSION").into())),
            (APPLICATION, "AtspiVersion") => Some(Value::Str("2.1".into())),
            (APPLICATION, "Id") => Some(Value::I32(b.app_id)),
            _ => None,
        }),
        (ACCESSIBLE, member) => match member {
            "GetRole" => ret("u", |r| {
                r.u32(75); // APPLICATION
            }),
            "GetRoleName" | "GetLocalizedRoleName" => ret("s", |r| {
                r.str("application");
            }),
            "GetChildAtIndex" => {
                let i = Args::new(&m.body).i32().unwrap_or(-1);
                ret("(so)", |r| {
                    match usize::try_from(i).ok().and_then(|i| frames.get(i)) {
                        Some(w) => r.obj(bus, &node_path(w.id, &[])),
                        None => r.obj(bus, NULL_PATH),
                    };
                })
            }
            "GetChildren" => ret("a(so)", |r| {
                r.array(8, |r| {
                    for w in &frames {
                        r.obj(bus, &node_path(w.id, &[]));
                    }
                });
            }),
            "GetIndexInParent" => ret("i", |r| {
                r.i32(-1);
            }),
            "GetRelationSet" => ret("a(ua(so))", |r| {
                r.array(8, |_| {});
            }),
            "GetState" => ret("au", |r| {
                r.array(4, |r| {
                    r.u32(0).u32(0);
                });
            }),
            "GetAttributes" => ret("a{ss}", |r| {
                r.str_dict(&[("toolkit", "Stipple")]);
            }),
            "GetApplication" => ret("(so)", |r| {
                r.obj(bus, ROOT_PATH);
            }),
            "GetInterfaces" => ret("as", |r| {
                r.strings(&[ACCESSIBLE, APPLICATION]);
            }),
            _ => Reply::Error(UNKNOWN_METHOD),
        },
        (APPLICATION, "GetLocale") => ret("s", |r| {
            r.str(&locale());
        }),
        // No peer-to-peer connection: clients stay on the bus.
        (APPLICATION, "GetApplicationBusAddress") => ret("s", |r| {
            r.str("");
        }),
        (APPLICATION, "RegisterEventListener" | "DeregisterEventListener") => {
            Reply::Return("", Vec::new())
        }
        _ => Reply::Error(UNKNOWN_METHOD),
    }
}

/// The answer to a call on a window's node, and the events it has the UI
/// thread deliver (an action).
fn node(b: &Bridge, n: &Node, m: &Message) -> (Reply, Option<Vec<Event>>) {
    let bus = b.bus.as_str();
    let mut args = Args::new(&m.body);
    let reply = match (m.interface.as_str(), m.member.as_str()) {
        ("org.freedesktop.DBus.Introspectable", "Introspect") => introspect(&n.interfaces()),
        (PROPERTIES, "Set") => Reply::Error(UNKNOWN_PROPERTY),
        (PROPERTIES, _) => properties(m, PROPERTY_NAMES, |iface, prop| {
            node_property(b, n, iface, prop)
        }),
        (ACCESSIBLE, member) => match member {
            "GetRole" => ret("u", |r| {
                r.u32(role(n.role()));
            }),
            "GetRoleName" | "GetLocalizedRoleName" => ret("s", |r| {
                r.str(role_name(n.role()));
            }),
            "GetChildAtIndex" => {
                let i = args.i32().unwrap_or(-1);
                ret("(so)", |r| {
                    match usize::try_from(i)
                        .ok()
                        .filter(|&i| i < n.node.children.len())
                    {
                        Some(i) => r.obj(bus, &n.child_path(i)),
                        None => r.obj(bus, NULL_PATH),
                    };
                })
            }
            "GetChildren" => ret("a(so)", |r| {
                r.array(8, |r| {
                    for i in 0..n.node.children.len() {
                        r.obj(bus, &n.child_path(i));
                    }
                });
            }),
            "GetIndexInParent" => ret("i", |r| {
                r.i32(match n.chain.last() {
                    Some(&i) => i as i32,
                    None => n.frame_index as i32,
                });
            }),
            "GetRelationSet" => ret("a(ua(so))", |r| {
                r.array(8, |_| {});
            }),
            "GetState" => ret("au", |r| {
                let [lo, hi] = n.states();
                r.array(4, |r| {
                    r.u32(lo).u32(hi);
                });
            }),
            "GetAttributes" => ret("a{ss}", |r| {
                r.str_dict(&[("toolkit", "Stipple")]);
            }),
            "GetApplication" => ret("(so)", |r| {
                r.obj(bus, ROOT_PATH);
            }),
            "GetInterfaces" => ret("as", |r| {
                r.strings(&n.interfaces());
            }),
            _ => Reply::Error(UNKNOWN_METHOD),
        },
        (COMPONENT, member) => return component(bus, n, member, &mut args),
        (ACTION, member) if clickable(n.role()) => match member {
            "GetName" | "GetLocalizedName" => ret("s", |r| {
                r.str("click");
            }),
            "GetDescription" => ret("s", |r| {
                r.str("Click");
            }),
            "GetKeyBinding" => ret("s", |r| {
                r.str("");
            }),
            "GetActions" => ret("a(sss)", |r| {
                r.array(8, |r| {
                    // One (name, description, key binding).
                    crate::platform::a11y::align(&mut r.0, 8);
                    r.str("click").str("Click").str("");
                });
            }),
            "DoAction" => {
                let ok = args.i32() == Some(0);
                return (
                    ret("b", |r| {
                        r.bool(ok);
                    }),
                    ok.then(|| n.click()),
                );
            }
            _ => Reply::Error(UNKNOWN_METHOD),
        },
        (TEXT, member) if text_role(n.role()) => text_call(n, member, &mut args),
        (EDITABLE_TEXT, member) if editable(n.role()) => {
            return editable_call(n, member, &mut args);
        }
        _ => Reply::Error(UNKNOWN_METHOD),
    };
    (reply, None)
}

fn node_property(b: &Bridge, n: &Node, iface: &str, prop: &str) -> Option<Value> {
    let text = n.text();
    Some(match (iface, prop) {
        (ACCESSIBLE, "Name") => Value::Str(n.name().to_string()),
        (ACCESSIBLE, "Description" | "AccessibleId" | "HelpText") => Value::Str(String::new()),
        (ACCESSIBLE, "Locale") => Value::Str(locale()),
        (ACCESSIBLE, "Parent") => {
            let path = if n.chain.is_empty() {
                ROOT_PATH.to_string()
            } else {
                node_path(n.win.id, &n.chain[..n.chain.len() - 1])
            };
            Value::Obj(b.bus.clone(), path)
        }
        (ACCESSIBLE, "ChildCount") => Value::I32(n.node.children.len() as i32),
        (ACTION, "NActions") if clickable(n.role()) => Value::I32(1),
        (TEXT, "CharacterCount") => Value::I32(text::char_count(text?)),
        (TEXT, "CaretOffset") => Value::I32(caret(n)?),
        (VALUE, "CurrentValue") => Value::F64(n.value()?),
        (VALUE, "MinimumValue") => {
            n.value()?;
            Value::F64(f64::MIN)
        }
        (VALUE, "MaximumValue") => {
            n.value()?;
            Value::F64(f64::MAX)
        }
        (VALUE, "MinimumIncrement") => {
            n.value()?;
            Value::F64(1.0)
        }
        (VALUE, "Text") => {
            n.value()?;
            Value::Str(n.node.name.clone())
        }
        _ => return None,
    })
}

/// Where the caret is: the tree doesn't say, so a focused field's caret is
/// at the end of its text (where typing leaves it), and other text has none.
fn caret(n: &Node) -> Option<i32> {
    let t = n.text()?;
    Some(if n.node.focused && editable(n.role()) {
        text::char_count(t)
    } else {
        0
    })
}

fn component(bus: &str, n: &Node, member: &str, args: &mut Args) -> (Reply, Option<Vec<Event>>) {
    let reply = match member {
        "GetExtents" => {
            let (x, y, w, h) = n.extents(args.u32().unwrap_or(COORD_SCREEN));
            ret("(iiii)", |r| {
                crate::platform::a11y::align(&mut r.0, 8);
                r.i32(x).i32(y).i32(w).i32(h);
            })
        }
        "GetPosition" => {
            let (x, y, _, _) = n.extents(args.u32().unwrap_or(COORD_SCREEN));
            ret("ii", |r| {
                r.i32(x).i32(y);
            })
        }
        "GetSize" => {
            let (_, _, w, h) = n.window_rect();
            ret("ii", |r| {
                r.i32(w).i32(h);
            })
        }
        "Contains" | "GetAccessibleAtPoint" => {
            let (x, y) = (args.i32().unwrap_or(0), args.i32().unwrap_or(0));
            let coord = args.u32().unwrap_or(COORD_SCREEN);
            // The point in window pixels, then in the tree's logical ones.
            let (ox, oy) = n.offset(coord);
            let scale = n.win.scale.max(f64::MIN_POSITIVE);
            let (lx, ly) = (f64::from(x - ox) / scale, f64::from(y - oy) / scale);
            if member == "Contains" {
                let inside = contains(n.node, lx, ly);
                ret("b", |r| {
                    r.bool(inside);
                })
            } else {
                let mut chain = n.chain.clone();
                let path = if hit(n.node, lx, ly, &mut chain) {
                    node_path(n.win.id, &chain)
                } else {
                    NULL_PATH.to_string()
                };
                ret("(so)", |r| {
                    r.obj(bus, &path);
                })
            }
        }
        "GetLayer" => ret("u", |r| {
            // WINDOW for the frame, WIDGET for the rest.
            r.u32(if n.chain.is_empty() { 7 } else { 3 });
        }),
        "GetMDIZOrder" => ret("n", |r| {
            r.i16(0);
        }),
        "GetAlpha" => ret("d", |r| {
            r.f64(1.0);
        }),
        "GrabFocus" => {
            // Focus moves with a click, which would also activate a button:
            // only the controls a click just focuses (fields, items) take it.
            use A11yRole::*;
            let activates = matches!(
                n.role(),
                Button | MenuItem | ComboBox | CheckBox | Switch | Tab | ColumnHeader
            );
            let ok = n.node.focused || (focusable(n.role()) && !activates);
            let events = (ok && !n.node.focused).then(|| n.click());
            return (
                ret("b", |r| {
                    r.bool(ok);
                }),
                events,
            );
        }
        "ScrollTo" | "ScrollToPoint" | "SetExtents" | "SetPosition" | "SetSize" => ret("b", |r| {
            r.bool(false);
        }),
        _ => Reply::Error(UNKNOWN_METHOD),
    };
    (reply, None)
}

fn contains(n: &A11yNode, x: f64, y: f64) -> bool {
    let (nx, ny, w, h) = n.bounds;
    x >= nx && y >= ny && x < nx + w && y < ny + h
}

/// The deepest descendant of `n` under `(x, y)`, the last painted first;
/// `chain` is extended to it. False when no child is there.
fn hit(n: &A11yNode, x: f64, y: f64, chain: &mut Vec<usize>) -> bool {
    for (i, c) in n.children.iter().enumerate().rev() {
        if contains(c, x, y) {
            chain.push(i);
            hit(c, x, y, chain);
            return true;
        }
    }
    false
}

fn text_call(n: &Node, member: &str, args: &mut Args) -> Reply {
    let t = n.text().unwrap_or_default();
    let count = text::char_count(t);
    let sii = |(s, a, b): (String, i32, i32)| {
        ret("sii", move |r| {
            r.str(&s).i32(a).i32(b);
        })
    };
    // A character's box: the node's split evenly.
    let char_box = |offset: i32, coord: u32| {
        let (x, y, w, h) = n.extents(coord);
        let per = if count > 0 { w / count } else { 0 };
        (x + per * offset.clamp(0, count), y, per, h)
    };
    match member {
        "GetText" => {
            let (s, e) = (args.i32().unwrap_or(0), args.i32().unwrap_or(-1));
            ret("s", |r| {
                r.str(&text::slice(t, s, e));
            })
        }
        "GetCharacterAtOffset" => {
            let c = text::char_at(t, args.i32().unwrap_or(0));
            ret("i", |r| {
                r.i32(c);
            })
        }
        "GetTextAtOffset" | "GetTextBeforeOffset" | "GetTextAfterOffset" => {
            let (o, kind) = (args.i32().unwrap_or(0), args.u32().unwrap_or(0));
            let which = match member {
                "GetTextBeforeOffset" => Which::Before,
                "GetTextAfterOffset" => Which::After,
                _ => Which::At,
            };
            sii(text::text_at(t, o, kind, which))
        }
        "GetStringAtOffset" => {
            let (o, g) = (args.i32().unwrap_or(0), args.u32().unwrap_or(0));
            sii(text::string_at(t, o, g))
        }
        "SetCaretOffset"
        | "AddSelection"
        | "RemoveSelection"
        | "SetSelection"
        | "ScrollSubstringTo"
        | "ScrollSubstringToPoint" => ret("b", |r| {
            r.bool(false);
        }),
        "GetNSelections" => ret("i", |r| {
            r.i32(0);
        }),
        "GetSelection" => ret("ii", |r| {
            r.i32(0).i32(0);
        }),
        "GetAttributeValue" => ret("s", |r| {
            r.str("");
        }),
        "GetAttributes" | "GetAttributeRun" => ret("a{ss}ii", |r| {
            r.str_dict(&[]).i32(0).i32(count);
        }),
        "GetDefaultAttributes" | "GetDefaultAttributeSet" => ret("a{ss}", |r| {
            r.str_dict(&[]);
        }),
        "GetCharacterExtents" => {
            let (o, coord) = (args.i32().unwrap_or(0), args.u32().unwrap_or(0));
            let (x, y, w, h) = char_box(o, coord);
            ret("iiii", |r| {
                r.i32(x).i32(y).i32(w).i32(h);
            })
        }
        "GetRangeExtents" => {
            let (s, e) = (args.i32().unwrap_or(0), args.i32().unwrap_or(0));
            let coord = args.u32().unwrap_or(0);
            let (x, y, _, h) = char_box(s, coord);
            let (x2, _, _, _) = char_box(e, coord);
            ret("iiii", |r| {
                r.i32(x).i32(y).i32(x2 - x).i32(h);
            })
        }
        "GetOffsetAtPoint" => {
            let (x, y) = (args.i32().unwrap_or(0), args.i32().unwrap_or(0));
            let coord = args.u32().unwrap_or(0);
            let (nx, ny, w, h) = n.extents(coord);
            let inside = x >= nx && y >= ny && x < nx + w && y < ny + h;
            let at = if inside && w > 0 {
                (i64::from(x - nx) * i64::from(count) / i64::from(w)) as i32
            } else {
                -1
            };
            ret("i", |r| {
                r.i32(at);
            })
        }
        "GetBoundedRanges" => ret("a(iisv)", |r| {
            r.array(8, |_| {});
        }),
        _ => Reply::Error(UNKNOWN_METHOD),
    }
}

/// `EditableText`: edits become a new text for the field, typed in through
/// the input-method path (the field's whole text replaced).
fn editable_call(n: &Node, member: &str, args: &mut Args) -> (Reply, Option<Vec<Event>>) {
    let t = n.node.name.as_str();
    // A password field's name never carries its text, so only replacing all
    // of it is exact.
    let knows_text = n.role() != A11yRole::PasswordField;
    let new = match member {
        "SetTextContents" => args.str(),
        "InsertText" if knows_text => {
            let (at, s, len) = (args.i32(), args.str(), args.i32());
            match (at, s, len) {
                (Some(at), Some(s), Some(len)) => {
                    let s: String = if len >= 0 {
                        s.chars().take(len as usize).collect()
                    } else {
                        s
                    };
                    let at = at.clamp(0, text::char_count(t));
                    Some(text::slice(t, 0, at) + &s + &text::slice(t, at, -1))
                }
                _ => None,
            }
        }
        "DeleteText" if knows_text => match (args.i32(), args.i32()) {
            (Some(s), Some(e)) if s <= e || e < 0 => {
                Some(text::slice(t, 0, s) + &text::slice(t, e.max(s), -1))
            }
            _ => None,
        },
        "CopyText" => return (Reply::Return("", Vec::new()), None),
        "InsertText" | "DeleteText" | "CutText" | "PasteText" => None,
        _ => return (Reply::Error(UNKNOWN_METHOD), None),
    };
    let events = new.map(|s| n.set_text(&s));
    let ok = events.is_some();
    (
        ret("b", |r| {
            r.bool(ok);
        }),
        events,
    )
}

/// The role's name, as `GetRoleName` gives it.
fn role_name(r: A11yRole) -> &'static str {
    use A11yRole::*;
    match r {
        Window => "frame",
        Group => "panel",
        Button => "push button",
        TextField => "entry",
        Text => "label",
        List => "list",
        ListItem => "list item",
        Table => "table",
        Row => "table row",
        Cell => "table cell",
        ColumnHeader => "table column header",
        Tree => "tree",
        TreeItem => "tree item",
        MenuBar => "menu bar",
        Menu => "menu",
        MenuItem => "menu item",
        ComboBox => "combo box",
        SpinButton => "spin button",
        CheckBox => "check box",
        Switch => "toggle button",
        Splitter => "split pane",
        Toolbar => "tool bar",
        Separator => "separator",
        Alert => "notification",
        TabList => "page tab list",
        Tab => "page tab",
        PasswordField => "password text",
        Dialog => "dialog",
    }
}
