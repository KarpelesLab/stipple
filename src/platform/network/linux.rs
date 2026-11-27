//! Linux: the default route over netlink (`NETLINK_ROUTE`), with
//! NetworkManager over D-Bus when it runs.
//!
//! The base is the kernel's own view, which every system has: a thread
//! joins the `RTMGRP_LINK`, `RTMGRP_IPV4_ROUTE` and `RTMGRP_IPV6_ROUTE`
//! groups, and on each message (a link going up or down, a route added or
//! removed) dumps the main table's default routes and the links again. The
//! default network is the default route with the lowest metric whose link is
//! up; its transport is what sysfs says of the link (`DEVTYPE=wlan`,
//! `wwan`), or Ethernet for an Ethernet-framed device.
//!
//! NetworkManager knows more: whether the connection is metered (its
//! `Metered` property, set by the user or guessed for a hotspot or a modem)
//! and its kind (`PrimaryConnectionType`). It's asked on the system bus each
//! time, and only believed when its primary connection runs on the
//! interface the kernel routes through (a process in a network namespace of
//! its own, say, sees other interfaces than NetworkManager does). The
//! thread also hears its `PropertiesChanged`, since metering can change
//! without the routes doing so. Without it, a cellular link counts as
//! metered. Linux has no system-wide data saver, so `constrained` stays
//! false.
//!
//! The message parsing is plain Rust over bytes, tested on every host.

use super::{Reading, Transport};

// Netlink message types and flags (`linux/netlink.h`, `linux/rtnetlink.h`).
const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;
const RTM_NEWLINK: u16 = 16;
const RTM_GETLINK: u16 = 18;
const RTM_NEWROUTE: u16 = 24;
const RTM_GETROUTE: u16 = 26;
const NLM_F_REQUEST: u16 = 1;
const NLM_F_DUMP: u16 = 0x300;
/// `nlmsghdr`: length, type, flags, sequence, port.
const NLMSG_HDR: usize = 16;
/// `rtmsg`.
const RTMSG: usize = 12;
/// `ifinfomsg`.
const IFINFOMSG: usize = 16;

// Route attributes (`RTA_*`) and the values the default route has.
const RTA_OIF: u16 = 4;
const RTA_GATEWAY: u16 = 5;
const RTA_PRIORITY: u16 = 6;
const RTA_MULTIPATH: u16 = 9;
const RTA_TABLE: u16 = 15;
const RTN_UNICAST: u8 = 1;
const RT_TABLE_MAIN: u32 = 254;
/// `RTNH_F_DEAD` and `RTNH_F_LINKDOWN`: a route the kernel keeps over a link
/// that is down.
const RTNH_F_DEAD_OR_LINKDOWN: u32 = 1 | 16;

// Link attributes (`IFLA_*`) and flags (`IFF_*`).
const IFLA_IFNAME: u16 = 3;
const IFLA_LINKINFO: u16 = 18;
const IFLA_INFO_KIND: u16 = 1;
const IFF_UP: u32 = 0x1;
const IFF_LOOPBACK: u32 = 0x8;
const IFF_RUNNING: u32 = 0x40;
const IFF_LOWER_UP: u32 = 0x1_0000;
// Link types (`ARPHRD_*`).
const ARPHRD_ETHER: u16 = 1;
const ARPHRD_RAWIP: u16 = 519;
const ARPHRD_NONE: u16 = 0xfffe;

/// The address families asked for.
const AF_INET: u8 = 2;
const AF_INET6: u8 = 10;

/// A default route in the main table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Route {
    pub family: u8,
    /// The interface it leaves through.
    pub oif: u32,
    /// The gateway's address (empty for a route straight onto a link).
    pub gateway: Vec<u8>,
    /// The metric: lower wins.
    pub priority: u32,
}

/// A network interface.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Link {
    pub index: u32,
    /// `ARPHRD_*`.
    pub kind_hw: u16,
    pub flags: u32,
    pub name: String,
    /// The driver of a virtual link (`IFLA_INFO_KIND`: `dummy`, `veth`,
    /// `wireguard`, `tun`, `bridge`…), `None` for hardware.
    pub kind: Option<String>,
}

