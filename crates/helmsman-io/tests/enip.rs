//! `enip-io` against the simulated E1213 over real TCP and UDP on
//! loopback: the conformance harness passes (DO->DI loops proven), link
//! errors and point errors are told apart, a lost connection is closed and
//! reopened with the outputs it held, exclusive ownership is released, and
//! a refused `Forward_Open` is a link error naming the device's code.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use helmsman_adapter::DeviceConfig;
use helmsman_adapter::device::lock_ref;
use helmsman_adapter::e1213::Application;
use helmsman_io::enip::{Connection, EnipDriver, Point, Spec};
use helmsman_io::enip_sim::E1213Sim;
use helmsman_io::harness::{self, Bench, Outcome};
use helmsman_io::sim::{Fault, Faults};
use helmsman_io::{Binding, Device, Dir, Driver, Error, IoMap, MapError, PointInfo, Signal, Ty, Value};
use proptest::prelude::*;

const FACTS: &str = include_str!("../../helmsman-proto/tests/fixtures/e1213-facts.eds");

fn driver(s: &E1213Sim) -> EnipDriver {
    EnipDriver::from_params(&s.endpoint(), &s.params()).unwrap()
}

fn with(s: &E1213Sim, extra: &[(&str, &str)]) -> BTreeMap<String, String> {
    let mut p = s.params();
    for (k, v) in extra {
        p.insert((*k).into(), (*v).into());
    }
    p
}

fn read(d: &mut EnipDriver, addr: &str) -> Result<Value, Error> {
    d.read(&[addr]).map(|v| v[0])
}

