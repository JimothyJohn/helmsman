//! A Universal Robot by its profile over Modbus TCP, against a simulator
//! loaded with what UR's simulator (software 10.13) answered on 2026-11-20: its
//! register values and the holes in its map, where a request that touches
//! an address the robot does not have is refused whole (Illegal Data
//! Address). docs/devices/universal-robots.md has the capture.

use std::collections::BTreeMap;
use std::ops::RangeInclusive;
use std::time::Duration;

use helmsman_io::modbus::{Client, app};
use helmsman_io::profile::Profile;
use helmsman_io::profile_driver::ProfileDriver;
use helmsman_io::sim::{Sim, State};
use helmsman_io::{Device, Dir, Driver, Error, Value};

const UR: &str = include_str!("../../../docs/devices/universal-robots-e-series.device");

/// The register and bit ranges the simulated robot answered, each read
/// one address at a time from 0 to 1100 (registers) and 0 to 700 (bits).
const REGISTERS: [RangeInclusive<u16>; 16] = [
    0..=11,
    16..=22,
    24..=33,
    128..=258,
    260..=266,
    270..=275,
    280..=285,
    290..=295,
    300..=305,
    310..=315,
    320..=325,
    400..=405,
    410..=415,
    420..=428,
    450..=451,
    768..=770,
];
const BITS: [RangeInclusive<u16>; 6] = [0..=9, 16..=25, 64..=143, 260..=265, 300..=427, 500..=627];

/// Register values as the robot read them (powered off, at the
/// simulator's start pose).
const VALUES: [(u16, &[u16]); 10] = [
    (256, &[10, 13, 3]),
    (260, &[0, 0, 0, 0, 0, 0, 1]),
    (270, &[4682, 4556, 4080, 5475, 1595, 6252]),
    (290, &[0, 2141, 1493, 233, 2, 0]),
    (300, &[26, 25, 25, 24, 24, 23]),
    (310, &[239, 239, 239, 239, 239, 239]),
    (320, &[65535, 65535, 65535, 65535, 0, 65535]),
    (400, &[0xfa60, 0xeefc, 0x07e4, 0xffff, 0x0c2c, 0x0027]),
    (768, &[253, 0, 0]),
    (128, &[0; 128]),
];

/// The joint angles the same robot gave on its realtime interface
/// (port 30003, `q_actual`), in rad.
const Q_ACTUAL: [f64; 6] = [-1.6007, -1.7271, -2.203, -0.808, 1.5951, -0.031];

/// Everything outside `have`, inside `0..=top`.
fn holes(have: &[RangeInclusive<u16>], top: u16) -> Vec<RangeInclusive<u16>> {
    let mut out = Vec::new();
    let mut next = 0u16;
    for r in have {
        if *r.start() > next {
            out.push(next..=r.start() - 1);
        }
        next = r.end() + 1;
    }
    out.push(next..=top);
    out
}

fn robot() -> Sim {
    let mut s = State::new(2048);
    s.absent_registers = holes(&REGISTERS, 2047);
    s.absent_bits = holes(&BITS, 2047);
    for (at, words) in VALUES {
        for (i, w) in words.iter().enumerate() {
            let a = usize::from(at) + i;
            s.holding[a] = *w;
            s.input[a] = *w;
        }
    }
    Sim::start(s).unwrap()
}

fn profile() -> Profile {
    Profile::parse(UR).unwrap_or_else(|e| panic!("{e:?}"))
}

fn profiled(sim: &Sim) -> ProfileDriver {
    let port = sim.endpoint().rsplit_once(':').unwrap().1.to_string();
    let params = BTreeMap::from([
        ("modbus-tcp.port".to_string(), port),
        ("modbus-tcp.timeout_ms".to_string(), "500".to_string()),
    ]);
    let line = Device {
        id: "ur".into(),
        driver: "device:universal-robots-e-series".into(),
        endpoint: "127.0.0.1".into(),
        params,
    };
    ProfileDriver::new(profile(), &line, Box::new(helmsman_io::open)).unwrap()
}

fn read(d: &mut ProfileDriver, ch: &str) -> Value {
    d.read(&[ch]).unwrap_or_else(|e| panic!("{ch}: {e}")).remove(0)
}

fn int(v: Value) -> i64 {
    match v {
        Value::Int(n) => n,
        other => panic!("not an int: {other:?}"),
    }
}

#[test]
fn every_channel_reads_on_a_robot_with_holes_alone_and_all_at_once() {
    let sim = robot();
    let mut d = profiled(&sim);
    let names: Vec<String> = profile()
        .channels
        .iter()
        .filter(|c| c.dir == Dir::In)
        .map(|c| format!("ch:{}", c.name))
        .collect();
    assert!(names.len() > 80, "{}", names.len());
    for n in &names {
        read(&mut d, n);
    }
    // One read of every channel: a driver that batches must still keep
    // each request inside the robot's map.
    let all: Vec<&str> = names.iter().map(String::as_str).collect();
    assert_eq!(d.read(&all).unwrap().len(), all.len());
    assert_eq!(d.transport().as_deref(), Some("modbus-tcp"));
}

