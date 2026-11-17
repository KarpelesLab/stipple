//! Network changes in the [`App`]: [`App::on_network`]. See `docs/network.md`.

use crate::App;
use crate::platform::NetworkStatus;

/// Hears network changes (see [`App::on_network`]).
pub(crate) type NetworkFn<S> = Box<dyn FnMut(&mut S, &NetworkStatus)>;

impl<S: 'static> App<S> {
    /// Call `hook` with the new [`NetworkStatus`] each time the default
    /// network changes: going offline or online, another transport, metered
    /// or constrained, and a new network while staying online (Wi-Fi to
    /// cellular: a new [`generation`](NetworkStatus::generation), which is
    /// when sockets opened on the old one break). The status at start isn't
    /// a change: read it with [`platform::network`](crate::platform::network).
    /// The views are built again after each change, so a view that only
    /// shows the state can read `platform::network` and needs no hook.
    ///
    /// This starts watching the network. On Android the app needs the
    /// `ACCESS_NETWORK_STATE` permission (`network = true` in `cargo-stipple`'s
    /// manifest adds it). See `docs/network.md`.
    ///
    /// ```no_run
    /// # use stipple::prelude::*;
    /// use stipple::platform::NetworkStatus;
    /// #[derive(Default)]
    /// struct State { offline: bool, generation: u64 }
    /// # fn view(_: &State, _: &mut Cx<State>) -> Element { spacer() }
    /// App::new(State::default(), view)
    ///     .on_network(|s, net: &NetworkStatus| {
    ///         s.offline = !net.online;
    ///         if net.online && net.generation != s.generation {
    ///             // Another network: reconnect what was open.
    ///             s.generation = net.generation;
    ///         }
    ///     })
    ///     .run();
    /// ```
    pub fn on_network(mut self, hook: impl FnMut(&mut S, &NetworkStatus) + 'static) -> Self {
        self.network = Some(Box::new(hook));
        // (Tests feed changes by hand.)
        #[cfg(not(test))]
        crate::platform::network::start();
        self
    }

    /// Start watching the network as the loop starts, if the app listens:
    /// on Android there may have been no activity yet when the hook was set.
    pub(crate) fn start_network(&self) {
        if self.network.is_some() {
            crate::platform::network::start();
        }
    }

    /// The network changed: tell the hook and rebuild the views (one may
    /// read [`platform::network`](crate::platform::network)). Returns
    /// whether the windows need drawing.
    pub(crate) fn network_changed(&mut self, status: &NetworkStatus) -> bool {
        if let Some(hook) = self.network.as_mut() {
            hook(&mut self.state, status);
        }
        for pane in &mut self.panes {
            pane.dirty = true;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::platform::{NetworkStatus, Transport};
    use crate::prelude::*;

    #[derive(Default)]
    struct State {
        online: bool,
        reconnects: u32,
        generation: u64,
    }

    fn view(_: &State, _: &mut Cx<State>) -> Element {
        spacer()
    }

    #[test]
    fn the_hook_hears_each_change() {
        let mut app = App::new(State::default(), view).on_network(|s, net| {
            s.online = net.online;
            if net.online && net.generation != s.generation {
                s.reconnects += 1;
                s.generation = net.generation;
            }
        });
        let wifi = NetworkStatus::new(true, Transport::Wifi);
        assert!(app.network_changed(&wifi));
        assert!(app.state.online);
        // Wi-Fi to cellular, online all along: a new generation.
        let cell = NetworkStatus::new(true, Transport::Cellular)
            .metered(true)
            .generation(2);
        app.network_changed(&cell);
        assert_eq!(app.state.reconnects, 2);
        app.network_changed(&NetworkStatus::new(false, Transport::Other).generation(3));
        assert!(!app.state.online);
    }

    #[test]
    fn without_a_hook_the_views_are_built_again() {
        let mut app = App::new(State::default(), view);
        for pane in &mut app.panes {
            pane.dirty = false;
        }
        assert!(app.network_changed(&NetworkStatus::new(true, Transport::Ethernet)));
        assert!(app.panes.iter().all(|p| p.dirty));
    }
}
