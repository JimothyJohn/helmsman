//! A map names a WASM driver (`wasm-driver-map`), and the driver is a file
//! (`wasm-driver-home`: `drivers/NAME.wasm` on the box or in the project;
//! `helmsman_plc::driver_dir`). Over real sockets, a real store, real
//! accounts, a real runtime scanning on its own thread, real device links
//! and the Modbus simulator over TCP as the device; the driver is the
//! Modbus/TCP guest helmsman-io ships as a test fixture.
//!
//! Adversarially: names that are paths never reach the file system, links
//! are never followed, a file that is not a v1 driver (or too large) is
//! refused at upload and again at load, only an engineer manages them, a
//! changed file runs from the next reload and never before, a project's
//! driver comes before the box's, a store written when drivers were blobs
//! (schema 3) still opens and rolls back, and concurrent uploads never
//! tear a file a load reads.

#![allow(clippy::many_single_char_names)] // sim, bench, response, proposal
#![allow(clippy::too_many_lines)] // each test walks one whole flow

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use helmsman_io::Value;
use helmsman_io::local::Channels;
use helmsman_io::sim::{Sim, State};
use helmsman_plc::api::swap::{self, Scene};
use helmsman_plc::api::{
    Api, Approval, Config, Devices, Extras, Limits, Live, MemAudit, MemProposals, More, Plant, Publisher,
    Shared,
};
use helmsman_plc::driver_dir::{self, DriverDir, Origin, Sources};
use helmsman_plc::drivers::{Drivers, MAX_DRIVERS};
use helmsman_plc::layout::Layout;
use helmsman_plc::link::LinkSet;
use helmsman_plc::policy::account::{Accounts, Logins};
use helmsman_plc::policy::pin::{Gate, Pins};
use helmsman_plc::policy::{FLOOR, Role, Tier, classify_with};
use helmsman_plc::project::{Projects, zip};
use helmsman_plc::record::{RecordConfig, Recorder};
use helmsman_plc::spec::Defaults;
use helmsman_plc::store::{ChangeSet, DB_FILE, Kind, Meta, ModuleFiles, SCHEMA, SnapshotId, Store};
use helmsman_plc::{Runtime, SignalTable, Window};
use serde_json::{Value as Json, json};

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const ALICE: &str = "alice-engineer-pw";
const BOB: &str = "bob-operator-pw";
const CAROL: &str = "carol-integrator-pw";

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

/// `tests/fixtures/store-v3-drivers/snapshots.db`, written by the build
/// that kept drivers as store blobs (#89, schema 3): [`running`] of
/// `map("modbus-tcp", "127.0.0.1:1")` (A), then of `map("wasm:modbus",
/// "127.0.0.1:1")` with the Modbus guest as its `modbus` driver blob (B).
const V3_A: &str = "4d75782a776dfb63aa89c9d0df430dcbc38da192b8e7d705def02d6f636cae2d";
const V3_B: &str = "5e54464623a5c6d0b7647145a2df8f401c4f20358f2f4213e04bf3a733af1d16";

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

