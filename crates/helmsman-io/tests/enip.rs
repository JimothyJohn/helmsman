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

    // The next read reopens and the outputs go back out with it; the first
    // O->T packet can land after that read returns.
    read(&mut d, "di:5").unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while lock_ref(&s.device()).assembly(102) != Some(&[0b10][..]) {
        assert!(Instant::now() < deadline, "DO-01 never came back");
        std::thread::sleep(Duration::from_millis(5));
    }
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
    // b's refused connect is tried again after a wait, not on the very
    // next read (`a_device_that_keeps_refusing_is_not_asked_every_read`).
    wait_value(&mut b, "di:2", Value::Bool(false), Duration::from_secs(4));
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
    let enip =
        helmsman_io::text::parse(include_str!("../../../docs/devices/moxa-iologik-e1213-enip.map")).unwrap();
    let modbus = helmsman_io::text::parse(include_str!(
        "../../../docs/devices/moxa-iologik-e1213-modbus.map"
    ))
    .unwrap();
    enip.map.validate().unwrap();
    modbus.map.validate().unwrap();
    let names = |m: &IoMap| {
        m.signals
            .iter()
            .map(|s| (s.name.clone(), s.ty, s.dir))
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&enip.map), names(&modbus.map));
}

/// On a box: two `enip-io` links and the box's own adapter consume on one
/// I/O endpoint, with no `io_port` anywhere; each link's O->T leaves from
/// the shared port and its T->O comes back to it.
#[test]
fn links_and_the_adapter_share_the_boxs_one_io_port() {
    use helmsman_adapter::{Adapter, Device as Target, ServerConfig};
    use helmsman_proto::endpoint::IoEndpoint;
    use std::net::{Ipv4Addr, SocketAddrV4};

    let ep = IoEndpoint::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).unwrap();
    let _adapter = Adapter::start_on(
        Target::new(DeviceConfig::opener_sample()),
        &ServerConfig {
            bind: Ipv4Addr::LOCALHOST,
            encap_port: 0,
            ..ServerConfig::default()
        },
        &ep,
    )
    .unwrap();
    let sims = [E1213Sim::start().unwrap(), E1213Sim::start().unwrap()];
    let mut links: Vec<Box<dyn Driver + Send>> = sims
        .iter()
        .map(|s| {
            let mut params = s.params();
            params.remove("io_port");
            let device = Device {
                id: "rio".into(),
                driver: "enip-io".into(),
                endpoint: s.endpoint(),
                params,
            };
            helmsman_io::open_on(&device, Some(&ep)).unwrap()
        })
        .collect();
    for (i, (s, d)) in sims.iter().zip(links.iter_mut()).enumerate() {
        s.force_di(9, i == 1);
        let deadline = Instant::now() + Duration::from_secs(3);
        while d.read(&["di:9"]).ok() != Some(vec![Value::Bool(i == 1)]) {
            assert!(Instant::now() < deadline, "link {i} never read its DI-09");
            std::thread::sleep(Duration::from_millis(5));
        }
        // Inputs can arrive before the link's first O->T packet does.
        while s.o_to_t_sources().is_empty() {
            assert!(Instant::now() < deadline, "link {i} never sent O->T");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(s.o_to_t_sources(), vec![ep.local_addr()], "link {i}");
    }
    assert_eq!(ep.stats().wrong_peer, 0);
}

/// Someone else holds the UDP port the inputs arrive on: a link error
/// that says which port and why.
#[test]
fn a_taken_io_port_is_a_link_error_that_says_so() {
    let s = E1213Sim::start().unwrap();
    let squatter = std::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, 0)).unwrap();
    let port = squatter.local_addr().unwrap().port().to_string();
    let mut d = EnipDriver::from_params(&s.endpoint(), &with(&s, &[("io_port", &port)])).unwrap();
    let e = read(&mut d, "di:0").unwrap_err();
    assert!(e.is_link(), "{e}");
    assert!(
        e.to_string()
            .contains(&format!("UDP {port}, where the device sends its inputs")),
        "{e}"
    );
}

