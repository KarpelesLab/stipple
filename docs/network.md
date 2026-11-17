# Network status

Whether the device is online, over what, whether the network costs money or
the user asked apps to save data, and each change as it comes: for an
"offline" banner, pausing large downloads on a metered network, and
reconnecting when the phone moves from Wi-Fi to cellular.

```rust,no_run
use stipple::platform::{self, NetworkStatus, Transport};
use stipple::prelude::*;

#[derive(Default)]
struct State {
    generation: u64,
}

fn view(_: &State, cx: &mut Cx<State>) -> Element {
    // Read on every build: the App builds the views again on each change.
    let net = platform::network();
    if !net.online {
        return label(cx, "Offline");
    }
    let over = if net.transport == Transport::Cellular { "mobile data" } else { "the network" };
    label(cx, format!("Online over {over}{}", if net.metered { " (metered)" } else { "" }))
}

fn main() {
    App::new(State::default(), view)
        .on_network(|s, net: &NetworkStatus| {
            if net.online && net.generation != s.generation {
                // Another default network: sockets opened on the old one
                // may be dead. Reconnect.
                s.generation = net.generation;
            }
        })
        .run();
}
```

## The status

`platform::network()` returns a `NetworkStatus`:

| Field | What it says |
|---|---|
| `online` | There is a network to reach the Internet through: a default route, a "satisfied" path. Not a promise that a given server answers, or that no captive portal is in the way. |
| `transport` | `Wifi`, `Cellular`, `Ethernet` or `Other` (a tunnel or VPN that hides what it runs over, Bluetooth tethering, a virtual link; and offline). `#[non_exhaustive]`. |
| `metered` | The user pays for what's sent: cellular data, a phone's hotspot, a network they marked as metered. Defer large downloads and backups. |
| `constrained` | The user asked apps to use less data: Low Data Mode (iOS, macOS), Data Saver while it restricts the app (Android), `saveData` (browsers), a network approaching or over its data limit (Windows). |
| `generation` | The default network's number. It changes each time the default network does, also when nothing else here changes: Wi-Fi to cellular, a cable plugged in, one Wi-Fi network to another, offline and back. Starts at 1; 0 before anything was heard, or where the platform can't tell. |

The first call starts watching the network (as `App::on_network` does) and
may wait a moment, once, for the platform's first answer. Where the platform
can't tell (no backend, the monitor failed to start), it reports online
over `Other`, generation 0, so an app doesn't show itself offline for want
of an API. It's safe from any thread.

`NetworkStatus::new(online, transport)` with `.metered()`, `.constrained()`
and `.generation()` builds one for an app's tests.

## Changes

Once the network is watched, each change comes to the event loop as
`Event::Network(status)`, and an `App`:

- calls the `App::on_network` hook with the new status, and
- builds its views again, so a view that reads `platform::network()` shows
  the change without a hook.

The status at start isn't a change: read it with `platform::network()`.

A change of the default network while staying online is what breaks open
connections (a socket bound to the Wi-Fi address after the phone moved to
cellular), and it's the one a plain online flag misses: compare
`generation` with the last one seen and reconnect when it differs.

## On each platform

| | How | Online | Transport | Metered | Constrained |
|---|---|---|---|---|---|
| macOS, iOS | Network.framework's `nw_path_monitor` (C API, loaded at run time), on a dispatch queue of its own | path `satisfied` | `nw_path_uses_interface_type` | `nw_path_is_expensive` | `nw_path_is_constrained` (Low Data Mode; macOS 10.15, iOS 13) |
| Android | `ConnectivityManager.registerDefaultNetworkCallback` (API 24), a `NetworkCallback` subclass Stipple assembles at run time | `NET_CAPABILITY_INTERNET` | `hasTransport` (a VPN carries its underlying transports from API 28) | no `NOT_METERED` (nor `TEMPORARILY_NOT_METERED`) | `getRestrictBackgroundStatus()` is `ENABLED` (Data Saver on, the app not exempt) |
| Windows | IP Helper: `NotifyRouteChange2`, `NotifyIpInterfaceChange`, and `NotifyNetworkConnectivityHintChange` (Windows 10 2004) | a route out (`GetBestInterfaceEx`), and the connectivity hint not "none" or "local access" | `GetIfEntry2`'s interface type | the hint's cost (fixed or variable) or roaming; without the hint, mobile broadband | the hint's approaching or over the data limit |
| Linux | netlink (`NETLINK_ROUTE`: links and IPv4/IPv6 routes), and NetworkManager over D-Bus when it runs | a default route in the main table over a link that's up | sysfs (`DEVTYPE=wlan`/`wwan`), else the link type; NetworkManager's `PrimaryConnectionType` for its own interface | NetworkManager's `Metered` (yes or guessed yes); without it, cellular | never: Linux has no system-wide data saver |
| Web | `navigator.onLine` with the `online`/`offline` events; `navigator.connection` where the browser has it (Chrome on Android) | `navigator.onLine` | `connection.type` | cellular | `connection.saveData` |

Notes:

- **Android** needs `android.permission.ACCESS_NETWORK_STATE` in the
  manifest: `network = true` in `cargo-stipple`'s `[android]` table adds it
  (see [packaging.md](packaging.md)). Without it, registering fails and the
  log says so. The callback is registered on the application context, once
  per process. Android reports the Wi-Fi network lost a moment before
  cellular takes over, so a lost default network waits a second for its
  replacement before it's reported offline: Wi-Fi to cellular is one change,
  not an offline flash. Before Android 7 (API 24) the network isn't watched.
  Data Saver switched on or off is heard through a receiver of
  `ACTION_RESTRICT_BACKGROUND_CHANGED` (`org.stipple.android.NetworkPolicy`,
  from the same dex); on Android 11 turning it on comes as a brief offline
  first (the network blocked, then back).
- **Windows**: `INetworkListManager` was the other candidate. Its events
  need a COM sink in an apartment with a message loop, and its connectivity
  flags stay the same when the default network moves between two connected
  networks (Wi-Fi to Ethernet), the change apps most need to hear; the cost
  is yet another interface. IP Helper's callbacks are plain C functions on
  the system's thread pool, and the connectivity hint is what the taskbar
  shows. Before Windows 10 2004, online is "has a route out".
- **Linux**: NetworkManager is believed only for the interface the kernel
  routes through, so a process in a network namespace of its own (a
  container, a sandbox) gets its own namespace's view. The netlink thread
  waits for a burst of changes (a link, its addresses, its routes) to end,
  150 ms, before reading the routes again.
- **The web**: desktop browsers don't say what the connection runs over
  (`Other`), and `navigator.onLine` is true on any network, Internet or not.
  A new connection type counts as a new network.

## Testing it

- Unit tests cover the netlink parsing (routes, multipath, links and their
  kinds), the Linux, Windows and Android mappings, and the numbering of
  networks (`cargo test network`).
- `examples/netstatus` shows the status and logs each change;
  `cargo run --example netstatus -- --watch 10` prints each change for ten
  seconds without a window.
- `.github/scripts/network-linux.sh target/debug/examples/netstatus` runs it
  in a user and network namespace of its own (`unshare -rn`, no root): it
  adds a dummy link with a default route, a second one with a lower metric,
  then deletes them, and checks the statuses go offline → online → online
  (new network) → online (new network) → offline. CI's Linux job runs it.
- `examples/androiddemo/test-network.sh` turns an emulator's Wi-Fi and mobile
  data off and on (`svc wifi`, `svc data`) and checks what androiddemo's
  `on_network` hook logs.
