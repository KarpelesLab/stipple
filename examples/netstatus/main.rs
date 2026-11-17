//! Network status: whether the device is online, over Wi-Fi, cellular or a
//! cable, whether the network is metered or the user asked to save data,
//! and each change as it comes (`App::on_network`), with the network's
//! generation, which changes when the default network does even while
//! staying online (Wi-Fi to cellular).
//!
//! ```sh
//! cargo run --example netstatus
//! # No window: print each change for 10 seconds (what CI's checks read).
//! cargo run --example netstatus -- --watch 10
//! ```
//!
//! In `--watch` mode each status is printed as
//! `netstatus: online=1 transport=Wifi metered=0 constrained=0 generation=3`.

use stipple::platform::{self, NetworkStatus, Transport};
use stipple::prelude::*;

#[derive(Default)]
struct State {
    /// The changes heard, newest first.
    log: Vec<String>,
}

fn describe(net: &NetworkStatus) -> String {
    format!(
        "online={} transport={:?} metered={} constrained={} generation={}",
        net.online as u8, net.transport, net.metered as u8, net.constrained as u8, net.generation
    )
}

fn transport_name(t: Transport) -> &'static str {
    match t {
        Transport::Wifi => "Wi-Fi",
        Transport::Cellular => "Cellular",
        Transport::Ethernet => "Ethernet",
        _ => "Other",
    }
}

fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    // Read on every build: the App rebuilds the views on each change.
    let net = platform::network();
    let headline = if !net.online {
        "Offline".to_string()
    } else {
        format!("Online over {}", transport_name(net.transport))
    };
    let flags = format!(
        "{} · {} · network #{}",
        if net.metered {
            "Metered"
        } else {
            "Not metered"
        },
        if net.constrained {
            "Saving data"
        } else {
            "Not constrained"
        },
        net.generation
    );
    let mut children = vec![
        heading(cx, "Network"),
        label(cx, headline),
        label(cx, flags),
        label(cx, "Changes:"),
    ];
    if state.log.is_empty() {
        children.push(label(cx, "(none yet: turn Wi-Fi off, unplug a cable…)"));
    }
    for line in state.log.iter().take(12) {
        children.push(label(cx, line.clone()));
    }
    column(children)
        .gap(theme.spacing.sm)
        .padding(Insets::uniform(theme.spacing.lg))
}

/// Print each status `network()` reports for `secs` seconds, without a
/// window. Exits 0.
#[cfg(not(target_arch = "wasm32"))]
fn watch(secs: u64) {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    let mut last = None;
    while std::time::Instant::now() < end {
        let net = platform::network();
        if last != Some(net) {
            println!("netstatus: {}", describe(&net));
            last = Some(net);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn main() {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let args: Vec<String> = std::env::args().collect();
        if let Some(i) = args.iter().position(|a| a == "--watch") {
            let secs = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(10);
            watch(secs);
            return;
        }
    }
    App::new(State::default(), view)
        .on_network(|s, net| {
            println!("netstatus: {}", describe(net));
            s.log.insert(0, describe(net));
        })
        .title("Stipple network")
        .logical_size(Size::new(520.0, 420.0))
        .run();
}

// In the browser, `web.rs` builds this file as a library and the page
// runs `main`'s app (on the web, `App::run` hands it to the page).
stipple::web_app!(main);
