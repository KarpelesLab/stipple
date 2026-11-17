//! Windows: IP Helper's route and interface notifications for the default
//! network, and its connectivity hint for whether it reaches the Internet
//! and what it costs.
//!
//! Why not `INetworkListManager`: its `ConnectivityChanged` events need a
//! COM sink in an apartment with a message loop, its connectivity flags
//! don't change when the default network moves from one connected network
//! to another (Wi-Fi to Ethernet, both "Internet"), which is the change an
//! app most needs, and its cost lives in yet another interface
//! (`INetworkCostManager`). IP Helper (`iphlpapi.dll`) calls plain C
//! callbacks on the system's thread pool instead:
//!
//! - `NotifyRouteChange2` and `NotifyIpInterfaceChange`, on every route and
//!   interface change (Vista and later). The default network is the
//!   interface `GetBestInterfaceEx` picks towards a public address (no
//!   packet is sent), IPv4 first, then IPv6; its transport is its
//!   `GetIfEntry2` type: Wi-Fi (`IF_TYPE_IEEE80211`), mobile broadband
//!   (`IF_TYPE_WWANPP`, `WWANPP2`), Ethernet, or other (tunnels, VPNs).
//! - `GetNetworkConnectivityHint` and `NotifyNetworkConnectivityHintChange`
//!   (Windows 10 2004 and later, looked up at run time): the system's own
//!   verdict on Internet access (what the taskbar shows) and the network's
//!   cost, which says metered (fixed or variable cost, roaming) and
//!   constrained (approaching or over the data limit the user set). Without
//!   it, online is "has a default route", and only mobile broadband counts
//!   as metered.

use super::{Reading, Transport};

// `NL_NETWORK_CONNECTIVITY_LEVEL_HINT`.
const LEVEL_UNKNOWN: i32 = 0;
const LEVEL_NONE: i32 = 1;
const LEVEL_LOCAL_ACCESS: i32 = 2;
// `NL_NETWORK_CONNECTIVITY_COST_HINT`.
const COST_FIXED: i32 = 2;
const COST_VARIABLE: i32 = 3;
// `IFTYPE`s.
const IF_TYPE_ETHERNET_CSMACD: u32 = 6;
const IF_TYPE_IEEE80211: u32 = 71;
const IF_TYPE_WWANPP: u32 = 243;
const IF_TYPE_WWANPP2: u32 = 244;

/// `NL_NETWORK_CONNECTIVITY_HINT`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Hint {
    pub level: i32,
    pub cost: i32,
    pub approaching_data_limit: u8,
    pub over_data_limit: u8,
    pub roaming: u8,
}

/// The transport of an interface of type `if_type`.
pub(crate) fn transport_of(if_type: u32) -> Transport {
    match if_type {
        IF_TYPE_IEEE80211 => Transport::Wifi,
        IF_TYPE_WWANPP | IF_TYPE_WWANPP2 => Transport::Cellular,
        IF_TYPE_ETHERNET_CSMACD => Transport::Ethernet,
        _ => Transport::Other,
    }
}

/// The reading for the best interface (its index and type, `None` with no
/// route out) and the connectivity hint, if the system has one.
pub(crate) fn reading(best: Option<(u32, u32)>, hint: Option<Hint>) -> Reading {
    let Some((index, if_type)) = best else {
        return Reading::OFFLINE;
    };
    let level = hint.map_or(LEVEL_UNKNOWN, |h| h.level);
    if matches!(level, LEVEL_NONE | LEVEL_LOCAL_ACCESS) {
        return Reading::OFFLINE;
    }
    let transport = transport_of(if_type);
    let metered = match hint {
        Some(h) if h.cost != 0 => matches!(h.cost, COST_FIXED | COST_VARIABLE) || h.roaming != 0,
        _ => transport == Transport::Cellular,
    };
    Reading {
        online: true,
        transport,
        metered,
        constrained: hint.is_some_and(|h| h.approaching_data_limit != 0 || h.over_data_limit != 0),
        network: ((index as u64) << 32 | if_type as u64) | 1 << 63,
    }
}

