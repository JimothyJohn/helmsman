//! Device profiles: one file gives a device's channels by name and their
//! address on every protocol it speaks, and renders the I/O map for any
//! of them. The Moxa E1213's profile is held to the two hand-written
//! maps it replaces, and driven through the simulated unit by channel
//! name.

use std::time::{Duration, Instant};

use helmsman_io::enip_sim::E1213Sim;
use helmsman_io::profile::{Profile, channel_name_ok, signal_name};
use helmsman_io::{Binding, Dir, Value, text};

const E1213: &str = include_str!("../../../docs/devices/moxa-iologik-e1213.device");

fn e1213() -> Profile {
    Profile::parse(E1213).unwrap()
}

#[test]
fn the_e1213_profile_reads_with_two_transports_and_forty_channels() {
    let p = e1213();
    assert_eq!(
        (p.vendor.as_str(), p.model.as_str(), p.kind.as_str()),
        ("Moxa", "ioLogik E1213", "remote-io")
    );
    let drivers: Vec<&str> = p.transports.iter().map(|t| t.driver.as_str()).collect();
    assert_eq!(drivers, ["enip-io", "modbus-tcp"]);
    assert_eq!(p.channels.len(), 12 + 12 + 8 + 8);
    assert_eq!(p.channels.iter().filter(|c| c.dir == Dir::Out).count(), 8);
}

/// The profile says, channel by channel, what the two maps written by
/// hand from the manual and the EDS say.
#[test]
fn the_profile_agrees_with_the_hand_written_maps_on_both_protocols() {
    let p = e1213();
    let named = [
        ("part_present", "DI-04"),
        ("door_closed", "DI-05"),
        ("clamp_closed", "DI-00"),
        ("parts_counted", "DI-04-count"),
        ("clamp_is_on", "DO-00-status"),
        ("clamp", "DO-00"),
        ("beacon", "DO-01"),
    ];
    let maps = [
        include_str!("../../../docs/devices/moxa-iologik-e1213-enip.map"),
        include_str!("../../../docs/devices/moxa-iologik-e1213-modbus.map"),
    ];
    for (transport, map) in maps.iter().enumerate() {
        let file = text::parse(map).unwrap();
        assert_eq!(file.map.devices[0].driver, p.transports[transport].driver);
        assert_eq!(file.map.signals.len(), named.len());
        for s in &file.map.signals {
            let channel = named.iter().find(|(n, _)| *n == s.name).unwrap().1;
            let Binding::Device { point, .. } = &s.binding else {
                panic!("{} is not on the device", s.name)
            };
            assert_eq!(
                p.point(transport, channel),
                Some(point.as_str()),
                "{} over transport {transport}",
                s.name
            );
        }
    }
}

#[test]
fn either_transport_renders_a_map_with_the_same_signals() {
    let p = e1213();
    let signals = |transport| {
        let map = p.map(transport, "rio", "192.168.3.60").unwrap();
        let file = text::parse(&map).unwrap();
        assert_eq!(file.map.validate(), Ok(()), "{map}");
        assert_eq!(file.safe.get("rio_do_00"), Some(&Value::Bool(false)));
        file.map
            .signals
            .iter()
            .map(|s| (s.name.clone(), s.ty, s.dir))
            .collect::<Vec<_>>()
    };
    let enip = signals(0);
    assert_eq!(enip, signals(1));
    assert_eq!(enip.len(), 40);
    assert_eq!(enip[16].0, "rio_di_04_count");
    assert!(
        p.map(0, "rio", "h")
            .unwrap()
            .starts_with("device rio enip-io h profile=moxa-e1213 rpi_ms=100\n")
    );
    assert!(
        p.map(1, "rio", "h")
            .unwrap()
            .starts_with("device rio modbus-tcp h unit=1\n")
    );
    assert_eq!(p.map(2, "rio", "h"), None);
}

