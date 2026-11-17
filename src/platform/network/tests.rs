use super::*;

fn wifi(network: u64) -> Reading {
    Reading {
        online: true,
        transport: Transport::Wifi,
        metered: false,
        constrained: false,
        network,
    }
}

#[test]
fn the_first_reading_is_generation_one() {
    let mut t = Tracker::default();
    let s = t.take(wifi(7)).expect("a first reading is a change");
    assert!(s.online);
    assert_eq!(s.transport, Transport::Wifi);
    assert_eq!(s.generation, 1);
}

#[test]
fn a_repeat_is_no_change() {
    let mut t = Tracker::default();
    t.take(wifi(7));
    assert_eq!(t.take(wifi(7)), None);
}

#[test]
fn a_new_default_network_while_online_is_a_change() {
    // Wi-Fi to cellular, both online: the case that breaks open sockets.
    let mut t = Tracker::default();
    t.take(wifi(7));
    let cell = Reading {
        transport: Transport::Cellular,
        metered: true,
        network: 9,
        ..wifi(0)
    };
    let s = t.take(cell).unwrap();
    assert!(s.online && s.metered);
    assert_eq!(s.generation, 2);
    // Another Wi-Fi network (another router) with nothing else different.
    t.take(wifi(7));
    let s = t.take(wifi(8)).expect("same transport, new network");
    assert_eq!(s.transport, Transport::Wifi);
    assert_eq!(s.generation, 4);
}

#[test]
fn the_same_network_turning_metered_keeps_its_generation() {
    let mut t = Tracker::default();
    t.take(wifi(7));
    let s = t
        .take(Reading {
            metered: true,
            ..wifi(7)
        })
        .unwrap();
    assert!(s.metered);
    assert_eq!(s.generation, 1);
    let s = t
        .take(Reading {
            metered: true,
            constrained: true,
            ..wifi(7)
        })
        .unwrap();
    assert!(s.constrained);
    assert_eq!(s.generation, 1);
}

#[test]
fn offline_readings_are_all_alike() {
    let mut t = Tracker::default();
    t.take(wifi(7));
    let s = t
        .take(Reading {
            online: false,
            ..wifi(7)
        })
        .unwrap();
    assert!(!s.online);
    // Offline reports no transport.
    assert_eq!(s.transport, Transport::Other);
    assert_eq!(s.generation, 2);
    // What an offline platform says besides doesn't matter.
    assert_eq!(t.take(Reading::OFFLINE), None);
    // Back on the same network: a new generation all the same.
    assert_eq!(t.take(wifi(7)).unwrap().generation, 3);
}

#[test]
fn a_status_can_be_built_for_tests() {
    let s = NetworkStatus::new(true, Transport::Cellular)
        .metered(true)
        .constrained(true)
        .generation(5);
    assert!(s.online && s.metered && s.constrained);
    assert_eq!(s.generation, 5);
}