#[test]
fn a_read_that_crosses_a_hole_is_refused_whole() {
    let sim = robot();
    let mut c = Client::new(&sim.endpoint(), 1, Duration::from_millis(500));
    assert_eq!(app::read_registers(&mut c, 3, 256, 3).unwrap(), vec![10, 13, 3]);
    // 259 is not in the map: 256..=260 is refused, though four of its
    // five addresses are.
    let e = app::read_registers(&mut c, 3, 256, 5).unwrap_err();
    assert!(!e.is_link(), "a refusal, not a link fault: {e}");
    assert!(matches!(e, Error::Exception(2)), "Illegal Data Address: {e:?}");
    // The box's inputs are bits 0..7 and the tool's 8..9; 10..15 are not
    // there, so a 16-bit read of the input word is refused.
    assert!(app::read_bits(&mut c, 2, 0, 10).is_ok());
    assert!(matches!(
        app::read_bits(&mut c, 2, 0, 16),
        Err(Error::Exception(2))
    ));
    // The link survives the refusals.
    assert_eq!(app::read_registers(&mut c, 4, 258, 1).unwrap(), vec![3]);
}

#[test]
fn the_state_reads_as_the_robot_reported_it() {
    let sim = robot();
    let mut d = profiled(&sim);
    assert_eq!(int(read(&mut d, "ch:controller-major")), 10);
    assert_eq!(int(read(&mut d, "ch:controller-minor")), 13);
    assert_eq!(int(read(&mut d, "ch:robot-mode")), 3, "power off");
    assert_eq!(read(&mut d, "ch:powered"), Value::Bool(false));
    assert_eq!(int(read(&mut d, "ch:base-mode")), 239, "joint power off");
    assert_eq!(int(read(&mut d, "ch:tool-mode")), 253);
}

#[test]
fn a_joint_angle_is_its_turns_and_its_angle_in_one_turn() {
    let sim = robot();
    let mut d = profiled(&sim);
    let joints = ["base", "shoulder", "elbow", "wrist1", "wrist2", "wrist3"];
    for (j, q) in joints.iter().zip(Q_ACTUAL) {
        let angle = int(read(&mut d, &format!("ch:{j}-angle")));
        let turns = int(read(&mut d, &format!("ch:{j}-turns")));
        assert!((0..=6283).contains(&angle), "{j}: {angle}");
        assert!(turns == -1 || turns == 0, "{j}: {turns}");
        #[allow(clippy::cast_precision_loss)] // mrad and turns are small.
        let rad = turns as f64 * std::f64::consts::TAU + angle as f64 / 1000.0;
        assert!(
            (rad - q).abs() < 0.001,
            "{j}: {rad} rad from the registers, {q} on 30003"
        );
    }
    // Read alone, the angle register is wrong for a negative joint: the
    // base is at -1.601 rad, and the register says +4.682.
    assert_eq!(int(read(&mut d, "ch:base-angle")), 4682);
}

#[test]
fn the_tool_centre_point_is_signed() {
    let sim = robot();
    let mut d = profiled(&sim);
    // -0.1440 m, -0.4356 m, 0.2020 m in tenths of a mm; rx -1 mrad.
    assert_eq!(int(read(&mut d, "ch:tcp-x")), -1440);
    assert_eq!(int(read(&mut d, "ch:tcp-y")), -4356);
    assert_eq!(int(read(&mut d, "ch:tcp-z")), 2020);
    assert_eq!(int(read(&mut d, "ch:tcp-rx")), -1);
    assert_eq!(int(read(&mut d, "ch:tcp-ry")), 3116);
    assert_eq!(int(read(&mut d, "ch:tcp-rz")), 39);
}

#[test]
fn writes_reach_the_general_purpose_registers_and_nothing_else() {
    let sim = robot();
    let mut d = profiled(&sim);
    d.write("ch:reg-130", Value::Int(1234)).unwrap();
    d.write("ch:reg-143", Value::Int(65535)).unwrap();
    d.write("ch:bool-in-2", Value::Bool(true)).unwrap();
    {
        let s = sim.state();
        assert_eq!((s.holding[130], s.holding[143]), (1234, 65535));
        assert!(s.coils[302]);
    }
    // A register is a 16-bit word with no sign: -1 and 65536 never leave
    // the box.
    let before = sim.state().requests;
    assert!(d.write("ch:reg-130", Value::Int(-1)).is_err());
    assert!(d.write("ch:reg-130", Value::Int(65_536)).is_err());
    assert!(d.write("ch:reg-130", Value::Bool(true)).is_err());
    // The robot's state is read only, and a name the profile lacks is no
    // channel.
    assert!(d.write("ch:robot-mode", Value::Int(7)).is_err());
    assert!(d.write("ch:DI-10", Value::Bool(true)).is_err());
    assert!(d.read(&["ch:reg-256"]).is_err());
    assert_eq!(sim.state().requests, before, "nothing refused was sent");
    assert_eq!(sim.state().holding[130], 1234);
}
