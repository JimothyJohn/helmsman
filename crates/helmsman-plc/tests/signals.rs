//! Signals in the scan: ST modules read and drive named signals through the
//! I/O map, against the Modbus simulator over real TCP and the box's own
//! channels. Links and scans are stepped by hand so every step is
//! deterministic; one test runs the threaded links for real.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use helmsman_iec::{FaultKind, ImageSizes, Value as St, compile};
use helmsman_io::local::Channels;
use helmsman_io::sim::{Fault as SimFault, Sim, State};
use helmsman_io::{Binding, Device, Dir, IoMap, Signal, Ty, Value};
use helmsman_plc::link::{Link, links, spawn};
use helmsman_plc::{BindError, ConfigError, Module, Runtime, SignalTable, Status, TableError, Window};

const NO_FAULTS: Vec<(String, helmsman_iec::Fault)> = Vec::new();

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn module(name: &str, src: &str) -> Module {
    let program = compile(src, ImageSizes { i: 0, q: 0, m: 0 }).unwrap_or_else(|d| panic!("{name}: {d:?}"));
    Module {
        name: name.into(),
        program,
        period: ms(10),
        i: Window::new(0, 0),
        q: Window::new(0, 0),
        params: Vec::new(),
    }
}

fn sig(name: &str, ty: Ty, dir: Dir, device: &str, point: &str) -> Signal {
    let binding = if device.is_empty() {
        Binding::Local {
            channel: point.into(),
        }
    } else {
        Binding::Device {
            device: device.into(),
            point: point.into(),
        }
    };
    Signal {
        name: name.into(),
        ty,
        dir,
        binding,
        unit: None,
        poll: None,
    }
}

/// Remote I/O on the simulator: coil 0 is wired back to discrete input 0.
fn rio() -> Sim {
    let mut st = State::new(16);
    st.wires.push((0, 0));
    Sim::start(st).unwrap()
}

fn map(sim: &Sim) -> IoMap {
    IoMap {
        devices: vec![Device {
            id: "rio".into(),
            driver: "modbus-tcp".into(),
            endpoint: sim.endpoint(),
            params: [("timeout_ms".to_string(), "300".to_string())].into(),
        }],
        signals: vec![
            sig("start", Ty::Bool, Dir::In, "rio", "di:1"),
            sig("level", Ty::Int, Dir::In, "rio", "hr:5"),
            sig("motor", Ty::Bool, Dir::Out, "rio", "coil:0"),
            sig("motor_fb", Ty::Bool, Dir::In, "rio", "di:0"),
            sig("lamp", Ty::Bool, Dir::Out, "", "do:0"),
        ],
    }
}

const LINE: &str = "
PROGRAM line
VAR_INPUT start : BOOL; level : INT; END_VAR
VAR_OUTPUT motor : BOOL; lamp : BOOL; END_VAR
  motor := start;
  lamp := level > 10;
END_PROGRAM";

const WATCH: &str = "
PROGRAM watch
VAR_INPUT Motor_FB : BOOL; END_VAR
VAR seen : BOOL; END_VAR
  seen := motor_fb;
END_PROGRAM";

struct Bench {
    sim: Sim,
    panel: Channels,
    table: Mutex<SignalTable>,
    links: Vec<Link>,
    rt: Runtime,
    now: u64,
}

impl Bench {
    fn new(safe: &[(&str, Value)]) -> Self {
        let sim = rio();
        let m = map(&sim);
        let safe: BTreeMap<String, Value> = safe.iter().map(|(k, v)| ((*k).into(), *v)).collect();
        let table = SignalTable::new(&m, &safe).unwrap();
        let panel = Channels::new();
        let links = links(&m, helmsman_io::open, Box::new(panel.clone())).unwrap();
        let rt =
            Runtime::with_signals(0, 0, vec![module("line", LINE), module("watch", WATCH)], &table).unwrap();
        Self {
            sim,
            panel,
            table: Mutex::new(table),
            links,
            rt,
            now: 0,
        }
    }