impl Link {
    /// Up and with a carrier.
    pub(crate) fn usable(&self) -> bool {
        self.flags & IFF_UP != 0
            && self.flags & IFF_LOOPBACK == 0
            && self.flags & (IFF_LOWER_UP | IFF_RUNNING) != 0
    }
}

/// A dump request (`NLM_F_DUMP`) of `kind` (`RTM_GETROUTE`, `RTM_GETLINK`)
/// for `family`.
pub(crate) fn dump_request(kind: u16, seq: u32, family: u8) -> Vec<u8> {
    let body = if kind == RTM_GETLINK {
        IFINFOMSG
    } else {
        RTMSG
    };
    let len = NLMSG_HDR + body;
    let mut out = Vec::with_capacity(len);
    out.extend_from_slice(&(len as u32).to_ne_bytes());
    out.extend_from_slice(&kind.to_ne_bytes());
    out.extend_from_slice(&(NLM_F_REQUEST | NLM_F_DUMP).to_ne_bytes());
    out.extend_from_slice(&seq.to_ne_bytes());
    out.extend_from_slice(&0u32.to_ne_bytes());
    out.push(family);
    out.resize(len, 0);
    out
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_ne_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_ne_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// The netlink messages in `buf`: each one's type and payload. Stops at a
/// malformed one.
pub(crate) fn messages(buf: &[u8]) -> Vec<(u16, &[u8])> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + NLMSG_HDR <= buf.len() {
        let Some(len) = u32_at(buf, at).map(|l| l as usize) else {
            break;
        };
        if len < NLMSG_HDR || at + len > buf.len() {
            break;
        }
        let kind = u16_at(buf, at + 4).unwrap_or(0);
        out.push((kind, &buf[at + NLMSG_HDR..at + len]));
        at += len.next_multiple_of(4);
    }
    out
}

/// The attributes (`rtattr`) in `b`: each one's type and value.
fn attributes(b: &[u8]) -> Vec<(u16, &[u8])> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 4 <= b.len() {
        let len = u16_at(b, at).unwrap_or(0) as usize;
        if len < 4 || at + len > b.len() {
            break;
        }
        // Without the nested and byte-order flags.
        let kind = u16_at(b, at + 2).unwrap_or(0) & 0x3fff;
        out.push((kind, &b[at + 4..at + len]));
        at += len.next_multiple_of(4);
    }
    out
}

/// The default route an `RTM_NEWROUTE` payload describes, if it is one: a
/// unicast route to `0/0` (or `::/0`) in the main table, over a link that
/// isn't down.
pub(crate) fn parse_route(payload: &[u8]) -> Option<Route> {
    if payload.len() < RTMSG {
        return None;
    }
    let family = payload[0];
    let dst_len = payload[1];
    let mut table = payload[4] as u32;
    let kind = payload[7];
    let flags = u32_at(payload, 8)?;
    if dst_len != 0 || kind != RTN_UNICAST || flags & RTNH_F_DEAD_OR_LINKDOWN != 0 {
        return None;
    }
    let mut route = Route {
        family,
        oif: 0,
        gateway: Vec::new(),
        priority: 0,
    };
    for (kind, value) in attributes(&payload[RTMSG..]) {
        match kind {
            RTA_TABLE => table = u32_at(value, 0)?,
            RTA_OIF => route.oif = u32_at(value, 0)?,
            RTA_GATEWAY => route.gateway = value.to_vec(),
            RTA_PRIORITY => route.priority = u32_at(value, 0)?,
            RTA_MULTIPATH => {
                // `rtnexthop`s: length, flags, hops, interface, attributes.
                // The first live one stands for the route.
                let mut at = 0;
                while at + 8 <= value.len() {
                    let len = u16_at(value, at)? as usize;
                    if len < 8 || at + len > value.len() {
                        break;
                    }
                    let nh_flags = value[at + 2] as u32;
                    let ifindex = u32_at(value, at + 4)?;
                    if route.oif == 0 && nh_flags & RTNH_F_DEAD_OR_LINKDOWN == 0 {
                        route.oif = ifindex;
                        for (k, v) in attributes(&value[at + 8..at + len]) {
                            if k == RTA_GATEWAY {
                                route.gateway = v.to_vec();
                            }
                        }
                    }
                    at += len.next_multiple_of(4);
                }
            }
            _ => {}
        }
    }
    (table == RT_TABLE_MAIN && route.oif != 0).then_some(route)
}