/// By channel name through the simulated unit: an output written is the
/// output the unit reports and the input its bench wire feeds.
#[test]
fn channels_drive_the_simulated_e1213_by_name() {
    let p = e1213();
    let sim = E1213Sim::start().unwrap();
    let map = p.map(0, "rio", &sim.endpoint()).unwrap();
    let mut device = text::parse(&map).unwrap().map.devices.remove(0);
    // The simulator listens on ports of its own.
    device.params.extend(sim.params());
    let mut d = helmsman_io::open(&device).unwrap();
    let point = |channel: &str| p.point(0, channel).unwrap().to_string();
    let wait = |d: &mut Box<dyn helmsman_io::Driver + Send>, channel: &str, want: Value| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match d.read(&[&point(channel)]) {
                Ok(v) if v[0] == want => return,
                other => assert!(Instant::now() < deadline, "{channel}: {other:?}, want {want:?}"),
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    wait(&mut d, "DO-01-status", Value::Bool(false));
    d.write(&point("DO-01"), Value::Bool(true)).unwrap();
    wait(&mut d, "DO-01-status", Value::Bool(true));
    wait(&mut d, "DO-00-status", Value::Bool(false));
    d.write(&point("DO-01"), Value::Bool(false)).unwrap();
    wait(&mut d, "DO-01-status", Value::Bool(false));
}

#[test]
fn a_profile_that_would_not_make_a_valid_map_is_refused_in_words() {
    let err = |src: &str| {
        Profile::parse(src)
            .unwrap_err()
            .into_iter()
            .map(|(n, e)| format!("{n}: {e}"))
            .collect::<Vec<_>>()
            .join(" | ")
    };
    let base = "helmsman-device 1\nmodel X\ntransport modbus-tcp unit=1\n";
    assert!(err("device 1\n").contains("the first line must be"));
    assert!(err("helmsman-device 1\n").contains("no model line"));
    assert!(err("helmsman-device 1\nmodel X\nin A bool modbus-tcp=di:0\n").contains("no transport line"));
    assert!(err(base).contains("no channels"));
    assert!(err(&format!("{base}in 1A bool modbus-tcp=di:0\n")).contains("channel \"1A\""));
    assert!(
        err(&format!(
            "{base}in A bool modbus-tcp=di:0\nin a bool modbus-tcp=di:1\n"
        ))
        .contains("given twice")
    );
    assert!(err(&format!("{base}in A word modbus-tcp=di:0\n")).contains("bool, int or real"));
    assert!(err(&format!("{base}transport modbus-tcp\n")).contains("transport modbus-tcp given twice"));
    assert!(err(&format!("{base}bogus line\n")).contains("expected vendor, model"));
    // What only shows once a map is rendered: a point of the wrong type,
    // a channel one transport cannot reach, an output on a read-only
    // point, a transport nobody declared.
    assert!(err(&format!("{base}in A int modbus-tcp=di:0\n")).contains("over modbus-tcp"));
    assert!(err(&format!("{base}out A bool modbus-tcp=di:0\n")).contains("over modbus-tcp"));
    assert!(err(&format!("{base}in A bool enip-io=di:0\n")).contains("channel A has no point"));
    assert!(err(&format!("{base}in A bool modbus-tcp=di:0 enip-io=di:0\n")).contains("not a transport"));
    assert!(err(&format!("{base}in A bool modbus-tcp=nonsense\n")).contains("over modbus-tcp"));
}

#[test]
fn names_follow_their_rules() {
    assert!(channel_name_ok("DI-04-count"));
    assert!(channel_name_ok("Speed_rpm"));
    for bad in ["", "4DI", "DI 4", "DI.4", &"a".repeat(33)] {
        assert!(!channel_name_ok(bad), "{bad}");
    }
    assert_eq!(signal_name("rio", "DI-04-count"), "rio_di_04_count");
}

#[test]
fn a_transport_may_give_rate_limits_the_map_never_sees() {
    use std::time::Duration;
    let p = e1213();
    assert_eq!(
        p.transports[0].limits(),
        Some((Duration::from_millis(100), Duration::from_secs(3)))
    );
    assert_eq!(p.transports[1].limits(), None);
    let map = p.map(0, "rio", "192.0.2.1").unwrap();
    let line = map.lines().next().unwrap();
    assert!(
        line.contains(" rpi_ms=100") && !line.contains("cycle_ms"),
        "{line}"
    );
    assert_eq!(
        p.transports[0]
            .driver_params()
            .map(|(k, _)| k)
            .collect::<Vec<_>>(),
        ["profile", "rpi_ms"]
    );
    for bad in [
        "cycle_ms=100",
        "cycle_ms=0..100",
        "cycle_ms=500..100",
        "cycle_ms=1..60001",
        "cycle_ms=a..b",
    ] {
        let src = E1213.replace("rpi_ms=100 cycle_ms=100..3000", &format!("rpi_ms=100 {bad}"));
        let errs = Profile::parse(&src).unwrap_err();
        assert!(
            errs.iter().any(|(_, e)| e.starts_with(bad) && e.contains("A..B")),
            "{bad}: {errs:?}"
        );
    }
}

/// `configure NAME` attaches a device to a configurator the box has; a
/// profile without the line has none, and a name the box does not know is
/// an error naming the line.
#[test]
fn a_profile_names_its_configurator_or_has_none() {
    assert_eq!(e1213().configure.as_deref(), Some("moxa-e1200"));
    let base = "helmsman-device 1\nmodel X\ntransport modbus-tcp unit=1\nin A bool modbus-tcp=di:0\n";
    assert_eq!(Profile::parse(base).unwrap().configure, None);
    let with = |line: &str| {
        Profile::parse(&format!(
            "helmsman-device 1\nmodel X\n{line}\ntransport modbus-tcp unit=1\nin A bool modbus-tcp=di:0\n"
        ))
    };
    assert_eq!(
        with("configure moxa-e1200").unwrap().configure.as_deref(),
        Some("moxa-e1200")
    );
    let errs = with("configure web-console").unwrap_err();
    assert!(
        errs.iter()
            .any(|(n, e)| *n == 3 && e.contains("web-console") && e.contains("moxa-e1200")),
        "{errs:?}"
    );
    assert!(with("configure").unwrap_err().iter().any(|(n, _)| *n == 3));
    assert!(
        with("configure moxa-e1200 extra")
            .unwrap_err()
            .iter()
            .any(|(n, _)| *n == 3)
    );
    let twice = Profile::parse(
        "helmsman-device 1\nmodel X\nconfigure moxa-e1200\nconfigure moxa-e1200\ntransport modbus-tcp unit=1\nin A bool modbus-tcp=di:0\n",
    )
    .unwrap_err();
    assert!(
        twice.iter().any(|(n, e)| *n == 4 && e.contains("twice")),
        "{twice:?}"
    );
}