    fn link(&mut self, name: &str) -> &mut Link {
        self.links.iter_mut().find(|l| l.name == name).unwrap()
    }

    /// Every link cycles once, then every module scans once.
    fn step(&mut self) {
        for l in &mut self.links {
            let _ = l.cycle(&self.table);
        }
        let faults = self
            .rt
            .tick_with(ms(self.now), &[], &mut self.table.lock().unwrap());
        assert_eq!(faults, NO_FAULTS);
        self.now += 10;
    }

    fn command(&self, name: &str) -> Value {
        let t = self.table.lock().unwrap();
        t.command(t.index(name).unwrap()).unwrap()
    }
}

#[test]
fn an_st_program_drives_a_modbus_coil_and_a_box_output_by_name() {
    let mut b = Bench::new(&[]);
    b.step();
    assert_eq!(b.rt.status("line"), Some(Status::Running));
    assert!(!b.sim.state().coils[0] && !b.panel.output(0));

    b.sim.state().discrete[1] = true;
    b.sim.state().holding[5] = 20;
    b.step(); // reads start and level, line computes motor and lamp
    b.step(); // writes them
    assert!(b.sim.state().coils[0], "coil 0 on the remote I/O");
    assert!(b.panel.output(0), "do:0 on the box");

    b.step(); // the wire from coil 0 brings di:0 back
    assert_eq!(b.rt.program("watch").unwrap().get("seen"), Some(St::Bool(true)));

    b.sim.state().holding[5] = 3;
    b.step();
    b.step();
    assert!(!b.panel.output(0), "lamp follows level");
    assert!(b.sim.state().coils[0]);
}

#[test]
fn outputs_are_written_on_change_only() {
    let mut b = Bench::new(&[]);
    b.step();
    b.step();
    let before = b.sim.state().requests;
    b.step();
    assert_eq!(b.sim.state().requests - before, 3, "three input reads, no writes");

    b.sim.state().discrete[1] = true;
    b.step();
    let before = b.sim.state().requests;
    b.step();
    assert_eq!(
        b.sim.state().requests - before,
        4,
        "one coil write plus the reads"
    );
}

#[test]
fn a_dropped_device_holds_its_readers_and_sends_their_outputs_safe() {
    let mut b = Bench::new(&[]);
    b.sim.state().discrete[1] = true;
    b.sim.state().holding[5] = 20;
    for _ in 0..3 {
        b.step();
    }
    assert!(b.sim.state().coils[0] && b.panel.output(0));

    for _ in 0..8 {
        b.sim.inject(SimFault::Drop);
    }
    b.step();
    assert!(!b.link("rio").is_up());
    assert_eq!(b.rt.status("line"), Some(Status::Held));
    assert_eq!(b.rt.status("watch"), Some(Status::Held));
    assert_eq!(b.command("motor"), Value::Bool(false), "safe");
    assert_eq!(
        b.command("lamp"),
        Value::Bool(false),
        "safe, though the box is fine"
    );
    b.step();
    assert!(!b.panel.output(0), "the box link wrote the safe lamp");

    b.sim.clear_faults();
    b.step(); // reconnects: writes motor's safe value, reads inputs back
    assert!(b.link("rio").is_up());
    assert!(!b.sim.state().coils[0]);
    assert_eq!(b.rt.status("line"), Some(Status::Running));
    b.step();
    assert!(b.sim.state().coils[0] && b.panel.output(0), "back to normal");
    assert!(b.rt.stats("line").unwrap().held >= 2);
}

