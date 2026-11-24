//! The Novanta IMS `MDrive` (Lexium Liberty, EtherNet/IP mode) by its
//! profile, against a simulated unit with the identity, the assemblies and
//! the connection points the bench unit showed on 2026-11-16
//! (docs/devices/novanta-mdrive.md). The simulator is a layout, not the
//! drive: what the real unit did with each byte is on that page.

use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use helmsman_adapter::DeviceConfig;
use helmsman_adapter::device::{ConnectionPoint, Identity, PointKind, lock_ref};
use helmsman_adapter::e1213::Application;
use helmsman_io::enip::EnipDriver;
use helmsman_io::enip_sim::E1213Sim;
use helmsman_io::profile::Profile;
use helmsman_io::{Dir, Driver, Ty, Value, text};

const MDRIVE: &str = include_str!("../../../docs/devices/novanta-mdrive.device");

fn profile() -> &'static Profile {
    static P: OnceLock<Profile> = OnceLock::new();
    P.get_or_init(|| Profile::parse(MDRIVE).unwrap())
}

/// The drive as the bench saw it: assembly 100 (16 bytes) out, 112 (36)
/// in with a run/idle header, configuration instance 1 of no bytes.
fn unit() -> E1213Sim {
    let config = DeviceConfig {
        identity: Identity {
            vendor_id: 243,
            device_type: 43,
            product_code: 15362,
            revision: (2, 36),
            status: 0,
            serial_number: 1,
            product_name: "MDrive".into(),
        },
        assemblies: vec![(1, 0), (100, 16), (112, 36)],
        points: vec![ConnectionPoint {
            kind: PointKind::ExclusiveOwner,
            configuration: 1,
            consumed: 112,
            produced: 100,
        }],
        consumed_run_idle: true,
        produced_run_idle: false,
        max_connections: 1,
        min_rpi: Duration::from_millis(1),
        max_rpi: Duration::from_millis(500),
        initial_watchdog: Duration::from_secs(10),
        connection_id_base: 0x0100_0000,
    };
    E1213Sim::start_with(config, Application::default()).unwrap()
}

/// The profile's own enip-io params, pointed at the simulator.
fn driver(s: &E1213Sim) -> EnipDriver {
    let mut p: BTreeMap<String, String> = profile().transports[0]
        .driver_params()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    p.insert("device_io_port".into(), s.params()["device_io_port"].clone());
    p.insert("io_port".into(), "0".into());
    EnipDriver::from_params(&s.endpoint(), &p).unwrap()
}

#[test]
fn the_profile_renders_the_map_the_bench_ran() {
    let p = profile();
    assert_eq!((p.vendor.as_str(), p.kind.as_str()), ("Novanta IMS", "drive"));
    let map = p.map(0, "drive", "192.168.33.1").unwrap();
    let mut lines = map.lines();
    assert_eq!(
        lines.next().unwrap(),
        "device drive enip-io 192.168.33.1 config=1 o2t=112:36 t2o=100:16 rpi_ms=20"
    );
    let rest: Vec<&str> = lines.collect();
    assert_eq!(rest.len(), 8, "{rest:?}");
    for want in [
        "drive in:0.0",
        "drive in:0.1",
        "drive in:4:i32",
        "drive in:8:i32",
        "drive out:8:u32",
        "drive out:17:u8",
        "drive out:18:u8",
        "drive out:20:i32",
    ] {
        assert!(rest.iter().any(|l| l.contains(want)), "{want} in {rest:?}");
    }
    // The box's own parser accepts what the profile renders.
    let file = text::parse(&map).unwrap();
    assert_eq!(file.map.devices.len(), 1);
    assert_eq!(file.map.signals.len(), 8);
}

/// MA is not proven and moves the shaft: no channel may reach bytes
/// 28..31 of the output assembly, nor any other field the bench did not
/// prove (MF 4..7, ER 12..15, CF 16, LG 24..27, RD/AS 32..33, 34..35).
#[test]
fn no_channel_touches_a_field_the_bench_did_not_prove() {
    let known_out = [(8usize, 4usize), (17, 1), (18, 1), (20, 4)];
    for c in &profile().channels {
        if c.dir != Dir::Out {
            continue;
        }
        let point = &c.points["enip-io"];
        let mut f = point.split(':');
        assert_eq!(f.next(), Some("out"), "{}", c.name);
        let at: usize = f.next().unwrap().parse().unwrap();
        let width = match f.next() {
            Some("u8" | "i8") => 1,
            Some("u32" | "i32") => 4,
            other => panic!("{}: {other:?}", c.name),
        };
        assert!(known_out.contains(&(at, width)), "{}: {point}", c.name);
        assert!(c.safe.as_deref().is_none_or(|s| s == "0"), "{}", c.name);
    }
    assert!(profile().channels.iter().all(|c| c.ty != Ty::Real));
}

#[test]
fn the_channels_reach_the_bytes_the_bench_saw_them_in() {
    let s = unit();
    let mut d = driver(&s);
    // Inputs: DE and MV in byte 0, position at 4, the error code at 8.
    let mut input = [0u8; 16];
    input[0] = 0b11;
    input[4..8].copy_from_slice(&(-123_456i32).to_le_bytes());
    input[8..12].copy_from_slice(&94i32.to_le_bytes());
    lock_ref(&s.device()).write_assembly(100, &input).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let got = d.read(&["in:0.0", "in:0.1", "in:4:i32", "in:8:i32"]);
        if let Ok(v) = &got
            && v[2] == Value::Int(-123_456)
        {
            assert_eq!(
                v,
                &[
                    Value::Bool(true),
                    Value::Bool(true),
                    Value::Int(-123_456),
                    Value::Int(94)
                ]
            );
            break;
        }
        assert!(Instant::now() < deadline, "{got:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
    // Outputs land in 112 where the profile says, and nowhere else.
    d.write("out:17:u8", Value::Int(1)).unwrap();
    d.write("out:18:u8", Value::Int(44)).unwrap();
    d.write("out:20:i32", Value::Int(-5000)).unwrap();
    d.write("out:8:u32", Value::Int(768_000)).unwrap();
    let mut want = [0u8; 36];
    want[8..12].copy_from_slice(&768_000u32.to_le_bytes());
    want[17] = 1;
    want[18] = 44;
    want[20..24].copy_from_slice(&(-5000i32).to_le_bytes());
    let deadline = Instant::now() + Duration::from_secs(3);
    while lock_ref(&s.device()).assembly(112) != Some(&want[..]) {
        assert!(
            Instant::now() < deadline,
            "{:?}",
            lock_ref(&s.device()).assembly(112)
        );
        let _ = d.read(&["in:0.0"]);
        std::thread::sleep(Duration::from_millis(5));
    }
    // Hostile values: out of range for the field's width, refused.
    assert!(d.write("out:17:u8", Value::Int(256)).is_err());
    assert!(d.write("out:8:u32", Value::Int(-1)).is_err());
    assert!(
        d.write("out:20:i32", Value::Int(i64::from(i32::MAX) + 1))
            .is_err()
    );
}