/// `device-by-identity`: a `cip=VENDOR:PRODUCT` endpoint is found by one
/// `ListIdentity` (sent where `sweep=` says: here the simulated E1213's
/// own address, a broadcast on a box) and the link runs on the address it
/// answered from; the driver reports that address. A wrong identity is
/// not found, and says how many devices answered.
#[test]
fn a_cip_identity_endpoint_is_found_by_list_identity_and_reports_its_address() {
    let sim = E1213Sim::start().unwrap();
    let encap: std::net::SocketAddrV4 = sim.endpoint().parse().unwrap();
    let resolved = helmsman_io::resolve::resolve(
        &helmsman_io::resolve::Identity::Cip {
            vendor: 991,
            product: 515,
            serial: None,
        },
        encap.port(),
        &helmsman_io::resolve::Resolver {
            arp: Box::new(|| Ok(String::new())),
            sweep: Some(encap),
            window: Duration::from_millis(300),
            modbus: None,
        },
    )
    .unwrap();
    assert_eq!(resolved, encap, "found where the simulator answers");
    let miss = helmsman_io::resolve::resolve(
        &helmsman_io::resolve::Identity::Cip {
            vendor: 991,
            product: 999,
            serial: None,
        },
        encap.port(),
        &helmsman_io::resolve::Resolver {
            arp: Box::new(|| Ok(String::new())),
            sweep: Some(encap),
            window: Duration::from_millis(300),
            modbus: None,
        },
    )
    .unwrap_err();
    assert!(miss.to_string().contains("no such device among the 1"), "{miss}");
    // A serial that does not match is a miss too.
    let miss = helmsman_io::resolve::resolve(
        &helmsman_io::resolve::Identity::Cip {
            vendor: 991,
            product: 515,
            serial: Some(1),
        },
        encap.port(),
        &helmsman_io::resolve::Resolver {
            arp: Box::new(|| Ok(String::new())),
            sweep: Some(encap),
            window: Duration::from_millis(300),
            modbus: None,
        },
    )
    .unwrap_err();
    assert!(miss.to_string().contains("no such device"), "{miss}");
}

/// `devices-repo`: a scan (one `ListIdentity`, here to the simulator's own
/// address) lists the simulated E1213 once, by identity, with the MAC the
/// ARP table gives for its address and the product name as text.
#[test]
fn a_scan_lists_the_simulated_e1213_once_with_its_identity() {
    let sim = E1213Sim::start().unwrap();
    let encap: std::net::SocketAddrV4 = sim.endpoint().parse().unwrap();
    let table = format!(
        "? ({}) at 0:90:e8:7e:af:be on lo0 ifscope [ethernet]\n",
        encap.ip()
    );
    let seen = helmsman_io::resolve::discover(&helmsman_io::resolve::Resolver {
        arp: Box::new(move || Ok(table.clone())),
        sweep: Some(encap),
        window: Duration::from_millis(300),
        modbus: None,
    })
    .unwrap();
    assert_eq!(seen.len(), 1, "{seen:?}");
    let s = &seen[0];
    assert_eq!(s.address, *encap.ip());
    assert_eq!((s.vendor_id, s.product_code, s.device_type), (991, 515, 12));
    assert_eq!(s.product_name, "E1213");
    assert_eq!(s.mac, Some([0x00, 0x90, 0xe8, 0x7e, 0xaf, 0xbe]));
    assert_eq!(s.cip(), "cip=991:515:0x00e12130");
    // No ARP entry: the device is still listed, without a MAC.
    let seen = helmsman_io::resolve::discover(&helmsman_io::resolve::Resolver {
        arp: Box::new(|| Err(std::io::Error::other("no arp here"))),
        sweep: Some(encap),
        window: Duration::from_millis(300),
        modbus: None,
    })
    .unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].mac, None);
}

fn scan_resolver(encap: std::net::SocketAddrV4) -> helmsman_io::resolve::Resolver {
    helmsman_io::resolve::Resolver {
        arp: Box::new(|| Ok(String::new())),
        sweep: Some(encap),
        window: Duration::from_millis(300),
        modbus: None,
    }
}

