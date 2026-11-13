//! An ST module drives a PROFINET IO device through the I/O map
//! (`profinet-io`): the simulated device of `if=sim`, whose outputs come
//! back as its inputs, behind the box's own link machinery. What the
//! module writes arrives at the device over cyclic frames and what the
//! device reports arrives in another module's input.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use helmsman_iec::{ImageSizes, Value as St, compile};
use helmsman_io::local::Channels;
use helmsman_io::text;
use helmsman_plc::link::{Link, links};
use helmsman_plc::{Module, Runtime, SignalTable, Status, Window};

const AXIS: &str = "
PROGRAM axis
VAR_OUTPUT speed : INT; enable : BOOL; END_VAR
VAR n : INT; END_VAR
  n := n + 1;
  speed := 1200;
  enable := n > 3;
END_PROGRAM";

const WATCH: &str = "
PROGRAM watch
VAR_INPUT speed_fb : INT; enabled : BOOL; END_VAR
VAR seen : INT; on : BOOL; END_VAR
  seen := speed_fb;
  on := enabled;
END_PROGRAM";

const MAP: &str = "
device drive profinet-io mdrive if=sim slots=0:0x1/1:0x1,1:0x30/1:0x1:i8,2:0x31/1:0x1:o8
out speed    int  drive out:0:i16 safe=0
out enable   bool drive out:2.0   safe=false
in  speed_fb int  drive in:0:i16
in  enabled  bool drive in:2.0
";

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
    table: Mutex<SignalTable>,
    links: Vec<Link>,
    rt: Runtime,
    start: Instant,
}

impl Bench {
    fn new() -> Self {
        let file = text::parse(MAP).unwrap();
        file.map.validate().unwrap();
        let table = SignalTable::new(&file.map, &file.safe).unwrap();
        let links = links(&file.map, helmsman_io::open, Box::new(Channels::new())).unwrap();
        let rt =
            Runtime::with_signals(0, 0, vec![module("axis", AXIS), module("watch", WATCH)], &table).unwrap();
        Self {
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

    fn up(&self) -> bool {
        self.links.iter().find(|l| l.name == "drive").unwrap().is_up()
    }

    /// Steps until `ok`, or panics after 10 s.
    fn until(&mut self, what: &str, ok: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ok(self) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            self.step();
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

fn watch(b: &Bench, name: &str) -> Option<St> {
    b.rt.program("watch").and_then(|p| p.get(name))
}

#[test]
fn a_module_drives_a_profinet_device_and_reads_it_back() {
    let mut b = Bench::new();
    // Until the device's startup completes the link is down and both
    // modules are held: nothing computes on data that is not there.
    b.step();
    assert!(!b.up());
    assert_eq!(b.rt.status("watch"), Some(Status::Held));

    b.until("the link", Bench::up);
    b.until("the speed, through the device and back", |b| {
        watch(b, "seen") == Some(St::Int(1200))
    });
    b.until("the enable bit", |b| watch(b, "on") == Some(St::Bool(true)));
    assert_eq!(b.rt.status("axis"), Some(Status::Running));
    assert_eq!(b.rt.status("watch"), Some(Status::Running));
}
