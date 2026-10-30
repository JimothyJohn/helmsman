//! Reloading the I/O map and the shift calendar on a running box
//! (`map-reload`): over real sockets, a real snapshot store, a real runtime
//! scanning on its own thread with the recorder and calendar in its scene,
//! real device links on their own threads, and the Modbus simulator over
//! TCP as the devices. A reload goes through the same approval path as any
//! change (engineer PIN and replay evidence); these check what reaches the
//! devices: what the new map dropped goes safe, what it added starts safe,
//! what it kept never glitches, and a refused reload changes nothing.

#![allow(clippy::many_single_char_names)] // sims a, b, c as the maps name them

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use helmsman_io::Value;
use helmsman_io::local::Channels;
use helmsman_io::sim::{Sim, State};
use helmsman_plc::api::swap::{self, Scene};
use helmsman_plc::api::{
    Api, Approval, Config, Devices, Limits, Live, MemAudit, MemProposals, Plant, Publisher, Shared,
};
use helmsman_plc::layout::Layout;
use helmsman_plc::link::LinkSet;
use helmsman_plc::policy::pin::{Gate, Pins};
use helmsman_plc::policy::{FLOOR, Role};
use helmsman_plc::record::{RecordConfig, Recorder};
use helmsman_plc::spec::Defaults;
use helmsman_plc::store::{ChangeSet, Meta, ModuleFiles, Store};
use helmsman_plc::{Runtime, SignalTable, Window};
use serde_json::{Value as Json, json};

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const ENGINEER: &str = "eng-135797";

/// 2026-10-26T00:00Z, a Monday.
const MONDAY: i64 = 1_792_972_800;

/// Drives the valve on device a and the horn on device b from `run`.
const CTL: (&str, &str) = (
    "\
name     ctl
author   Test <test@example.com>
scan_ms  2
reads    run
drives   valve horn
",
    "
PROGRAM ctl
VAR_INPUT run : BOOL; END_VAR
VAR_OUTPUT valve : BOOL; horn : BOOL; END_VAR
  valve := run;
  horn := run;
END_PROGRAM",
);

/// [`CTL`] without the horn: what a map without device b runs.
const CTL_VALVE: (&str, &str) = (
    "\
name     ctl
author   Test <test@example.com>
scan_ms  2
reads    run
drives   valve
",
    "
PROGRAM ctl
VAR_INPUT run : BOOL; END_VAR
VAR_OUTPUT valve : BOOL; END_VAR
  valve := run;
END_PROGRAM",
);

/// Lights the lamp from a level on device c.
const FILL: (&str, &str) = (
    "\
name     fill
author   Test <test@example.com>
scan_ms  2
reads    level
drives   lamp
",
    "
PROGRAM fill
VAR_INPUT level : DINT; END_VAR
VAR_OUTPUT lamp : BOOL; END_VAR
  lamp := level > 10;
END_PROGRAM",
);

const CAL_TO_14: &str = "zone UTC\nshift 1 mon-fri 06:00-14:00\n";
const CAL_TO_16: &str = "zone UTC\nshift 1 mon-fri 06:00-16:00\n";

/// Devices a and b on simulators, `run` on a's discrete input 0, the valve
/// on a's coil 0, the horn on b's coil 0, a spare output nobody drives on
/// b's coil 3.
fn map(a: &Sim, b: &Sim) -> String {
    format!(
        "\
device a modbus-tcp {} unit=1 timeout_ms=1000 poll_ms=5
device b modbus-tcp {} unit=1 timeout_ms=1000 poll_ms=5
in  run   bool a di:0
out valve bool a coil:0 safe=false
out horn  bool b coil:0 safe=false
out spare bool b coil:3 safe=false
",
        a.endpoint(),
        b.endpoint()
    )
}

/// Device c, which nothing answers at, and the lamp the `fill` module
/// drives from it on a's coil 2.
fn with_dead_c(base: &str, lamp_safe: bool) -> String {
    format!(
        "{base}device c modbus-tcp 127.0.0.1:1 unit=1 timeout_ms=50 poll_ms=20\n\
         in  level int  c hr:0\n\
         out lamp  bool a coil:2 safe={lamp_safe}\n"
    )
}

/// The map without device b and its signals.
fn drop_b(map: &str) -> String {
    map.lines()
        .filter(|l| !l.contains(" b ") && !l.starts_with("device b"))
        .fold(String::new(), |mut out, l| {
            out.push_str(l);
            out.push('\n');
            out
        })
}

