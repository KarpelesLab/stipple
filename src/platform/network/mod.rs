//! Network connectivity: whether the device is online, over what, whether
//! the network costs money or the user asked to save data, and the changes,
//! delivered as [`Event::Network`](crate::platform::Event::Network) (see
//! [`App::on_network`](crate::App::on_network)). See `docs/network.md`.
//!
//! Each platform watches its default network on its own thread or callback
//! (Network.framework's `nw_path_monitor` on macOS and iOS, a
//! `ConnectivityManager.NetworkCallback` on Android, route and connectivity
//! hint notifications on Windows, netlink and NetworkManager on Linux, the
//! page's `online`/`offline` events on the web) and hands each reading to
//! [`report`], which tells a change from a repeat, numbers the default
//! networks ([`NetworkStatus::generation`]) and posts the event.

use std::sync::{Condvar, Mutex};

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) mod android_caps;
#[cfg(any(target_os = "macos", target_os = "ios"))]
mod apple;
#[cfg(any(target_os = "linux", test))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) mod linux;
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows;

/// What carries the default network's traffic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Transport {
    /// Wi-Fi.
    Wifi,
    /// A cellular (mobile data) connection.
    Cellular,
    /// A wired connection.
    Ethernet,
    /// Anything else (a VPN or other tunnel that hides what it runs over,
    /// Bluetooth tethering, a virtual interface), or nothing: what an
    /// offline status reports.
    Other,
}

/// The state of the default network, from [`network`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct NetworkStatus {
    /// Whether there is a network to reach the Internet through (a default
    /// route, a "satisfied" path): no promise that a given server answers,
    /// or that a captive portal isn't in the way.
    pub online: bool,
    /// What the default network runs over ([`Transport::Other`] offline).
    pub transport: Transport,
    /// Whether the user pays for what's sent (cellular data, a hotspot, a
    /// network marked as metered): defer large downloads and backups.
    pub metered: bool,
    /// Whether the user asked apps to use less data: Low Data Mode on iOS
    /// and macOS, Data Saver on Android (while it restricts this app),
    /// `saveData` in a browser, a network over its data limit on Windows.
    pub constrained: bool,
    /// The default network's number, which changes each time the default
    /// network does (Wi-Fi to cellular, a cable plugged in, going offline
    /// and back) even when nothing else here does: a socket opened on the
    /// previous network may be dead, so reconnect when it changes. Starts at
    /// 1 (0 before anything was heard).
    pub generation: u64,
}

impl NetworkStatus {
    /// A status, for tests that feed an app's
    /// [`on_network`](crate::App::on_network) hook by hand: not metered, not
    /// constrained, generation 1.
    pub fn new(online: bool, transport: Transport) -> Self {
        Self {
            online,
            transport,
            metered: false,
            constrained: false,
            generation: 1,
        }
    }

    /// The same status, metered or not.
    pub fn metered(mut self, metered: bool) -> Self {
        self.metered = metered;
        self
    }

    /// The same status, constrained or not.
    pub fn constrained(mut self, constrained: bool) -> Self {
        self.constrained = constrained;
        self
    }

    /// The same status with another generation.
    pub fn generation(mut self, generation: u64) -> Self {
        self.generation = generation;
        self
    }

    /// What's reported where the platform can't tell (no backend, the
    /// monitor failed to start): online, over something unknown, so an app
    /// doesn't show itself offline for want of an API.
    const UNKNOWN: NetworkStatus = NetworkStatus {
        online: true,
        transport: Transport::Other,
        metered: false,
        constrained: false,
        generation: 0,
    };
}

/// One reading of a platform's default network.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Reading {
    pub online: bool,
    pub transport: Transport,
    pub metered: bool,
    pub constrained: bool,
    /// What identifies the default network on this platform (an interface
    /// index and gateway, a network handle; `0` offline): a new value is a
    /// new network.
    pub network: u64,
}

impl Reading {
    /// No network.
    pub(crate) const OFFLINE: Reading = Reading {
        online: false,
        transport: Transport::Other,
        metered: false,
        constrained: false,
        network: 0,
    };
}

/// Tells a change from a repeat and numbers the networks.
#[derive(Debug, Default)]
pub(crate) struct Tracker {
    last: Option<Reading>,
    status: Option<NetworkStatus>,
}

impl Tracker {
    /// Take `reading`: the new status if anything in it changed.
    pub(crate) fn take(&mut self, reading: Reading) -> Option<NetworkStatus> {
        let mut r = reading;
        if !r.online {
            r = Reading::OFFLINE;
        }
        if self.last == Some(r) {
            return None;
        }
        let prev = self.status.map_or(0, |s| s.generation);
        let new_network = self
            .last
            .is_none_or(|l| l.network != r.network || l.online != r.online);
        let status = NetworkStatus {
            online: r.online,
            transport: r.transport,
            metered: r.metered,
            constrained: r.constrained,
            generation: if new_network { prev + 1 } else { prev },
        };
        self.last = Some(r);
        let changed = self.status != Some(status);
        self.status = Some(status);
        changed.then_some(status)
    }

