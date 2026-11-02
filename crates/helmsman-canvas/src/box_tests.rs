//! The box data source against the real box API (helmsman-plc's, with a
//! runtime scanning a conveyor cell on its own thread): what the canvas
//! makes of the JSON and event streams the API really produces.

use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use helmsman_diagram::{Scan, annotate, explain, lower, lower_table};
use helmsman_iec::{Image, ImageSizes, Value, compile};
use helmsman_plc::api::{
    Api, Catalog, Config, Devices, Limits, Live, MemAudit, MemProposals, ModuleInfo, Publisher, Shared,
    render,
};
use helmsman_plc::{Module as Slot, Runtime, Sample, SignalTable, Window};
use serde_json::Value as Json;

use crate::remote::{BoxScan, BoxState, Cmd, Fetch, Link, Msg, Stream};
use crate::st_view::Viewer;

const SIZES: ImageSizes = ImageSizes { i: 1, q: 1, m: 0 };

/// The bench's module: a conveyor cell with a start/stop latch, a motor
/// block with a nested IF, a blinker, a part counter, a CASE and a stack
/// light.
pub const DEMO: &str = "FUNCTION_BLOCK motor
VAR_INPUT run, fault : BOOL; END_VAR
VAR_OUTPUT on : BOOL; END_VAR
IF run THEN
  IF NOT fault THEN
    on := TRUE;
  ELSE
    on := FALSE;
  END_IF;
ELSE
  on := FALSE;
END_IF;
END_FUNCTION_BLOCK

PROGRAM cell
VAR
  start AT %IX0.0 : BOOL;
  stop AT %IX0.1 : BOOL;
  jam AT %IX0.2 : BOOL;
  conveyor AT %QX0.0 : BOOL;
  green AT %QX0.1 : BOOL;
  red AT %QX0.2 : BOOL;
  running : BOOL;
  m : motor;
  blink : TON;
  flash : BOOL;
  parts : CTU;
  count : INT;
  lane : INT;
