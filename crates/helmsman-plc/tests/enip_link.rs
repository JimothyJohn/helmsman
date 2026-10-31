//! An ST module drives a simulated Moxa ioLogik E1213 through the I/O map
//! (`enip-io`, Class 1 over real UDP on loopback): the output reaches the
//! device and comes back on the wired input; when the device falls silent the
//! link goes down, the module is held and its output goes safe; once the
//! device is back the link reopens the connection and the module runs.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use helmsman_iec::{ImageSizes, Value as St, compile};
use helmsman_io::enip_sim::E1213Sim;
use helmsman_io::local::Channels;
use helmsman_io::sim::{Fault, Faults};
use helmsman_io::text;
use helmsman_plc::link::{Link, links};
use helmsman_plc::{Module, Runtime, SignalTable, Status, Window};

const LINE: &str = "
PROGRAM line
VAR_INPUT start : BOOL; END_VAR
VAR_OUTPUT clamp : BOOL; END_VAR
  clamp := start;
END_PROGRAM";

const WATCH: &str = "
PROGRAM watch
VAR_INPUT clamp_fb : BOOL; END_VAR
VAR seen : BOOL; END_VAR
  seen := clamp_fb;
END_PROGRAM";

fn module(name: &str, src: &str) -> Module {
    let program = compile(src, ImageSizes { i: 0, q: 0, m: 0 }).unwrap();
    Module {
        name: name.into(),
        program: program.into(),
        period: Duration::from_millis(10),
        i: Window::new(0, 0),
        q: Window::new(0, 0),
        params: Vec::new(),
    }
}

struct Bench {
    sim: E1213Sim,
    table: Mutex<SignalTable>,
    links: Vec<Link>,
    rt: Runtime,
    start: Instant,
}

impl Bench {
    fn new() -> Self {
        let sim = E1213Sim::start().unwrap();
        let params: Vec<String> = sim.params().iter().map(|(k, v)| format!("{k}={v}")).collect();
        let src = format!(
            "device rio enip-io {} {}\n\
             in start bool rio di:9\n\
             out clamp bool rio do:0 safe=false\n\
             in clamp_fb bool rio di:0\n",
            sim.endpoint(),
            params.join(" ")
        );
        let file = text::parse(&src).unwrap();
        file.map.validate().unwrap();
        let table = SignalTable::new(&file.map, &file.safe).unwrap();
        let links = links(&file.map, helmsman_io::open, Box::new(Channels::new())).unwrap();
        let rt =
            Runtime::with_signals(0, 0, vec![module("line", LINE), module("watch", WATCH)], &table).unwrap();
        Self {
            sim,
            table: Mutex::new(table),
            links,
            rt,
            start: Instant::now(),
        }
    }

    fn step(&mut self) {
        for l in &mut self.links {
            let _ = l.cycle(&self.table);
        }
        self.rt
            .tick_with(self.start.elapsed(), &[], &mut self.table.lock().unwrap());
    }

    fn rio_up(&self) -> bool {
        self.links.iter().find(|l| l.name == "rio").unwrap().is_up()
    }

    /// Steps until `ok`, or panics after 5 s.
    fn until(&mut self, what: &str, ok: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ok(self) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            self.step();
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

fn seen(b: &Bench) -> Option<St> {
    b.rt.program("watch").and_then(|p| p.get("seen"))
}

#[test]
fn a_module_drives_the_e1213_and_is_held_while_it_is_silent() {
    let mut b = Bench::new();
    b.until("the link", Bench::rio_up);
    b.sim.force_di(9, true);
    b.until("clamp on, through the wire to DI-00", |b| {
        seen(b) == Some(St::Bool(true))
    });
    assert_eq!(b.rt.status("line"), Some(Status::Running));

    b.sim.inject(Fault::Silent);
    b.until("the link to drop", |b| !b.rio_up());
    b.step();
    assert_eq!(b.rt.status("line"), Some(Status::Held));
    assert_eq!(b.rt.status("watch"), Some(Status::Held));
    {
        let t = b.table.lock().unwrap();
        let clamp = t.index("clamp").unwrap();
        assert_eq!(t.command(clamp), Some(helmsman_io::Value::Bool(false)), "safe");
    }

    b.sim.clear_faults();
    b.until("the link back", Bench::rio_up);
    b.until("line running", |b| b.rt.status("line") == Some(Status::Running));
    b.until("clamp back on", |b| seen(b) == Some(St::Bool(true)));
    assert_eq!(
        b.sim.device().lock().unwrap().connections().len(),
        1,
        "the old connection was closed or forgotten, not leaked"
    );
}