fn files((manifest, source): (&str, &str)) -> ModuleFiles {
    ModuleFiles {
        manifest: manifest.into(),
        source: source.into(),
    }
}

fn change(modules: &[(&str, &str)], io_map: &str, calendar: Option<&str>) -> ChangeSet {
    ChangeSet {
        modules: modules
            .iter()
            .map(|m| {
                let name = m.0.lines().next().unwrap().split_whitespace().nth(1).unwrap();
                (name.to_string(), files(*m))
            })
            .collect::<BTreeMap<_, _>>(),
        io_map: Some(io_map.into()),
        calendar: calendar.map(str::to_string),
    }
}

fn defaults() -> Defaults {
    Defaults {
        period: Duration::from_millis(2),
        i: Window::new(0, 0),
        q: Window::new(0, 0),
        m: 16,
    }
}

fn meta() -> Meta {
    Meta {
        author: "Test".into(),
        prompt: String::new(),
        approver: None,
        time_ms: 1,
    }
}

fn tmp(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "reload-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A simulator with `run` on (discrete input 0).
fn sim() -> Sim {
    let mut st = State::new(16);
    st.discrete[0] = true;
    Sim::start(st).unwrap()
}

fn coil(s: &Sim, n: usize) -> bool {
    s.state().coils[n]
}

fn requests(s: &Sim) -> u64 {
    s.state().requests
}

/// The box: the scan thread (runtime, table, calendar and recorder in its
/// scene, the swap channel served between ticks, the wall clock from
/// `clock`), the device links, the API over all of it, and a store whose
/// head is what it runs.
struct Bench {
    api: Option<Api>,
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    scan: Option<JoinHandle<()>>,
    table: Arc<Mutex<SignalTable>>,
    links: Arc<Mutex<LinkSet>>,
    store: PathBuf,
    clock: Arc<AtomicI64>,
    ticks: Arc<AtomicU64>,
}

impl Bench {
    #[allow(clippy::too_many_lines)]
    fn new(cs: &ChangeSet, wall: i64) -> Self {
        let d = defaults();
        let layout = Layout::build(cs.io_map.as_deref(), cs.calendar.as_deref()).unwrap();
        let modules = swap::compile_set(cs, &d).unwrap();
        let mut rt = Runtime::with_signals(32, 32, modules, &layout.table).unwrap();
        let catalog = swap::catalog_of(&rt, &layout.table, cs);
        let store = tmp("store");
        Store::open(&store).unwrap().commit(cs, &meta()).unwrap();
        let recorder = Recorder::new(&layout.table, RecordConfig::default(), None);
        let table = Arc::new(Mutex::new(layout.table.clone()));
        let devices = Arc::new(Devices::new(Vec::new()));
        let dv = Arc::clone(&devices);
        let mut set = LinkSet::new(
            Arc::clone(&table),
            Arc::new(helmsman_io::open),
            Arc::new(|| Box::new(Channels::new())),
            Arc::new(move |name: &str, err: Option<&helmsman_io::Error>| {
                dv.set(name, err.map(ToString::to_string));
            }),
        );
        let started = set.start(layout.map.as_ref().unwrap()).unwrap();
        for n in &started.added {
            devices.add(n);
        }
        let links = Arc::new(Mutex::new(set));
        let (handle, swapper) = swap::channel(Duration::from_secs(5));
        let handle = handle.with_links(Arc::clone(&links));
        let mut pins = Pins::default();
        pins.set(Role::Engineer, ENGINEER, 1, [2; 16]).unwrap();
        let t0 = Instant::now();
        let approval = Approval {
            policy: FLOOR,
            gate: Gate::new(pins, FLOOR.lockout, Arc::new(move || t0)),
            plant: Plant {
                table: layout.table.clone(),
                i_len: 32,
                q_len: 32,
                defaults: d,
                record: Some(recorder.tap()),
            },
            swap: handle,
        };
        let live = Arc::new(Live::default());
        let cfg = Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            token: TOKEN.into(),
            limits: Limits {
                deadline: Duration::from_secs(5),
                ..Limits::default()
            },
            stream_every: Duration::from_millis(20),
            memory: 16,
            log: Arc::new(|_: &str| {}),
        };
        let shared = Shared {
            catalog,
            live: Arc::clone(&live),
            devices,
            audit: Box::new(MemAudit::new(100_000)),
            proposals: Box::new(MemProposals::new(10_000)),
            running: Some(cs.clone()),
            snapshots: Some(Mutex::new(Store::open(&store).unwrap())),
            approval: Some(approval),
        };
        let api = Api::start(cfg, shared).unwrap();
        let addr = api.addr();
        let stop = Arc::new(AtomicBool::new(false));
        let clock = Arc::new(AtomicI64::new(wall));
        let ticks = Arc::new(AtomicU64::new(0));
        let scan = {
            let (stop, table, clock, ticks) = (
                Arc::clone(&stop),
                Arc::clone(&table),
                Arc::clone(&clock),
                Arc::clone(&ticks),
            );
            let mut publisher = Publisher::new(live, Duration::from_millis(5));
            let mut scene = Scene {
                calendar: layout.calendar,
                recorder: Some(recorder),
                wall: None,
            };
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    {
                        let mut t = table.lock().unwrap_or_else(PoisonError::into_inner);
                        scene.wall = Some(clock.load(Ordering::SeqCst));
                        swapper.serve_scene(&mut rt, &mut t, Some(&mut scene));
                        if let Some(c) = &scene.calendar {
                            c.update(scene.wall, &mut t);
                        }
                        let now = t0.elapsed();
                        if let Some(r) = &mut scene.recorder {
                            r.observe(now, &t);
                        }
                        rt.tick_with(now, &[], &mut t);
                        publisher.publish(now, &rt, &t);
                    }
                    ticks.fetch_add(1, Ordering::Relaxed);
                    thread::sleep(Duration::from_millis(1));
                }
            })
        };
        let b = Self {
            api: Some(api),
            addr,
            stop,
            scan: Some(scan),
            table,
            links,
            store,
            clock,
            ticks,
        };
        b.settle();
        b
    }

    /// A few more ticks, so the recorder has a window and links a cycle.
    fn settle(&self) {
        let from = self.ticks.load(Ordering::Relaxed);
        wait("ticks", || self.ticks.load(Ordering::Relaxed) > from + 30);
    }

    fn value(&self, signal: &str) -> Option<Value> {
        let t = self.table.lock().unwrap_or_else(PoisonError::into_inner);
        t.index(signal).and_then(|i| t.value(i))
    }

    fn links(&self) -> Vec<String> {
        let mut v = self.links.lock().unwrap_or_else(PoisonError::into_inner).names();
        v.sort();
        v
    }

    fn devices(&self) -> Vec<String> {
        let mut v: Vec<String> = get(self.addr, "/api/devices")
            .json()
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["name"].as_str().unwrap().to_string())
            .collect();
        v.sort();
        v
    }

    fn head(&self) -> String {
        Store::open(&self.store)
            .unwrap()
            .head()
            .unwrap()
            .unwrap()
            .id
            .to_string()
    }

    /// Puts `cs` in the store without running it (committed, then head
    /// rolled back to what runs), so a rollback can name it.
    fn stage(&self, cs: &ChangeSet) -> String {
        let mut s = Store::open(&self.store).unwrap();
        let head = s.head().unwrap().unwrap().id;
        let id = s.commit(cs, &meta()).unwrap();
        s.rollback(&head, &meta()).unwrap();
        id.to_string()
    }

    fn replay(&self, path: &str) -> (u64, Json) {
        let r = post(self.addr, &format!("{path}/replay"), &json!({}));
        assert_eq!(r.status, 201, "{r:?}");
        let j = r.json();
        (j["replay"].as_u64().unwrap(), j)
    }

    /// Replays and rolls back to snapshot `id` with the engineer's PIN.
    fn rollback(&self, id: &str) -> Resp {
        let path = format!("/api/snapshots/{id}");
        let (n, _) = self.replay(&path);
        post(
            self.addr,
            &format!("{path}/rollback"),
            &json!({ "role": "engineer", "pin": ENGINEER, "replay": n }),
        )
    }

    /// Drafts a proposal; its id.
    fn propose(&self, body: &Json) -> u64 {
        let r = post(self.addr, "/api/proposals", body);
        assert_eq!(r.status, 201, "{r:?}");
        r.json()["id"].as_u64().unwrap()
    }

    /// Replays and applies proposal `id` with the engineer's PIN.
    fn apply(&self, id: u64) -> Resp {
        let path = format!("/api/proposals/{id}");
        let (n, _) = self.replay(&path);
        post(
            self.addr,
            &format!("{path}/apply"),
            &json!({ "role": "engineer", "pin": ENGINEER, "replay": n }),
        )
    }

    fn module_state(&self, name: &str) -> String {
        get(self.addr, &format!("/api/modules/{name}")).json()["status"]["state"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

impl Drop for Bench {
    fn drop(&mut self) {
        self.api.take();
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.scan.take() {
            let _ = t.join();
        }
        self.links
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .shutdown();
    }
}

fn wait(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if ok() {
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
    panic!("timed out waiting for {what}");
}

/// Watches device coils and table commands on its own thread, as fast as
/// it can, and keeps every sample that is not what it must be.
struct Watch {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<(u64, Vec<String>)>>,
}

impl Watch {
    /// `coils`: (simulator, coil, value it must hold); `commands`: (signal,
    /// command it must have).
    fn start(
        sims: Vec<(Arc<Sim>, usize, bool)>,
        table: &Arc<Mutex<SignalTable>>,
        commands: Vec<(&'static str, bool)>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let (st, table) = (Arc::clone(&stop), Arc::clone(table));
        let thread = thread::spawn(move || {
            let mut samples = 0u64;
            let mut bad = Vec::new();
            while !st.load(Ordering::Relaxed) {
                for (s, n, want) in &sims {
                    let got = s.state().coils[*n];
                    if got != *want && bad.len() < 20 {
                        bad.push(format!("{} coil {n} was {got}", s.endpoint()));
                    }
                }
                {
                    let t = table.lock().unwrap_or_else(PoisonError::into_inner);
                    for (name, want) in &commands {
                        let got = t.index(name).and_then(|i| t.command(i));
                        if got != Some(Value::Bool(*want)) && bad.len() < 20 {
                            bad.push(format!("command {name} was {got:?}"));
                        }
                    }
                }
                samples += 1;
                thread::sleep(Duration::from_micros(200));
            }
            (samples, bad)
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }

    /// Stops watching; panics with the bad samples, if any.
    fn finish(mut self) -> u64 {
        self.stop.store(true, Ordering::Relaxed);
        let (n, bad) = self.thread.take().unwrap().join().unwrap();
        assert_eq!(bad, Vec::<String>::new(), "glitches");
        assert!(n > 10, "only {n} samples");
        n
    }
}

#[derive(Debug)]
struct Resp {
    status: u16,
    body: String,
}

impl Resp {
    fn json(&self) -> Json {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("not JSON ({e}): {self:?}"))
    }

    fn error(&self) -> String {
        self.json()["error"].as_str().unwrap_or_default().to_string()
    }
}

fn request(addr: SocketAddr, method: &str, path: &str, body: &str) -> Resp {
    let raw = format!(
        "{method} {path} HTTP/1.1\r\nHost: box\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    s.write_all(raw.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let text = String::from_utf8_lossy(&out).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    Resp {
        status: head.get(9..12).and_then(|s| s.parse().ok()).unwrap_or(0),
        body: body.into(),
    }
}

fn get(addr: SocketAddr, path: &str) -> Resp {
    request(addr, "GET", path, "")
}

fn post(addr: SocketAddr, path: &str, body: &Json) -> Resp {
    request(addr, "POST", path, &body.to_string())
}

/// The box running [`CTL`] on [`map`], with a, b live and the valve and
/// horn on.
fn running(a: &Sim, b: &Sim) -> (ChangeSet, Bench) {
    let cs = change(&[CTL], &map(a, b), None);
    let bench = Bench::new(&cs, MONDAY);
    wait("valve and horn on", || coil(a, 0) && coil(b, 0));
    (cs, bench)
}

#[test]
fn rebinding_an_output_sends_the_old_coil_safe_and_the_command_to_the_new_one() {
    let (a, b) = (Arc::new(sim()), Arc::new(sim()));
    let (_, bench) = running(&a, &b);
    let watch = Watch::start(
        vec![(Arc::clone(&b), 0, true)],
        &bench.table,
        vec![("valve", true), ("horn", true)],
    );
    let moved = map(&a, &b).replace("valve bool a coil:0", "valve bool a coil:1");
    let id = bench.propose(&json!({ "kind": "io_map", "text": moved }));
    let r = bench.apply(id);
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(r.json()["tier"], "output-affecting");
    wait("the new coil on", || coil(&a, 1));
    assert!(!coil(&a, 0), "the old coil went to its safe value");
    assert_eq!(r.json()["reload"]["safed"], json!(["a coil:0"]));
    bench.settle();
    watch.finish();
    let sig = get(bench.addr, "/api/signals/valve").json();
    assert_eq!(sig["binding"]["point"], "coil:1", "{sig}");
    assert_eq!(bench.links(), ["a", "b"]);
}

#[test]
fn removing_a_device_mid_traffic_sends_its_outputs_safe_and_stops_its_link() {
    let (a, b) = (Arc::new(sim()), Arc::new(sim()));
    let (_, bench) = running(&a, &b);
    let without_b = drop_b(&map(&a, &b));
    let target = bench.stage(&change(&[CTL_VALVE], &without_b, None));
    let watch = Watch::start(
        vec![(Arc::clone(&a), 0, true)],
        &bench.table,
        vec![("valve", true)],
    );
    let r = bench.rollback(&target);
    assert_eq!(r.status, 200, "{r:?}");
    assert!(!coil(&b, 0), "the horn went safe on b before its link stopped");
    assert_eq!(r.json()["reload"]["links_removed"], json!(["b"]));
    let before = requests(&b);
    thread::sleep(Duration::from_millis(100));
    assert_eq!(requests(&b), before, "nothing talks to b any more");
    bench.settle();
    watch.finish();
    assert_eq!(bench.links(), ["a"]);
    assert_eq!(bench.devices(), ["a"]);
    assert_eq!(bench.value("horn"), None);
    assert_eq!(bench.head(), target);
}

#[test]
fn removing_a_device_that_does_not_answer_still_reloads_and_says_what_stayed_unsafe() {
    let a = Arc::new(sim());
    let b = sim();
    let (_, bench) = running(&a, &b);
    let without_b = drop_b(&map(&a, &b));
    let target = bench.stage(&change(&[CTL_VALVE], &without_b, None));
    drop(b);
    let r = bench.rollback(&target);
    assert_eq!(r.status, 200, "{r:?}");
    let notes = r.json()["reload"]["notes"].to_string();
    assert!(notes.contains("b coil:0"), "{notes}");
    assert_eq!(bench.links(), ["a"]);
    assert!(coil(&a, 0));
}

#[test]
fn adding_an_unreachable_device_holds_its_readers_and_nothing_else() {
    let (a, b) = (Arc::new(sim()), Arc::new(sim()));
    let (_, bench) = running(&a, &b);
    let target = bench.stage(&change(&[CTL, FILL], &with_dead_c(&map(&a, &b), false), None));
    let watch = Watch::start(
        vec![
            (Arc::clone(&a), 0, true),
            (Arc::clone(&b), 0, true),
            (Arc::clone(&a), 2, false),
        ],
        &bench.table,
        vec![("valve", true), ("horn", true)],
    );
    let path = format!("/api/snapshots/{target}");
    let (n, evidence) = bench.replay(&path);
    let reload = &evidence["reload"];
    assert_eq!(reload["io_map_changed"], true, "{evidence}");
    assert_eq!(reload["complete"], false);
    assert_eq!(
        reload["unrecorded"],
        json!([{ "module": "fill", "signal": "level" }])
    );
    assert!(reload["note"].as_str().unwrap().contains("fill held"), "{reload}");
    let r = post(
        bench.addr,
        &format!("{path}/rollback"),
        &json!({ "role": "engineer", "pin": ENGINEER, "replay": n }),
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(r.json()["reload"]["links_added"], json!(["c"]));
    wait("fill held", || bench.module_state("fill") == "held");
    wait("c reported down", || {
        get(bench.addr, "/api/devices")
            .json()
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["name"] == "c" && d["state"] == "down")
    });
    assert_eq!(bench.module_state("ctl"), "running");
    assert_eq!(bench.value("level"), None);
    assert_eq!(bench.value("lamp"), Some(Value::Bool(false)));
    bench.settle();
    watch.finish();
    assert_eq!(bench.links(), ["a", "b", "c"]);
}

#[test]
fn a_new_safe_value_reaches_the_device_for_what_no_healthy_module_drives() {
    let (a, b) = (Arc::new(sim()), Arc::new(sim()));
    let cs = change(&[CTL, FILL], &with_dead_c(&map(&a, &b), false), None);
    let bench = Bench::new(&cs, MONDAY);
    wait("valve on, fill held", || {
        coil(&a, 0) && bench.module_state("fill") == "held"
    });
    assert!(!coil(&a, 2) && !coil(&b, 3));
    let watch = Watch::start(
        vec![(Arc::clone(&a), 0, true), (Arc::clone(&b), 0, true)],
        &bench.table,
        vec![("valve", true), ("horn", true)],
    );
    let safer = with_dead_c(&map(&a, &b), true).replace("coil:3 safe=false", "coil:3 safe=true");
    let id = bench.propose(&json!({ "kind": "io_map", "text": safer }));
    let r = bench.apply(id);
    assert_eq!(r.status, 200, "{r:?}");
    // The held module's lamp and the undriven spare, at once.
    assert_eq!(bench.value("lamp"), Some(Value::Bool(true)));
    assert_eq!(bench.value("spare"), Some(Value::Bool(true)));
    wait("both on their devices", || coil(&a, 2) && coil(&b, 3));
    bench.settle();
    watch.finish();
}

#[test]
fn rolling_back_across_a_map_change_restores_the_old_map() {
    let (a, b) = (Arc::new(sim()), Arc::new(sim()));
    let (cs, bench) = running(&a, &b);
    let original = cs.id().to_string();
    let moved = map(&a, &b).replace("valve bool a coil:0", "valve bool a coil:1");
    let r = bench.apply(bench.propose(&json!({ "kind": "io_map", "text": moved })));
    assert_eq!(r.status, 200, "{r:?}");
    wait("moved", || coil(&a, 1) && !coil(&a, 0));
    let undo = r.json()["undo"].as_str().unwrap().to_string();
    assert_eq!(undo, format!("/api/snapshots/{original}/rollback"));
    let watch = Watch::start(
        vec![(Arc::clone(&b), 0, true)],
        &bench.table,
        vec![("valve", true)],
    );
    let r = bench.rollback(&original);
    assert_eq!(r.status, 200, "{r:?}");
    wait("back", || coil(&a, 0) && !coil(&a, 1));
    assert_eq!(bench.head(), original);
    let sig = get(bench.addr, "/api/signals/valve").json();
    assert_eq!(sig["binding"]["point"], "coil:0", "{sig}");
    let snaps = get(bench.addr, "/api/snapshots").json();
    assert_eq!(snaps["running"], original, "{snaps}");
    bench.settle();
    watch.finish();
}

#[test]
fn a_calendar_change_flips_in_shift_in_the_reload_step_and_then_on_its_own_times() {
    let (a, b) = (Arc::new(sim()), Arc::new(sim()));
    let cs = change(&[CTL], &map(&a, &b), Some(CAL_TO_14));
    let bench = Bench::new(&cs, MONDAY + 15 * 3600);
    assert_eq!(bench.value("in_shift"), Some(Value::Bool(false)));
    let id = bench.propose(&json!({ "kind": "calendar", "text": CAL_TO_16 }));
    let path = format!("/api/proposals/{id}");
    let (n, evidence) = bench.replay(&path);
    assert_eq!(evidence["reload"]["calendar_changed"], true, "{evidence}");
    // Watch in_shift from here: once it is on it must stay on, without a
    // missing tick between the old calendar and the new.
    let seen = Arc::new(Mutex::new(Vec::<Option<Value>>::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let watcher = {
        let (seen, stop, table) = (Arc::clone(&seen), Arc::clone(&stop), Arc::clone(&bench.table));
        thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let v = {
                    let t = table.lock().unwrap_or_else(PoisonError::into_inner);
                    t.index("in_shift").and_then(|i| t.value(i))
                };
                let mut s = seen.lock().unwrap_or_else(PoisonError::into_inner);
                if s.last() != Some(&v) {
                    s.push(v);
                }
                drop(s);
                thread::sleep(Duration::from_micros(200));
            }
        })
    };
    let r = post(
        bench.addr,
        &format!("{path}/apply"),
        &json!({ "role": "engineer", "pin": ENGINEER, "replay": n }),
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(r.json()["tier"], "logic", "the calendar drives the same outputs");
    assert_eq!(bench.value("in_shift"), Some(Value::Bool(true)));
    bench.clock.store(MONDAY + 16 * 3600 - 1, Ordering::SeqCst);
    bench.settle();
    assert_eq!(bench.value("in_shift"), Some(Value::Bool(true)));
    bench.clock.store(MONDAY + 16 * 3600, Ordering::SeqCst);
    bench.settle();
    assert_eq!(bench.value("in_shift"), Some(Value::Bool(false)));
    stop.store(true, Ordering::Relaxed);
    watcher.join().unwrap();
    let seen = seen.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let f = Some(Value::Bool(false));
    let t = Some(Value::Bool(true));
    assert_eq!(seen, [f, t, f], "in_shift never missing, flipped once each way");
}

#[test]
fn a_reload_whose_device_link_will_not_start_changes_nothing() {
    let (a, b) = (Arc::new(sim()), Arc::new(sim()));
    let (cs, bench) = running(&a, &b);
    let bad = format!(
        "{}device d modbus-tcp 127.0.0.1:9 unit=999\nin  dsig bool d di:0\n",
        map(&a, &b).replace("valve bool a coil:0", "valve bool a coil:1")
    );
    let watch = Watch::start(
        vec![
            (Arc::clone(&a), 0, true),
            (Arc::clone(&b), 0, true),
            (Arc::clone(&a), 1, false),
        ],
        &bench.table,
        vec![("valve", true), ("horn", true)],
    );
    let id = bench.propose(&json!({ "kind": "io_map", "text": bad }));
    let r = bench.apply(id);
    assert_eq!(r.status, 409, "{r:?}");
    assert!(r.error().contains("device link would not start"), "{r:?}");
    assert_eq!(r.json()["changed"], false);
    assert_eq!(bench.head(), cs.id().to_string());
    assert_eq!(get(bench.addr, "/api/signals/dsig").status, 404);
    assert_eq!(bench.links(), ["a", "b"]);
    assert_eq!(bench.devices(), ["a", "b"]);
    let ra = requests(&a);
    bench.settle();
    thread::sleep(Duration::from_millis(50));
    assert!(
        requests(&a) > ra,
        "a is still polled (b has no inputs, and nothing changed to write)"
    );
    watch.finish();
    // And a module set that does not fit the new map is refused whole too.
    let no_horn = map(&a, &b).replace("out horn  bool b coil:0 safe=false\n", "");
    let r = post(
        bench.addr,
        "/api/proposals",
        &json!({ "kind": "io_map", "text": no_horn }),
    );
    assert_eq!(r.status, 422, "the draft check names the module: {r:?}");
    let target = bench.stage(&change(&[CTL], &no_horn, None));
    let path = format!("/api/snapshots/{target}");
    let r = post(bench.addr, &format!("{path}/replay"), &json!({}));
    assert_eq!(r.status, 422, "{r:?}");
}

#[test]
fn concurrent_reloads_one_wins_whole_and_the_links_match_it() {
    let (a, b, c) = (Arc::new(sim()), Arc::new(sim()), Arc::new(sim()));
    let (_, bench) = running(&a, &b);
    let to_c = format!(
        "{}device c modbus-tcp {} unit=1 timeout_ms=1000 poll_ms=5\nin  level int c hr:0\n",
        map(&a, &b),
        c.endpoint()
    );
    let moved = map(&a, &b).replace("valve bool a coil:0", "valve bool a coil:1");
    let ids = [
        bench.propose(&json!({ "kind": "io_map", "text": to_c })),
        bench.propose(&json!({ "kind": "io_map", "text": moved })),
    ];
    let replays: Vec<u64> = ids
        .iter()
        .map(|id| bench.replay(&format!("/api/proposals/{id}")).0)
        .collect();
    let addr = bench.addr;
    let go = Arc::new(std::sync::Barrier::new(2));
    let threads: Vec<_> = ids
        .iter()
        .zip(replays)
        .map(|(&id, n)| {
            let go = Arc::clone(&go);
            thread::spawn(move || {
                go.wait();
                post(
                    addr,
                    &format!("/api/proposals/{id}/apply"),
                    &json!({ "role": "engineer", "pin": ENGINEER, "replay": n }),
                )
            })
        })
        .collect();
    let rs: Vec<Resp> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    let mut statuses: Vec<u16> = rs.iter().map(|r| r.status).collect();
    statuses.sort_unstable();
    assert_eq!(statuses, [200, 409], "{rs:?}");
    let won = rs.iter().position(|r| r.status == 200).unwrap();
    assert!(rs[1 - won].error().contains("stale"), "{rs:?}");
    if won == 0 {
        assert_eq!(bench.links(), ["a", "b", "c"]);
        wait("c polled", || requests(&c) > 0);
        assert!(coil(&a, 0) && !coil(&a, 1));
    } else {
        assert_eq!(bench.links(), ["a", "b"]);
        wait("moved", || coil(&a, 1) && !coil(&a, 0));
        assert_eq!(requests(&c), 0);
    }
    assert_eq!(bench.devices(), bench.links());
}

#[test]
fn fifty_reloads_never_glitch_what_they_leave_unchanged() {
    let (a, b, c) = (Arc::new(sim()), Arc::new(sim()), Arc::new(sim()));
    let (cs, bench) = running(&a, &b);
    let m0 = map(&a, &b);
    let variants = [
        cs.clone(),
        // A device added, with an input nobody reads.
        change(
            &[CTL],
            &format!(
                "{m0}device c modbus-tcp {} unit=1 timeout_ms=1000 poll_ms=5\nin  level int c hr:0 unit=mm\n",
                c.endpoint()
            ),
            None,
        ),
        // a's poll rate (a new driver for a), run's own rate, horn's unit.
        change(
            &[CTL],
            &m0.replace(
                "timeout_ms=1000 poll_ms=5\nin  run   bool a di:0",
                "timeout_ms=900 poll_ms=4\nin  run   bool a di:0 poll_ms=3",
            )
            .replace(
                "horn  bool b coil:0 safe=false",
                "horn  bool b coil:0 safe=false unit=on",
            ),
            None,
        ),
        // The spare's safe value, and a calendar.
        change(
            &[CTL],
            &m0.replace("coil:3 safe=false", "coil:3 safe=true"),
            Some(CAL_TO_14),
        ),
    ];
    assert_ne!(
        variants[2].io_map, variants[0].io_map,
        "the variant changes a's line"
    );
    let ids: Vec<String> = std::iter::once(cs.id().to_string())
        .chain(variants[1..].iter().map(|v| bench.stage(v)))
        .collect();
    let watch = Watch::start(
        vec![(Arc::clone(&a), 0, true), (Arc::clone(&b), 0, true)],
        &bench.table,
        vec![("valve", true), ("horn", true)],
    );
    // A fixed walk over the variants: deterministic, never the same twice
    // in a row.
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut at = 0usize;
    let mut reloads = 0;
    while reloads < 50 {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let next = usize::try_from(seed >> 62).unwrap();
        if next == at {
            continue;
        }
        let r = bench.rollback(&ids[next]);
        assert_eq!(r.status, 200, "reload {reloads} to variant {next}: {r:?}");
        at = next;
        reloads += 1;
        let want: Vec<&str> = if next == 1 {
            vec!["a", "b", "c"]
        } else {
            vec!["a", "b"]
        };
        assert_eq!(bench.links(), want, "after reload {reloads}");
        let spare = coil(&b, 3);
        if next == 3 {
            wait("spare safe=true", || coil(&b, 3));
        } else if spare {
            wait("spare safe=false", || !coil(&b, 3));
        }
    }
    bench.settle();
    let samples = watch.finish();
    assert!(samples > 100, "{samples}");
    assert_eq!(bench.head(), ids[at]);
}

#[test]
fn evidence_binds_to_the_map_a_replay_of_another_map_never_counts() {
    let (a, b) = (Arc::new(sim()), Arc::new(sim()));
    let (cs, bench) = running(&a, &b);
    let to_1 = map(&a, &b).replace("valve bool a coil:0", "valve bool a coil:1");
    let to_2 = map(&a, &b).replace("valve bool a coil:0", "valve bool a coil:2");
    let p1 = bench.propose(&json!({ "kind": "io_map", "text": to_1 }));
    let p2 = bench.propose(&json!({ "kind": "io_map", "text": to_2 }));
    let (n1, ev) = bench.replay(&format!("/api/proposals/{p1}"));
    assert_eq!(ev["reload"]["complete"], true, "{ev}");
    // Same modules, same calendar, another map: p1's evidence is not p2's.
    let r = post(
        bench.addr,
        &format!("/api/proposals/{p2}/apply"),
        &json!({ "role": "engineer", "pin": ENGINEER, "replay": n1 }),
    );
    assert_eq!(r.status, 428, "{r:?}");
    assert_eq!(bench.head(), cs.id().to_string());
    assert!(coil(&a, 0) && !coil(&a, 2));
}