/// Reads until `addr` is `want`, or panics after `within`.
fn wait_value(d: &mut EnipDriver, addr: &str, want: Value, within: Duration) {
    let deadline = Instant::now() + within;
    loop {
        match read(d, addr) {
            Ok(v) if v == want => return,
            other if Instant::now() > deadline => panic!("{addr}: {other:?}, want {want:?}"),
            _ => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}

/// Reads until a read fails, or panics after `within`.
fn wait_error(d: &mut EnipDriver, addr: &str, within: Duration) -> Error {
    let deadline = Instant::now() + within;
    loop {
        match read(d, addr) {
            Err(e) => return e,
            Ok(v) if Instant::now() > deadline => panic!("{addr} still reads {v:?}"),
            Ok(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}

fn e1213_bench() -> Bench {
    Bench {
        points: vec![
            "di:0".into(),
            "di:11".into(),
            "in:2:u32".into(),
            "in:50.0".into(),
            "do:7".into(),
        ],
        loops: (0..4).map(|n| (format!("do:{n}"), format!("di:{n}"))).collect(),
        absent: Some("in:51.0".into()),
        settle: Duration::from_secs(2),
        ..Bench::default()
    }
}

#[test]
fn the_harness_passes_the_driver_against_the_simulated_e1213() {
    let s = E1213Sim::start().unwrap();
    let mut d = driver(&s);
    let checks = harness::run(&mut d, &e1213_bench(), Some(&s));
    assert!(
        checks.iter().all(|c| c.outcome == Outcome::Pass),
        "{}",
        harness::render(&checks)
    );
    assert_eq!(checks.len(), 14, "{}", harness::render(&checks));
}

/// A driver that claims every write landed and never sends it.
struct Liar(EnipDriver);

impl Driver for Liar {
    fn parse(&self, addr: &str) -> Result<PointInfo, Error> {
        self.0.parse(addr)
    }
    fn read(&mut self, addrs: &[&str]) -> Result<Vec<Value>, Error> {
        self.0.read(addrs)
    }
    fn write(&mut self, _: &str, _: Value) -> Result<(), Error> {
        Ok(())
    }
}

#[test]
fn the_harness_catches_a_driver_that_pretends_to_write() {
    let s = E1213Sim::start().unwrap();
    let mut d = Liar(driver(&s));
    let bench = Bench {
        settle: Duration::from_millis(400),
        ..e1213_bench()
    };
    let checks = harness::run(&mut d, &bench, None);
    let failed: Vec<_> = checks
        .iter()
        .filter(|c| matches!(c.outcome, Outcome::Fail(_)))
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(
        failed,
        [
            "loop do:0 -> di:0",
            "loop do:1 -> di:1",
            "loop do:2 -> di:2",
            "loop do:3 -> di:3"
        ]
    );
}

#[test]
fn inputs_counters_and_do_status_land_where_the_manual_says() {
    let s = E1213Sim::start().unwrap();
    let mut d = driver(&s);
    s.force_di(9, true);
    wait_value(&mut d, "di:9", Value::Bool(true), Duration::from_secs(2));
    assert_eq!(read(&mut d, "in:1.1").unwrap(), Value::Bool(true));
    assert_eq!(read(&mut d, "in:0:u16").unwrap(), Value::Int(1 << 9));
    // DI-09's counter at 2 + 4 * 9, one rising edge.
    assert_eq!(read(&mut d, "in:38:u32").unwrap(), Value::Int(1));

    d.write("out:0:u8", Value::Int(0b1010_0101)).unwrap();
    wait_value(
        &mut d,
        "in:50:u8",
        Value::Int(0b1010_0101),
        Duration::from_secs(2),
    );
    assert_eq!(read(&mut d, "do:5").unwrap(), Value::Bool(true));
    assert_eq!(read(&mut d, "in:50.7").unwrap(), Value::Bool(true));
    // DO-00 and DO-02 are wired to DI-00 and DI-02.
    wait_value(
        &mut d,
        "in:0:u16",
        Value::Int(0b10_0000_0101),
        Duration::from_secs(2),
    );
    assert!(
        lock_ref(&s.device()).scanner_running(),
        "O->T carries the run header, in run"
    );
}

#[test]
fn point_errors_leave_the_link_up() {
    let s = E1213Sim::start().unwrap();
    let mut d = driver(&s);
    read(&mut d, "di:0").unwrap();
    for (addr, value) in [
        ("in:51.0", None),
        ("in:50:u16", None),
        ("di:12", None),
        ("do:8", None),
        ("out:0:u8", Some(Value::Int(256))),
        ("out:0:i8", Some(Value::Int(-129))),
    ] {
        let e = match value {
            None => read(&mut d, addr).unwrap_err(),
            Some(v) => d.write(addr, v).unwrap_err(),
        };
        assert!(matches!(e, Error::Point(_)), "{addr}: {e}");
        assert!(!e.is_link(), "{addr}: {e}");
    }
    assert!(matches!(
        d.write("di:0", Value::Bool(true)),
        Err(Error::ReadOnly(_))
    ));
    assert!(matches!(d.write("do:0", Value::Int(1)), Err(Error::Type { .. })));
    assert!(matches!(read(&mut d, "dx:0"), Err(Error::Address(_))));
    read(&mut d, "di:0").unwrap();
    assert_eq!(d.opens, 1, "no point error reopened the connection");
}

#[test]
fn a_lost_connection_is_a_link_error_and_comes_back_with_its_outputs() {
    let s = E1213Sim::start().unwrap();
    let mut d = driver(&s);
    d.write("do:1", Value::Bool(true)).unwrap();
    wait_value(&mut d, "di:1", Value::Bool(true), Duration::from_secs(2));

    s.inject(Fault::Reboot);
    // What a power cycle leaves: no connection, outputs off.
    lock_ref(&s.device()).write_assembly(102, &[0]).unwrap();
    let e = wait_error(&mut d, "di:1", Duration::from_secs(3));
    assert!(e.is_link(), "{e}");
    s.clear_faults();

    // The next read reopens; the first O->T packet already carries DO-01.
    read(&mut d, "di:5").unwrap();
    assert_eq!(lock_ref(&s.device()).assembly(102), Some(&[0b10][..]));
    wait_value(&mut d, "di:1", Value::Bool(true), Duration::from_secs(2));
    assert_eq!(d.opens, 2);
}

#[test]
fn a_silent_target_times_out_and_its_ownership_is_released() {
    let s = E1213Sim::start().unwrap();
    let mut d = driver(&s);
    read(&mut d, "di:0").unwrap();
    s.inject(Fault::Silent);
    let started = Instant::now();
    let e = wait_error(&mut d, "di:0", Duration::from_secs(3));
    assert!(e.is_link() && e.to_string().contains("timed out"), "{e}");
    // 100 ms RPI x 2^(1+2): no sooner than the connection's timeout.
    assert!(
        started.elapsed() >= Duration::from_millis(700),
        "{:?}",
        started.elapsed()
    );
    // The driver closed it: the target holds no connection, so the
    // exclusive owner can be taken again at once.
    assert_eq!(lock_ref(&s.device()).connections().len(), 0);
    s.clear_faults();
    read(&mut d, "di:0").unwrap();
}

#[test]
fn t_to_o_of_the_wrong_size_is_a_link_error() {
    let s = E1213Sim::start().unwrap();
    let mut d = driver(&s);
    read(&mut d, "di:0").unwrap();
    for f in [Fault::ShortInput, Fault::Garbage] {
        s.inject(f);
        let e = wait_error(&mut d, "di:0", Duration::from_secs(2));
        assert!(
            matches!(e, Error::Protocol(_)) && e.to_string().contains("51"),
            "{f:?}: {e}"
        );
        s.clear_faults();
        read(&mut d, "di:0").unwrap();
    }
}

#[test]
fn a_second_owner_is_refused_until_the_first_lets_go() {
    let s = E1213Sim::start().unwrap();
    let mut a = driver(&s);
    a.write("do:2", Value::Bool(true)).unwrap();
    let mut b = driver(&s);
    let e = read(&mut b, "di:0").unwrap_err();
    assert!(
        e.is_link() && e.to_string().contains("0x0106"),
        "ownership conflict: {e}"
    );

    // An input-only connection may share the inputs meanwhile, and
    // drives nothing.
    let mut watcher =
        EnipDriver::from_params(&s.endpoint(), &with(&s, &[("connection", "input-only")])).unwrap();
    wait_value(&mut watcher, "di:2", Value::Bool(true), Duration::from_secs(2));
    assert!(matches!(
        watcher.write("do:0", Value::Bool(true)),
        Err(Error::Point(_))
    ));

    drop(a); // Forward_Close
    read(&mut b, "di:0").unwrap();
    wait_value(&mut b, "di:2", Value::Bool(false), Duration::from_secs(2));
}

#[test]
fn refused_forward_opens_are_link_errors_with_the_devices_code() {
    let s = E1213Sim::start().unwrap();
    // Sizes the device doesn't have: O->T 0x0127, T->O 0x0128, each with
    // the size it wants.
    for (extra, code) in [
        (vec![("o2t", "102:2"), ("t2o", "101:51")], "0x0127"),
        (vec![("o2t", "102:1"), ("t2o", "101:50")], "0x0128"),
        (vec![("o2t", "103:1"), ("t2o", "101:51")], "0x012F"),
        (
            vec![("o2t", "102:1"), ("t2o", "101:51"), ("rpi_ms", "50")],
            "0x0111",
        ),
        (
            vec![("o2t", "102:1"), ("t2o", "101:51"), ("rpi_ms", "4000")],
            "0x0111",
        ),
    ] {
        let mut p = with(&s, &[("config", "100")]);
        p.remove("profile");
        for (k, v) in extra {
            p.insert(k.into(), v.into());
        }
        let mut d = EnipDriver::from_params(&s.endpoint(), &p).unwrap();
        let e = read(&mut d, "in:0.0").unwrap_err();
        assert!(e.is_link() && e.to_string().contains(code), "{p:?}: {e}");
    }
    assert_eq!(lock_ref(&s.device()).connections().len(), 0);
}

#[test]
fn an_electronic_key_refuses_another_revision() {
    let mut cfg = DeviceConfig::moxa_e1213();
    cfg.identity.revision = (3, 0);
    let s = E1213Sim::start_with(cfg, Application::bench()).unwrap();
    let mut d = EnipDriver::from_params(&s.endpoint(), &with(&s, &[("key", "exact")])).unwrap();
    let e = read(&mut d, "di:0").unwrap_err();
    assert!(e.is_link() && e.to_string().contains("0x0116"), "{e}");

    let s = E1213Sim::start().unwrap();
    let mut d = EnipDriver::from_params(&s.endpoint(), &with(&s, &[("key", "exact")])).unwrap();
    read(&mut d, "di:0").unwrap();
}

#[test]
fn nobody_home_is_a_link_error() {
    let s = E1213Sim::start().unwrap();
    let params = s.params();
    let endpoint = s.endpoint();
    drop(s);
    let mut d = EnipDriver::from_params(&endpoint, &params).unwrap();
    let e = read(&mut d, "di:0").unwrap_err();
    assert!(e.is_link(), "{e}");
    let e = d.write("do:0", Value::Bool(true)).unwrap_err();
    assert!(e.is_link(), "{e}");
}

static TMP: AtomicU64 = AtomicU64::new(0);

fn temp_eds(text: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!(
        "helmsman-enip-{}-{}.eds",
        std::process::id(),
        TMP.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&p, text).unwrap();
    p
}

#[test]
fn an_eds_file_gives_the_profiles_connection() {
    let eds = helmsman_proto::eds::Eds::parse(FACTS).unwrap();
    for c in [Connection::ExclusiveOwner, Connection::InputOnly] {
        let mut from_eds = Spec::from_eds(&eds, c).unwrap();
        let profile = Spec::moxa_e1213(c);
        // Channel counts come from the manual, not the EDS.
        from_eds.di_channels = profile.di_channels;
        from_eds.do_channels = profile.do_channels;
        assert_eq!(from_eds, profile, "{c:?}");
    }

    let s = E1213Sim::start().unwrap();
    let path = temp_eds(FACTS);
    let mut p = with(&s, &[("eds", path.to_str().unwrap())]);
    p.remove("profile");
    let mut d = EnipDriver::from_params(&s.endpoint(), &p).unwrap();
    d.write("do:3", Value::Bool(true)).unwrap();
    wait_value(&mut d, "di:3", Value::Bool(true), Duration::from_secs(2));
    drop(d);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn bad_params_are_refused_before_any_traffic() {
    let s = E1213Sim::start().unwrap();
    let missing = std::env::temp_dir().join("helmsman-no-such.eds");
    let not_eds = temp_eds("this is not an EDS");
    for (extra, remove_profile, what) in [
        (vec![("rpi_ms", "50")], false, "outside the device's 100..=3000"),
        (vec![("rpi_ms", "3001")], false, "outside"),
        (vec![("rpi_ms", "0")], false, "at least 1"),
        (vec![("multiplier", "8")], false, "0..=7"),
        (vec![("profile", "acme-9000")], false, "known profiles"),
        (vec![("eds", "x.eds")], false, "exactly one"),
        (vec![("bogus", "1")], false, "not an enip-io param"),
        (
            vec![("connection", "listen-only")],
            false,
            "exclusive-owner or input-only",
        ),
        (vec![("run_idle", "off")], false, "only with explicit"),
        (vec![], true, "exactly one"),
        (vec![("o2t", "102:1")], true, "explicit needs"),
        (
            vec![("config", "100"), ("o2t", "102"), ("t2o", "101:51")],
            true,
            "POINT:BYTES",
        ),
        (
            vec![("config", "100"), ("o2t", "102:1"), ("t2o", "101:600")],
            true,
            "at most 505",
        ),
        (
            vec![
                ("config", "100"),
                ("o2t", "102:1"),
                ("t2o", "101:51"),
                ("key", "exact"),
            ],
            true,
            "needs an identity",
        ),
        (
            vec![("eds", missing.to_str().unwrap())],
            true,
            "helmsman-no-such.eds",
        ),
        (vec![("eds", not_eds.to_str().unwrap())], true, "EDS line 1"),
    ] {
        let mut p = s.params();
        if remove_profile {
            p.remove("profile");
        }
        for (k, v) in extra {
            p.insert(k.into(), v.into());
        }
        let e = EnipDriver::from_params(&s.endpoint(), &p).unwrap_err();
        assert!(
            matches!(e, Error::Address(_)) && e.to_string().contains(what),
            "{p:?}: {e}"
        );
    }
    std::fs::remove_file(not_eds).unwrap();
    assert_eq!(lock_ref(&s.device()).connections().len(), 0);
}

fn device(params: &[(&str, &str)]) -> Device {
    Device {
        id: "rio".into(),
        driver: "enip-io".into(),
        endpoint: "192.168.3.60".into(),
        params: params.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect(),
    }
}

fn signal(name: &str, ty: Ty, dir: Dir, point: &str) -> Signal {
    Signal {
        name: name.into(),
        ty,
        dir,
        binding: Binding::Device {
            device: "rio".into(),
            point: point.into(),
        },
        unit: None,
        poll: None,
    }
}

#[test]
fn the_map_checks_points_against_the_profile() {
    let map = |params: &[(&str, &str)], signals: Vec<Signal>| IoMap {
        devices: vec![device(params)],
        signals,
    };
    let ok = map(
        &[("profile", "moxa-e1213")],
        vec![
            signal("a", Ty::Bool, Dir::In, "di:11"),
            signal("b", Ty::Int, Dir::In, "in:46:u32"),
            signal("c", Ty::Bool, Dir::Out, "do:7"),
            signal("d", Ty::Bool, Dir::Out, "out:0.0"),
        ],
    );
    assert_eq!(ok.validate(), Ok(()));

    let errs = map(
        &[("profile", "moxa-e1213")],
        vec![
            signal("a", Ty::Bool, Dir::In, "di:12"),
            signal("b", Ty::Int, Dir::In, "in:48:u32"),
            signal("c", Ty::Bool, Dir::Out, "do:0"),
            signal("d", Ty::Bool, Dir::Out, "out:0.0"),
            signal("e", Ty::Int, Dir::Out, "out:0:u8"),
            signal("f", Ty::Real, Dir::In, "di:1"),
        ],
    )
    .validate()
    .unwrap_err();
    let shown: Vec<String> = errs.iter().map(ToString::to_string).collect();
    assert!(
        shown[0].contains("di:12") && shown[0].contains("12 such channels"),
        "{shown:?}"
    );
    assert!(
        shown[1].contains("past the end of the 51-byte input"),
        "{shown:?}"
    );
    assert!(
        matches!(&errs[2], MapError::TwoWriters { a, b } if a == "c" && b == "d"),
        "{shown:?}"
    );
    assert!(
        matches!(&errs[3], MapError::TwoWriters { a, b } if a == "c" && b == "e"),
        "{shown:?}"
    );
    assert!(matches!(errs[4], MapError::TypeMismatch { .. }), "{shown:?}");
    assert_eq!(errs.len(), 5, "{shown:?}");

    let input_only = map(
        &[("profile", "moxa-e1213"), ("connection", "input-only")],
        vec![signal("c", Ty::Bool, Dir::Out, "do:0")],
    )
    .validate()
    .unwrap_err();
    assert!(
        input_only.iter().any(|e| matches!(e, MapError::NotWritable(_))
            || e.to_string().contains("input-only connection has no outputs")),
        "{input_only:?}"
    );
    let bad_params = map(
        &[("profile", "nope")],
        vec![signal("a", Ty::Bool, Dir::In, "di:0")],
    )
    .validate()
    .unwrap_err();
    assert!(
        bad_params[0].to_string().contains("known profiles"),
        "{bad_params:?}"
    );
}

#[test]
fn point_syntax() {
    for good in [
        "di:0",
        "do:65535",
        "in:0.7",
        "out:3.0",
        "in:2:u32",
        "out:0:f32",
        "in:9:i8",
    ] {
        Point::parse(good).unwrap_or_else(|e| panic!("{good}: {e}"));
    }
    for bad in [
        "",
        "di",
        "di:",
        "di:-1",
        "di:+1",
        "di:65536",
        "di:0x1",
        "in:0.8",
        "in:0",
        "in:0:u64",
        "in:.1",
        "out:1.",
        "DI:0",
        "di:0 ",
        "in:0:u8:x",
        "ai:0",
        "in:0.1.2",
        "do:1:u8",
    ] {
        assert!(matches!(Point::parse(bad), Err(Error::Address(_))), "{bad:?}");
    }
}

proptest! {
    #[test]
    fn point_parse_never_panics(s in "\\PC{0,24}") {
        let _ = Point::parse(&s);
        let _ = helmsman_io::enip::parse_on(&BTreeMap::from([("profile".into(), "moxa-e1213".into())]), &s);
    }

    #[test]
    fn params_never_panic(kv in proptest::collection::btree_map("[a-z_0-9]{1,14}", "[ -~]{0,12}", 0..6)) {
        let _ = helmsman_io::enip::Params::parse(&kv);
    }

    /// Whatever fits a format reads back as written; whatever doesn't is
    /// a point error and leaves the image as it was.
    #[test]
    fn values_round_trip_through_the_output_image(n in any::<i64>(), fmt in 0usize..6, at in 0u16..4) {
        let fmts = ["u8", "i8", "u16", "i16", "u32", "i32"];
        let (lo, hi): (i64, i64) = [
            (0, 255), (-128, 127), (0, 65535), (-32768, 32767),
            (0, 0xFFFF_FFFF), (i64::from(i32::MIN), i64::from(i32::MAX)),
        ][fmt];
        let width = [1u16, 1, 2, 2, 4, 4][fmt];
        let p = Point::parse(&format!("out:{at}:{}", fmts[fmt])).unwrap();
        let mut image = [0x5Au8; 6];
        let fits_image = usize::from(at + width) <= image.len();
        match p.write_into(&mut image, Value::Int(n)) {
            Ok(()) => {
                prop_assert!((lo..=hi).contains(&n) && fits_image);
                prop_assert_eq!(p.read_from(&image), Some(Value::Int(n)));
            }
            Err(Error::Point(_)) => {
                prop_assert!(!(lo..=hi).contains(&n) || !fits_image);
                prop_assert_eq!(image, [0x5A; 6]);
            }
            Err(e) => prop_assert!(false, "{e}"),
        }
        // The E1213's output is one byte: only u8/i8 at 0 fit it.
        let spec = Spec::moxa_e1213(Connection::ExclusiveOwner);
        prop_assert_eq!(spec.check("x", &p).is_ok(), width == 1 && at == 0);
    }

    #[test]
    fn bits_touch_only_their_bit(byte in 0u16..3, bit in 0u8..8, on in any::<bool>(), fill in any::<u8>()) {
        let p = Point::parse(&format!("out:{byte}.{bit}")).unwrap();
        let mut image = [fill; 3];
        p.write_into(&mut image, Value::Bool(on)).unwrap();
        for (i, b) in image.iter().enumerate() {
            let want = if i == usize::from(byte) {
                if on { fill | (1 << bit) } else { fill & !(1 << bit) }
            } else {
                fill
            };
            prop_assert_eq!(*b, want);
        }
        prop_assert_eq!(p.read_from(&image), Some(Value::Bool(on)));
        prop_assert_eq!(p.out_bits(), vec![format!("{byte}.{bit}")]);
    }
}

#[test]
fn reals_need_to_fit_f32() {
    let p = Point::parse("out:0:f32").unwrap();
    for (x, ok) in [
        (1.5, true),
        (-0.0, true),
        (f64::NAN, false),
        (f64::INFINITY, false),
        (1e39, false),
    ] {
        let mut image = [0u8; 4];
        assert_eq!(p.write_into(&mut image, Value::Real(x)).is_ok(), ok, "{x}");
    }
    let mut image = [0u8; 4];
    p.write_into(&mut image, Value::Real(1.5)).unwrap();
    assert_eq!(image, 1.5f32.to_le_bytes());
    assert_eq!(p.read_from(&image), Some(Value::Real(1.5)));
    assert!(matches!(
        p.write_into(&mut image, Value::Int(1)),
        Err(Error::Type { .. })
    ));
    assert_eq!(p.read_from(&image[..3]), None);
}

#[test]
fn faults_the_e1213_simulator_lists_are_its_own() {
    let s = E1213Sim::start().unwrap();
    let names: Vec<_> = s.fault_checks().into_iter().map(|(n, _)| n).collect();
    assert_eq!(
        names,
        ["silent target", "short input", "garbage input", "target reboots"]
    );
}

/// The device doc's example maps parse, validate, and bind the same
/// signal names over both transports.
#[test]
fn the_e1213_example_maps_validate() {
    let enip = helmsman_io::text::parse(include_str!("../../../docs/devices/moxa-iologik-e1213-enip.map")).unwrap();
    let modbus =
        helmsman_io::text::parse(include_str!("../../../docs/devices/moxa-iologik-e1213-modbus.map")).unwrap();
    enip.map.validate().unwrap();
    modbus.map.validate().unwrap();
    let names = |m: &IoMap| m.signals.iter().map(|s| (s.name.clone(), s.ty, s.dir)).collect::<Vec<_>>();
    assert_eq!(names(&enip.map), names(&modbus.map));
}