#[cfg(all(not(test), target_os = "windows"))]
pub(super) use os::start;

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod os {
    use super::{Hint, reading};
    use std::ffi::c_void;
    use std::sync::{Mutex, OnceLock};

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *const c_void;
    }

    type Handle = *mut c_void;
    type RouteCallback = unsafe extern "system" fn(*mut c_void, *mut c_void, i32);
    type HintCallback = unsafe extern "system" fn(*mut c_void, Hint);

    /// IP Helper's functions, looked up once.
    struct Api {
        best_interface_ex: unsafe extern "system" fn(*const u8, *mut u32) -> u32,
        get_if_entry2: unsafe extern "system" fn(*mut u8) -> u32,
        notify_route_change2:
            unsafe extern "system" fn(u16, RouteCallback, *mut c_void, u8, *mut Handle) -> u32,
        notify_ip_interface_change:
            unsafe extern "system" fn(u16, RouteCallback, *mut c_void, u8, *mut Handle) -> u32,
        /// Windows 10 2004 and later.
        get_hint: Option<unsafe extern "system" fn(*mut Hint) -> i32>,
        notify_hint:
            Option<unsafe extern "system" fn(HintCallback, *mut c_void, u8, *mut Handle) -> i32>,
    }

    // Each symbol's type is its field's.
    #[allow(clippy::missing_transmute_annotations)]
    fn api() -> Option<&'static Api> {
        static API: OnceLock<Option<Api>> = OnceLock::new();
        API.get_or_init(|| unsafe {
            let name: Vec<u16> = "iphlpapi.dll\0".encode_utf16().collect();
            let module = LoadLibraryW(name.as_ptr());
            if module.is_null() {
                return None;
            }
            let get = |name: &[u8]| GetProcAddress(module, name.as_ptr());
            macro_rules! f {
                ($name:literal) => {{
                    let p = get($name);
                    if p.is_null() {
                        return None;
                    }
                    std::mem::transmute::<*const c_void, _>(p)
                }};
            }
            let get_hint = get(b"GetNetworkConnectivityHint\0");
            let notify_hint = get(b"NotifyNetworkConnectivityHintChange\0");
            Some(Api {
                best_interface_ex: f!(b"GetBestInterfaceEx\0"),
                get_if_entry2: f!(b"GetIfEntry2\0"),
                notify_route_change2: f!(b"NotifyRouteChange2\0"),
                notify_ip_interface_change: f!(b"NotifyIpInterfaceChange\0"),
                get_hint: (!get_hint.is_null()).then(|| std::mem::transmute(get_hint)),
                notify_hint: (!notify_hint.is_null()).then(|| std::mem::transmute(notify_hint)),
            })
        })
        .as_ref()
    }

    // `MIB_IF_ROW2`: 1352 bytes, `InterfaceIndex` at 8, `Type` at 1128.
    const IF_ROW2_SIZE: usize = 1352;
    const IF_ROW2_INDEX: usize = 8;
    const IF_ROW2_TYPE: usize = 1128;
    const AF_INET: u16 = 2;
    const AF_INET6: u16 = 23;
    const AF_UNSPEC: u16 = 0;

    /// The interface traffic to the Internet leaves through, and its type.
    fn best_interface(api: &Api) -> Option<(u32, u32)> {
        // `sockaddr_in` to 8.8.8.8, then `sockaddr_in6` to 2001:4860::8888:
        // only the routing table is consulted.
        let mut v4 = [0u8; 16];
        v4[..2].copy_from_slice(&AF_INET.to_ne_bytes());
        v4[4..8].copy_from_slice(&[8, 8, 8, 8]);
        let mut v6 = [0u8; 28];
        v6[..2].copy_from_slice(&AF_INET6.to_ne_bytes());
        v6[8..24].copy_from_slice(&[
            0x20, 0x01, 0x48, 0x60, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x88, 0x88,
        ]);
        let mut index = 0u32;
        let found = [&v4[..], &v6[..]]
            .iter()
            .any(|addr| unsafe { (api.best_interface_ex)(addr.as_ptr(), &mut index) } == 0);
        if !found || index == 0 {
            return None;
        }
        // 8-aligned, as the struct is.
        let mut row = vec![0u64; IF_ROW2_SIZE / 8];
        let bytes = row.as_mut_ptr().cast::<u8>();
        unsafe {
            bytes.add(IF_ROW2_INDEX).cast::<u32>().write(index);
            if (api.get_if_entry2)(bytes) != 0 {
                return Some((index, 0));
            }
            Some((index, bytes.add(IF_ROW2_TYPE).cast::<u32>().read()))
        }
    }

    fn hint(api: &Api) -> Option<Hint> {
        let get = api.get_hint?;
        let mut h = Hint::default();
        (unsafe { get(&mut h) } == 0).then_some(h)
    }

    /// Read the network and report it. One at a time: the callbacks come on
    /// several pool threads, and the last to read must be the last to
    /// report.
    fn refresh() {
        static ONE: Mutex<()> = Mutex::new(());
        let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(api) = api() {
            super::super::report(reading(best_interface(api), hint(api)));
        }
    }

    unsafe extern "system" fn changed(_: *mut c_void, _: *mut c_void, _: i32) {
        refresh();
    }

    unsafe extern "system" fn hint_changed(_: *mut c_void, _: Hint) {
        refresh();
    }

    /// Read the network, then register for its changes.
    pub(crate) fn start() {
        let Some(api) = api() else {
            eprintln!("stipple: IP Helper isn't available: can't watch the network");
            return;
        };
        refresh();
        // The handles live as long as the process.
        let mut handle: Handle = std::ptr::null_mut();
        let null = std::ptr::null_mut();
        unsafe {
            let mut ok = (api.notify_route_change2)(AF_UNSPEC, changed, null, 0, &mut handle) == 0;
            ok &= (api.notify_ip_interface_change)(AF_UNSPEC, changed, null, 0, &mut handle) == 0;
            if let Some(notify) = api.notify_hint {
                ok &= notify(hint_changed, null, 0, &mut handle) == 0;
            }
            if !ok {
                eprintln!("stipple: can't register for all network notifications");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interface_types() {
        assert_eq!(transport_of(71), Transport::Wifi);
        assert_eq!(transport_of(243), Transport::Cellular);
        assert_eq!(transport_of(6), Transport::Ethernet);
        // A tunnel (a VPN's adapter).
        assert_eq!(transport_of(131), Transport::Other);
    }

    #[test]
    fn the_hint_decides_online_and_cost() {
        let wifi = Some((12, IF_TYPE_IEEE80211));
        // No hint (before Windows 10 2004): a route out is online.
        let r = reading(wifi, None);
        assert!(r.online && !r.metered && !r.constrained);
        assert_eq!(r.transport, Transport::Wifi);
        assert!(reading(Some((3, IF_TYPE_WWANPP)), None).metered);
        // Internet access, on a metered hotspot near its limit.
        let h = Hint {
            level: 3,
            cost: COST_FIXED,
            approaching_data_limit: 1,
            ..Hint::default()
        };
        let r = reading(wifi, Some(h));
        assert!(r.online && r.metered && r.constrained);
        // Unrestricted, but roaming.
        let roaming = Hint {
            level: 3,
            cost: 1,
            roaming: 1,
            ..Hint::default()
        };
        assert!(reading(wifi, Some(roaming)).metered);
        // Local access only: offline, whatever the route.
        let local = Hint {
            level: LEVEL_LOCAL_ACCESS,
            ..Hint::default()
        };
        assert_eq!(reading(wifi, Some(local)), Reading::OFFLINE);
        assert_eq!(reading(None, Some(h)), Reading::OFFLINE);
        // Another interface is another network.
        let eth = reading(Some((4, IF_TYPE_ETHERNET_CSMACD)), Some(h));
        assert_ne!(eth.network, reading(wifi, Some(h)).network);
    }
}
