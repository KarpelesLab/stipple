//! What Android's `NetworkCapabilities` of the default network make of a
//! [`Reading`] (the JNI side is in the Android backend's `network`; this is
//! plain Rust, tested on every host).

use super::{Reading, Transport};

// `NetworkCapabilities.TRANSPORT_*` and `NET_CAPABILITY_*`.
pub(crate) const TRANSPORT_CELLULAR: i32 = 0;
pub(crate) const TRANSPORT_WIFI: i32 = 1;
pub(crate) const TRANSPORT_ETHERNET: i32 = 3;
pub(crate) const CAPABILITY_NOT_METERED: i32 = 11;
pub(crate) const CAPABILITY_INTERNET: i32 = 12;
pub(crate) const CAPABILITY_TEMPORARILY_NOT_METERED: i32 = 25;
/// `ConnectivityManager.RESTRICT_BACKGROUND_STATUS_ENABLED`: Data Saver is
/// on and the app isn't exempt.
pub(crate) const RESTRICT_BACKGROUND_STATUS_ENABLED: i32 = 3;

/// The capabilities read: which transports and capabilities it has.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Caps {
    pub wifi: bool,
    pub cellular: bool,
    pub ethernet: bool,
    pub internet: bool,
    pub not_metered: bool,
    /// API 30: a 5G plan's unmetered spell.
    pub temporarily_not_metered: bool,
}

/// The reading for the default network's `caps` (`None`: no default
/// network), its `Network.getNetworkHandle()` and
/// `getRestrictBackgroundStatus()`.
pub(crate) fn reading(caps: Option<Caps>, handle: i64, restrict_background: i32) -> Reading {
    let Some(c) = caps.filter(|c| c.internet) else {
        return Reading::OFFLINE;
    };
    // A VPN's network carries its underlying transports too (from API 28);
    // one that hides them is Other.
    let transport = if c.wifi {
        Transport::Wifi
    } else if c.cellular {
        Transport::Cellular
    } else if c.ethernet {
        Transport::Ethernet
    } else {
        Transport::Other
    };
    Reading {
        online: true,
        transport,
        metered: !(c.not_metered || c.temporarily_not_metered),
        constrained: restrict_background == RESTRICT_BACKGROUND_STATUS_ENABLED,
        network: (handle as u64) | 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_make_a_reading() {
        let wifi = Caps {
            wifi: true,
            internet: true,
            not_metered: true,
            ..Caps::default()
        };
        let r = reading(Some(wifi), 432, 1);
        assert!(r.online && !r.metered && !r.constrained);
        assert_eq!(r.transport, Transport::Wifi);
        let cell = Caps {
            cellular: true,
            internet: true,
            ..Caps::default()
        };
        let r = reading(Some(cell), 812, RESTRICT_BACKGROUND_STATUS_ENABLED);
        assert!(r.online && r.metered && r.constrained);
        assert_eq!(r.transport, Transport::Cellular);
        assert_ne!(r.network, reading(Some(wifi), 432, 1).network);
        // Unmetered 5G for now.
        let five_g = Caps {
            temporarily_not_metered: true,
            ..cell
        };
        assert!(!reading(Some(five_g), 812, 1).metered);
        // A network without Internet (a device's own Wi-Fi), or none.
        let local = Caps {
            internet: false,
            ..wifi
        };
        assert_eq!(reading(Some(local), 432, 1), Reading::OFFLINE);
        assert_eq!(reading(None, 0, 1), Reading::OFFLINE);
    }
}