/// The link an `RTM_NEWLINK` payload describes.
pub(crate) fn parse_link(payload: &[u8]) -> Option<Link> {
    if payload.len() < IFINFOMSG {
        return None;
    }
    let mut link = Link {
        kind_hw: u16_at(payload, 2)?,
        index: u32_at(payload, 4)?,
        flags: u32_at(payload, 8)?,
        ..Link::default()
    };
    for (kind, value) in attributes(&payload[IFINFOMSG..]) {
        match kind {
            IFLA_IFNAME => {
                let end = value.iter().position(|&b| b == 0).unwrap_or(value.len());
                link.name = String::from_utf8_lossy(&value[..end]).into_owned();
            }
            IFLA_LINKINFO => {
                for (k, v) in attributes(value) {
                    if k == IFLA_INFO_KIND {
                        let end = v.iter().position(|&b| b == 0).unwrap_or(v.len());
                        link.kind = Some(String::from_utf8_lossy(&v[..end]).into_owned());
                    }
                }
            }
            _ => {}
        }
    }
    Some(link)
}

/// The default route traffic takes: the lowest metric over a usable link
/// (IPv4 first on a tie, as the kernel's address selection would mostly
/// have it).
pub(crate) fn best_default<'a>(routes: &'a [Route], links: &[Link]) -> Option<&'a Route> {
    routes
        .iter()
        .filter(|r| links.iter().any(|l| l.index == r.oif && l.usable()))
        .min_by_key(|r| (r.priority, r.family != AF_INET))
}

/// What sysfs says of a link: its `uevent`'s `DEVTYPE` and whether it has
/// a `wireless` directory.
#[derive(Clone, Debug, Default)]
pub(crate) struct Sysfs {
    pub devtype: Option<String>,
    pub wireless: bool,
}

impl Sysfs {
    fn read(name: &str) -> Sysfs {
        if name.is_empty() || name.contains('/') || name.starts_with('.') {
            return Sysfs::default();
        }
        let dir = std::path::Path::new("/sys/class/net").join(name);
        let devtype = std::fs::read_to_string(dir.join("uevent"))
            .ok()
            .and_then(|u| {
                u.lines()
                    .find_map(|l| l.strip_prefix("DEVTYPE=").map(str::to_string))
            });
        Sysfs {
            devtype,
            wireless: dir.join("wireless").exists() || dir.join("phy80211").exists(),
        }
    }
}

/// The transport of `link`, from the kernel's view of it.
pub(crate) fn transport_of(link: &Link, sys: &Sysfs) -> Transport {
    match sys.devtype.as_deref() {
        Some("wlan") => return Transport::Wifi,
        Some("wwan") => return Transport::Cellular,
        _ => {}
    }
    if sys.wireless {
        return Transport::Wifi;
    }
    match link.kind.as_deref() {
        // A container's end of a virtual cable is its wire.
        Some("veth") => Transport::Ethernet,
        // Tunnels, bridges, dummies: what's under them isn't known here.
        Some(_) => Transport::Other,
        None => match link.kind_hw {
            ARPHRD_ETHER => Transport::Ethernet,
            // Modems in raw-IP mode (qmi_wwan, mhi).
            ARPHRD_RAWIP => Transport::Cellular,
            ARPHRD_NONE if link.name.starts_with("wwan") => Transport::Cellular,
            _ => Transport::Other,
        },
    }
}

/// The transport of a NetworkManager connection type
/// (`PrimaryConnectionType`), if it names one.
pub(crate) fn nm_transport(kind: &str) -> Option<Transport> {
    Some(match kind {
        "802-11-wireless" | "wifi" | "802-11-olpc-mesh" | "wifi-p2p" => Transport::Wifi,
        "gsm" | "cdma" => Transport::Cellular,
        "802-3-ethernet" | "ethernet" | "pppoe" => Transport::Ethernet,
        "" => return None,
        _ => Transport::Other,
    })
}