END_VAR
running := (start OR running) AND NOT stop;
m(run := running, fault := jam);
conveyor := m.on;
blink(IN := NOT blink.Q, PT := T#400ms);
IF blink.Q THEN flash := NOT flash; END_IF;
parts(CU := m.on AND flash, R := stop, PV := 1000);
count := parts.CV;
CASE count MOD 3 OF
  0: lane := 1;
  1: lane := 2;
ELSE
  lane := 3;
END_CASE;
IF jam THEN
  red := flash;
  green := FALSE;
ELSIF m.on THEN
  green := TRUE;
  red := FALSE;
ELSE
  green := FALSE;
  red := FALSE;
END_IF;
END_PROGRAM
";

/// The scripted shift repeats every this many ms.
const SHIFT_MS: u64 = 12_000;

/// The scripted shift's `start`, `stop` and `jam` at `ms`: start at 0.2 s,
/// a jam from 6 to 7.5 s, stop at 10 s.
#[must_use]
pub const fn scripted(ms: u64) -> [bool; 3] {
    let t = ms % SHIFT_MS;
    [
        t >= 200 && t < 500,
        t >= 10_000 && t < 10_300,
        t >= 6_000 && t < 7_500,
    ]
}

/// [`DEMO`] on a real runtime, the way the box runs it, with the
/// real API over it. The scripted shift drives `%IX0.0..2`.
pub struct Bench {
    api: Option<Api>,
    pub addr: SocketAddr,
    stop: Arc<AtomicBool>,
    scan: Option<JoinHandle<()>>,
}

impl Drop for Bench {
    fn drop(&mut self) {
        self.api.take();
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.scan.take() {
            let _ = t.join();
        }
    }
}

impl Bench {
    pub fn new() -> Self {
        Self::with(&[("cell", DEMO)])
    }

    /// `programs` (name, source) as modules of one box, each with a byte
    /// of `%I` and of `%Q` of its own, in order; the scripted shift drives
    /// the first one's.
    pub fn with(programs: &[(&str, &str)]) -> Self {
        let n = programs.len();
        let slots = programs
            .iter()
            .enumerate()
            .map(|(k, (name, src))| Slot {
                name: (*name).into(),
                program: compile(src, SIZES).unwrap().into(),
                period: Duration::from_millis(2),
                i: Window::new(k, 1),
                q: Window::new(k, 1),
                params: Vec::new(),
            })
            .collect();
        let parsed = helmsman_io::text::parse("in ready bool box di:0\n").unwrap();
        let table = SignalTable::new(&parsed.map, &parsed.safe).unwrap();
        let mut rt = Runtime::with_signals(n, n, slots, &table).unwrap();
        let modules: Vec<ModuleInfo> = programs
            .iter()
            .map(|(name, src)| ModuleInfo::of(&rt, name, (*name).into(), (*src).into(), None).unwrap())
            .collect();
        let catalog = Catalog {
            signals: Catalog::signals(&table, Some(&parsed.map), &modules),
            modules,
        };
        let live = Arc::new(Live::default());
        let cfg = Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            token: "0".repeat(64),
            limits: Limits::default(),
            stream_every: Duration::from_millis(10),
            memory: 0,
            log: Arc::new(|_: &str| {}),
        };
        let shared = Shared {
            catalog,
            live: Arc::clone(&live),
            devices: Arc::new(Devices::new(["box".to_string()])),
            audit: Box::new(MemAudit::new(10)),
            proposals: Box::new(MemProposals::new(10)),
            running: None,
            snapshots: None,
            approval: None,
            chat: None,
        };
        let api = Api::start(cfg, shared).unwrap();
        let addr = api.addr();
        let stop = Arc::new(AtomicBool::new(false));
        let table = Arc::new(Mutex::new(table));
        let ready = table.lock().unwrap().index("ready").unwrap();
        let scan = {
            let stop = Arc::clone(&stop);
            let mut publisher = Publisher::new(live, Duration::from_millis(5));
            thread::spawn(move || {
                let t0 = Instant::now();
                while !stop.load(Ordering::Relaxed) {
                    let now = t0.elapsed();
                    let ms = u64::try_from(now.as_millis()).unwrap_or(u64::MAX);
                    let byte = scripted(ms)
                        .iter()
                        .enumerate()
                        .fold(0u8, |b, (i, &on)| b | (u8::from(on) << i));
                    let mut t = table.lock().unwrap_or_else(PoisonError::into_inner);
                    t.set_input(ready, Some(helmsman_io::Value::Bool(true)));
                    let mut image = vec![0u8; n];
                    image[0] = byte;
                    rt.tick_with(now, &image, &mut t);
                    publisher.publish(now, &rt, &t);
                    drop(t);
                    thread::sleep(Duration::from_millis(1));
                }
            })
        };
        Self {
            api: Some(api),
            addr,
            stop,
            scan: Some(scan),
        }
    }

    /// `GET path`: the status and body.
    pub fn get(&self, path: &str) -> (u16, String) {
        let mut s = TcpStream::connect(self.addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        write!(s, "GET {path} HTTP/1.1\r\nHost: box\r\n\r\n").unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        let (head, body) = out.split_once("\r\n\r\n").unwrap();
        (head[9..12].parse().unwrap(), body.to_string())
    }

    /// Opens the event stream at `path` and returns its first `n` events
    /// (name, data), parsed the way `EventSource` does.
    pub fn events(&self, path: &str, n: usize) -> Vec<(String, String)> {
        let mut s = TcpStream::connect(self.addr).unwrap();
        s.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
        write!(s, "GET {path} HTTP/1.1\r\nHost: box\r\n\r\n").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut buf = String::new();
        let mut chunk = [0u8; 8192];
        loop {
            let events = sse(&buf);
            if events.len() >= n {
                return events.into_iter().take(n).collect();
            }
            assert!(Instant::now() < deadline, "{n} events from {path}: {buf:.300}");
            if let Ok(k) = s.read(&mut chunk) {
                assert!(k > 0, "{path} closed: {buf:.300}");
                buf.push_str(&String::from_utf8_lossy(&chunk[..k]));
            }
        }
    }
}

/// Complete events in an event-stream body (after its HTTP head): blocks
/// separated by a blank line, `event:` and `data:` fields; comments and
/// `retry:` skipped.
pub fn sse(raw: &str) -> Vec<(String, String)> {
    let body = raw.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    let mut out = Vec::new();
    let mut blocks: Vec<&str> = body.split("\n\n").collect();
    blocks.pop(); // the last one may be partial
    for b in blocks {
        let (mut name, mut data) = ("message".to_string(), None::<String>);
        for line in b.lines() {
            if let Some(v) = line.strip_prefix("event: ") {
                name = v.into();
            } else if let Some(v) = line.strip_prefix("data: ") {
                data = Some(data.map_or_else(|| v.into(), |d| format!("{d}\n{v}")));
            }
        }
        if let Some(d) = data {
            out.push((name, d));
        }
    }
    out
}

/// What a browser would have received, in order, watching `b` until a
/// traced scan with the conveyor running arrived.
pub fn watch(b: &Bench) -> (BoxState, Vec<Msg>) {
    let mut s = BoxState::new();
    let mut seen = Vec::new();
    let mut now = 0.0;
    let mut feed = |s: &mut BoxState, m: Msg, now: f64| {
        seen.push(m.clone());
        s.apply(m, now);
    };
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        for cmd in s.poll(now) {
            match cmd {
                Cmd::Get(what, url) => {
                    let (status, body) = b.get(&url);
                    let r = if status == 200 {
                        Ok(body)
                    } else {
                        Err(format!("HTTP {status}"))
                    };
                    feed(&mut s, Msg::Got(what, r), now);
                }
                Cmd::Listen(stream, url) => {
                    feed(&mut s, Msg::Opened(stream), now);
                    for (name, data) in b.events(&url, 1) {
                        feed(&mut s, Msg::Event(stream, name, data), now);
                    }
                }
                Cmd::Close(_) => {}
            }
        }
        let running = s.scan.as_ref().and_then(|t| t.get("conveyor")) == Some(Value::Bool(true));
        if running && s.lowered.is_some() {
            return (s, seen);
        }
        assert!(Instant::now() < deadline, "no running scan: {s:?}");
        // Ask again for a fresh trace, as the stream would deliver one.
        let name = s.selected.clone().unwrap();
        for (ev, data) in b.events(&crate::remote::trace_url(&name), 1) {
            feed(&mut s, Msg::Event(Stream::Trace, ev, data), now);
        }
        now += 0.1;
    }
}

