use super::*;

/// An `rtattr`, padded.
fn attr(kind: u16, value: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&((4 + value.len()) as u16).to_ne_bytes());
    out.extend_from_slice(&kind.to_ne_bytes());
    out.extend_from_slice(value);
    while out.len() % 4 != 0 {
        out.push(0);
    }
    out
}

/// A netlink message of `kind` around `payload`.
fn message(kind: u16, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&((NLMSG_HDR + payload.len()) as u32).to_ne_bytes());
    out.extend_from_slice(&kind.to_ne_bytes());
    out.extend_from_slice(&2u16.to_ne_bytes()); // NLM_F_MULTI
    out.extend_from_slice(&1u32.to_ne_bytes());
    out.extend_from_slice(&0u32.to_ne_bytes());
    out.extend_from_slice(payload);
    while out.len() % 4 != 0 {
        out.push(0);
    }
    out
}

/// An `rtmsg` and its attributes.
fn route(family: u8, dst_len: u8, table: u8, kind: u8, flags: u32, attrs: &[Vec<u8>]) -> Vec<u8> {
    let mut out = vec![family, dst_len, 0, 0, table, 3, 0, kind];
    out.extend_from_slice(&flags.to_ne_bytes());
    for a in attrs {
        out.extend_from_slice(a);
    }
    out
}

/// An `ifinfomsg` and its attributes.
fn link(index: u32, hw: u16, flags: u32, name: &str, kind: Option<&str>) -> Vec<u8> {
    let mut out = vec![0, 0];
    out.extend_from_slice(&hw.to_ne_bytes());
    out.extend_from_slice(&index.to_ne_bytes());
    out.extend_from_slice(&flags.to_ne_bytes());
    out.extend_from_slice(&0xffff_ffffu32.to_ne_bytes());
    let mut n = name.as_bytes().to_vec();
    n.push(0);
    out.extend_from_slice(&attr(IFLA_IFNAME, &n));
    if let Some(k) = kind {
        out.extend_from_slice(&attr(IFLA_LINKINFO, &attr(IFLA_INFO_KIND, k.as_bytes())));
    }
    out
}

const UP: u32 = IFF_UP | IFF_RUNNING | IFF_LOWER_UP;

fn u32b(v: u32) -> Vec<u8> {
    v.to_ne_bytes().to_vec()
}

#[test]
fn a_dump_request_is_a_header_and_an_empty_body() {
    let r = dump_request(RTM_GETROUTE, 7, AF_INET6);
    assert_eq!(r.len(), NLMSG_HDR + RTMSG);
    assert_eq!(u32_at(&r, 0), Some(r.len() as u32));
    assert_eq!(u16_at(&r, 4), Some(RTM_GETROUTE));
    assert_eq!(u16_at(&r, 6), Some(NLM_F_REQUEST | NLM_F_DUMP));
    assert_eq!(u32_at(&r, 8), Some(7));
    assert_eq!(r[NLMSG_HDR], AF_INET6);
    assert_eq!(dump_request(RTM_GETLINK, 1, 0).len(), NLMSG_HDR + IFINFOMSG);
}

#[test]
fn a_default_route_is_parsed() {
    let p = route(
        AF_INET,
        0,
        254,
        RTN_UNICAST,
        0,
        &[
            attr(RTA_TABLE, &u32b(254)),
            attr(RTA_GATEWAY, &[10, 0, 0, 1]),
            attr(RTA_OIF, &u32b(3)),
            attr(RTA_PRIORITY, &u32b(600)),
        ],
    );
    let r = parse_route(&p).unwrap();
    assert_eq!(r.oif, 3);
    assert_eq!(r.gateway, [10, 0, 0, 1]);
    assert_eq!(r.priority, 600);
}

#[test]
fn other_routes_are_not_default_ones() {
    let oif = attr(RTA_OIF, &u32b(3));
    // A subnet route.
    assert_eq!(
        parse_route(&route(
            AF_INET,
            24,
            254,
            RTN_UNICAST,
            0,
            std::slice::from_ref(&oif)
        )),
        None
    );
    // Another table's default (wg-quick's, a VPN's policy table).
    assert_eq!(
        parse_route(&route(
            AF_INET,
            0,
            252,
            RTN_UNICAST,
            0,
            &[oif.clone(), attr(RTA_TABLE, &u32b(51820))]
        )),
        None
    );
    // An unreachable default (type 7).
    assert_eq!(
        parse_route(&route(AF_INET6, 0, 254, 7, 0, std::slice::from_ref(&oif))),
        None
    );
    // A route the kernel keeps over a link that's down.
    assert_eq!(
        parse_route(&route(AF_INET6, 0, 254, RTN_UNICAST, 16, &[oif])),
        None
    );
}