/// Whether NetworkManager's `Metered` (`NMMetered`: 1 yes, 3 guessed yes; 2
/// and 4 no, 0 unknown) says the connection is metered.
pub(crate) fn nm_metered(value: u32) -> Option<bool> {
    match value {
        1 | 3 => Some(true),
        2 | 4 => Some(false),
        _ => None,
    }
}

/// What NetworkManager says of its primary connection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct NmView {
    /// The interface it carries IP on.
    pub interface: String,
    pub kind: String,
    pub metered: Option<bool>,
}

/// The reading the kernel's routes and links make, with NetworkManager's
/// view of the same interface.
pub(crate) fn reading(
    routes: &[Route],
    links: &[Link],
    sysfs: impl Fn(&str) -> Sysfs,
    nm: Option<&NmView>,
) -> Reading {
    let Some(route) = best_default(routes, links) else {
        return Reading::OFFLINE;
    };
    let Some(link) = links.iter().find(|l| l.index == route.oif) else {
        return Reading::OFFLINE;
    };
    let mut transport = transport_of(link, &sysfs(&link.name));
    let mut metered = None;
    if let Some(nm) = nm.filter(|nm| nm.interface == link.name) {
        if let Some(t) = nm_transport(&nm.kind) {
            transport = t;
        }
        metered = nm.metered;
    }
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (route.oif, &link.name, &route.gateway).hash(&mut h);
    Reading {
        online: true,
        transport,
        metered: metered.unwrap_or(transport == Transport::Cellular),
        constrained: false,
        network: h.finish() | 1,
    }
}

/// The routes and links in a dump's answers.
pub(crate) fn collect(buf: &[u8], routes: &mut Vec<Route>, links: &mut Vec<Link>) -> bool {
    let mut done = false;
    for (kind, payload) in messages(buf) {
        match kind {
            RTM_NEWROUTE => routes.extend(parse_route(payload)),
            RTM_NEWLINK => links.extend(parse_link(payload)),
            NLMSG_DONE | NLMSG_ERROR => done = true,
            _ => {}
        }
    }
    done
}

#[cfg(any(target_os = "linux", target_os = "fullrust"))]
#[cfg_attr(test, allow(unused_imports))]
pub(crate) use os::start;

#[cfg(all(any(target_os = "linux", target_os = "fullrust"), test))]
use os::read;

#[cfg(any(target_os = "linux", target_os = "fullrust"))]
#[allow(unsafe_code)]
#[cfg_attr(test, allow(dead_code))]
mod os {
    use super::*;
    use crate::platform::a11y::{DBus, put_string, read_string, read_u32};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    const AF_NETLINK: i32 = 16;
    const SOCK_RAW: i32 = 3;
    const SOCK_CLOEXEC: i32 = 0o2_000_000;
    const NETLINK_ROUTE: i32 = 0;
    const RTMGRP_LINK: u32 = 0x1;
    const RTMGRP_IPV4_ROUTE: u32 = 0x40;
    const RTMGRP_IPV6_ROUTE: u32 = 0x400;
    const POLLIN: i16 = 1;

    #[repr(C)]
    struct SockaddrNl {
        family: u16,
        pad: u16,
        pid: u32,
        groups: u32,
    }

    #[repr(C)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }

    unsafe extern "C" {
        fn socket(domain: i32, kind: i32, protocol: i32) -> i32;
        fn bind(fd: i32, addr: *const SockaddrNl, len: u32) -> i32;
        fn send(fd: i32, buf: *const u8, len: usize, flags: i32) -> isize;
        fn recv(fd: i32, buf: *mut u8, len: usize, flags: i32) -> isize;
        fn poll(fds: *mut PollFd, n: std::ffi::c_ulong, timeout: i32) -> i32;
    }

    /// A netlink route socket in `groups` (0: for requests only).
    fn open(groups: u32) -> Option<OwnedFd> {
        let fd = unsafe { socket(AF_NETLINK, SOCK_RAW | SOCK_CLOEXEC, NETLINK_ROUTE) };
        if fd < 0 {
            return None;
        }
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let addr = SockaddrNl {
            family: AF_NETLINK as u16,
            pad: 0,
            pid: 0,
            groups,
        };
        let size = std::mem::size_of::<SockaddrNl>() as u32;
        (unsafe { bind(fd.as_raw_fd(), &addr, size) } == 0).then_some(fd)
    }