/// `scan-every-interface`: a sweep asked from two local sockets, both
/// reaching the one device, lists it once; with no local address to ask
/// from, one unbound socket asks and the device is listed all the same.
#[test]
fn a_sweep_from_two_sockets_lists_a_device_once_and_none_falls_back() {
    let sim = E1213Sim::start().unwrap();
    let encap: std::net::SocketAddrV4 = sim.endpoint().parse().unwrap();
    let lo = *encap.ip();
    let r = scan_resolver(encap);
    let seen = helmsman_io::resolve::discover_from(&r, &[lo, lo]).unwrap();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].address, lo);
    let seen = helmsman_io::resolve::discover_from(&r, &[]).unwrap();
    assert_eq!(seen.len(), 1, "{seen:?}");
}

/// A source that cannot be bound (an address this host does not have) is
/// passed over, and when none works the unbound ask still finds the device.
#[test]
fn a_source_that_cannot_be_bound_is_passed_over() {
    let sim = E1213Sim::start().unwrap();
    let encap: std::net::SocketAddrV4 = sim.endpoint().parse().unwrap();
    let gone = std::net::Ipv4Addr::new(192, 0, 2, 77); // TEST-NET-1: never ours
    let r = scan_resolver(encap);
    let seen = helmsman_io::resolve::discover_from(&r, &[gone, *encap.ip()]).unwrap();
    assert_eq!(seen.len(), 1, "{seen:?}");
    let seen = helmsman_io::resolve::discover_from(&r, &[gone]).unwrap();
    assert_eq!(seen.len(), 1, "falls back to the unbound ask: {seen:?}");
}

/// A direct ask finds the device at its address; a group address is
/// refused before anything is sent, and so is a box with no sweep.
#[test]
fn a_direct_ask_finds_the_device_and_refuses_group_addresses() {
    let sim = E1213Sim::start().unwrap();
    let encap: std::net::SocketAddrV4 = sim.endpoint().parse().unwrap();
    let r = scan_resolver(encap);
    let s = helmsman_io::resolve::discover_one(&r, *encap.ip())
        .unwrap()
        .unwrap();
    assert_eq!((s.vendor_id, s.product_code), (991, 515));
    for bad in [
        std::net::Ipv4Addr::BROADCAST,
        std::net::Ipv4Addr::UNSPECIFIED,
        std::net::Ipv4Addr::new(224, 0, 0, 1),
        std::net::Ipv4Addr::new(239, 255, 255, 250),
    ] {
        let e = helmsman_io::resolve::discover_one(&r, bad).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput, "{bad}");
    }
    let off = helmsman_io::resolve::Resolver {
        sweep: None,
        ..scan_resolver(encap)
    };
    assert!(helmsman_io::resolve::discover_one(&off, *encap.ip()).is_err());
}

/// The E1213's layout with a 3-byte configuration assembly: a block that
/// takes its channel setup from the controller, as the Murrelektronik
/// IMPACT67 E DIO8 does with its instance 109 out of the box.
fn configurable() -> E1213Sim {
    let mut c = DeviceConfig::moxa_e1213();
    c.assemblies[0] = (100, 3);
    E1213Sim::start_with(c, Application::bench()).unwrap()
}

fn explicit(s: &E1213Sim, extra: &[(&str, &str)]) -> BTreeMap<String, String> {
    let mut p = with(s, &[("config", "100"), ("o2t", "102:1"), ("t2o", "101:51")]);
    p.remove("profile");
    for (k, v) in extra {
        p.insert((*k).into(), (*v).into());
    }
    p
}

#[test]
fn configuration_data_reaches_the_device_and_again_after_a_reconnect() {
    let s = configurable();
    let p = explicit(&s, &[("config_data", "00Ff01")]);
    let mut d = EnipDriver::from_params(&s.endpoint(), &p).unwrap();
    read(&mut d, "di:0").unwrap();
    assert_eq!(lock_ref(&s.device()).assembly(100), Some(&[0x00, 0xFF, 0x01][..]));

    // A power cycle: the connection gone, the channel setup back at the
    // device's own.
    s.inject(Fault::Reboot);
    lock_ref(&s.device()).write_assembly(100, &[0; 3]).unwrap();
    let e = wait_error(&mut d, "di:0", Duration::from_secs(3));
    assert!(e.is_link(), "{e}");
    s.clear_faults();
    read(&mut d, "di:0").unwrap();
    assert_eq!(d.opens, 2);
    assert_eq!(lock_ref(&s.device()).assembly(100), Some(&[0x00, 0xFF, 0x01][..]));
}