#[test]
fn after_a_drop_every_output_is_written_again() {
    // motor's safe value is ON, so its command never changes across the
    // drop; the device lost its outputs meanwhile, so the link must not
    // trust what it wrote before.
    let mut b = Bench::new(&[("motor", Value::Bool(true))]);
    b.sim.state().discrete[1] = true;
    b.step();
    b.step();
    assert!(b.sim.state().coils[0]);

    for _ in 0..8 {
        b.sim.inject(SimFault::Drop);
    }
    b.step();
    assert_eq!(b.command("motor"), Value::Bool(true));
    b.sim.state().coils[0] = false; // the device's own comm-loss watchdog
    b.sim.clear_faults();
    b.step();
    assert!(b.sim.state().coils[0], "rewritten on reconnect");
}

#[test]
fn an_input_that_does_not_fit_its_port_holds_the_module() {
    let mut b = Bench::new(&[]);
    b.sim.state().holding[5] = 40_000; // u16 on the wire, past INT
    b.step();
    assert_eq!(b.rt.status("line"), Some(Status::Held));
    assert_eq!(
        b.rt.status("watch"),
        Some(Status::Running),
        "watch does not read level"
    );
    b.sim.state().holding[5] = 7;
    b.step();
    b.step();
    assert_eq!(b.rt.status("line"), Some(Status::Running));
}

#[test]
fn a_faulting_module_sends_its_output_signals_to_their_declared_safe_values() {
    let sim = rio();
    let m = IoMap {
        devices: map(&sim).devices,
        signals: vec![
            sig("brake", Ty::Bool, Dir::Out, "", "do:0"),
            sig("speed", Ty::Real, Dir::Out, "rio", "hr:2:f32"),
            sig("go", Ty::Bool, Dir::In, "", "di:0"),
        ],
    };
    let safe = [("brake".to_string(), Value::Bool(true))].into();
    let mut table = SignalTable::new(&m, &safe).unwrap();
    let src = "
PROGRAM drive
VAR_INPUT go : BOOL; END_VAR
VAR_OUTPUT brake : BOOL := TRUE; speed : REAL; END_VAR
VAR zero : REAL; END_VAR
  brake := FALSE;
  speed := 1.5;
  IF go THEN speed := 1.0 / zero; END_IF;
END_PROGRAM";
    let mut rt = Runtime::with_signals(0, 0, vec![module("drive", src)], &table).unwrap();
    let go = table.index("go").unwrap();
    let (brake, speed) = (table.index("brake").unwrap(), table.index("speed").unwrap());
    assert_eq!(table.command(brake), Some(Value::Bool(true)), "starts safe");

    table.set_input(go, Some(Value::Bool(false)));
    assert_eq!(rt.tick_with(ms(0), &[], &mut table), NO_FAULTS);
    assert_eq!(table.command(brake), Some(Value::Bool(false)));
    assert_eq!(table.command(speed), Some(Value::Real(1.5)));

    table.set_input(go, Some(Value::Bool(true)));
    let faults = rt.tick_with(ms(10), &[], &mut table);
    assert_eq!(faults.len(), 1);
    assert_eq!(
        faults[0].1.kind,
        FaultKind::OutputRange,
        "an infinite REAL cannot leave the box"
    );
    assert_eq!(table.command(brake), Some(Value::Bool(true)), "declared safe");
    assert_eq!(table.command(speed), Some(Value::Real(0.0)), "default safe");
}