    /// Send a dump request and gather what it answers.
    fn dump(fd: &OwnedFd, req: &[u8], routes: &mut Vec<Route>, links: &mut Vec<Link>) -> bool {
        if unsafe { send(fd.as_raw_fd(), req.as_ptr(), req.len(), 0) } < 0 {
            return false;
        }
        let mut buf = vec![0u8; 32 * 1024];
        loop {
            let n = unsafe { recv(fd.as_raw_fd(), buf.as_mut_ptr(), buf.len(), 0) };
            if n <= 0 {
                return false;
            }
            if collect(&buf[..n as usize], routes, links) {
                return true;
            }
        }
    }

    /// The kernel's default routes and links now.
    fn kernel() -> Option<(Vec<Route>, Vec<Link>)> {
        let fd = open(0)?;
        let (mut routes, mut links) = (Vec::new(), Vec::new());
        let ok = dump(
            &fd,
            &dump_request(RTM_GETROUTE, 1, AF_INET),
            &mut routes,
            &mut links,
        ) && dump(
            &fd,
            &dump_request(RTM_GETROUTE, 2, AF_INET6),
            &mut routes,
            &mut links,
        ) && dump(
            &fd,
            &dump_request(RTM_GETLINK, 3, 0),
            &mut routes,
            &mut links,
        );
        ok.then_some((routes, links))
    }

    /// A property of NetworkManager's (`path`, `interface`), as its variant's
    /// signature and bytes.
    fn property(
        bus: &mut DBus,
        path: &str,
        interface: &str,
        name: &str,
    ) -> Option<(String, Vec<u8>)> {
        let mut body = Vec::new();
        put_string(&mut body, interface);
        put_string(&mut body, name);
        let reply = bus
            .call_with_body(
                "org.freedesktop.NetworkManager",
                path,
                "org.freedesktop.DBus.Properties",
                "Get",
                "ss",
                &body,
            )
            .ok()?;
        let len = *reply.first()? as usize;
        let sig = std::str::from_utf8(reply.get(1..1 + len)?)
            .ok()?
            .to_string();
        Some((sig, reply))
    }

    /// The offset of a `v` reply's value.
    fn value_offset(reply: &[u8]) -> usize {
        reply.first().map_or(0, |&len| 1 + len as usize + 1)
    }

    fn string_property(bus: &mut DBus, path: &str, interface: &str, name: &str) -> Option<String> {
        let (sig, reply) = property(bus, path, interface, name)?;
        if sig != "s" && sig != "o" {
            return None;
        }
        read_string(&reply, &mut value_offset(&reply)).ok()
    }

    fn u32_property(bus: &mut DBus, path: &str, interface: &str, name: &str) -> Option<u32> {
        let (sig, reply) = property(bus, path, interface, name)?;
        if sig != "u" {
            return None;
        }
        read_u32(&reply, &mut value_offset(&reply)).ok()
    }

    /// The first object path of an `ao` property.
    fn first_path_property(
        bus: &mut DBus,
        path: &str,
        interface: &str,
        name: &str,
    ) -> Option<String> {
        let (sig, reply) = property(bus, path, interface, name)?;
        if sig != "ao" {
            return None;
        }
        let mut off = value_offset(&reply);
        let count = read_u32(&reply, &mut off).ok()?;
        (count > 0)
            .then(|| read_string(&reply, &mut off).ok())
            .flatten()
    }

    const NM: &str = "org.freedesktop.NetworkManager";
    const NM_PATH: &str = "/org/freedesktop/NetworkManager";

    /// NetworkManager's primary connection: its interface, kind and
    /// metering.
    fn nm_view(bus: &mut DBus) -> Option<NmView> {
        let primary = string_property(bus, NM_PATH, NM, "PrimaryConnection")?;
        if primary == "/" {
            return None;
        }
        let device = first_path_property(
            bus,
            &primary,
            "org.freedesktop.NetworkManager.Connection.Active",
            "Devices",
        )?;
        let interface = string_property(
            bus,
            &device,
            "org.freedesktop.NetworkManager.Device",
            "IpInterface",
        )
        .filter(|s| !s.is_empty())
        .or_else(|| {
            string_property(
                bus,
                &device,
                "org.freedesktop.NetworkManager.Device",
                "Interface",
            )
        })?;
        Some(NmView {
            interface,
            kind: string_property(bus, NM_PATH, NM, "PrimaryConnectionType").unwrap_or_default(),
            metered: u32_property(bus, NM_PATH, NM, "Metered").and_then(nm_metered),
        })
    }

