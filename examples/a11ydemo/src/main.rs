//! Exercises the hand-written D-Bus client in `stipple::platform::a11y` (no
//! `zbus`/`dbus` crate) — the foundation of the AT-SPI accessibility bridge.
//!
//! - no args: connect to the session bus and the AT-SPI bus, print our unique
//!   names (the SASL handshake + `Hello` + `org.a11y.Bus.GetAddress`).
//! - `serve`: claim a bus name and answer incoming method calls
//!   (`Peer.Ping`, `Introspectable.Introspect`) — the bidirectional half the
//!   accessibility-tree export needs.
//!
//! The CI a11y job runs both modes inside a private `dbus-run-session`.

#[cfg(target_os = "linux")]
const INTROSPECT_XML: &str = r#"<!DOCTYPE node PUBLIC "-//freedesktop//DTD D-BUS Object Introspection 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd">
<node name="/org/stippleui/a11y">
  <interface name="org.freedesktop.DBus.Introspectable">
    <method name="Introspect"><arg name="data" type="s" direction="out"/></method>
  </interface>
  <interface name="org.freedesktop.DBus.Peer">
    <method name="Ping"/>
    <method name="GetMachineId"><arg name="machine_uuid" type="s" direction="out"/></method>
  </interface>
  <interface name="org.a11y.atspi.Accessible"/>
</node>"#;

/// Map a Stipple accessibility role to an `org.a11y.atspi` role number.
#[cfg(target_os = "linux")]
fn atspi_role(role: stipple::core::Role) -> u32 {
    use stipple::core::Role;
    match role {
        Role::Window => 27,    // ATSPI_ROLE_FRAME
        Role::Group => 54,     // ATSPI_ROLE_PANEL
        Role::Button => 44,    // ATSPI_ROLE_PUSH_BUTTON
        Role::TextField => 80, // ATSPI_ROLE_ENTRY
        Role::Text => 29,      // ATSPI_ROLE_LABEL
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    {
        use stipple::core::AccessNode;
        use stipple::platform::a11y::{AtspiTree, DBus};
        use stipple::prelude::*;
        let serve = std::env::args().nth(1).as_deref() == Some("serve");

        if serve {
            // Build a real Stipple UI and derive its accessibility tree, then
            // expose the *whole* tree over AT-SPI.
            let mut app = App::new((), |_s: &(), cx: &mut Cx<()>| {
                let theme = *cx.theme();
                panel(
                    &theme,
                    vec![label(&theme, "Hello"), button_labeled(&theme, "OK")],
                )
            });
            app.render_once();
            let root = app.accessibility_tree().expect("accessibility tree");

            // Flatten the AccessNode tree into the AT-SPI tree (parents first).
            fn add(tree: &mut AtspiTree, node: &AccessNode, parent: Option<usize>) {
                let name = if parent.is_none() && node.name.is_empty() {
                    "Stipple" // the window root takes the app name
                } else {
                    &node.name
                };
                let idx = tree.push(atspi_role(node.role), name, parent);
                for c in &node.children {
                    add(tree, c, Some(idx));
                }
            }
            let mut tree = AtspiTree::new();
            add(&mut tree, &root, None);

            let r = &tree.nodes[0];
            println!(
                "a11y root: role={} name={:?} children={}",
                r.role,
                r.name,
                r.children.len()
            );
            // Print the full flattened tree so the served hierarchy is visible.
            for (i, n) in tree.nodes.iter().enumerate() {
                println!(
                    "a11y node[{i}]: role={} name={:?} parent={:?} children={:?}",
                    n.role, n.name, n.parent, n.children
                );
            }

            let mut bus = match DBus::connect_session() {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("D-Bus connect failed: {e}");
                    std::process::exit(1);
                }
            };
            match bus.request_name("org.stippleui.A11yDemo") {
                Ok(code) => println!("RequestName org.stippleui.A11yDemo -> {code}"),
                Err(e) => {
                    eprintln!("RequestName failed: {e}");
                    std::process::exit(1);
                }
            }
            // Answer method calls until the connection drops (CI kills us).
            if let Err(e) = bus.serve_atspi_tree(&tree, INTROSPECT_XML) {
                eprintln!("serve ended: {e}");
            }
            return;
        }

        match DBus::connect_session() {
            Ok(bus) => println!("D-Bus connected: unique name {}", bus.unique_name()),
            Err(e) => {
                eprintln!("D-Bus connect failed: {e}");
                std::process::exit(1);
            }
        }
        match DBus::connect_a11y() {
            Ok(bus) => println!("AT-SPI bus connected: unique name {}", bus.unique_name()),
            Err(e) => {
                eprintln!("AT-SPI connect failed: {e}");
                std::process::exit(1);
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        println!("a11ydemo: D-Bus bridge is Linux-only");
    }
}