#[test]
fn a_box_trace_reads_exactly_like_the_trace_it_was_rendered_from() {
    // Every scan of the scripted shift, rendered by the API's own
    // renderer and read back: the diagram cannot tell them apart.
    let mut prog = compile(DEMO, SIZES).unwrap();
    let map = prog.trace_map();
    let module = lower(DEMO, &map).unwrap();
    let mut image = Image::new(SIZES);
    let view = Viewer::new(DEMO, module.clone());
    let mut checked = 0;
    for k in 0..1300u64 {
        let ms = k * 10;
        let byte = scripted(ms)
            .iter()
            .enumerate()
            .fold(0u8, |b, (i, &on)| b | (u8::from(on) << i));
        image.i[0] = byte;
        let (r, trace) = prog.scan_traced(&mut image, Duration::from_millis(ms));
        if k % 7 != 0 {
            continue;
        }
        let sample = Sample {
            scan: k + 1,
            fault: r.err(),
            trace,
        };
        let wire = serde_json::to_string(&render::trace("cell", &sample)).unwrap();
        let remote = BoxScan::parse(&serde_json::from_str::<Json>(&wire).unwrap()).unwrap();
        assert_eq!(remote.scan, k + 1);
        assert_eq!(remote.module, "cell");
        if checked == 0 {
            assert_eq!(lower_table(DEMO, &remote.table).unwrap(), module, "same diagram");
        }
        for scope in &view.scopes {
            let d = &module.pous[scope.pou].diagram;
            assert_eq!(
                annotate(d, &remote, &scope.prefix),
                annotate(d, &sample.trace, &scope.prefix),
                "scan {k}, {}",
                scope.label
            );
        }
        for (path, _) in map.vars() {
            assert_eq!(remote.get(path), sample.trace.get(path), "{path} at scan {k}");
        }
        let words = helmsman_diagram::Words::of(&module);
        for out in ["CONVEYOR", "RED", "GREEN", "LANE", "COUNT", "M.ON", "BLINK.Q"] {
            assert_eq!(
                explain(&module, &remote, out, 4),
                explain(&module, &sample.trace, out, 4),
                "why {out} at scan {k}"
            );
            // Explain mode's first layer reads the same in the browser.
            assert_eq!(
                helmsman_diagram::phrase(&explain(&module, &remote, out, 4), &words),
                helmsman_diagram::phrase(&explain(&module, &sample.trace, out, 4), &words),
                "said for {out} at scan {k}"
            );
        }
        checked += 1;
    }
    assert!(checked > 150);
}