#[test]
fn a_multipath_default_takes_its_first_live_hop() {
    let mut hops = Vec::new();
    for (flags, ifindex, gw) in [(1u8, 4u32, [10, 0, 0, 9]), (0, 5, [10, 0, 1, 1])] {
        let gw = attr(RTA_GATEWAY, &gw);
        hops.extend_from_slice(&((8 + gw.len()) as u16).to_ne_bytes());
        hops.push(flags);
        hops.push(0);
        hops.extend_from_slice(&ifindex.to_ne_bytes());
        hops.extend_from_slice(&gw);
    }
    let p = route(
        AF_INET,
        0,
        254,
        RTN_UNICAST,
        0,
        &[attr(RTA_MULTIPATH, &hops)],
    );
    let r = parse_route(&p).unwrap();
    assert_eq!((r.oif, r.gateway.as_slice()), (5, &[10, 0, 1, 1][..]));
}

#[test]
fn links_are_parsed_with_their_kind() {
    let l = parse_link(&link(2, ARPHRD_ETHER, UP, "dummy0", Some("dummy"))).unwrap();
    assert_eq!(l.index, 2);
    assert_eq!(l.name, "dummy0");
    assert_eq!(l.kind.as_deref(), Some("dummy"));
    assert!(l.usable());
    let down = parse_link(&link(2, ARPHRD_ETHER, IFF_UP, "eth0", None)).unwrap();
    assert_eq!(down.kind, None);
    assert!(!down.usable(), "no carrier");
}

#[test]
fn a_dump_is_split_into_messages_and_ends_at_done() {
    let mut buf = message(
        RTM_NEWROUTE,
        &route(AF_INET, 0, 254, RTN_UNICAST, 0, &[attr(RTA_OIF, &u32b(2))]),
    );
    buf.extend(message(
        RTM_NEWLINK,
        &link(2, ARPHRD_ETHER, UP, "eth0", None),
    ));
    let (mut routes, mut links) = (Vec::new(), Vec::new());
    assert!(!collect(&buf, &mut routes, &mut links), "no NLMSG_DONE yet");
    buf.extend(message(NLMSG_DONE, &0u32.to_ne_bytes()));
    let (mut routes2, mut links2) = (Vec::new(), Vec::new());
    assert!(collect(&buf, &mut routes2, &mut links2));
    assert_eq!((routes2.len(), links2.len()), (1, 1));
    // A truncated message stops the walk without panicking.
    assert!(messages(&buf[..buf.len() - 3]).len() <= 3);
    assert!(messages(&[1, 2, 3]).is_empty());
}

fn no_sysfs(_: &str) -> Sysfs {
    Sysfs::default()
}

#[test]
fn the_lowest_metric_over_a_usable_link_wins() {
    let links = vec![
        parse_link(&link(2, ARPHRD_ETHER, UP, "eth0", None)).unwrap(),
        parse_link(&link(3, ARPHRD_ETHER, UP, "wlan0", None)).unwrap(),
        parse_link(&link(4, ARPHRD_ETHER, IFF_UP, "eth1", None)).unwrap(),
    ];
    let r = |oif, priority| Route {
        family: AF_INET,
        oif,
        gateway: vec![10, 0, 0, oif as u8],
        priority,
    };
    let routes = vec![r(3, 600), r(2, 100), r(4, 10)];
    assert_eq!(best_default(&routes, &links).unwrap().oif, 2);
    let wifi = |name: &str| Sysfs {
        devtype: (name == "wlan0").then(|| "wlan".into()),
        wireless: false,
    };
    let wired = reading(&routes, &links, wifi, None);
    assert!(wired.online);
    assert_eq!(wired.transport, Transport::Ethernet);
    assert!(!wired.metered);
    // The cable comes out: Wi-Fi takes over, still online, a new network.
    let routes = vec![r(3, 600), r(4, 10)];
    let wireless = reading(&routes, &links, wifi, None);
    assert!(wireless.online);
    assert_eq!(wireless.transport, Transport::Wifi);
    assert_ne!(wireless.network, wired.network);
    // No default route over a usable link: offline.
    assert_eq!(
        reading(&[r(4, 10)], &links, no_sysfs, None),
        Reading::OFFLINE
    );
    assert_eq!(reading(&[], &links, no_sysfs, None), Reading::OFFLINE);
}