    /// A connection to the system bus, if NetworkManager is on it.
    fn nm_bus() -> Option<DBus> {
        let addr = std::env::var("DBUS_SYSTEM_BUS_ADDRESS")
            .unwrap_or_else(|_| "unix:path=/run/dbus/system_bus_socket".into());
        let mut bus = DBus::connect_address(&addr).ok()?;
        // A bus or a NetworkManager that doesn't answer mustn't hold the
        // monitor up.
        bus.set_read_timeout(Some(std::time::Duration::from_millis(500)));
        string_property(&mut bus, NM_PATH, NM, "Version")?;
        Some(bus)
    }

    /// The network now, with NetworkManager's view when `bus` has one.
    fn read_with(bus: Option<&mut DBus>) -> Reading {
        let Some((routes, links)) = kernel() else {
            return Reading::OFFLINE;
        };
        let nm = bus.and_then(nm_view);
        reading(&routes, &links, Sysfs::read, nm.as_ref())
    }

    /// The network now.
    #[cfg(test)]
    pub(crate) fn read() -> Reading {
        read_with(nm_bus().as_mut())
    }

    /// Start the monitor thread.
    pub(crate) fn start() {
        // The first reading before returning, so `network()` has it at once.
        let mut bus = nm_bus();
        super::super::report(read_with(bus.as_mut()));
        let Some(watch) = open(RTMGRP_LINK | RTMGRP_IPV4_ROUTE | RTMGRP_IPV6_ROUTE) else {
            eprintln!("stipple: can't watch the network (netlink)");
            return;
        };
        if let Some(bus) = bus.as_mut() {
            let _ = bus.add_match(&format!(
                "type='signal',sender='{NM}',path='{NM_PATH}',\
                 interface='org.freedesktop.DBus.Properties',member='PropertiesChanged'"
            ));
        }
        let spawned = std::thread::Builder::new()
            .name("stipple-network".into())
            .spawn(move || watch_loop(watch, bus));
        if spawned.is_err() {
            eprintln!("stipple: can't start the network monitor thread");
        }
    }

    fn watch_loop(watch: OwnedFd, mut bus: Option<DBus>) {
        let mut buf = vec![0u8; 32 * 1024];
        loop {
            let mut fds = [
                PollFd {
                    fd: watch.as_raw_fd(),
                    events: POLLIN,
                    revents: 0,
                },
                PollFd {
                    fd: bus.as_ref().map_or(-1, |b| b.raw_fd()),
                    events: POLLIN,
                    revents: 0,
                },
            ];
            if unsafe { poll(fds.as_mut_ptr(), 2, -1) } < 0 {
                continue;
            }
            // Changes come in bursts (an address, then routes, then the
            // link): let the burst end, then read once.
            let mut heard = false;
            loop {
                let mut any = false;
                if fds[0].revents != 0 {
                    // Nothing to keep: the dump says it all. (ENOBUFS after
                    // an overflow is a change too.)
                    let _ = unsafe { recv(watch.as_raw_fd(), buf.as_mut_ptr(), buf.len(), 0) };
                    any = true;
                }
                if fds[1].revents != 0 {
                    match bus.as_mut().map(|b| b.read_message()) {
                        Some(Ok(m)) => any |= m.mtype == 4 && m.member == "PropertiesChanged",
                        // NetworkManager's bus went away: carry on with the kernel.
                        Some(Err(_)) => bus = None,
                        None => {}
                    }
                }
                heard |= any;
                fds[0].revents = 0;
                fds[1].revents = 0;
                fds[1].fd = bus.as_ref().map_or(-1, |b| b.raw_fd());
                if unsafe { poll(fds.as_mut_ptr(), 2, 150) } <= 0 {
                    break;
                }
            }
            if heard {
                super::super::report(read_with(bus.as_mut()));
            }
        }
    }
}

#[cfg(test)]
mod tests;