#[test]
fn the_canvas_watches_a_live_box_through_its_api() {
    let b = Bench::new();
    let (s, seen) = watch(&b);
    assert_eq!(s.link(), Link::Connected);
    assert_eq!(s.selected.as_deref(), Some("cell"), "the only module is picked");
    assert_eq!(s.modules.len(), 1);
    assert_eq!(s.modules[0].state, "running", "{:?}", s.modules);
    assert!(s.problem.is_none(), "{:?}", s.problem);
    assert!(s.seq.is_some(), "a values event arrived");
    let ready = s
        .signals
        .iter()
        .find(|r| r.name == "ready")
        .expect("the ready signal");
    assert_eq!(ready.value, Some(Value::Bool(true)));
    assert_eq!((ready.dir.as_str(), ready.quality.as_str()), ("in", "good"));
    // What was asked for, in a browser's words.
    let gets: Vec<&Fetch> = seen
        .iter()
        .filter_map(|m| match m {
            Msg::Got(f, Ok(_)) => Some(f),
            _ => None,
        })
        .collect();
    assert!(
        gets.contains(&&Fetch::Modules) && gets.contains(&&Fetch::Module("cell".into())),
        "{gets:?}"
    );

    // The diagram the canvas draws, with the box's scan on it.
    let l = s.lowered.as_ref().unwrap();
    let scan = s.scan.as_ref().unwrap();
    let view = Viewer::new(&l.src, l.module.clone());
    assert_eq!(view.outputs(), ["CONVEYOR", "RED", "GREEN"]);
    let live = view.live(Some(scan as &dyn Scan)).unwrap();
    let d = view.diagram();
    let conveyor = d
        .nodes
        .iter()
        .find(|n| {
            n.kind
                == helmsman_diagram::NodeKind::Write {
                    path: "CONVEYOR".into(),
                }
        })
        .unwrap();
    assert_eq!(live.values[conveyor.id.0 as usize], [Some(Value::Bool(true))]);
    assert!(live.ran[conveyor.id.0 as usize]);
    let why = explain(&l.module, scan, "conveyor", 4).outline();
    assert!(why[0].starts_with("CONVEYOR = TRUE because line"), "{why:#?}");
    // The line a click on it sends the ST editor to is its own.
    let line = view.src.lines().nth(conveyor.pos.line as usize - 1).unwrap();
    assert_eq!(line.trim(), "conveyor := m.on;");
}

#[test]
fn an_unknown_module_and_a_dead_box_are_told_apart() {
    let b = Bench::new();
    let (status, _) = b.get(&crate::remote::module_url("nope"));
    assert_eq!(status, 404);
    let mut s = BoxState::new();
    s.poll(0.0);
    s.apply(Msg::Got(Fetch::Modules, Ok(b.get("/api/modules").1)), 0.0);
    s.select("nope");
    s.poll(0.0);
    s.apply(
        Msg::Got(Fetch::Module("nope".into()), Err("HTTP 404".into())),
        0.0,
    );
    assert!(s.poll(4.9).is_empty(), "retried after the backoff, not at once");
    assert_eq!(
        s.poll(5.0),
        [Cmd::Get(
            Fetch::Module("nope".into()),
            crate::remote::module_url("nope")
        )]
    );
    // The box going away: its port refuses, the stream fails.
    let addr = b.addr;
    drop(b);
    assert!(TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_err());
    s.apply(Msg::Opened(Stream::Values), 3.0);
    for k in 0..3 {
        s.apply(Msg::Failed(Stream::Values), 10.0 + f64::from(k) * 60.0);
        s.poll(10.0 + f64::from(k) * 60.0 + 59.0);
    }
    assert!(
        matches!(s.link(), Link::Unreachable { failures: 3, .. }),
        "{:?}",
        s.link()
    );
}

#[test]
fn the_box_tree_nests_the_programs_instances_and_a_refetch_keeps_the_arrangement() {
    use eframe::egui::{self, Pos2, RawInput, Rect, Vec2};

    let b = Bench::new();
    let (_, seen) = watch(&b);
    let ctx = egui::Context::default();
    let mut bv = crate::box_view::BoxView::new();
    bv.step(&ctx, seen.clone(), 0.0);
    let frame = |bv: &mut crate::box_view::BoxView| {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 860.0))),
            ..Default::default()
        };
        let mut chrome = crate::editor::Chrome::default();
        let mut out = ctx.run_ui(input, |ui| bv.show(ui, &mut chrome));
        out.textures_delta.clear();
    };
    frame(&mut bv);
    let v = bv.view.as_mut().expect("the module's diagram");
    let rows: Vec<(String, usize)> = v
        .tree_rows()
        .iter()
        .map(|r| (v.scopes[r.scope].prefix.to_ascii_lowercase(), r.depth))
        .collect();
    assert_eq!(
        rows,
        [(String::new(), 0), ("m.".to_string(), 1)],
        "the program calls m"
    );
    v.open(1);
    assert_eq!(v.trail(), [0, 1]);
    v.open(0);
    // Rearranged, then the box sends the same source again.
    let n = v.diagram().nodes[0].id;
    v.move_node(n, egui::Vec2::new(0.0, 500.0));
    let moved = v.diagram().nodes[0].rect;
    frame(&mut bv);
    bv.state.lowered.as_mut().unwrap().generation += 1;
    bv.step(&ctx, vec![], 1.0);
    frame(&mut bv);
    let v = bv.view.as_ref().unwrap();
    assert_eq!(
        v.diagram().nodes[0].rect,
        moved,
        "the arrangement survives a refetch"
    );
}