#[test]
fn ports_that_do_not_fit_the_map_are_refused() {
    let sim = rio();
    let table = SignalTable::new(&map(&sim), &BTreeMap::new()).unwrap();
    let cases = [
        (
            "PROGRAM p VAR_INPUT nope : BOOL; END_VAR END_PROGRAM",
            BindError::UnknownSignal("NOPE".into()),
        ),
        (
            "PROGRAM p VAR_OUTPUT start : BOOL; END_VAR END_PROGRAM",
            BindError::DrivesInput("START".into()),
        ),
        (
            "PROGRAM p VAR_INPUT start : INT; END_VAR END_PROGRAM",
            BindError::Type {
                port: "START".into(),
                signal: Ty::Bool,
                st: helmsman_iec::Ty::Int,
            },
        ),
        (
            "PROGRAM p VAR_INPUT level : REAL; END_VAR END_PROGRAM",
            BindError::Type {
                port: "LEVEL".into(),
                signal: Ty::Int,
                st: helmsman_iec::Ty::Real,
            },
        ),
        (
            "PROGRAM p VAR_INPUT level : TIME; END_VAR END_PROGRAM",
            BindError::Type {
                port: "LEVEL".into(),
                signal: Ty::Int,
                st: helmsman_iec::Ty::Time,
            },
        ),
    ];
    for (src, want) in cases {
        let err = Runtime::with_signals(0, 0, vec![module("p", src)], &table).unwrap_err();
        assert_eq!(
            err,
            vec![ConfigError::Port {
                module: "p".into(),
                err: want
            }],
            "{src}"
        );
    }
    // Wide integers may come in (range-checked each scan) but not go out.
    let m = IoMap {
        devices: map(&sim).devices,
        signals: vec![
            sig("count", Ty::Int, Dir::Out, "rio", "hr:0:u32"),
            sig("total", Ty::Int, Dir::In, "rio", "hr:2:u32"),
        ],
    };
    let t = SignalTable::new(&m, &BTreeMap::new()).unwrap();
    let ok = "PROGRAM p VAR_INPUT total : ULINT; END_VAR VAR_OUTPUT count : LINT; END_VAR END_PROGRAM";
    assert!(Runtime::with_signals(0, 0, vec![module("p", ok)], &t).is_ok());
    let wide = "PROGRAM p VAR_OUTPUT count : ULINT; END_VAR END_PROGRAM";
    assert!(Runtime::with_signals(0, 0, vec![module("p", wide)], &t).is_err());

    let ports_without_signals = Runtime::new(0, 0, vec![module("line", LINE)]).unwrap_err();
    assert_eq!(ports_without_signals.len(), 4, "{ports_without_signals:?}");
}

#[test]
fn two_modules_driving_one_signal_are_refused() {
    let sim = rio();
    let table = SignalTable::new(&map(&sim), &BTreeMap::new()).unwrap();
    let other = "PROGRAM other VAR_OUTPUT LAMP : BOOL; END_VAR lamp := TRUE; END_PROGRAM";
    let err =
        Runtime::with_signals(0, 0, vec![module("line", LINE), module("other", other)], &table).unwrap_err();
    assert_eq!(
        err,
        vec![ConfigError::TwoWriters {
            signal: "LAMP".into(),
            a: "line".into(),
            b: "other".into()
        }]
    );
    assert_eq!(err[0].to_string(), "line and other both drive signal LAMP");
    // Reading another module's output signal is fine: it reads the command.
    let reader = "PROGRAM r VAR_INPUT lamp : BOOL; END_VAR END_PROGRAM";
    assert!(Runtime::with_signals(0, 0, vec![module("line", LINE), module("r", reader)], &table).is_ok());
}

#[test]
fn the_table_refuses_ambiguous_names_and_bad_safe_values() {
    let sim = rio();
    let mut m = map(&sim);
    m.signals.push(sig("LAMP", Ty::Bool, Dir::Out, "", "do:1"));
    let safe: BTreeMap<String, Value> = [
        ("start".to_string(), Value::Bool(true)),
        ("motor".to_string(), Value::Int(1)),
        ("ghost".to_string(), Value::Bool(true)),
    ]
    .into();
    let err = SignalTable::new(&m, &safe).unwrap_err();
    assert_eq!(
        err,
        vec![
            TableError::SameName {
                a: "lamp".into(),
                b: "LAMP".into()
            },
            TableError::SafeNotOutput("ghost".into()),
            TableError::SafeType("motor".into()),
            TableError::SafeNotOutput("start".into()),
        ]
    );
}