#[test]
fn no_configuration_data_leaves_the_devices_own() {
    let s = configurable();
    lock_ref(&s.device()).write_assembly(100, &[7, 8, 9]).unwrap();
    let mut d = EnipDriver::from_params(&s.endpoint(), &explicit(&s, &[])).unwrap();
    read(&mut d, "di:0").unwrap();
    assert_eq!(lock_ref(&s.device()).assembly(100), Some(&[7, 8, 9][..]));
}

#[test]
fn configuration_data_of_another_size_is_refused_with_the_devices_code() {
    let s = configurable();
    for hex in ["00", "0001", "0001020304", &"00".repeat(480)] {
        let p = explicit(&s, &[("config_data", hex)]);
        let mut d = EnipDriver::from_params(&s.endpoint(), &p).unwrap();
        let e = read(&mut d, "di:0").unwrap_err();
        assert!(
            e.is_link() && e.to_string().contains("0x0126 (invalid configuration size)"),
            "{hex}: {e}"
        );
        assert_eq!(lock_ref(&s.device()).assembly(100), Some(&[0; 3][..]), "{hex}");
    }
    assert_eq!(lock_ref(&s.device()).connections().len(), 0);
}

#[test]
fn bad_configuration_data_is_refused_before_any_traffic() {
    let s = configurable();
    for (hex, what) in [
        ("", "hex digits"),
        ("0", "hex digits"),
        ("000", "hex digits"),
        ("0g", "hex digits"),
        ("0x00", "hex digits"),
        ("00 ff", "hex digits"),
        ("+1", "hex digits"),
        ("\u{ff10}\u{ff10}", "hex digits"),
        (&"00".repeat(481), "hex digits"),
    ] {
        let e = EnipDriver::from_params(&s.endpoint(), &explicit(&s, &[("config_data", hex)])).unwrap_err();
        assert!(!e.is_link() && e.to_string().contains(what), "{hex:?}: {e}");
    }
    // The built-in profile has no configuration assembly to fill.
    let e = EnipDriver::from_params(&s.endpoint(), &with(&s, &[("config_data", "00")])).unwrap_err();
    assert!(e.to_string().contains("only with eds= or explicit"), "{e}");
    assert_eq!(lock_ref(&s.device()).connections().len(), 0);
}

/// The facts EDS with an exclusive owner that sends `Assem100` as
/// configuration; `members` is that assembly's member list.
fn eds_with_config(params: &str, members: &str) -> String {
    let eds = FACTS
        .replace(
            "    Assem101 = \"Input Data\"",
            &format!(
                "    Assem100 = \"Configuration\", , 3, 0x0001, ,, {members};\n    Assem101 = \"Input Data\""
            ),
        )
        .replace(
            "        ,,                              $ target config\n        \"Exclusive Owner\"",
            "        , Assem100,\n        \"Exclusive Owner\"",
        )
        .replace("[Assembly]", &format!("{params}\n[Assembly]"));
    assert!(
        eds.contains("Assem100") && eds.contains(", Assem100,"),
        "the fixture moved"
    );
    eds
}

const MODE_PARAMS: &str = "    Param10 = 0, ,, 0x0000, 0xC6, 1, \"Mode\", \"\", \"\", 0, 1, 1, ,,,, ,,,, ;
    Param11 = 0, ,, 0x0000, 0xC6, 1, \"Pin 4\", \"\", \"\", 0, 255, 240, ,,,, ,,,, ;
    Param12 = 0, ,, 0x0000, 0xC6, 1, \"Pin 2\", \"\", \"\", 0, 255, 0, ,,,, ,,,, ;";

