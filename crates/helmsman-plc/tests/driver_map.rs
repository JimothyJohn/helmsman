//! A map names a WASM driver (`wasm-driver-map`): a driver is a blob in
//! the change set, proposed and approved at the engineer tier
//! (`wasm-driver-home`), and a device line `device ID wasm:NAME ...` runs
//! on it. Over real sockets, a real store, a real runtime scanning on its
//! own thread, real device links and the Modbus simulator over TCP as the
//! device; the driver is the Modbus/TCP guest helmsman-io ships as a test
//! fixture. Adversarially: an unapproved name never reaches a map, a blob
//! that is not a v1 driver never reaches a proposal, an operator never
//! approves one, and rollback brings the old driver's bytes back.

#![allow(clippy::many_single_char_names)] // sim, bench, response, proposal

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use helmsman_io::Value;
use helmsman_io::local::Channels;
use helmsman_io::sim::{Sim, State};
use helmsman_plc::api::swap::{self, Scene};
use helmsman_plc::api::{
    Api, Approval, Config, Devices, Extras, Limits, Live, MemAudit, MemProposals, Plant, Publisher, Shared,
};
use helmsman_plc::drivers::Drivers;
use helmsman_plc::layout::Layout;
use helmsman_plc::link::LinkSet;
use helmsman_plc::policy::pin::{Gate, Pins};
use helmsman_plc::policy::{FLOOR, Role, Tier, classify};
use helmsman_plc::record::{RecordConfig, Recorder};
use helmsman_plc::spec::Defaults;
use helmsman_plc::store::{ChangeSet, DB_FILE, Meta, ModuleFiles, SCHEMA, SCHEMA_V2, Store};
use helmsman_plc::{Runtime, SignalTable, Window};
use serde_json::{Value as Json, json};

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const ENGINEER: &str = "eng-135797";
const OPERATOR: &str = "op-246810";

/// The Modbus/TCP driver as a `helmsman-driver/v1` guest (helmsman-io's
/// fixture, built by scripts/build-wasm-drivers.sh).
const MODBUS: &[u8] = include_bytes!("../../helmsman-io/guests/modbus-tcp.wasm");

/// A valid v1 driver that never touches its socket: every point is a
/// read-only INT, and every read answers 42.
const FORTY_TWO: &str = r#"(module
  (import "helmsman-driver/v1" "point" (func $point (param i32 i32)))
  (import "helmsman-driver/v1" "value" (func $value (param i32 i64)))
  (memory (export "memory") 1)
  (func (export "buffer") (param i32) (result i32) (i32.const 1024))
  (func (export "parse") (param i32 i32) (call $point (i32.const 1) (i32.const 0)))
  (func (export "write") (param i32 i32 i32 i64))
  (func (export "read") (param $p i32) (param $n i32)
    (local $end i32)
    (local.set $end (i32.add (local.get $p) (local.get $n)))
    (block $done
      (loop $next
        (br_if $done (i32.ge_u (local.get $p) (local.get $end)))
        (call $value (i32.const 1) (i64.const 42))
        (local.set $p (i32.add (local.get $p) (i32.add (i32.load16_u (local.get $p)) (i32.const 2))))
        (br $next)))))"#;

/// Reads `level`, lights `lamp` above 10.
const WATCH: (&str, &str) = (
    "\
name     watch
author   Test <test@example.com>
scan_ms  2
reads    level
drives   lamp
",
    "
PROGRAM watch
VAR_INPUT level : DINT; END_VAR
VAR_OUTPUT lamp : BOOL; END_VAR
  lamp := level > 10;
END_PROGRAM",
);

/// `level` on holding register 0 of the device at `endpoint`, spoken to
/// by `driver`; the lamp on the box's own output.
fn map(driver: &str, endpoint: &str) -> String {
    format!(
        "\
device rio {driver} {endpoint} unit=1 timeout_ms=1000 poll_ms=5
in  level int  rio hr:0
out lamp  bool box do:0 safe=false
"
    )
}

fn wat(src: &str) -> Vec<u8> {
    wat::parse_str(src).unwrap()
}

fn b64(b: &[u8]) -> String {
    helmsman_plc::api::photo::base64(b)
}