#[test]
fn reads_of_the_wrong_type_count_as_missing() {
    let sim = rio();
    let mut t = SignalTable::new(&map(&sim), &BTreeMap::new()).unwrap();
    let (start, level, motor) = (
        t.index("START").unwrap(),
        t.index("level").unwrap(),
        t.index("motor").unwrap(),
    );
    t.set_input(start, Some(Value::Int(1)));
    assert_eq!(t.value(start), None);
    t.set_input(level, Some(Value::Int(4)));
    assert_eq!(t.value(level), Some(Value::Int(4)));
    t.set_input(level, None);
    assert_eq!(t.value(level), None);
    t.set_input(motor, Some(Value::Bool(true)));
    assert_eq!(
        t.command(motor),
        Some(Value::Bool(false)),
        "a read cannot set a command"
    );
    assert_eq!(t.value(99), None);
    t.set_input(99, Some(Value::Bool(true)));
}

#[test]
fn threaded_links_carry_a_scan_to_the_device_and_report_the_drop() {
    let sim = rio();
    let m = map(&sim);
    let table = Arc::new(Mutex::new(SignalTable::new(&m, &BTreeMap::new()).unwrap()));
    let panel = Channels::new();
    let mut rt = Runtime::with_signals(0, 0, vec![module("line", LINE)], &table.lock().unwrap()).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let events = Arc::new(Mutex::new(Vec::new()));
    let handles: Vec<_> = links(&m, helmsman_io::open, Box::new(panel.clone()))
        .unwrap()
        .into_iter()
        .map(|l| {
            let ev = Arc::clone(&events);
            spawn(l, Arc::clone(&table), Arc::clone(&stop), move |name, err| {
                ev.lock().unwrap().push((name.to_string(), err.is_none()));
            })
        })
        .collect();
    sim.state().discrete[1] = true;
    let start = Instant::now();
    let wait = |what: &str, rt: &mut Runtime, ok: &dyn Fn() -> bool| {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !ok() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            rt.tick_with(start.elapsed(), &[], &mut table.lock().unwrap());
            std::thread::sleep(ms(5));
        }
    };
    wait("coil 0", &mut rt, &|| sim.state().coils[0]);
    for _ in 0..50 {
        sim.inject(SimFault::Drop);
    }
    wait("the hold", &mut rt, &|| rio_went_down(&events));
    sim.clear_faults();
    sim.state().discrete[1] = false;
    wait("coil 0 off", &mut rt, &|| !sim.state().coils[0]);
    stop.store(true, Ordering::Relaxed);
    for h in handles {
        h.join().unwrap();
    }
    let ev = events.lock().unwrap();
    assert!(ev.contains(&("rio".to_string(), true)), "{ev:?}");
    assert!(ev.contains(&("rio".to_string(), false)), "{ev:?}");
    assert!(ev.contains(&("box".to_string(), true)), "{ev:?}");
}

/// The rio link reported going down.
fn rio_went_down(events: &Mutex<Vec<(String, bool)>>) -> bool {
    events.lock().unwrap().iter().any(|(n, up)| n == "rio" && !up)
}

fn table_of(src: &str) -> SignalTable {
    let p = helmsman_io::text::parse(src).unwrap();
    SignalTable::new(&p.map, &p.safe).unwrap()
}