#[test]
fn an_eds_with_parameter_defaults_sends_them_as_configuration() {
    let s = configurable();
    let path = temp_eds(&eds_with_config(
        MODE_PARAMS,
        "8, Param10, 8, Param11, 8, Param12",
    ));
    let mut p = with(&s, &[("eds", path.to_str().unwrap())]);
    p.remove("profile");
    let mut d = EnipDriver::from_params(&s.endpoint(), &p).unwrap();
    read(&mut d, "di:0").unwrap();
    assert_eq!(lock_ref(&s.device()).assembly(100), Some(&[1, 240, 0][..]));
    drop(d);

    // config_data= replaces the defaults, and must be the EDS's size.
    p.insert("config_data".into(), "000f0f".into());
    let mut d = EnipDriver::from_params(&s.endpoint(), &p).unwrap();
    read(&mut d, "di:0").unwrap();
    assert_eq!(lock_ref(&s.device()).assembly(100), Some(&[0, 15, 15][..]));
    drop(d);
    p.insert("config_data".into(), "000f".into());
    let e = EnipDriver::from_params(&s.endpoint(), &p).unwrap_err();
    assert!(
        e.to_string().contains("2 bytes, the EDS's connection wants 3"),
        "{e}"
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn an_eds_that_wants_configuration_and_gives_no_defaults_asks_for_it() {
    let s = configurable();
    // Members the EDS does not spell out, a param without a default, a
    // default too wide for its bits, and members short of the size.
    for (params, members) in [
        ("", "24, "),
        (
            "    Param10 = 0, ,, 0x0000, 0xC6, 1, \"Mode\", \"\", \"\", 0, 1, , ,,,, ,,,, ;",
            "8, Param10, 16, ",
        ),
        (MODE_PARAMS, "4, Param11, 20, Param12"),
        (MODE_PARAMS, "8, Param10, 8, Param11, 8, Param99"),
    ] {
        let path = temp_eds(&eds_with_config(params, members));
        let mut p = with(&s, &[("eds", path.to_str().unwrap())]);
        p.remove("profile");
        let e = EnipDriver::from_params(&s.endpoint(), &p).unwrap_err();
        assert!(
            !e.is_link() && e.to_string().contains("wants 3 bytes of configuration data"),
            "{members:?}: {e}"
        );
        p.insert("config_data".into(), "010203".into());
        let mut d = EnipDriver::from_params(&s.endpoint(), &p).unwrap();
        read(&mut d, "di:0").unwrap();
        assert_eq!(
            lock_ref(&s.device()).assembly(100),
            Some(&[1, 2, 3][..]),
            "{members:?}"
        );
        drop(d);
        std::fs::remove_file(path).unwrap();
    }
}

/// What a box does when it stops: writes the safe values and lets the
/// driver go. The write changes the O->T image, which the producer sends
/// at its next RPI; closing before that left the device holding the old
/// outputs (a Murrelektronik IMPACT67 keeps its output assembly as it was
/// after a `Forward_Close`, 2026-11-16). The last image goes out before
/// the close.
#[test]
fn an_output_written_just_before_the_driver_goes_reaches_the_device() {
    let s = E1213Sim::start().unwrap();
    for round in 0..5 {
        let mut d = driver(&s);
        d.write("do:2", Value::Bool(true)).unwrap();
        wait_value(&mut d, "di:2", Value::Bool(true), Duration::from_secs(2));
        d.write("do:2", Value::Bool(false)).unwrap();
        drop(d);
        // The close is answered after the last packet was sent; the
        // simulator takes that packet off its socket within a tick.
        let deadline = Instant::now() + Duration::from_secs(2);
        while lock_ref(&s.device()).assembly(102) != Some(&[0][..]) {
            assert!(
                Instant::now() < deadline,
                "round {round}: DO-02 still on after the driver went"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(lock_ref(&s.device()).connections().len(), 0);
    }
}

/// A Novanta `MDrive` resets the TCP session about 2.5 s after the
/// `Forward_Open` and goes on exchanging I/O (2026-11-16). The link must
/// not notice, and when the driver goes the connection is still closed,
/// over a session of its own, instead of being left to time out with the
/// device's outputs held.
#[test]
fn a_device_that_drops_the_idle_session_keeps_its_link_and_is_still_closed() {
    let s = E1213Sim::start_idle(
        DeviceConfig::moxa_e1213(),
        Application::bench(),
        Duration::from_millis(150),
    )
    .unwrap();
    // Multiplier 3: the simulator would only time the connection out
    // after 3.2 s, far past the deadline below.
    let mut d = EnipDriver::from_params(&s.endpoint(), &with(&s, &[("multiplier", "3")])).unwrap();
    d.write("do:1", Value::Bool(true)).unwrap();
    wait_value(&mut d, "di:1", Value::Bool(true), Duration::from_secs(2));
    let deadline = Instant::now() + Duration::from_secs(3);
    while s.sessions() > 0 {
        assert!(Instant::now() < deadline, "the simulator kept the idle session");
        read(&mut d, "di:1").unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    // Still exchanging: an output written now reaches the device.
    d.write("do:0", Value::Bool(true)).unwrap();
    wait_value(&mut d, "di:0", Value::Bool(true), Duration::from_secs(2));
    assert_eq!(d.opens, 1);
    assert_eq!(lock_ref(&s.device()).connections().len(), 1);

    drop(d);
    let deadline = Instant::now() + Duration::from_millis(1500);
    while !lock_ref(&s.device()).connections().is_empty() {
        assert!(
            Instant::now() < deadline,
            "the connection was left open for the device to time out"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The facts EDS with its exclusive owner's O->T real-time format
/// declared modeless, as the Novanta `MDrive`'s EDS declares its own while
/// the drive takes a run/idle header (its `Forward_Open` at the EDS's size
/// is refused 0x0127 with the size it wants, four bytes more; 2026-11-16).
fn modeless_eds() -> std::path::PathBuf {
    let eds = FACTS.replace("0x44640405,", "0x44640005,");
    assert_ne!(eds, FACTS, "the fixture moved");
    temp_eds(&eds)
}

#[test]
fn an_eds_that_leaves_out_the_run_idle_header_is_corrected_from_the_refusal() {
    let s = E1213Sim::start().unwrap();
    let path = modeless_eds();
    let mut p = with(&s, &[("eds", path.to_str().unwrap())]);
    p.remove("profile");
    let mut d = EnipDriver::from_params(&s.endpoint(), &p).unwrap();
    assert!(!d.connection().o_to_t_run_idle, "the EDS says modeless");
    // The outputs arrive where they belong, not four bytes late.
    d.write("do:3", Value::Bool(true)).unwrap();
    wait_value(&mut d, "di:3", Value::Bool(true), Duration::from_secs(2));
    assert_eq!(lock_ref(&s.device()).assembly(102), Some(&[0b1000][..]));
    assert!(d.connection().o_to_t_run_idle);
    assert_eq!(d.opens, 2, "one refusal, one corrected open");
    assert_eq!(lock_ref(&s.device()).connections().len(), 1);
    drop(d);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn a_header_the_device_does_not_take_is_dropped_from_the_refusal() {
    // The other way round, both directions: a target without a run/idle
    // header O->T, and with one T->O, against explicit sizes that assume
    // the usual (header out, none in).
    let mut c = DeviceConfig::moxa_e1213();
    c.consumed_run_idle = false;
    c.produced_run_idle = true;
    let s = E1213Sim::start_with(c, Application::bench()).unwrap();
    let mut d = EnipDriver::from_params(&s.endpoint(), &explicit(&s, &[])).unwrap();
    // Inputs read where they are (the simulated wiring only follows an
    // owner whose header says run, so an input is held on instead), and
    // the output byte lands in the output assembly.
    s.force_di(2, true);
    wait_value(&mut d, "di:2", Value::Bool(true), Duration::from_secs(2));
    assert_eq!(read(&mut d, "di:1").unwrap(), Value::Bool(false));
    d.write("do:0", Value::Bool(true)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while lock_ref(&s.device()).assembly(102) != Some(&[1][..]) {
        assert!(Instant::now() < deadline, "DO-00 never arrived");
        std::thread::sleep(Duration::from_millis(5));
    }
    let spec = d.connection();
    assert!(!spec.o_to_t_run_idle && spec.t_to_o_run_idle);
    assert_eq!(d.opens, 3, "one refusal a direction, then the open");
}

#[test]
fn a_size_refusal_that_is_not_the_header_stays_a_refusal() {
    // Four bytes more wanted, but because the assembly is four bytes
    // longer than the map says: a run/idle header sent there would put
    // its run bit into the first output byte.
    let mut config = DeviceConfig::moxa_e1213();
    config.consumed_run_idle = false;
    config.assemblies[2] = (102, 5);
    let s = E1213Sim::start_with(config, Application::bench()).unwrap();
    let p = explicit(&s, &[("run_idle", "off")]);
    let mut d = EnipDriver::from_params(&s.endpoint(), &p).unwrap();
    let e = read(&mut d, "in:0.0").unwrap_err();
    assert!(e.is_link() && e.to_string().contains("0x0127"), "{e}");
    assert_eq!(d.opens, 1, "nothing was retried");
    assert!(!d.connection().o_to_t_run_idle);
    assert_eq!(lock_ref(&s.device()).assembly(102), Some(&[0; 5][..]));

    // Sizes off by something else are refused as before, once.
    let s = E1213Sim::start().unwrap();
    for (o2t, t2o, code) in [("102:3", "101:51", "0x0127"), ("102:1", "101:47", "0x0128")] {
        let p = explicit(&s, &[("o2t", o2t), ("t2o", t2o)]);
        let mut d = EnipDriver::from_params(&s.endpoint(), &p).unwrap();
        let e = read(&mut d, "in:0.0").unwrap_err();
        assert!(e.is_link() && e.to_string().contains(code), "{o2t} {t2o}: {e}");
        assert_eq!(d.opens, 1);
    }
    assert_eq!(lock_ref(&s.device()).connections().len(), 0);
}

/// A link on a port of its own asks the device, in the `Forward_Open`, to
/// send its inputs there. A Murrelektronik IMPACT67 E and a Novanta
/// `MDrive` ignore that and send to UDP 2222 (2026-11-16); the link then
/// sees an open connection with no data, and its error says what to look
/// at.
#[test]
fn no_data_on_a_port_of_the_links_own_names_the_port() {
    let s = E1213Sim::start().unwrap();
    s.inject(Fault::Silent);
    let mut d = driver(&s);
    let e = read(&mut d, "di:0").unwrap_err();
    let text = e.to_string();
    assert!(
        e.is_link() && text.contains("no T->O data within") && text.contains("to UDP 2222 whatever port"),
        "{text}"
    );
    assert_eq!(lock_ref(&s.device()).connections().len(), 0, "closed again");
}

/// A box's link asks its driver every scan, which is every few
/// milliseconds. Against a device that answers and refuses (another owner,
/// the wrong configuration instance) each ask used to be a TCP session
/// and a `Forward_Open`: 41 in 5 s against a Murrelektronik IMPACT67 on
/// 2026-11-16. A refused or failed connect is not tried again at once;
/// the reads in between say what the last attempt said.
#[test]
fn a_device_that_keeps_refusing_is_not_asked_every_read() {
    let s = E1213Sim::start().unwrap();
    // A size the device does not have: refused 0x0127 every time.
    let p = explicit(&s, &[("o2t", "102:3")]);
    let mut d = EnipDriver::from_params(&s.endpoint(), &p).unwrap();
    let until = Instant::now() + Duration::from_millis(700);
    let mut reads = 0u32;
    while Instant::now() < until {
        let e = read(&mut d, "in:0.0").unwrap_err();
        assert!(e.is_link() && e.to_string().contains("0x0127"), "{e}");
        reads += 1;
        std::thread::sleep(Duration::from_millis(2));
    }
    // 39 reads in 700 ms on a loaded macos-latest runner (2026-11-22): the
    // 2 ms sleep is coarse there. 20 is still several reads per open.
    assert!(reads >= 20, "the loop itself ran: {reads} reads");
    assert!(
        (2..=5).contains(&d.opens),
        "{} Forward_Opens for {reads} reads in 700 ms",
        d.opens
    );
    assert_eq!(lock_ref(&s.device()).connections().len(), 0);
}

/// The wait after a failed connect is short at first and has a ceiling,
/// so a device that comes back is found again within seconds, and a
/// connection that was up and is lost is reopened at the next read.
#[test]
fn a_device_that_comes_back_is_connected_again_without_being_asked_to() {
    let s = E1213Sim::start().unwrap();
    let mut first = driver(&s);
    read(&mut first, "di:0").unwrap();
    // A second owner is refused for as long as the first holds on.
    let mut second = driver(&s);
    for _ in 0..40 {
        let e = read(&mut second, "di:0").unwrap_err();
        assert!(e.is_link() && e.to_string().contains("0x0106"), "{e}");
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(first);
    // Nothing tells the second driver: its next attempt, a wait away,
    // gets in.
    wait_value(&mut second, "di:0", Value::Bool(false), Duration::from_secs(5));
    assert!(second.opens <= 6, "{} opens", second.opens);
}
