//! The node contract: what `helmsman-nodecore` renders is read by
//! [`crate::remote`] exactly as a box's API is, so a node on the LAN is a
//! box to this canvas. If a field name drifts on either side, this is
//! where it shows.

use helmsman_nodecore::api::{
    self, DeviceView, ModuleView, NodeView, SignalValue, SignalView, Spread, State, Timing,
};
use helmsman_nodecore::settings::Settings;

use crate::remote::{BoxState, Fetch, Msg, Stream};

fn view() -> NodeView {
    NodeView {
        module: Some(ModuleView {
            name: "chase".into(),
            scan_ms: 10.0,
            state: State::Running,
            scans: 500,
            overruns: 1,
            last_overrun_ms_ago: Some(2500),
            timing: Some(Timing {
                window: 500,
                exec: Spread {
                    min: 90.0,
                    mean: 120.0,
                    max: 300.0,
                    p99: 250.0,
                },
                late: Spread {
                    min: 0.0,
                    mean: 5.0,
                    max: 40.0,
                    p99: 30.0,
                },
            }),
            source: "PROGRAM chase\nVAR x : BOOL; END_VAR\nEND_PROGRAM\n".into(),
        }),
        signals: vec![
            SignalView {
                name: "di0".into(),
                dir: "in",
                value: Some(SignalValue::Bool(true)),
                quality: "good",
                point_error: None,
            },
            SignalView {
                name: "counter".into(),
                dir: "in",
                value: Some(SignalValue::Int(42)),
                quality: "stale",
                point_error: Some("link down".into()),
            },
        ],
        devices: vec![DeviceView {
            name: "rio".into(),
            state: "down",
            error: Some("connect timed out".into()),
            address: Some("192.168.3.60:502".into()),
        }],
        seq: 7,
        at_ms: 70_000,
        uptime_ms: 90_000,
        heap_used: 1,
        heap_peak: 2,
        heap_size: 3,
        mac: [1, 2, 3, 4, 5, 6],
        ip: Some([10, 0, 0, 50]),
        link_up: true,
        version: helmsman_nodecore::VERSION,
        board: "test",
        io_map: "device rio modbus-tcp 192.168.3.60\n".into(),
    }
}

#[test]
fn the_canvas_reads_a_node_as_it_reads_a_box() {
    let v = view();
    let mut s = BoxState::new();
    s.apply(Msg::Got(Fetch::Modules, Ok(api::modules(&v).to_string())), 1.0);
    assert_eq!(s.modules.len(), 1);
    let m = &s.modules[0];
    assert_eq!(m.name, "chase");
    assert_eq!(m.state, "running");
    assert_eq!(m.scans, 500);
    assert_eq!(m.scan_ms, Some(10.0));
    assert_eq!(m.overruns, 1);
    assert_eq!(m.overrun_ago, Some(2.5));
    let t = m.timing.expect("timing parsed");
    assert_eq!(t.window, 500);
    assert_eq!(t.exec.p99, 250.0);
    assert_eq!(t.late.max, 40.0);

    s.apply(Msg::Opened(Stream::Values), 1.1);
    s.apply(
        Msg::Event(Stream::Values, "values".into(), api::values_event(&v).to_string()),
        1.2,
    );
    assert_eq!(s.seq, Some(7));
    assert_eq!(s.values_at_ms, Some(70_000));
    assert_eq!(s.signals.len(), 2);
    assert_eq!(s.signals[0].name, "di0");
    assert_eq!(s.signals[0].dir, "in");
    assert_eq!(s.signals[0].quality, "good");
    assert_eq!(s.signals[1].quality, "stale");
    assert_eq!(s.signals[1].point_error.as_deref(), Some("link down"));
    assert_eq!(s.modules[0].name, "chase", "the event's modules replace the list");

    s.apply(Msg::Got(Fetch::Devices, Ok(api::devices(&v).to_string())), 1.3);
    assert_eq!(s.devices.len(), 1);
    assert_eq!(s.devices[0].state, "down");
    assert_eq!(s.devices[0].error.as_deref(), Some("connect timed out"));
    assert_eq!(s.devices[0].address.as_deref(), Some("192.168.3.60:502"));

    s.apply(Msg::Got(Fetch::IoMap, Ok(api::io_map(&v).to_string())), 1.4);
    assert_eq!(s.io_map.as_deref(), Some("device rio modbus-tcp 192.168.3.60\n"));

    let detail = api::module_detail(v.module.as_ref().unwrap());
    s.apply(
        Msg::Got(Fetch::Source("chase".into()), Ok(detail.to_string())),
        1.5,
    );
    // The Monitor's source fetch lands in the watched programs; a node
    // has no traces, so it stays source-only. Parsing it must not fail.
    assert!(s.problem.is_none());
}

#[test]
fn a_faulted_node_module_shows_its_fault() {
    let mut v = view();
    v.module.as_mut().unwrap().state = State::Faulted("division by zero".into());
    let mut s = BoxState::new();
    s.apply(Msg::Got(Fetch::Modules, Ok(api::modules(&v).to_string())), 1.0);
    assert_eq!(s.modules[0].state, "faulted");
    assert_eq!(s.modules[0].fault.as_deref(), Some("division by zero"));
}

#[test]
fn the_beacon_parses_as_the_box_reads_beacons() {
    // The box's parser is in helmsman-plc; its contract test (tests/site.rs)
    // holds the format. Here: six lines, the names in order.
    let settings = Settings {
        name: "press-4".into(),
        site: "line-a".into(),
        ..Settings::default()
    };
    let b = api::beacon(&view(), &settings);
    let keys: Vec<&str> = b.lines().map(|l| l.split(' ').next().unwrap()).collect();
    assert_eq!(keys, ["helmsman-site", "site", "id", "name", "api", "version"]);
    assert!(b.len() <= 512);
}