fn sha(b: &[u8]) -> String {
    driver_dir::sha256(b)
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

/// Writes `bytes` as `<state>/drivers/NAME.wasm` by hand (not through the
/// API, so not checked).
fn place(state: &Path, name: &str, bytes: &[u8]) {
    let dir = state.join("drivers");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{name}.wasm")), bytes).unwrap();
}

/// A simulator holding `level` in register 0.
fn sim(level: u16) -> Sim {
    let mut st = State::new(16);
    st.holding[0] = level;
    Sim::start(st).unwrap()
}

fn wait(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
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
    body: Vec<u8>,
}

impl Resp {
    fn json(&self) -> Json {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!(
                "not JSON ({e}): {} {}",
                self.status,
                String::from_utf8_lossy(&self.body)
            )
        })
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

/// One request; `bearer` empty sends no `Authorization`.
fn send(addr: SocketAddr, method: &str, path: &str, bearer: &str, ctype: &str, body: &[u8]) -> Resp {
    let auth = if bearer.is_empty() {
        String::new()
    } else {
        format!("Authorization: Bearer {bearer}\r\n")
    };
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: box\r\n{auth}Content-Type: {ctype}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    s.write_all(head.as_bytes()).unwrap();
    // The box may answer (413) and close before a refused body is sent.
    let _ = s.write_all(body);
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let split = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or(out.len());
    let head = String::from_utf8_lossy(&out[..split]).into_owned();
    Resp {
        status: head.get(9..12).and_then(|s| s.parse().ok()).unwrap_or(0),
        body: out.get(split + 4..).unwrap_or_default().to_vec(),
    }
}

fn post(addr: SocketAddr, path: &str, bearer: &str, body: &Json) -> Resp {
    send(
        addr,
        "POST",
        path,
        bearer,
        "application/json",
        body.to_string().as_bytes(),
    )
}

fn io_map(text: &str) -> Json {
    json!({ "kind": "io_map", "text": text })
}

/// The box as `helmsman-plc --projects` runs it, minus the CIP adapter:
/// the scan thread (runtime, table, recorder, the swap channel served
/// between ticks), the device links, the API with real accounts (alice an
/// engineer, bob an operator, carol an integrator), a store whose head it
/// runs, and a state directory holding `drivers/` and `projects/`.
struct Bench {
    api: Option<Api>,
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    scan: Option<JoinHandle<()>>,
    table: Arc<Mutex<SignalTable>>,
    links: Arc<Mutex<LinkSet>>,
    store: PathBuf,
    state: PathBuf,
    ticks: Arc<AtomicU64>,
}

impl Bench {
    /// A box whose store holds `cs`; `setup` sees the state directory
    /// first (to place driver files by hand).
    fn new(cs: &ChangeSet, setup: impl FnOnce(&Path)) -> Self {
        let store = tmp("store");
        Store::open(&store).unwrap().commit(cs, &meta()).unwrap();
        let state = tmp("state");
        std::fs::create_dir_all(&state).unwrap();
        setup(&state);
        Self::start(store, state)
    }

    /// The box booting the head of the store in `store`, its drivers
    /// found as the binary finds them (the running project's, then the
    /// box's).
    fn start(store: PathBuf, state: PathBuf) -> Self {
        let d = defaults();
        let mut a = Accounts::default();
        a.add("alice", Role::Engineer, ALICE, 1, [1; 16]).unwrap();
        a.add("bob", Role::Operator, BOB, 1, [2; 16]).unwrap();
        a.add("carol", Role::Integrator, CAROL, 1, [3; 16]).unwrap();
        a.save(&state).unwrap();
        let cs = {
            let s = Store::open(&store).unwrap();
            let head = s.head().unwrap().unwrap().id;
            s.get(&head).unwrap()
        };
        let projects = Projects::in_state_dir(&state).unwrap();
        let sources = Sources {
            project: projects
                .running()
                .and_then(|r| projects.folder(&r.name).map(|dir| (r.name, dir))),
            boxed: Some(DriverDir::in_state_dir(&state).root().to_path_buf()),
        };
        let found = sources.find_text(cs.io_map.as_deref());
        let layout = driver_dir::layout(cs.io_map.as_deref(), cs.calendar.as_deref(), &found).unwrap();
        let modules = swap::compile_set(&cs, &d).unwrap();
        let mut rt = Runtime::with_signals(32, 32, modules, &layout.table).unwrap();
        let catalog = swap::catalog_of(&rt, &layout.table, &cs);
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
        let plan = set
            .plan_with(layout.map.as_ref().unwrap(), &found.drivers)
            .unwrap();
        let stopped = set.stop(&plan);
        let started = set.finish(plan, stopped);
        for n in &started.added {
            devices.add(n);
        }
        let links = Arc::new(Mutex::new(set));
        let (handle, swapper) = swap::channel(Duration::from_secs(5));
        let handle = handle.with_links(Arc::clone(&links));
        let t0 = Instant::now();
        let now: helmsman_plc::policy::pin::Clock = Arc::new(move || t0);
        let approval = Approval {
            policy: FLOOR,
            gate: Gate::new(Pins::default(), FLOOR.lockout, Arc::clone(&now)).with_logins(Logins::new(
                state.clone(),
                1,
                FLOOR.lockout,
                now,
            )),
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
                deadline: Duration::from_secs(10),
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
            running: Some(cs),
            snapshots: Some(Mutex::new(Store::open(&store).unwrap())),
            approval: Some(approval),
            chat: None,
        };
        let more = More {
            projects: Some(projects),
            forces: None,
            drivers: Some(DriverDir::in_state_dir(&state)),
            running_drivers: found.drivers,
            trend_pins: None,
        };
        let api = Api::start_box(cfg, shared, Extras::default(), None, more).unwrap();
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
            state,
            ticks,
        };
        b.settle(30);
        b
    }

    /// `n` more scans.
    fn settle(&self, n: u64) {
        let from = self.ticks.load(Ordering::Relaxed);
        wait("scans", || self.ticks.load(Ordering::Relaxed) > from + n);
    }

    fn value(&self, signal: &str) -> Option<Value> {
        let t = self.table.lock().unwrap_or_else(PoisonError::into_inner);
        t.index(signal).and_then(|i| t.value(i))
    }

    fn head(&self) -> (SnapshotId, Kind) {
        let e = Store::open(&self.store).unwrap().head().unwrap().unwrap();
        (e.id, e.kind)
    }

    fn login(&self, user: &str, password: &str) -> String {
        let r = post(
            self.addr,
            "/api/login",
            "",
            &json!({ "user": user, "password": password }),
        );
        assert_eq!(r.status, 200, "{r:?}");
        r.json()["token"].as_str().unwrap().to_string()
    }

    fn put_driver(&self, bearer: &str, name: &str, bytes: &[u8]) -> Resp {
        send(
            self.addr,
            "PUT",
            &format!("/api/drivers/{name}"),
            bearer,
            "application/wasm",
            bytes,
        )
    }

    fn delete_driver(&self, bearer: &str, name: &str) -> Resp {
        send(
            self.addr,
            "DELETE",
            &format!("/api/drivers/{name}"),
            bearer,
            "application/json",
            b"",
        )
    }

    fn list(&self, bearer: &str) -> Resp {
        send(self.addr, "GET", "/api/drivers", bearer, "application/json", b"")
    }

    fn propose(&self, body: &Json) -> Resp {
        post(self.addr, "/api/proposals", TOKEN, body)
    }

    /// Drafts a proposal that must check; its id and JSON.
    fn draft(&self, body: &Json) -> (u64, Json) {
        let r = self.propose(body);
        assert_eq!(r.status, 201, "{r:?}");
        let j = r.json();
        (j["id"].as_u64().unwrap(), j)
    }

    /// Replays `path` (a proposal, snapshot or project) then posts `then`
    /// citing it, as session `s`.
    fn approve(&self, s: &str, path: &str, then: &str) -> Resp {
        let r = post(self.addr, &format!("{path}/replay"), s, &json!({}));
        if r.status != 201 {
            return r;
        }
        let n = r.json()["replay"].as_u64().unwrap();
        post(self.addr, &format!("{path}/{then}"), s, &json!({ "replay": n }))
    }

    fn apply(&self, s: &str, id: u64) -> Resp {
        self.approve(s, &format!("/api/proposals/{id}"), "apply")
    }

    fn rollback(&self, s: &str, id: &str) -> Resp {
        self.approve(s, &format!("/api/snapshots/{id}"), "rollback")
    }

    fn deploy(&self, s: &str, project: &str) -> Resp {
        self.approve(s, &format!("/api/projects/{project}"), "deploy")
    }

    fn upload(&self, s: &str, project: &str, zip: &[u8]) -> Resp {
        send(
            self.addr,
            "POST",
            &format!("/api/projects/{project}/zip"),
            s,
            "application/zip",
            zip,
        )
    }

    fn box_file(&self, name: &str) -> Option<Vec<u8>> {
        std::fs::read(self.state.join("drivers").join(format!("{name}.wasm"))).ok()
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

// ---- the whole path, through the box -----------------------------------------

#[test]
fn the_modbus_guest_reads_the_simulator_end_to_end_from_a_driver_file() {
    let s = sim(7);
    let b = Bench::new(&running(&map("modbus-tcp", &s.endpoint())), |_| {});
    wait("level 7 natively", || b.value("level") == Some(Value::Int(7)));
    let alice = b.login("alice", ALICE);
    let guest = map("wasm:modbus", &s.endpoint());

    // No file yet: the map cannot name it, and says which file it wants.
    let r = b.propose(&io_map(&guest));
    assert_eq!(r.status, 422, "{r:?}");
    assert!(
        r.said().contains("no such WASM driver") && r.said().contains("drivers/modbus.wasm"),
        "{r:?}"
    );

    // An engineer uploads it: checked, written, listed; nothing reloads.
    let r = b.put_driver(&alice, "modbus", MODBUS);
    assert_eq!(r.status, 201, "{r:?}");
    let j = r.json();
    assert_eq!(j["sha256"], sha(MODBUS), "{j}");
    assert_eq!(j["ok"], true, "{j}");
    assert_eq!(j["map_name"], "wasm:modbus", "{j}");
    assert_eq!(j["replaced"], false, "{j}");
    assert_eq!(b.box_file("modbus").as_deref(), Some(MODBUS), "byte for byte");
    let l = b.list(&alice);
    assert_eq!(l.status, 200, "{l:?}");
    assert_eq!(
        l.json()["drivers"],
        json!([{ "name": "modbus", "map_name": "wasm:modbus", "bytes": MODBUS.len(), "sha256": sha(MODBUS), "ok": true, "problems": [] }])
    );

    // Now the map names it, and the device is read through the guest.
    let (id, p) = b.draft(&io_map(&guest));
    assert_eq!(p["approval"]["tier"], "output-affecting", "{p}");
    let r = b.apply(&alice, id);
    assert_eq!(r.status, 200, "{r:?}");
    let applied = r.json();
    assert_eq!(
        applied["reload"]["drivers"],
        json!({ "modbus": { "from": "the box", "sha256": sha(MODBUS) } }),
        "{applied}"
    );
    s.state().holding[0] = 12;
    wait("level 12 through the guest", || {
        b.value("level") == Some(Value::Int(12))
    });
    wait("the module lights the lamp", || {
        let t = b.table.lock().unwrap_or_else(PoisonError::into_inner);
        t.index("lamp").and_then(|i| t.command(i)) == Some(Value::Bool(true))
    });
}

#[test]
fn a_changed_driver_file_is_swapped_in_at_the_next_reload_and_never_mid_scan() {
    let s = sim(7);
    let guest = map("wasm:modbus", &s.endpoint());
    let b = Bench::new(&running(&guest), |state| place(state, "modbus", MODBUS));
    wait("level 7 through the guest", || {
        b.value("level") == Some(Value::Int(7))
    });
    let first = b.head().0.to_string();
    let alice = b.login("alice", ALICE);

    // A new version under the same name: written, and nothing that runs
    // notices. The link reads the device through the old bytes, scan after
    // scan (the new guest would answer 42 whatever the device holds).
    let r = b.put_driver(&alice, "modbus", &wat(FORTY_TWO));
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(r.json()["replaced"], true, "{r:?}");
    s.state().holding[0] = 9;
    wait("level 9 through the old bytes", || {
        b.value("level") == Some(Value::Int(9))
    });
    b.settle(200);
    assert_eq!(b.value("level"), Some(Value::Int(9)), "never mid-scan");

    // The next reload takes it: even a program-only change, whose map is
    // the same, loads the changed driver, and is held to the tier a driver
    // change is (it never rides in on a lesser one).
    let src = WATCH.1.replace("level > 10", "level > 11");
    let (id, p) = b.draft(&json!({ "kind": "module", "manifest": WATCH.0, "source": src }));
    assert_eq!(p["approval"]["tier"], "output-affecting", "{p}");
    assert!(
        p["approval"]["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["why"]
                .as_str()
                .unwrap()
                .contains("WASM driver modbus changed (its file)")),
        "{p}"
    );
    let r = b.apply(&alice, id);
    assert_eq!(r.status, 200, "{r:?}");
    let applied = r.json();
    assert_eq!(
        applied["reload"]["drivers"]["modbus"]["sha256"],
        sha(&wat(FORTY_TWO)),
        "{applied}"
    );
    wait("level 42 from the new file", || {
        b.value("level") == Some(Value::Int(42))
    });

    // Rollback across it: the old snapshot's map on the file as it is now
    // (the old bytes uploaded again): the device read through them.
    let r = b.put_driver(&alice, "modbus", MODBUS);
    assert_eq!(r.status, 200, "{r:?}");
    let r = b.rollback(&alice, &first);
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(
        r.json()["reload"]["drivers"]["modbus"]["sha256"],
        sha(MODBUS),
        "{r:?}"
    );
    s.state().holding[0] = 3;
    wait("level 3 through the old guest again", || {
        b.value("level") == Some(Value::Int(3))
    });
}

#[test]
fn a_projects_own_driver_comes_before_the_boxs_and_travels_in_its_zip() {
    let s = sim(7);
    let guest = map("wasm:modbus", &s.endpoint());
    let b = Bench::new(&running(&map("modbus-tcp", &s.endpoint())), |state| {
        place(state, "modbus", MODBUS);
    });
    let alice = b.login("alice", ALICE);
    let forty = wat(FORTY_TWO);
    let files = |driver: Option<&[u8]>| {
        let mut v = vec![
            ("io.map".to_string(), guest.clone().into_bytes()),
            ("watch.st".to_string(), WATCH.1.as_bytes().to_vec()),
        ];
        if let Some(d) = driver {
            v.push(("drivers/modbus.wasm".to_string(), d.to_vec()));
        }
        v.sort();
        zip::write(&v).unwrap()
    };

    // Project `line` carries its own modbus (the 42 guest); the box has the
    // real one. A deploy runs the project's.
    let r = b.upload(&alice, "line", &files(Some(&forty)));
    assert_eq!(r.status, 201, "{r:?}");
    assert_eq!(r.json()["drivers"], json!(["modbus"]), "{r:?}");
    let listed = send(b.addr, "GET", "/api/projects/line", "", "application/json", b"").json();
    assert_eq!(
        listed["drivers"],
        json!([{ "name": "modbus", "bytes": forty.len(), "sha256": sha(&forty) }]),
        "{listed}"
    );
    let r = b.deploy(&alice, "line");
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(
        r.json()["reload"]["drivers"]["modbus"],
        json!({ "from": "project line", "sha256": sha(&forty) }),
        "{r:?}"
    );
    wait("level 42: the project's driver, not the box's", || {
        b.value("level") == Some(Value::Int(42))
    });

    // Its .zip carries the driver, and round-trips byte for byte.
    let z = send(
        b.addr,
        "GET",
        "/api/projects/line/zip",
        "",
        "application/json",
        b"",
    );
    assert_eq!(z.status, 200, "{z:?}");
    let entries = zip::read(&z.body).unwrap();
    let carried = entries.iter().find(|e| e.name == "drivers/modbus.wasm").unwrap();
    assert_eq!(carried.data, forty);
    let r = b.upload(&alice, "copy", &z.body);
    assert_eq!(r.status, 201, "{r:?}");
    let again = send(
        b.addr,
        "GET",
        "/api/projects/copy/zip",
        "",
        "application/json",
        b"",
    );
    assert_eq!(again.body, z.body, "download, upload, download: the same bytes");
    // A copy made on the box carries it too.
    let r = post(
        b.addr,
        "/api/projects",
        &alice,
        &json!({ "name": "twin", "from": "line" }),
    );
    assert_eq!(r.status, 201, "{r:?}");
    let twin = send(
        b.addr,
        "GET",
        "/api/projects/twin/zip",
        "",
        "application/json",
        b"",
    );
    assert_eq!(twin.body, z.body);

    // A project without one runs the box's.
    let r = b.upload(&alice, "plain", &files(None));
    assert_eq!(r.status, 201, "{r:?}");
    let r = b.deploy(&alice, "plain");
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(
        r.json()["reload"]["drivers"]["modbus"]["from"],
        "the box",
        "{r:?}"
    );
    s.state().holding[0] = 8;
    wait("level 8 through the box's driver", || {
        b.value("level") == Some(Value::Int(8))
    });

    // The box's file is what the running map loads now: not deleted.
    let r = b.delete_driver(&alice, "modbus");
    assert_eq!(r.status, 409, "{r:?}");
    assert_eq!(b.box_file("modbus").as_deref(), Some(MODBUS));
    // With the project's in front of it again, it may go.
    let r = b.deploy(&alice, "line");
    assert_eq!(r.status, 200, "{r:?}");
    let r = b.delete_driver(&alice, "modbus");
    assert_eq!(r.status, 200, "{r:?}");
    assert!(b.box_file("modbus").is_none());
    let r = b.delete_driver(&alice, "modbus");
    assert_eq!(r.status, 404, "{r:?}");

    // A bad driver in an archive is refused whole: nothing written.
    let r = b.upload(&alice, "junk", &files(Some(b"\0asm junk")));
    assert_eq!(r.status, 422, "{r:?}");
    assert!(r.error().contains("drivers/modbus.wasm"), "{r:?}");
    let r = send(b.addr, "GET", "/api/projects/junk", "", "application/json", b"");
    assert_eq!(r.status, 404, "{r:?}");
}

// ---- who, which names, which bytes ---------------------------------------------

#[test]
fn only_an_engineer_lists_uploads_or_deletes_a_driver() {
    let s = sim(7);
    let b = Bench::new(&running(&map("modbus-tcp", &s.endpoint())), |_| {});
    let bob = b.login("bob", BOB);
    let carol = b.login("carol", CAROL);
    for (who, bearer) in [
        ("no credential", ""),
        ("the box token", TOKEN),
        ("an operator", bob.as_str()),
    ] {
        let r = b.list(bearer);
        assert_eq!(r.status, 403, "{who}: {r:?}");
        if bearer.is_empty() {
            continue; // a write without a credential is 401 before any route
        }
        let r = b.put_driver(bearer, "modbus", MODBUS);
        assert_eq!(r.status, 403, "{who}: {r:?}");
        assert!(r.error().contains("engineer"), "{who}: {r:?}");
        let r = b.delete_driver(bearer, "modbus");
        assert_eq!(r.status, 403, "{who}: {r:?}");
    }
    let r = b.put_driver("", "modbus", MODBUS);
    assert_eq!(r.status, 401, "{r:?}");
    assert!(b.box_file("modbus").is_none(), "nothing written");
    // An integrator is above an engineer.
    let r = b.put_driver(&carol, "modbus", MODBUS);
    assert_eq!(r.status, 201, "{r:?}");
    let r = b.delete_driver(&bob, "modbus");
    assert_eq!(r.status, 403, "{r:?}");
    assert!(b.box_file("modbus").is_some(), "an operator deletes nothing");
    // Every write and refusal is audited with who it was.
    let audit = send(b.addr, "GET", "/api/audit?limit=100", "", "application/json", b"").json();
    let lines: Vec<String> = audit["entries"]
        .as_array()
        .or_else(|| audit.as_array())
        .unwrap()
        .iter()
        .map(|e| format!("{} {} {}", e["status"], e["user"], e["detail"]))
        .collect();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("403") && l.contains("bob") && l.contains("not an engineer")),
        "{lines:#?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("201") && l.contains("carol") && l.contains("uploaded")),
        "{lines:#?}"
    );
}