    fn status(&self) -> Option<NetworkStatus> {
        self.status
    }
}

struct Shared {
    tracker: Mutex<Tracker>,
    #[cfg_attr(test, allow(dead_code))]
    /// Signalled on each reading, for [`network`]'s first wait.
    heard: Condvar,
}

fn shared() -> &'static Shared {
    static SHARED: Shared = Shared {
        tracker: Mutex::new(Tracker {
            last: None,
            status: None,
        }),
        heard: Condvar::new(),
    };
    &SHARED
}

/// Take a platform's `reading`; returns the new status when it changed,
/// without posting it (the web delivers it itself). The first reading is the
/// baseline, not a change: `None`.
#[cfg_attr(test, allow(dead_code))]
pub(crate) fn note(reading: Reading) -> Option<NetworkStatus> {
    let s = shared();
    let mut tracker = s.tracker.lock().unwrap_or_else(|e| e.into_inner());
    let first = tracker.status().is_none();
    let changed = tracker.take(reading).filter(|_| !first);
    drop(tracker);
    s.heard.notify_all();
    changed
}

/// Take a platform's `reading`, from any thread, and post
/// [`Event::Network`](crate::platform::Event::Network) when it changed.
#[cfg_attr(any(test, target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn report(reading: Reading) {
    if let Some(status) = note(reading) {
        crate::platform::shell::post(crate::platform::Event::Network(status));
    }
}

/// Start watching the network, once per process (again later where the
/// first try found nothing to watch yet, as on Android before the activity).
pub(crate) fn start() {
    // Held while starting, so two threads can't start two monitors.
    static STARTED: Mutex<bool> = Mutex::new(false);
    let mut started = STARTED.lock().unwrap_or_else(|e| e.into_inner());
    if !*started {
        *started = platform_start();
    }
}

/// Start the platform's monitor; whether it's running (or there's none to
/// run, and never will be).
fn platform_start() -> bool {
    #[cfg(test)]
    {
        true
    }
    #[cfg(all(not(test), target_os = "linux"))]
    {
        linux::start();
        true
    }
    #[cfg(all(not(test), any(target_os = "macos", target_os = "ios")))]
    {
        apple::start();
        true
    }
    #[cfg(all(not(test), target_os = "windows"))]
    {
        windows::start();
        true
    }
    #[cfg(all(not(test), target_os = "android"))]
    {
        crate::platform::backend::android::network::start()
    }
    // The page reports on its own (web/stipple.js), and elsewhere there's
    // nothing to watch.
    #[cfg(all(
        not(test),
        not(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "ios",
            target_os = "windows",
            target_os = "android"
        ))
    ))]
    {
        true
    }
}

/// The default network's state now: online or not, over what, metered,
/// constrained, and its [generation](NetworkStatus::generation).
///
/// The first call starts watching the network (as
/// [`App::on_network`](crate::App::on_network) does); from then on the
/// event loop delivers [`Event::Network`](crate::platform::Event::Network)
/// on each change and an [`App`](crate::App) rebuilds its views, so a view
/// may read this every time it's built. It may wait a moment for the
/// platform's first answer, once. Where the platform can't tell, it reports
/// online over [`Transport::Other`], generation 0. Safe from any thread.
///
/// ```no_run
/// use stipple::platform::{network, Transport};
/// let net = network();
/// if !net.online {
///     println!("offline");
/// } else if net.metered || net.transport == Transport::Cellular {
///     println!("on mobile data: no auto-play");
/// }
/// ```
pub fn network() -> NetworkStatus {
    start();
    let s = shared();
    #[cfg_attr(any(test, target_arch = "wasm32"), allow(unused_mut))]
    let mut tracker = s.tracker.lock().unwrap_or_else(|e| e.into_inner());
    // The page reports before the app starts; elsewhere, the platform's
    // first answer comes from its own thread (a dispatch queue, a binder
    // thread), usually within milliseconds: wait for it once, not on every
    // call.
    #[cfg(not(any(test, target_arch = "wasm32")))]
    if tracker.status().is_none() {
        static WAITED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !WAITED.swap(true, std::sync::atomic::Ordering::AcqRel) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
            while tracker.status().is_none() {
                let now = std::time::Instant::now();
                if now >= deadline {
                    break;
                }
                tracker = s
                    .heard
                    .wait_timeout(tracker, deadline - now)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
        }
    }
    tracker.status().unwrap_or(NetworkStatus::UNKNOWN)
}

#[cfg(test)]
mod tests;
