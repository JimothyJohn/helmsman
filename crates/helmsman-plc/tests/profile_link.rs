//! A device by its profile on the box (PROTOCOLS.md step 1): the map
//! names `device:moxa-e1213` and binds channels (`ch:DI-04`), the box
//! finds the project's `moxa-e1213.device` when the map loads, and the
//! link runs the same map over EtherNet/IP when the simulated E1213's one
//! connection is free and over Modbus TCP when another controller owns it,
//! reporting which. A profile that is missing or malformed, and a channel
//! the profile does not have, are map errors naming the file.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use helmsman_iec::{ImageSizes, compile};
use helmsman_io::enip::EnipDriver;
use helmsman_io::enip_sim::E1213Sim;
use helmsman_io::local::Channels;
use helmsman_io::sim::{Sim, State};
use helmsman_io::{Driver as _, Value};
use helmsman_plc::driver_dir::{self, Sources};
use helmsman_plc::link::LinkSet;
use helmsman_plc::{Module, Runtime, SignalTable, Window};

/// The program: the clamp follows the part sensor. It names channels'
/// signals and nothing of a protocol.
const LINE: &str = "
PROGRAM line
VAR_INPUT part : BOOL; END_VAR
VAR_OUTPUT clamp : BOOL; END_VAR
  clamp := part;
END_PROGRAM";

fn runtime(table: &SignalTable) -> Runtime {
    let program = compile(LINE, ImageSizes { i: 0, q: 0, m: 0 }).unwrap();
    let module = Module {
        name: "line".into(),
        program: program.into(),
        period: Duration::from_millis(10),
        i: Window::new(0, 0),
        q: Window::new(0, 0),
        params: Vec::new(),
    };
    Runtime::with_signals(0, 0, vec![module], table).unwrap()
}