#[test]
fn a_driver_name_that_is_a_path_never_reaches_the_file_system() {
    let s = sim(7);
    let b = Bench::new(&running(&map("modbus-tcp", &s.endpoint())), |_| {});
    let alice = b.login("alice", ALICE);
    let parent = b.state.parent().unwrap().to_path_buf();
    for name in [
        "..",
        "%2e%2e",
        "..%2fevil",
        "%2e%2e%2fevil",
        "a%2fb",
        "a%5cb",
        "..%5c..%5cevil",
        "Modbus",
        "modbus.wasm",
        "%00x",
        "x%00",
        ".hidden",
        "-x",
        "%20x",
        "m%D0%BEdbus",
        &"a".repeat(49),
    ] {
        let r = b.put_driver(&alice, name, MODBUS);
        assert!(matches!(r.status, 400 | 404), "{name}: {r:?}");
        let r = b.delete_driver(&alice, name);
        assert!(matches!(r.status, 400 | 404), "{name}: {r:?}");
    }
    assert!(
        !b.state.join("drivers").exists() || std::fs::read_dir(b.state.join("drivers")).unwrap().count() == 0,
        "nothing written in drivers/"
    );
    for p in [
        b.state.join("evil.wasm"),
        b.state.join("evil"),
        parent.join("evil.wasm"),
        parent.join("evil"),
        b.state.join("drivers").join("a"),
    ] {
        assert!(!p.exists(), "{} was written", p.display());
    }
    // A map line naming one is refused as not a driver name, before any
    // file is looked for.
    for driver in [
        "wasm:../modbus",
        "wasm:Modbus",
        "wasm:a/b",
        "wasm:modbus.wasm",
        "wasm:..",
        "wasm:",
    ] {
        let r = b.propose(&io_map(&map(driver, &s.endpoint())));
        assert_eq!(r.status, 422, "{driver}: {r:?}");
    }
    let r = b.propose(&io_map(&map("wasm:../modbus", &s.endpoint())));
    assert!(r.said().contains("not a driver name"), "{r:?}");
}