#[test]
fn a_reloaded_table_keeps_exactly_the_signals_whose_name_type_direction_and_binding_held() {
    let mut old = table_of(
        "device a modbus-tcp 127.0.0.1:1\n\
         in  run   bool a di:0\n\
         in  level int  a hr:0\n\
         out valve bool a coil:0 safe=false\n\
         out horn  bool a coil:1 safe=false\n\
         out lamp  bool a coil:2 safe=false\n",
    );
    let ix = |t: &SignalTable, n: &str| t.index(n).unwrap();
    old.set_input(ix(&old, "run"), Some(Value::Bool(true)));
    old.set_input(ix(&old, "level"), Some(Value::Int(7)));
    let mut rt = Runtime::with_signals(
        0,
        0,
        vec![module(
            "m",
            "PROGRAM m VAR_INPUT run : BOOL; END_VAR VAR_OUTPUT valve : BOOL; horn : BOOL; lamp : BOOL; END_VAR \
             valve := run; horn := run; lamp := run; END_PROGRAM",
        )],
        &old,
    )
    .unwrap();
    rt.tick_with(ms(0), &[], &mut old);
    old.set_point_error(ix(&old, "run"), Some("flaky".into()));
    let mut new = table_of(
        "device a modbus-tcp 127.0.0.1:1\n\
         out lamp  bool a coil:2 safe=true\n\
         in  level real a hr:0:f32\n\
         out horn  bool a coil:5 safe=false\n\
         in  run   bool a di:0 poll_ms=7\n\
         out valve bool a coil:0 safe=false unit=on\n\
         out extra bool a coil:9 safe=true\n",
    );
    let from = new.carry_from(&old);
    assert_eq!(new.epoch(), old.epoch() + 1);
    // The same signal: poll rate, unit and safe value are not its identity.
    assert_eq!(new.value(ix(&new, "run")), Some(Value::Bool(true)));
    assert_eq!(new.point_error(ix(&new, "run")), Some("flaky"));
    assert_eq!(new.command(ix(&new, "valve")), Some(Value::Bool(true)));
    assert_eq!(
        new.command(ix(&new, "lamp")),
        Some(Value::Bool(true)),
        "a new safe value keeps the command"
    );
    assert_eq!(new.safe(ix(&new, "lamp")), Some(Value::Bool(true)));
    // Another signal under the same name: retyped, rebound, or new.
    assert_eq!(new.value(ix(&new, "level")), None, "retyped: missing until read");
    assert_eq!(
        new.command(ix(&new, "horn")),
        Some(Value::Bool(false)),
        "rebound: starts safe"
    );
    assert_eq!(
        new.command(ix(&new, "extra")),
        Some(Value::Bool(true)),
        "added: starts safe"
    );
    assert_eq!(from[ix(&new, "run")], Some(ix(&old, "run")));
    assert_eq!(from[ix(&new, "horn")], None);
    assert_eq!(new.point(ix(&new, "horn")), Some(("a", "coil:5")));
}

#[test]
fn rebinding_onto_a_reloaded_table_makes_what_nothing_healthy_drives_safe_or_refuses_whole() {
    let src = "device a modbus-tcp 127.0.0.1:1\n\
               in  run   bool a di:0\n\
               out valve bool a coil:0 safe=false\n\
               out spare bool a coil:1 safe=false\n";
    let mut old = table_of(src);
    let m = || {
        module(
            "m",
            "PROGRAM m VAR_INPUT run : BOOL; END_VAR VAR_OUTPUT valve : BOOL; END_VAR valve := run; END_PROGRAM",
        )
    };
    let mut rt = Runtime::with_signals(0, 0, vec![m()], &old).unwrap();
    old.set_input(old.index("run").unwrap(), Some(Value::Bool(true)));
    rt.tick_with(ms(0), &[], &mut old);
    // A map without `run`: the module no longer binds, and nothing changes.
    let mut bad = table_of("device a modbus-tcp 127.0.0.1:1\nout valve bool a coil:0\n");
    bad.carry_from(&old);
    assert!(rt.rebind(vec![m()], &mut bad).is_err());
    assert_eq!(rt.status("m"), Some(Status::Running));
    // Held, then new safe values: its output and the undriven spare take
    // them in the reload step itself.
    old.set_input(old.index("run").unwrap(), None);
    rt.tick_with(ms(10), &[], &mut old);
    assert_eq!(rt.status("m"), Some(Status::Held));
    let mut new = table_of(&src.replace("safe=false", "safe=true"));
    new.carry_from(&old);
    rt.rebind(vec![m()], &mut new).unwrap();
    assert_eq!(new.command(new.index("valve").unwrap()), Some(Value::Bool(true)));
    assert_eq!(new.command(new.index("spare").unwrap()), Some(Value::Bool(true)));
}