/// Scans the program until `ok`, or panics after 5 s.
fn scan_until(rt: &mut Runtime, table: &Mutex<SignalTable>, what: &str, mut ok: impl FnMut() -> bool) {
    let (start, deadline) = (Instant::now(), Instant::now() + Duration::from_secs(5));
    while !ok() {
        assert!(Instant::now() < deadline, "{what}: not within 5 s");
        rt.tick_with(
            start.elapsed(),
            &[],
            &mut table.lock().unwrap_or_else(PoisonError::into_inner),
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

const E1213: &str = include_str!("../../../docs/devices/moxa-iologik-e1213.device");

fn tmp(name: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "helmsman-profile-link-{}-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        name
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("projects/line")).unwrap();
    dir
}

fn wait(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ok() {
        assert!(Instant::now() < deadline, "{what}: not within 5 s");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn port(endpoint: &str) -> &str {
    endpoint.rsplit_once(':').unwrap().1
}

/// The Modbus face of the unit, DO-00..03 wired to DI-00..03.
fn modbus() -> Sim {
    let mut s = State::new(64);
    s.wires = (0..4).map(|n| (n, n)).collect();
    Sim::start(s).unwrap()
}

/// The map: one device line naming the profile, channels by name, no
/// protocol anywhere but the per-transport ports that reach the two
/// simulators on loopback.
fn map(enip: &E1213Sim, mb: &Sim) -> String {
    let ports: Vec<String> = enip
        .params()
        .into_iter()
        .filter(|(k, _)| k != "profile" && k != "rpi_ms")
        .map(|(k, v)| format!("enip-io.{k}={v}"))
        .collect();
    format!(
        "device rio device:moxa-e1213 127.0.0.1 poll_ms=20 enip-io.port={} {} modbus-tcp.port={} modbus-tcp.timeout_ms=500\n\
         in  part  bool rio ch:DI-04\n\
         in  fb    bool rio ch:DI-01\n\
         out clamp bool rio ch:DO-01 safe=false\n",
        port(&enip.endpoint()),
        ports.join(" "),
        port(&mb.endpoint())
    )
}

type Reports = Arc<Mutex<Vec<(String, Option<String>, Option<String>)>>>;

/// Loads `text` through the project's folder and starts its links: the
/// table, the link set (kept alive), and every report (name, error,
/// transport).
fn start(root: &Path, text: &str) -> (Arc<Mutex<SignalTable>>, LinkSet, Reports, Runtime) {
    let project = root.join("projects/line");
    let sources = Sources {
        project: Some(("line".into(), project)),
        boxed: Some(root.join("drivers")),
    };
    let found = sources.find_text(Some(text));
    assert!(found.problems.is_empty(), "{found:?}");
    let layout = driver_dir::layout(Some(text), None, &found).unwrap();
    let rt = runtime(&layout.table);
    let table = Arc::new(Mutex::new(layout.table));
    let reports: Reports = Arc::default();
    let r = Arc::clone(&reports);
    let mut set = LinkSet::new(
        Arc::clone(&table),
        Arc::new(helmsman_io::open),
        Arc::new(|| Box::new(Channels::new())),
        Arc::new(
            move |name: &str,
                  err: Option<&helmsman_io::Error>,
                  _: Option<String>,
                  transport: Option<String>| {
                r.lock().unwrap_or_else(PoisonError::into_inner).push((
                    name.into(),
                    err.map(ToString::to_string),
                    transport,
                ));
            },
        ),
    );
    let plan = set
        .plan_with(layout.map.as_ref().unwrap(), &found.drivers)
        .unwrap();
    let stopped = set.stop(&plan);
    set.finish(plan, stopped);
    (table, set, reports, rt)
}

fn value(table: &Mutex<SignalTable>, name: &str) -> Option<Value> {
    let t = table.lock().unwrap_or_else(PoisonError::into_inner);
    t.index(name).and_then(|i| t.value(i))
}

/// The transport the last up report named, if any report said up.
fn last_up(reports: &Reports) -> Option<String> {
    reports
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .rev()
        .find(|(_, err, _)| err.is_none())
        .and_then(|(_, _, t)| t.clone())
}

#[test]
fn the_same_map_runs_over_ethernet_ip_then_over_modbus_when_another_controller_owns_the_unit() {
    let root = tmp("both");
    std::fs::write(root.join("projects/line/moxa-e1213.device"), E1213).unwrap();
    let (enip, mb) = (E1213Sim::start().unwrap(), modbus());
    let text = map(&enip, &mb);

    // The unit is free: EtherNet/IP, the profile's first transport.
    let (table, set, reports, mut rt) = start(&root, &text);
    enip.force_di(4, true);
    wait("part over EtherNet/IP", || {
        value(&table, "part") == Some(Value::Bool(true))
    });
    wait("the link reports its transport", || {
        last_up(&reports).as_deref() == Some("enip-io")
    });
    scan_until(
        &mut rt,
        &table,
        "the clamp reaches DO-01 and comes back on DI-01",
        || value(&table, "fb") == Some(Value::Bool(true)),
    );
    assert_eq!(mb.accepted(), 0, "Modbus was never opened");
    drop(set);
    drop(table);

    // Another controller owns the unit's one connection: the same map,
    // over Modbus, nothing changed but the world.
    let mut owner = EnipDriver::from_params(&enip.endpoint(), &enip.params()).unwrap();
    owner.read(&["di:0"]).unwrap();
    let (table, set, reports, mut rt) = start(&root, &text);
    mb.state().discrete[4] = true;
    wait("part over Modbus", || {
        value(&table, "part") == Some(Value::Bool(true))
    });
    wait("the link reports Modbus", || {
        last_up(&reports).as_deref() == Some("modbus-tcp")
    });
    scan_until(
        &mut rt,
        &table,
        "the clamp reaches coil 1 and comes back on DI-01",
        || value(&table, "fb") == Some(Value::Bool(true)),
    );
    assert!(mb.state().coils[1]);
    drop(set);
}

#[test]
fn a_missing_or_malformed_profile_and_an_unknown_channel_are_map_errors_naming_the_file() {
    let root = tmp("errors");
    let project = root.join("projects/line");
    let sources = Sources {
        project: Some(("line".into(), project.clone())),
        boxed: Some(root.join("drivers")),
    };
    let text = "device rio device:moxa-e1213 127.0.0.1\nin part bool rio ch:DI-04\n";

    // Missing: a problem naming the file and the device.
    let found = sources.find_text(Some(text));
    assert_eq!(found.problems.len(), 1, "{found:?}");
    assert!(
        found.problems[0].contains("device rio: device:moxa-e1213: no file moxa-e1213.device"),
        "{found:?}"
    );
    let e = driver_dir::layout(Some(text), None, &found).unwrap_err();
    assert!(e[0].contains("moxa-e1213.device"), "{e:?}");

    // Malformed: the file and the line.
    let bad = E1213.replace("in  DI-04        bool", "in  DI-04        wobble");
    std::fs::write(project.join("moxa-e1213.device"), &bad).unwrap();
    let found = sources.find_text(Some(text));
    assert!(
        found.problems.iter().any(|p| {
            p.starts_with("device rio: device:moxa-e1213: moxa-e1213.device:")
                && p.contains("in project line")
                && p.contains("wobble")
        }),
        "{found:?}"
    );
    assert!(driver_dir::layout(Some(text), None, &found).is_err());

    // Not a profile name: never reaches the file system.
    let found = sources.find_text(Some(
        "device rio device:../x 127.0.0.1\nin part bool rio ch:DI-04\n",
    ));
    assert!(found.problems[0].contains("not a profile name"), "{found:?}");

    // Good profile, channel it does not have: a point error at load, no
    // link opened.
    std::fs::write(project.join("moxa-e1213.device"), E1213).unwrap();
    let found = sources.find_text(Some(text));
    assert!(found.problems.is_empty(), "{found:?}");
    assert!(found.drivers.contains_key("moxa-e1213.device"));
    let nope = "device rio device:moxa-e1213 127.0.0.1\nin part bool rio ch:DI-99\nin x int rio ch:DI-04\nout y bool rio ch:DI-00\n";
    let e = driver_dir::layout(Some(nope), None, &found).unwrap_err();
    assert!(
        e.iter()
            .any(|l| l.contains("ch:DI-99") && l.contains("has no channel DI-99")),
        "{e:?}"
    );
    assert!(
        e.iter()
            .any(|l| l.starts_with("io.map: x: signal is Int, point is Bool")),
        "{e:?}"
    );
    assert!(
        e.iter()
            .any(|l| l.starts_with("io.map: y: ") && l.contains("read-only")),
        "{e:?}"
    );
    // And two signals on one channel are two writers.
    let two = "device rio device:moxa-e1213 127.0.0.1\nout a bool rio ch:DO-00\nout b bool rio ch:do-00\n";
    let e = driver_dir::layout(Some(two), None, &found).unwrap_err();
    assert!(
        e.iter().any(|l| l == "io.map: a and b drive the same point"),
        "{e:?}"
    );
    // A param that is not the line's is refused before anything opens.
    let own = "device rio device:moxa-e1213 127.0.0.1 unit=2\nin part bool rio ch:DI-04\n";
    let e = driver_dir::layout(Some(own), None, &found).unwrap_err();
    assert!(
        e.iter().any(|l| l.contains("unit") && l.contains("profile")),
        "{e:?}"
    );

    // The box's own folder serves a bare box (no project).
    std::fs::create_dir_all(root.join("drivers")).unwrap();
    std::fs::write(root.join("drivers/moxa-e1213.device"), E1213).unwrap();
    let found = Sources::boxed(&root.join("drivers")).find_text(Some(text));
    assert!(found.problems.is_empty(), "{found:?}");
    assert!(driver_dir::layout(Some(text), None, &found).is_ok());
}