#[cfg(unix)]
#[test]
fn a_link_in_the_driver_folder_is_never_followed() {
    let s = sim(7);
    let outside = tmp("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let target = outside.join("real.wasm");
    std::fs::write(&target, MODBUS).unwrap();
    let b = Bench::new(&running(&map("modbus-tcp", &s.endpoint())), |state| {
        std::fs::create_dir_all(state.join("drivers")).unwrap();
        std::os::unix::fs::symlink(&target, state.join("drivers/link.wasm")).unwrap();
    });
    let alice = b.login("alice", ALICE);
    // A map naming it: refused, saying why.
    let r = b.propose(&io_map(&map("wasm:link", &s.endpoint())));
    assert_eq!(r.status, 422, "{r:?}");
    assert!(
        r.said().contains("drivers/link.wasm in the box") && r.said().contains("links are not followed"),
        "{r:?}"
    );
    // Listed with the problem; an upload does not write through it.
    let l = b.list(&alice).json();
    assert_eq!(l["drivers"][0]["name"], "link", "{l}");
    assert_eq!(l["drivers"][0]["ok"], false, "{l}");
    let r = b.put_driver(&alice, "link", &wat(FORTY_TWO));
    assert_eq!(r.status, 409, "{r:?}");
    assert_eq!(
        std::fs::read(&target).unwrap(),
        MODBUS,
        "the link's target untouched"
    );
    let r = b.delete_driver(&alice, "link");
    assert_eq!(r.status, 404, "{r:?}");
    assert!(target.exists());

    // A driver folder that is itself a link: nothing in it loads.
    let state = tmp("linked-state");
    std::fs::create_dir_all(&state).unwrap();
    std::os::unix::fs::symlink(&outside, state.join("drivers")).unwrap();
    std::fs::write(outside.join("modbus.wasm"), MODBUS).unwrap();
    let found = Sources::boxed(&state.join("drivers")).find_text(Some(&map("wasm:modbus", "127.0.0.1:9")));
    assert!(found.drivers.is_empty(), "{found:?}");
    assert!(
        found
            .problems
            .iter()
            .any(|p| p.contains("links are not followed")),
        "{found:?}"
    );
    let d = DriverDir::in_state_dir(&state);
    assert_eq!(d.put("x", MODBUS).unwrap_err().status, 500);
    assert!(!outside.join("x.wasm").exists(), "nothing written through it");
}

#[test]
fn an_oversized_or_invalid_wasm_is_refused_at_upload_and_again_at_load() {
    let s = sim(7);
    let b = Bench::new(&running(&map("modbus-tcp", &s.endpoint())), |state| {
        place(state, "bad", b"\0asm junk");
        place(state, "big", &vec![0; driver_dir::MAX_DRIVER + 1]);
        place(
            state,
            "module",
            &wat(r#"(module (memory (export "memory") 1) (func (export "scan")))"#),
        );
    });
    let alice = b.login("alice", ALICE);
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
        let r = b.put_driver(&alice, "rio-x", &bytes);
        assert_eq!(r.status, 422, "{why}: {r:?}");
        assert_eq!(r.error(), "not a driver this box can run", "{why}: {r:?}");
        assert!(
            r.json()["problems"][0]
                .as_str()
                .unwrap()
                .starts_with("helmsman-driver/v1: "),
            "{why}: {r:?}"
        );
    }
    let r = b.put_driver(&alice, "rio-x", &vec![0; driver_dir::MAX_DRIVER + 1]);
    assert_eq!(r.status, 413, "{r:?}");
    let r = b.put_driver(&alice, "rio-x", &vec![0; 3 << 19]);
    assert_eq!(r.status, 413, "past the body limit: {r:?}");
    let r = send(
        b.addr,
        "PUT",
        "/api/drivers/rio-x",
        &alice,
        "application/json",
        MODBUS,
    );
    assert_eq!(r.status, 415, "{r:?}");
    assert!(b.box_file("rio-x").is_none(), "nothing written");

    // Files put there by hand are checked when a map loads them, and the
    // map error names the file.
    for (name, says) in [
        ("bad", "helmsman-driver/v1"),
        ("big", "larger than"),
        ("module", "helmsman-driver/v1"),
    ] {
        let r = b.propose(&io_map(&map(&format!("wasm:{name}"), &s.endpoint())));
        assert_eq!(r.status, 422, "{name}: {r:?}");
        let said = r.said();
        assert!(
            said.contains(&format!("drivers/{name}.wasm in the box")) && said.contains(says),
            "{name}: {said}"
        );
    }
    let listed = b.list(&alice).json();
    let ok: Vec<(String, bool)> = listed["drivers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                d["name"].as_str().unwrap().to_string(),
                d["ok"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        ok,
        [
            ("bad".into(), false),
            ("big".into(), false),
            ("module".into(), false)
        ]
    );

    // A valid driver whose guest refuses the map's points: the point named.
    let bools = wat(&FORTY_TWO.replace(
        "(call $point (i32.const 1) (i32.const 0))",
        "(call $point (i32.const 0) (i32.const 0))",
    ));
    let r = b.put_driver(&alice, "bools", &bools);
    assert_eq!(r.status, 201, "{r:?}");
    let r = b.propose(&io_map(&map("wasm:bools", &s.endpoint())));
    assert_eq!(r.status, 422, "{r:?}");
    assert!(r.said().contains("level"), "{r:?}");
}

#[test]
fn a_driver_is_not_a_proposal_any_more() {
    let s = sim(7);
    let b = Bench::new(&running(&map("modbus-tcp", &s.endpoint())), |_| {});
    let r = b.propose(&json!({ "kind": "driver", "wasm": "AA==" }));
    assert_eq!(r.status, 400, "{r:?}");
    assert!(r.error().contains("PUT /api/drivers/NAME"), "{r:?}");
    let r = b.propose(&json!({ "kind": "driver", "name": "modbus", "wasm": "AA==" }));
    assert_eq!(r.status, 400, "{r:?}");
}

// ---- a store from when drivers were blobs (schema 3) ----------------------------

/// A copy of the fixture, so a test never writes the checked-in file.
fn v3_store() -> PathBuf {
    let dir = tmp("v3");
    std::fs::create_dir_all(&dir).unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/store-v3-drivers")
        .join(DB_FILE);
    std::fs::copy(fixture, dir.join(DB_FILE)).unwrap();
    dir
}

#[test]
fn a_schema_3_store_with_drivers_opens_with_them_ignored() {
    let dir = v3_store();
    let mut s = Store::open(&dir).unwrap();
    assert_eq!(s.schema().unwrap(), SCHEMA);
    let (a, b) = (SnapshotId::parse(V3_A).unwrap(), SnapshotId::parse(V3_B).unwrap());
    assert_eq!(
        s.list().unwrap().iter().map(|e| e.id).collect::<Vec<_>>(),
        [b, a],
        "the hash chain checks"
    );
    // A had no driver: the same content, the same id as ever.
    assert_eq!(s.get(&a).unwrap(), running(&map("modbus-tcp", "127.0.0.1:1")));
    // B is read without its driver blob, which is named for the log.
    let loaded = s.load(&b).unwrap();
    assert_eq!(loaded.ignored_drivers, ["modbus"]);
    assert_eq!(loaded.content, running(&map("wasm:modbus", "127.0.0.1:1")));
    let b_now = loaded.content.id();
    assert_ne!(b_now, b, "the id of what this build reads differs");
    // Rolling back to B commits B's content as its own snapshot, recorded
    // as a rollback to B; doing it again writes nothing.
    s.commit(&running(&map("modbus-tcp", "127.0.0.1:1")), &meta())
        .unwrap();
    assert_eq!(s.rollback(&b, &meta()).unwrap(), b_now);
    let head = s.head().unwrap().unwrap();
    assert_eq!((head.id, head.kind), (b_now, Kind::Rollback { to: b }));
    assert_eq!(s.rollback(&b, &meta()).unwrap(), b_now);
    assert_eq!(s.list().unwrap().len(), 3 + 1, "one entry for the two rollbacks");
    assert_eq!(s.get(&b_now).unwrap(), loaded.content);
    assert_eq!(s.schema().unwrap(), SCHEMA, "still a schema 3 store");
    drop(s);
    // And after reopening, every entry still checks.
    assert_eq!(Store::open(&dir).unwrap().list().unwrap().len(), 4);
    // Without a driver file its map does not load, and says which file.
    let empty = tmp("no-drivers");
    let found = Sources::boxed(&empty).find_text(loaded.content.io_map.as_deref());
    let e = driver_dir::layout(loaded.content.io_map.as_deref(), None, &found).unwrap_err();
    assert!(e.iter().any(|l| l.contains("drivers/modbus.wasm")), "{e:?}");
}

#[test]
fn a_box_boots_a_schema_3_head_on_its_driver_file_and_rolls_back_across_the_change() {
    // The fixture's device is on a closed port: 42 can only come from the
    // file, never from the old blob (the real Modbus guest).
    let state = tmp("v3-state");
    std::fs::create_dir_all(&state).unwrap();
    place(&state, "modbus", &wat(FORTY_TWO));
    let b = Bench::start(v3_store(), state);
    wait("level 42 from the driver file", || {
        b.value("level") == Some(Value::Int(42))
    });
    let alice = b.login("alice", ALICE);
    let r = b.rollback(&alice, V3_A);
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(b.head().0.to_string(), V3_A);
    // Back to B, by the id the old build gave it.
    let r = b.rollback(&alice, V3_B);
    assert_eq!(r.status, 200, "{r:?}");
    let now = running(&map("wasm:modbus", "127.0.0.1:1")).id();
    assert_eq!(r.json()["snapshot"], now.to_string(), "{r:?}");
    assert_eq!(
        b.head(),
        (
            now,
            Kind::Rollback {
                to: SnapshotId::parse(V3_B).unwrap()
            }
        )
    );
    wait("level 42 again", || b.value("level") == Some(Value::Int(42)));
}

// ---- finding files: precedence, problems, races -----------------------------------

#[test]
fn a_project_driver_comes_first_and_a_bad_one_never_falls_back_to_the_boxs() {
    let root = tmp("sources");
    let boxed = root.join("box");
    let project = root.join("projects/line");
    std::fs::create_dir_all(&boxed).unwrap();
    std::fs::create_dir_all(project.join("drivers")).unwrap();
    let forty = wat(FORTY_TWO);
    std::fs::write(boxed.join("modbus.wasm"), MODBUS).unwrap();
    std::fs::write(boxed.join("other.wasm"), MODBUS).unwrap();
    std::fs::write(project.join("drivers/modbus.wasm"), &forty).unwrap();
    let sources = Sources {
        project: Some(("line".into(), project.clone())),
        boxed: Some(boxed.clone()),
    };
    let text = "device a wasm:modbus 127.0.0.1:9\ndevice b wasm:other 127.0.0.1:9\ndevice c wasm:modbus 127.0.0.1:8\nout lamp bool box do:0\n";
    let found = sources.find_text(Some(text));
    assert!(found.problems.is_empty(), "{found:?}");
    assert_eq!(found.drivers["modbus"], forty, "the project's");
    assert_eq!(
        found.drivers["other"], MODBUS,
        "the box's, for what the project lacks"
    );
    assert_eq!(found.origin["modbus"], Origin::Project("line".into()));
    assert_eq!(found.origin["other"], Origin::Box);
    assert_eq!(found.drivers.len(), 2, "only what the map names");
    // A bad project file is the answer: the box's good one is not used.
    std::fs::write(project.join("drivers/modbus.wasm"), b"\0asm junk").unwrap();
    let found = sources.find_text(Some(text));
    assert!(!found.drivers.contains_key("modbus"), "{found:?}");
    assert_eq!(found.problems.len(), 2, "one per device naming it: {found:?}");
    assert!(
        found.problems[0].starts_with("device a: wasm:modbus: drivers/modbus.wasm in project line: "),
        "{found:?}"
    );
    // Neither has it: left to the map's own check, which names the file.
    let found = sources.find_text(Some("device a wasm:ghost 127.0.0.1:9\nout lamp bool box do:0\n"));
    assert!(found.problems.is_empty() && found.drivers.is_empty(), "{found:?}");
    let e = Layout::build_with(
        Some("device a wasm:ghost 127.0.0.1:9\nout lamp bool box do:0\n"),
        None,
        &found.drivers,
    )
    .unwrap_err();
    assert!(e.iter().any(|l| l.contains("drivers/ghost.wasm")), "{e:?}");
}

#[test]
fn the_box_folder_holds_at_most_its_cap_and_replacing_is_not_adding() {
    let state = tmp("cap");
    let d = DriverDir::in_state_dir(&state);
    for n in 0..MAX_DRIVERS {
        let (_, replaced) = d.put(&format!("d{n}"), MODBUS).unwrap();
        assert!(!replaced);
    }
    assert_eq!(d.put("one-more", MODBUS).unwrap_err().status, 507);
    let (_, replaced) = d.put("d0", &wat(FORTY_TWO)).unwrap();
    assert!(replaced, "a replacement fits at the cap");
    assert_eq!(d.list().unwrap().len(), MAX_DRIVERS);
    d.delete("d1").unwrap();
    d.put("one-more", MODBUS).unwrap();
}

/// Uploads racing loads: every load reads one whole version, never a
/// torn or empty file, and the folder holds no temporaries after.
#[test]
fn concurrent_uploads_never_tear_a_file_a_load_reads() {
    let state = tmp("race");
    let d = Arc::new(DriverDir::in_state_dir(&state));
    let forty = wat(FORTY_TWO);
    d.put("modbus", MODBUS).unwrap();
    let versions = [sha(MODBUS), sha(&forty)];
    let done = Arc::new(AtomicBool::new(false));
    let writers: Vec<_> = (0..4)
        .map(|w| {
            let (d, forty) = (Arc::clone(&d), forty.clone());
            thread::spawn(move || {
                for i in 0..25 {
                    let bytes = if (w + i) % 2 == 0 {
                        MODBUS
                    } else {
                        forty.as_slice()
                    };
                    d.put("modbus", bytes).unwrap();
                }
            })
        })
        .collect();
    let reader = {
        let (state, done, versions) = (state.clone(), Arc::clone(&done), versions.clone());
        thread::spawn(move || {
            let sources = Sources::boxed(&state.join("drivers"));
            let text = map("wasm:modbus", "127.0.0.1:9");
            let mut reads = 0;
            while !done.load(Ordering::Relaxed) || reads < 10 {
                let found = sources.find_text(Some(&text));
                assert!(found.problems.is_empty(), "{found:?}");
                assert!(versions.contains(&sha(&found.drivers["modbus"])));
                reads += 1;
            }
            reads
        })
    };
    for w in writers {
        w.join().unwrap();
    }
    done.store(true, Ordering::Relaxed);
    assert!(reader.join().unwrap() >= 10);
    let left: Vec<String> = std::fs::read_dir(state.join("drivers"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, ["modbus.wasm"], "no temporaries left");
}

// ---- the map's own check, and the tier ---------------------------------------------

#[test]
fn map_validation_refuses_a_driver_it_was_not_given() {
    let have = Drivers::from([("modbus".to_string(), MODBUS.to_vec())]);
    let ok = map("wasm:modbus", "127.0.0.1:9");
    let l = Layout::build_with(Some(&ok), None, &have).unwrap();
    assert_eq!(l.drivers, have);
    for (why, text, drivers) in [
        ("unknown name", map("wasm:ghost", "127.0.0.1:9"), have.clone()),
        ("none given", ok.clone(), Drivers::new()),
        ("case differs", map("wasm:Modbus", "127.0.0.1:9"), have.clone()),
        ("empty name", map("wasm:", "127.0.0.1:9"), have.clone()),
        (
            "a device no signal uses",
            "device spare wasm:ghost 127.0.0.1:9\nout lamp bool box do:0\n".to_string(),
            have.clone(),
        ),
    ] {
        let e = Layout::build_with(Some(&text), None, &drivers).unwrap_err();
        assert!(
            e.iter().any(|l| l.contains("no such WASM driver")),
            "{why}: {e:?}"
        );
    }
    assert!(Layout::build(Some(&ok), None).is_err());
    // The guest parses the points: one it refuses is refused.
    let bad = ok.replace("hr:0", "zz:0");
    let e = Layout::build_with(Some(&bad), None, &have).unwrap_err();
    assert!(e.iter().any(|l| l.contains("level")), "{e:?}");
    // And a device param the guest's configure refuses.
    let bad = ok.replace("unit=1", "unit=x");
    assert!(Layout::build_with(Some(&bad), None, &have).is_err());
}

/// The box build leaves `enip-io` out (TODO.md `enip-in-box`): a proposed
/// or reloaded map that names it is refused as not in this build, used by
/// a signal or not, while the same device over Modbus/TCP passes.
#[cfg(not(feature = "enip-io"))]
#[test]
fn map_validation_refuses_enip_io_as_not_in_this_build() {
    for text in [
        "device rio enip-io 127.0.0.1:44818\nin start bool rio di:0\nout lamp bool box do:0\n",
        "device rio enip-io 127.0.0.1:44818\nout lamp bool box do:0\n",
    ] {
        for drivers in [
            Drivers::new(),
            Drivers::from([("enip-io".to_string(), MODBUS.to_vec())]),
        ] {
            let e = Layout::build_with(Some(text), None, &drivers).unwrap_err();
            assert!(
                e.iter()
                    .any(|l| l.contains("device rio:") && l.contains("enip-io: not in this build")),
                "{text}: {e:?}"
            );
        }
    }
    let modbus = "device rio modbus-tcp 127.0.0.1:502\nin start bool rio di:0\nout lamp bool box do:0\n";
    Layout::build_with(Some(modbus), None, &Drivers::new()).unwrap();
}

#[test]
fn any_change_to_the_driver_bytes_a_map_loads_is_output_affecting() {
    let base = running(&map("wasm:modbus", "127.0.0.1:9"));
    let none = Drivers::new();
    let v1 = Drivers::from([("modbus".to_string(), MODBUS.to_vec())]);
    let v2 = Drivers::from([("modbus".to_string(), wat(FORTY_TWO))]);
    for (from, to, verb) in [
        (&none, &v1, "added"),
        (&v1, &v2, "changed"),
        (&v1, &none, "dropped"),
    ] {
        let c = classify_with(&base, &base, from, to);
        assert_eq!(c.tier, Tier::OutputAffecting, "{verb}: {c:?}");
        assert!(
            c.reasons.iter().any(|(_, w)| w.contains(verb))
                && c.reasons.iter().all(|(_, w)| w != "no change"),
            "{verb}: {c:?}"
        );
    }
    let same = classify_with(&base, &base, &v1, &v1.clone());
    assert_eq!(same.tier, Tier::Param, "{same:?}");
    assert!(FLOOR.rule(Tier::OutputAffecting).role >= Role::Engineer);
}