#[test]
fn transports_from_the_kernel() {
    let l = |hw, name: &str, kind: Option<&str>| Link {
        kind_hw: hw,
        name: name.into(),
        kind: kind.map(Into::into),
        ..Link::default()
    };
    let none = Sysfs::default();
    assert_eq!(
        transport_of(&l(ARPHRD_ETHER, "eno1", None), &none),
        Transport::Ethernet
    );
    assert_eq!(
        transport_of(&l(ARPHRD_RAWIP, "wwan0", None), &none),
        Transport::Cellular
    );
    assert_eq!(
        transport_of(&l(ARPHRD_NONE, "wwan0", None), &none),
        Transport::Cellular
    );
    assert_eq!(
        transport_of(&l(ARPHRD_NONE, "wg0", Some("wireguard")), &none),
        Transport::Other
    );
    assert_eq!(
        transport_of(&l(ARPHRD_ETHER, "eth0", Some("veth")), &none),
        Transport::Ethernet
    );
    assert_eq!(
        transport_of(&l(ARPHRD_ETHER, "dummy0", Some("dummy")), &none),
        Transport::Other
    );
    assert_eq!(transport_of(&l(512, "ppp0", None), &none), Transport::Other);
    let wlan = Sysfs {
        devtype: Some("wlan".into()),
        wireless: false,
    };
    assert_eq!(
        transport_of(&l(ARPHRD_ETHER, "wlp2s0", None), &wlan),
        Transport::Wifi
    );
    let wwan = Sysfs {
        devtype: Some("wwan".into()),
        wireless: false,
    };
    assert_eq!(
        transport_of(&l(ARPHRD_ETHER, "usb0", None), &wwan),
        Transport::Cellular
    );
}

#[test]
fn network_manager_is_believed_for_its_own_interface() {
    let links = vec![parse_link(&link(2, ARPHRD_ETHER, UP, "wlp2s0", None)).unwrap()];
    let routes = vec![Route {
        family: AF_INET,
        oif: 2,
        gateway: vec![192, 168, 1, 1],
        priority: 600,
    }];
    // A phone's hotspot: Wi-Fi, metered (guessed).
    let nm = NmView {
        interface: "wlp2s0".into(),
        kind: "802-11-wireless".into(),
        metered: nm_metered(3),
    };
    let r = reading(&routes, &links, no_sysfs, Some(&nm));
    assert_eq!(r.transport, Transport::Wifi);
    assert!(r.metered);
    // NetworkManager describing another interface (another namespace's).
    let other = NmView {
        interface: "eno1".into(),
        ..nm
    };
    let r = reading(&routes, &links, no_sysfs, Some(&other));
    assert_eq!(r.transport, Transport::Ethernet);
    assert!(!r.metered);
}

#[test]
fn network_manager_values() {
    assert_eq!(nm_transport("802-11-wireless"), Some(Transport::Wifi));
    assert_eq!(nm_transport("gsm"), Some(Transport::Cellular));
    assert_eq!(nm_transport("802-3-ethernet"), Some(Transport::Ethernet));
    assert_eq!(nm_transport("wireguard"), Some(Transport::Other));
    assert_eq!(nm_transport(""), None);
    assert_eq!(nm_metered(0), None);
    assert_eq!(nm_metered(1), Some(true));
    assert_eq!(nm_metered(2), Some(false));
    assert_eq!(nm_metered(4), Some(false));
}

/// The real system answers a dump without the parser choking (what it
/// answers depends on the machine).
#[cfg(target_os = "linux")]
#[test]
fn the_kernel_answers() {
    let r = read();
    if !r.online {
        assert_eq!(r, Reading::OFFLINE);
    }
}