fn running(io_map: &str) -> ChangeSet {
    ChangeSet {
        modules: BTreeMap::from([(
            "watch".to_string(),
            ModuleFiles {
                manifest: WATCH.0.into(),
                code: WATCH.1.into(),
            },
        )]),
        io_map: Some(io_map.into()),
        calendar: None,
        drivers: Drivers::new(),
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
        "driver-map-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A simulator holding `level` in register 0.
fn sim(level: u16) -> Sim {
    let mut st = State::new(16);
    st.holding[0] = level;
    Sim::start(st).unwrap()
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

    /// The error and its problems, one string.
    fn said(&self) -> String {
        let j = self.json();
        format!("{} {}", j["error"], j["problems"])
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

/// The box as `helmsman-plc` runs it, minus the CIP adapter: the scan
/// thread (runtime, table, recorder, the swap channel served between
/// ticks), the device links, the API, and a store whose head it runs.
struct Bench {
    api: Option<Api>,
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    scan: Option<JoinHandle<()>>,
    table: Arc<Mutex<SignalTable>>,
    links: Arc<Mutex<LinkSet>>,
    store: PathBuf,
    ticks: Arc<AtomicU64>,
}

impl Bench {
    #[allow(clippy::too_many_lines)]
    fn new(cs: &ChangeSet) -> Self {
        let d = defaults();
        let layout = Layout::build_with(cs.io_map.as_deref(), cs.calendar.as_deref(), &cs.drivers).unwrap();
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
        let plan = set.plan_with(layout.map.as_ref().unwrap(), &cs.drivers).unwrap();
        let stopped = set.stop(&plan);
        let started = set.finish(plan, stopped);
        for n in &started.added {
            devices.add(n);
        }
        let links = Arc::new(Mutex::new(set));
        let (handle, swapper) = swap::channel(Duration::from_secs(5));
        let handle = handle.with_links(Arc::clone(&links));
        let mut pins = Pins::default();
        pins.set(Role::Engineer, ENGINEER, 1, [2; 16]).unwrap();
        pins.set(Role::Operator, OPERATOR, 1, [3; 16]).unwrap();
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
            chat: None,
        };
        let api = Api::start_with(cfg, shared, Extras::default()).unwrap();
        let addr = api.addr();
        let stop = Arc::new(AtomicBool::new(false));
        let ticks = Arc::new(AtomicU64::new(0));
        let scan = {
            let (stop, table, ticks) = (Arc::clone(&stop), Arc::clone(&table), Arc::clone(&ticks));
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
                        swapper.serve_scene(&mut rt, &mut t, Some(&mut scene));
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

    fn head(&self) -> ChangeSet {
        let s = Store::open(&self.store).unwrap();
        let id = s.head().unwrap().unwrap().id;
        s.get(&id).unwrap()
    }

    fn replay(&self, path: &str) -> u64 {
        let r = post(self.addr, &format!("{path}/replay"), &json!({}));
        assert_eq!(r.status, 201, "{r:?}");
        r.json()["replay"].as_u64().unwrap()
    }

    fn propose(&self, body: &Json) -> Resp {
        post(self.addr, "/api/proposals", body)
    }

    /// Drafts a proposal that must check; its id and JSON.
    fn draft(&self, body: &Json) -> (u64, Json) {
        let r = self.propose(body);
        assert_eq!(r.status, 201, "{r:?}");
        let j = r.json();
        (j["id"].as_u64().unwrap(), j)
    }

    /// Replays and applies proposal `id` as `role` with `pin`.
    fn apply_as(&self, id: u64, role: &str, pin: &str) -> Resp {
        let path = format!("/api/proposals/{id}");
        let n = self.replay(&path);
        post(
            self.addr,
            &format!("{path}/apply"),
            &json!({ "role": role, "pin": pin, "replay": n }),
        )
    }

    fn apply(&self, id: u64) -> Json {
        let r = self.apply_as(id, "engineer", ENGINEER);
        assert_eq!(r.status, 200, "{r:?}");
        r.json()
    }

    fn rollback(&self, id: &str) -> Resp {
        let path = format!("/api/snapshots/{id}");
        let n = self.replay(&path);
        post(
            self.addr,
            &format!("{path}/rollback"),
            &json!({ "role": "engineer", "pin": ENGINEER, "replay": n }),
        )
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

fn driver(name: &str, bytes: &[u8]) -> Json {
    json!({ "kind": "driver", "name": name, "wasm": b64(bytes) })
}

fn io_map(text: &str) -> Json {
    json!({ "kind": "io_map", "text": text })
}

// ---- the whole path, through the box -----------------------------------------

#[test]
fn a_map_line_on_the_modbus_guest_reads_the_simulator_end_to_end_and_follows_each_approved_version() {
    let s = sim(7);
    let native = map("modbus-tcp", &s.endpoint());
    let guest = map("wasm:modbus", &s.endpoint());
    let b = Bench::new(&running(&native));
    wait("level 7 natively", || b.value("level") == Some(Value::Int(7)));

    // The map cannot name the driver before it is approved.
    let r = b.propose(&io_map(&guest));
    assert_eq!(r.status, 422, "{r:?}");
    assert!(
        r.said().contains("not a WASM driver this box has approved"),
        "{r:?}"
    );

    // Approving it: output-affecting, so an operator never can.
    let (id, p) = b.draft(&driver("modbus", MODBUS));
    assert_eq!(p["kind"], "driver", "{p}");
    assert_eq!(p["approval"]["tier"], "output-affecting", "{p}");
    assert_eq!(p["wasm"]["abi"], "helmsman-driver/v1", "{p}");
    let r = b.apply_as(id, "operator", OPERATOR);
    assert_eq!(r.status, 403, "{r:?}");
    assert!(r.error().contains("needs the engineer role"), "{r:?}");
    assert!(b.head().drivers.is_empty(), "nothing applied");
    let first = b.apply(id);
    assert_eq!(b.head().drivers["modbus"], MODBUS);

    // Now the map names it, and the device is read through the guest.
    let (id, _) = b.draft(&io_map(&guest));
    b.apply(id);
    s.state().holding[0] = 12;
    wait("level 12 through the guest", || {
        b.value("level") == Some(Value::Int(12))
    });
    wait("the module lights the lamp", || {
        let t = b.table.lock().unwrap_or_else(PoisonError::into_inner);
        t.index("lamp").and_then(|i| t.command(i)) == Some(Value::Bool(true))
    });
    let shown = get(
        b.addr,
        &format!("/api/snapshots/{}", first["snapshot"].as_str().unwrap()),
    );
    assert_eq!(shown.status, 200, "{shown:?}");

    // A new version of the driver under the same map line: the reload
    // reopens the link on it (42 is the new guest's, not the device's).
    let before = b.head();
    let (id, p) = b.draft(&driver("modbus", &wat(FORTY_TWO)));
    assert_eq!(p["replaces"], true, "{p}");
    assert_eq!(p["devices"], json!(["rio"]), "{p}");
    let applied = b.apply(id);
    assert!(applied["reload"].is_object(), "{applied}");
    wait("level 42 from the new version", || {
        b.value("level") == Some(Value::Int(42))
    });

    // Rollback: the old driver's bytes, and the device read through them.
    let r = b.rollback(&before.id().to_string());
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(b.head().drivers["modbus"], MODBUS, "the old bytes, byte for byte");
    assert_eq!(b.head(), before);
    s.state().holding[0] = 3;
    wait("level 3 through the old guest again", || {
        b.value("level") == Some(Value::Int(3))
    });
}

// ---- refused at proposal -------------------------------------------------------

#[test]
fn a_blob_that_is_not_a_v1_driver_is_refused_at_proposal() {
    let s = sim(7);
    let b = Bench::new(&running(&map("modbus-tcp", &s.endpoint())));
    let head = b.head();
    let module = wat(r#"(module
      (import "helmsman/v1/in" "level" (func (result i64)))
      (memory (export "memory") 1)
      (func (export "scan")))"#);
    let wasi = wat(&FORTY_TWO.replacen(
        "(memory",
        r#"(import "wasi_snapshot_preview1" "fd_write" (func (param i32 i32 i32 i32) (result i32)))
  (memory"#,
        1,
    ));
    let connect = wat(&FORTY_TWO.replacen(
        "(memory",
        r#"(import "helmsman-driver/v1" "connect" (func (param i32 i32) (result i32)))
  (memory"#,
        1,
    ));
    let no_read = wat(&FORTY_TWO.replace(r#"(export "read")"#, ""));
    let huge = wat(&FORTY_TWO.replace(
        r#"(memory (export "memory") 1)"#,
        r#"(memory (export "memory") 64)"#,
    ));
    let start = wat(&FORTY_TWO.replacen("(memory", "(func $boot) (start $boot)\n  (memory", 1));
    let traps = wat(&FORTY_TWO.replacen(
        "(memory (export \"memory\") 1)",
        "(memory (export \"memory\") 1)\n  (data (i32.const 65535) \"xx\")",
        1,
    ));
    for (why, bytes) in [
        ("junk", b"\0asm junk".to_vec()),
        ("empty", Vec::new()),
        ("a WASM module, not a driver", module),
        ("imports WASI", wasi),
        ("imports a connect", connect),
        ("exports no read", no_read),
        ("declares 4 MiB of memory", huge),
        ("has a start function", start),
        ("a data segment past its memory", traps),
    ] {
        let r = b.propose(&driver("rio-x", &bytes));
        assert_eq!(r.status, 422, "{why}: {r:?}");
        assert_eq!(r.error(), "not a driver this box can run", "{why}: {r:?}");
    }
    for (why, body, status) in [
        ("a name that is a path", driver("../x", &wat(FORTY_TWO)), 422),
        ("an empty name", driver("", &wat(FORTY_TWO)), 422),
        ("a name too long", driver(&"x".repeat(65), &wat(FORTY_TWO)), 422),
        (
            "base64 with whitespace",
            json!({ "kind": "driver", "name": "x", "wasm": format!("{} ", b64(&wat(FORTY_TWO))) }),
            400,
        ),
        (
            "no name",
            json!({ "kind": "driver", "wasm": b64(&wat(FORTY_TWO)) }),
            400,
        ),
        ("no binary", json!({ "kind": "driver", "name": "x" }), 400),
        (
            "a field of another kind",
            json!({ "kind": "driver", "name": "x", "wasm": b64(&wat(FORTY_TWO)), "text": "" }),
            400,
        ),
        (
            "a module proposal naming a driver",
            json!({ "kind": "module", "name": "x", "manifest": WATCH.0, "source": WATCH.1 }),
            400,
        ),
    ] {
        let r = b.propose(&body);
        assert_eq!(r.status, status, "{why}: {r:?}");
    }
    assert_eq!(b.head(), head, "nothing stored or applied");
    // The same checks run for a draft that is only checked.
    let r = post(b.addr, "/api/proposals/check", &driver("rio-x", b"\0asm junk"));
    assert_eq!(r.status, 422, "{r:?}");
}

#[test]
fn a_new_version_that_cannot_parse_the_running_maps_points_is_refused_at_proposal() {
    let s = sim(7);
    let mut cs = running(&map("wasm:modbus", &s.endpoint()));
    cs.drivers.insert("modbus".into(), MODBUS.to_vec());
    let b = Bench::new(&cs);
    wait("level 7 through the guest", || {
        b.value("level") == Some(Value::Int(7))
    });
    // Every point is a BOOL to this one; the map reads `level` as an int.
    let bools = wat(&FORTY_TWO.replace(
        "(call $point (i32.const 1) (i32.const 0))",
        "(call $point (i32.const 0) (i32.const 0))",
    ));
    let r = b.propose(&driver("modbus", &bools));
    assert_eq!(r.status, 422, "{r:?}");
    assert!(r.said().contains("level"), "{r:?}");
    assert_eq!(b.head(), cs);
}

// ---- the map's own check ------------------------------------------------------

#[test]
fn map_validation_refuses_a_driver_name_that_is_not_approved() {
    let approved = Drivers::from([("modbus".to_string(), MODBUS.to_vec())]);
    let ok = map("wasm:modbus", "127.0.0.1:9");
    let l = Layout::build_with(Some(&ok), None, &approved).unwrap();
    assert_eq!(l.drivers, approved);
    for (why, text, drivers) in [
        ("unknown name", map("wasm:ghost", "127.0.0.1:9"), approved.clone()),
        ("nothing approved", ok.clone(), Drivers::new()),
        (
            "case differs",
            map("wasm:Modbus", "127.0.0.1:9"),
            approved.clone(),
        ),
        ("empty name", map("wasm:", "127.0.0.1:9"), approved.clone()),
        (
            "a device no signal uses",
            "device spare wasm:ghost 127.0.0.1:9\nout lamp bool box do:0\n".to_string(),
            approved.clone(),
        ),
    ] {
        let e = Layout::build_with(Some(&text), None, &drivers).unwrap_err();
        assert!(
            e.iter()
                .any(|l| l.contains("not a WASM driver this box has approved")),
            "{why}: {e:?}"
        );
    }
    // Without the box's driver set (the map's own check), never.
    assert!(Layout::build(Some(&ok), None).is_err());
    // The guest parses the points: one it refuses is refused.
    let bad = ok.replace("hr:0", "zz:0");
    let e = Layout::build_with(Some(&bad), None, &approved).unwrap_err();
    assert!(e.iter().any(|l| l.contains("level")), "{e:?}");
    // And a device param the guest's configure refuses.
    let bad = ok.replace("unit=1", "unit=x");
    assert!(Layout::build_with(Some(&bad), None, &approved).is_err());
}

// ---- store, policy ------------------------------------------------------------

#[test]
fn a_driver_round_trips_through_the_store_and_rollback_restores_its_old_bytes() {
    let dir = tmp("rollback");
    let mut s = Store::open(&dir).unwrap();
    let base = running(&map("modbus-tcp", "127.0.0.1:9"));
    s.commit(&base, &meta()).unwrap();
    let mut v1 = base.clone();
    v1.drivers.insert("modbus".into(), MODBUS.to_vec());
    let id1 = s.commit(&v1, &meta()).unwrap();
    assert_eq!(s.schema().unwrap(), SCHEMA);
    assert!(
        v1.tree().lines().any(|l| l.starts_with("wasm-driver modbus ")),
        "{}",
        v1.tree()
    );
    let mut v2 = v1.clone();
    v2.drivers.insert("modbus".into(), wat(FORTY_TWO));
    let id2 = s.commit(&v2, &meta()).unwrap();
    assert_ne!(id1, id2);
    assert_eq!(s.get(&id2).unwrap().drivers["modbus"], wat(FORTY_TWO));
    s.rollback(&id1, &meta()).unwrap();
    let head = s.head().unwrap().unwrap().id;
    assert_eq!(head, id1);
    assert_eq!(s.get(&head).unwrap().drivers["modbus"], MODBUS);
    drop(s);
    let s = Store::open(&dir).unwrap();
    assert_eq!(s.get(&id1).unwrap(), v1, "after reopening too");
    // Drivers are checked as the store takes them: names and count.
    let mut bad = base.clone();
    bad.drivers.insert("../x".into(), MODBUS.to_vec());
    assert!(bad.validate().is_err());
    let mut many = base;
    for n in 0..=helmsman_plc::drivers::MAX_DRIVERS {
        many.drivers.insert(format!("d{n}"), vec![0]);
    }
    assert!(many.validate().is_err());
}

#[test]
fn a_store_without_drivers_keeps_its_version_until_it_holds_one() {
    let dir = tmp("schema");
    let wasm_module = running(&map("modbus-tcp", "127.0.0.1:9"));
    drop(Store::open(&dir).unwrap());
    // As an older build left it: a v2 store (WASM modules, no drivers).
    rusqlite::Connection::open(dir.join(DB_FILE))
        .unwrap()
        .pragma_update(None, "user_version", SCHEMA_V2)
        .unwrap();
    let mut s = Store::open(&dir).unwrap();
    s.commit(&wasm_module, &meta()).unwrap();
    assert_eq!(s.schema().unwrap(), SCHEMA_V2, "a v2 build still opens it");
    let mut cs = wasm_module;
    cs.drivers.insert("modbus".into(), MODBUS.to_vec());
    // One the store refuses changes nothing.
    let mut bad = cs.clone();
    bad.drivers.insert("../x".into(), MODBUS.to_vec());
    assert!(s.commit(&bad, &meta()).is_err());
    assert_eq!(s.schema().unwrap(), SCHEMA_V2);
    s.commit(&cs, &meta()).unwrap();
    assert_eq!(s.schema().unwrap(), SCHEMA, "a v2 build is told to upgrade");
}

#[test]
fn any_driver_change_is_output_affecting() {
    let base = running(&map("modbus-tcp", "127.0.0.1:9"));
    let mut added = base.clone();
    added.drivers.insert("modbus".into(), MODBUS.to_vec());
    let mut changed = added.clone();
    changed.drivers.insert("modbus".into(), wat(FORTY_TWO));
    for (a, b) in [(&base, &added), (&added, &changed), (&added, &base)] {
        let c = classify(a, b);
        assert_eq!(c.tier, Tier::OutputAffecting, "{c:?}");
    }
    assert!(FLOOR.rule(Tier::OutputAffecting).role >= Role::Engineer);
}
