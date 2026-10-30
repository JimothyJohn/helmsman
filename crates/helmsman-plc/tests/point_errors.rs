//! One bad point must not take its whole device offline. A write the device
//! (or the driver) refuses for that point alone, such as a value its
//! register can't hold or a Modbus exception, is a *point* error: the link
//! stays up, the other signals on the device keep flowing, the error is
//! recorded on the signal, and the point is retried. Only transport errors
//! (I/O, malformed replies) mark the link down.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use helmsman_iec::{ImageSizes, compile};
use helmsman_io::local::Channels;
use helmsman_io::sim::{Fault as SimFault, Sim, State};
use helmsman_io::{Value, text};
use helmsman_plc::link::{Link, links};
use helmsman_plc::{Module, Runtime, SignalTable, Status, Window};

fn module(name: &str, src: &str) -> Module {
    Module {
        name: name.into(),
        program: compile(src, ImageSizes { i: 0, q: 0, m: 0 }).unwrap(),
        period: Duration::from_millis(10),
        i: Window::new(0, 0),
        q: Window::new(0, 0),
        params: Vec::new(),
    }
}

struct Bench {
    sim: Sim,
    table: Mutex<SignalTable>,
    links: Vec<Link>,
    rt: Runtime,
    now: u64,
}

impl Bench {
    /// `rio` on the simulator: `n` (hr:2, i32) in; `big` (hr:0, u16) and
    /// `motor` (coil:1) out; `ghost` (hr:100, past the simulator's 16
    /// registers) in. Module `copy` drives big from n and motor on; module
    /// `haunt` reads ghost.
    fn new() -> Self {
        let sim = Sim::start(State::new(16)).unwrap();
        let src = format!(
            "device rio modbus-tcp {} timeout_ms=300
in  n     int  rio hr:2:i32
out big   int  rio hr:0
out motor bool rio coil:1
in  ghost int  rio hr:100
out lamp  bool box do:0
",
            sim.endpoint()
        );
        let file = text::parse(&src).unwrap();
        let table = SignalTable::new(&file.map, &BTreeMap::new()).unwrap();
        let links = links(&file.map, helmsman_io::open, Box::new(Channels::new())).unwrap();
        let copy = "PROGRAM copy
            VAR_INPUT n : DINT; END_VAR
            VAR_OUTPUT big : DINT; motor : BOOL; END_VAR
              big := n; motor := TRUE;
            END_PROGRAM";
        let haunt = "PROGRAM haunt
            VAR_INPUT ghost : DINT; END_VAR
            VAR_OUTPUT lamp : BOOL; END_VAR
              lamp := ghost > 0;
            END_PROGRAM";
        let rt =
            Runtime::with_signals(0, 0, vec![module("copy", copy), module("haunt", haunt)], &table).unwrap();
        Self {
            sim,
            table: Mutex::new(table),
            links,
            rt,
            now: 0,
        }
    }

    fn rio(&mut self) -> &mut Link {
        self.links.iter_mut().find(|l| l.name == "rio").unwrap()
    }

    fn set_n(&self, n: i32) {
        let v = n.cast_unsigned();
        let mut st = self.sim.state();
        st.holding[2] = u16::try_from(v >> 16).unwrap();
        st.holding[3] = u16::try_from(v & 0xFFFF).unwrap();
    }

    /// Every link cycles once (errors are the test's to inspect), then
    /// every module scans once.
    fn step(&mut self) {
        for l in &mut self.links {
            let _ = l.cycle(&self.table);
        }
        self.rt.tick_with(
            Duration::from_millis(self.now),
            &[],
            &mut self.table.lock().unwrap(),
        );
        self.now += 10;
    }

    fn error(&self, name: &str) -> Option<String> {
        let t = self.table.lock().unwrap();
        t.point_error(t.index(name).unwrap()).map(str::to_owned)
    }

    fn value(&self, name: &str) -> Option<Value> {
        let t = self.table.lock().unwrap();
        t.value(t.index(name).unwrap())
    }
}

#[test]
fn a_value_the_register_cannot_hold_stays_on_its_point() {
    let mut b = Bench::new();
    b.set_n(70_000); // past u16
    b.step(); // reads n
    b.step(); // copy drove big = 70000; the write is refused
    assert!(b.rio().is_up(), "a refused value is not a dead device");
    assert!(b.sim.state().coils[1], "motor on the same device was written");
    assert_eq!(b.sim.state().holding[0], 0, "nothing reached hr:0");
    let err = b.error("big").expect("the refusal is recorded on big");
    assert!(err.contains("70000"), "{err}");
    assert_eq!(b.error("motor"), None);
    assert_eq!(b.rt.status("copy"), Some(Status::Running), "copy is not held");
    assert_eq!(b.value("n"), Some(Value::Int(70_000)), "inputs keep flowing");

    b.set_n(5);
    b.step();
    b.step();
    assert_eq!(b.sim.state().holding[0], 5, "retried once it fits");
    assert_eq!(b.error("big"), None, "and the error clears");
}

#[test]
fn an_input_the_device_refuses_goes_missing_alone() {
    let mut b = Bench::new();
    b.set_n(7);
    b.step();
    assert!(b.rio().is_up());
    assert_eq!(
        b.value("ghost"),
        None,
        "hr:100 is an illegal address on this device"
    );
    assert!(b.error("ghost").is_some());
    assert_eq!(
        b.value("n"),
        Some(Value::Int(7)),
        "n on the same device still reads"
    );
    b.step();
    assert_eq!(
        b.rt.status("haunt"),
        Some(Status::Held),
        "only ghost's reader is held"
    );
    assert_eq!(b.rt.status("copy"), Some(Status::Running));
    assert_eq!(b.sim.state().holding[0], 7);
}

#[test]
fn transport_errors_still_take_the_link_down() {
    let mut b = Bench::new();
    b.set_n(1);
    b.step();
    b.step();
    for _ in 0..8 {
        b.sim.inject(SimFault::Drop);
    }
    b.step();
    assert!(!b.rio().is_up());
    assert_eq!(b.value("n"), None);
    assert_eq!(b.rt.status("copy"), Some(Status::Held));
    b.sim.clear_faults();
    b.step();
    assert!(b.rio().is_up(), "back once it answers");
}

#[test]
fn driver_errors_say_whether_the_link_is_at_fault() {
    use helmsman_io::Error;
    let io = || Error::Io(std::io::Error::other("x"));
    assert!(io().is_link());
    assert!(Error::Protocol("bad frame".into()).is_link());
    for e in [
        Error::Exception(2),
        Error::Point("70000 does not fit u16".into()),
        Error::ReadOnly("di:0".into()),
        Error::Address("hr:x".into()),
        Error::Type {
            want: helmsman_io::Ty::Bool,
            got: helmsman_io::Ty::Int,
        },
    ] {
        assert!(!e.is_link(), "{e}");
    }
}
