//! The approval path over real sockets, a real snapshot store and a real
//! runtime scanning on its own thread with the recorder running: applying
//! and rolling back are checked against tier, policy, role, PIN and sim
//! evidence, land as one swap plus one commit, and are audited whatever
//! happens. Nothing here is mocked; the only test hook is the PIN gate's
//! clock (so a 30 s lockout does not take 30 s).

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use helmsman_iec::{ImageSizes, compile};
use helmsman_io::Value;
use helmsman_plc::api::{
    Api, Approval, Catalog, Config, Devices, Limits, Live, MemAudit, MemProposals, ModuleInfo, Plant,
    Publisher, Shared, swap,
};
use helmsman_plc::manifest::ModuleDir;
use helmsman_plc::policy::pin::{Gate, Pins};
use helmsman_plc::policy::{FLOOR, Policy, Role};
use helmsman_plc::record::{RecordConfig, Recorder};
use helmsman_plc::spec::Defaults;
use helmsman_plc::store::{ChangeSet, Meta, ModuleFiles, Store};
use helmsman_plc::{Runtime, SignalTable, Window};
use serde_json::{Value as Json, json};

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const OPERATOR: &str = "op-246810";
const ENGINEER: &str = "eng-135797";
const INTEGRATOR: &str = "int-975311";

const MANIFEST: &str = "\
name     conveyor
author   Test <test@example.com>
prompt   Light the lamp while start is held and the speed is up.
scan_ms  2
reads    start
drives   lamp
param    speed real 1.5 min=0 max=10
";

const SOURCE: &str = "
PROGRAM conveyor
VAR_INPUT start : BOOL; speed : REAL; END_VAR
VAR_OUTPUT lamp : BOOL; END_VAR
VAR n : UDINT; END_VAR
  n := n + 1;
  lamp := start AND speed > 1.0;
END_PROGRAM";

/// The same outputs, other logic: a `logic` change.
const SOURCE_V2: &str = "
PROGRAM conveyor
VAR_INPUT start : BOOL; speed : REAL; END_VAR
VAR_OUTPUT lamp : BOOL; END_VAR
VAR n : UDINT; END_VAR
  n := n + 1;
  lamp := start AND speed > 0.25;
END_PROGRAM";

/// Drives the horn too: output-affecting.
const SOURCE_HORN: &str = "
PROGRAM conveyor
VAR_INPUT start : BOOL; speed : REAL; END_VAR
VAR_OUTPUT lamp : BOOL; horn : BOOL; END_VAR
VAR n : UDINT; END_VAR
  n := n + 1;
  lamp := start AND speed > 1.0;
  horn := start;
END_PROGRAM";

const MAP: &str = "\
in  start bool box di:0
out lamp  bool box do:0 safe=false
out horn  bool box do:1 safe=false
";

fn defaults() -> Defaults {
    Defaults {
        period: Duration::from_millis(2),
        i: Window::new(0, 0),
        q: Window::new(0, 0),
        m: 16,
    }
}

fn running() -> ChangeSet {
    ChangeSet {
        modules: BTreeMap::from([(
            "conveyor".to_string(),
            ModuleFiles {
                manifest: MANIFEST.into(),
                code: SOURCE.into(),
            },
        )]),
        io_map: Some(MAP.into()),
        calendar: None,
        drivers: BTreeMap::new(),
    }
}

fn tmp(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "approval-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn pins() -> Pins {
    let mut p = Pins::default();
    // One iteration keeps the tests fast; the binary refuses fewer than
    // pin::MIN_ITERATIONS in its file (tests/policy.rs).
    p.set(Role::Operator, OPERATOR, 1, [1; 16]).unwrap();
    p.set(Role::Engineer, ENGINEER, 1, [2; 16]).unwrap();
    p.set(Role::Integrator, INTEGRATOR, 1, [3; 16]).unwrap();
    p
}

struct Setup {
    policy: Policy,
    store: bool,
    swap_wait: Duration,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            policy: FLOOR,
            store: true,
            swap_wait: Duration::from_secs(5),
        }
    }
}

/// The box: a runtime scanning on its own thread with a recorder and the
/// swap channel served between ticks, the API over it, and a store whose
/// head is [`running`].
struct Bench {
    api: Option<Api>,
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    /// While set, the scan thread does nothing (a stalled scheduler).
    pause: Arc<AtomicBool>,
    scan: Option<JoinHandle<()>>,
    table: Arc<Mutex<SignalTable>>,
    store: Option<PathBuf>,
    /// Seconds added to the PIN gate's clock.
    clock: Arc<AtomicU64>,
    logs: Arc<Mutex<Vec<String>>>,
}

impl Bench {
    fn new() -> Self {
        Self::with(Setup::default())
    }

    #[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
    fn with(setup: Setup) -> Self {
        let d = defaults();
        let md = ModuleDir::from_text(Path::new("conveyor"), MANIFEST, SOURCE.into(), &d).unwrap();
        let sizes = ImageSizes {
            i: md.spec.i.len,
            q: md.spec.q.len,
            m: md.spec.m,
        };
        let module = md.module(compile(SOURCE, sizes).unwrap()).unwrap();
        let parsed = helmsman_io::text::parse(MAP).unwrap();
        let table = SignalTable::new(&parsed.map, &parsed.safe).unwrap();
        let mut rt = Runtime::with_signals(32, 32, vec![module], &table).unwrap();
        let start = table.index("start").unwrap();
        let modules = vec![
            ModuleInfo::of(
                &rt,
                "conveyor",
                "conveyor".into(),
                SOURCE.into(),
                Some(md.manifest.clone()),
            )
            .unwrap(),
        ];
        let catalog = Catalog {
            signals: Catalog::signals(&table, Some(&parsed.map), &modules),
            modules,
        };
        let store = setup.store.then(|| {
            let dir = tmp("store");
            Store::open(&dir)
                .unwrap()
                .commit(
                    &running(),
                    &Meta {
                        author: "Test".into(),
                        prompt: String::new(),
                        approver: None,
                        time_ms: 1,
                    },
                )
                .unwrap();
            dir
        });
        let mut recorder = Recorder::new(&table, RecordConfig::default(), None);
        let (handle, swapper) = swap::channel(setup.swap_wait);
        let t0 = Instant::now();
        let clock = Arc::new(AtomicU64::new(0));
        let c = Arc::clone(&clock);
        let approval = Approval {
            policy: setup.policy,
            gate: Gate::new(
                pins(),
                setup.policy.lockout,
                Arc::new(move || t0 + Duration::from_secs(c.load(Ordering::SeqCst))),
            ),
            plant: Plant {
                table: table.clone(),
                i_len: 32,
                q_len: 32,
                defaults: d,
                record: Some(recorder.tap()),
            },
            swap: handle,
        };
        let table = Arc::new(Mutex::new(table));
        let live = Arc::new(Live::default());
        let logs = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&logs);
        let cfg = Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            token: TOKEN.into(),
            limits: Limits {
                deadline: Duration::from_secs(5),
                ..Limits::default()
            },
            stream_every: Duration::from_millis(20),
            memory: 16,
            log: Arc::new(move |l: &str| sink.lock().unwrap_or_else(PoisonError::into_inner).push(l.into())),
        };
        let shared = Shared {
            catalog,
            live: Arc::clone(&live),
            devices: Arc::new(Devices::new(["box".to_string()])),
            audit: Box::new(MemAudit::new(10_000)),
            proposals: Box::new(MemProposals::new(1000)),
            running: Some(running()),
            snapshots: store.as_ref().map(|d| Mutex::new(Store::open(d).unwrap())),
            approval: Some(approval),
            chat: None,
        };
        let api = Api::start(cfg, shared).unwrap();
        let addr = api.addr();
        let stop = Arc::new(AtomicBool::new(false));
        let pause = Arc::new(AtomicBool::new(false));
        let ticks = Arc::new(AtomicU64::new(0));
        let scan = {
            let (stop, pause, table, ticks) = (
                Arc::clone(&stop),
                Arc::clone(&pause),
                Arc::clone(&table),
                Arc::clone(&ticks),
            );
            let mut publisher = Publisher::new(live, Duration::from_millis(5));
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    if !pause.load(Ordering::Relaxed) {
                        let mut t = table.lock().unwrap_or_else(PoisonError::into_inner);
                        t.set_input(start, Some(Value::Bool(true)));
                        swapper.serve(&mut rt, &mut t);
                        let now = t0.elapsed();
                        recorder.observe(now, &t);
                        rt.tick_with(now, &[], &mut t);
                        publisher.publish(now, &rt, &t);
                        ticks.fetch_add(1, Ordering::Relaxed);
                    }
                    thread::sleep(Duration::from_millis(1));
                }
            })
        };
        let b = Self {
            api: Some(api),
            addr,
            stop,
            pause,
            scan: Some(scan),
            table,
            store,
            clock,
            logs,
        };
        wait("a recorded window", || ticks.load(Ordering::Relaxed) > 50);
        wait("the lamp on", || b.cmd("lamp") == Some(Value::Bool(true)));
        b
    }

    fn cmd(&self, signal: &str) -> Option<Value> {
        let t = self.table.lock().unwrap_or_else(PoisonError::into_inner);
        t.command(t.index(signal).unwrap())
    }

    fn head(&self) -> Option<String> {
        let s = Store::open(self.store.as_ref()?).unwrap();
        s.head().unwrap().map(|e| e.id.to_string())
    }

    fn entries(&self) -> usize {
        self.store
            .as_ref()
            .map_or(0, |d| Store::open(d).unwrap().list().unwrap().len())
    }

    fn logs(&self) -> Vec<String> {
        self.logs.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn propose(&self, body: &Json) -> (u64, Json) {
        let r = post(self.addr, "/api/proposals", body);
        assert_eq!(r.status, 201, "{r:?}");
        let j = r.json();
        (j["id"].as_u64().unwrap(), j)
    }

    fn param(&self, value: f64) -> u64 {
        self.propose(&json!({ "kind": "param", "module": "conveyor", "param": "speed", "value": value }))
            .0
    }

    fn module(&self, manifest: &str, source: &str) -> (u64, Json) {
        self.propose(&json!({ "kind": "module", "manifest": manifest, "source": source }))
    }

    fn replay(&self, path: &str) -> u64 {
        let r = post(self.addr, &format!("{path}/replay"), &json!({}));
        assert_eq!(r.status, 201, "{r:?}");
        r.json()["replay"].as_u64().unwrap()
    }

    fn apply(&self, id: u64, role: &str, pin: &str, replay: Option<u64>) -> Resp {
        post(
            self.addr,
            &format!("/api/proposals/{id}/apply"),
            &approve_body(role, pin, replay),
        )
    }

    fn speed(&self) -> Json {
        get(self.addr, "/api/modules/conveyor").json()["manifest"]["params"][0]["value"].clone()
    }
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

fn approve_body(role: &str, pin: &str, replay: Option<u64>) -> Json {
    match replay {
        Some(n) => json!({ "role": role, "pin": pin, "replay": n }),
        None => json!({ "role": role, "pin": pin }),
    }
}

fn wait(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if ok() {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out waiting for {what}");
}

#[derive(Debug)]
struct Resp {
    status: u16,
    head: String,
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

fn request(addr: SocketAddr, method: &str, path: &str, auth: bool, body: &str) -> Resp {
    let auth = if auth {
        format!("Authorization: Bearer {TOKEN}\r\n")
    } else {
        String::new()
    };
    let raw = format!(
        "{method} {path} HTTP/1.1\r\nHost: box\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    s.write_all(raw.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let text = String::from_utf8_lossy(&out).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    Resp {
        status: head.get(9..12).and_then(|s| s.parse().ok()).unwrap_or(0),
        head: head.into(),
        body: body.into(),
    }
}

fn get(addr: SocketAddr, path: &str) -> Resp {
    request(addr, "GET", path, false, "")
}

fn post(addr: SocketAddr, path: &str, body: &Json) -> Resp {
    request(addr, "POST", path, true, &body.to_string())
}

fn audit(addr: SocketAddr) -> Vec<Json> {
    get(addr, "/api/audit?limit=1000")
        .json()
        .as_array()
        .cloned()
        .unwrap()
}

/// Every apply, rollback and replay in the audit log says when, which
/// snapshots, and (past parsing the body) which role; no entry, log line
/// or response anywhere carries a PIN.
fn assert_audit_complete(b: &Bench) {
    let log = audit(b.addr);
    let mut n = 0;
    for e in &log {
        let path = e["path"].as_str().unwrap();
        let acting = path.ends_with("/apply") || path.ends_with("/rollback");
        if !(acting || path.ends_with("/replay")) {
            continue;
        }
        n += 1;
        assert!(e["at"].as_u64().unwrap() > 0, "{e}");
        let status = e["status"].as_u64().unwrap();
        if status != 404 {
            assert!(e["snapshot"].is_string(), "names the target: {e}");
        }
        if acting && ![400, 404, 401].contains(&status) {
            assert!(e["role"].is_string(), "names the role: {e}");
            assert!(e["base"].is_string(), "names what ran: {e}");
        }
    }
    assert!(n > 0, "nothing to check: {log:?}");
    let all = format!("{log:?}{:?}", b.logs());
    for pin in [OPERATOR, ENGINEER, INTEGRATOR, "guess-"] {
        assert!(!all.contains(pin), "{pin} leaked: {all}");
    }
}

// ---- param: live, with undo ------------------------------------------------

#[test]
fn a_param_change_applies_live_with_an_operator_pin_and_undoes_in_one_call() {
    let b = Bench::new();
    let before = b.head().unwrap();
    let id = b.param(0.5);
    let draft = get(b.addr, &format!("/api/proposals/{id}")).json();
    assert_eq!(draft["approval"]["tier"], "param", "{draft}");
    assert_eq!(
        draft["approval"]["needs"],
        json!({ "role": "operator", "replay": false })
    );

    let r = b.apply(id, "operator", OPERATOR, None);
    assert_eq!(r.status, 200, "{r:?}");
    let j = r.json();
    assert_eq!(j["tier"], "param");
    assert_eq!(j["previous"], json!(before));
    assert_eq!(b.head().unwrap(), j["snapshot"].as_str().unwrap(), "committed");
    assert_eq!(b.speed(), 0.5, "the API describes what runs now");
    wait("the param to reach the scan", || {
        b.cmd("lamp") == Some(Value::Bool(false))
    });
    assert_eq!(
        get(b.addr, &format!("/api/proposals/{id}")).json()["status"],
        "applied"
    );
    let entry = &Store::open(b.store.as_ref().unwrap()).unwrap().list().unwrap()[0];
    assert_eq!(entry.meta.approver.as_deref(), Some("operator (PIN)"));
    assert_eq!(entry.meta.author, "box API (param change)");

    // Applying the same draft again: it is stale now.
    assert_eq!(b.apply(id, "operator", OPERATOR, None).status, 409);

    // Undo is one call, through the same policy: a param step back.
    let undo = j["undo"].as_str().unwrap();
    let u = post(b.addr, undo, &approve_body("operator", OPERATOR, None));
    assert_eq!(u.status, 200, "{u:?}");
    assert_eq!(u.json()["tier"], "param");
    assert_eq!(b.head().unwrap(), before);
    assert_eq!(b.speed(), 1.5);
    wait("the lamp back on", || b.cmd("lamp") == Some(Value::Bool(true)));
    assert_eq!(b.entries(), 3, "initial, apply, rollback");
    assert_audit_complete(&b);
}

// ---- roles ------------------------------------------------------------------

#[test]
fn the_operator_role_can_never_apply_logic_or_output_affecting_and_spends_no_pin_trying() {
    let b = Bench::new();
    let (logic, d1) = b.module(MANIFEST, SOURCE_V2);
    assert_eq!(d1["approval"]["tier"], "logic", "{d1}");
    let horn = MANIFEST.replace("drives   lamp", "drives   lamp horn");
    let (output, d2) = b.module(&horn, SOURCE_HORN);
    assert_eq!(d2["approval"]["tier"], "output-affecting", "{d2}");
    let r1 = b.replay(&format!("/api/proposals/{logic}"));
    let r2 = b.replay(&format!("/api/proposals/{output}"));
    let head = b.head();
    for _ in 0..8 {
        for (id, rep) in [(logic, r1), (output, r2)] {
            for pin in [OPERATOR, "guess-0000"] {
                let r = b.apply(id, "operator", pin, Some(rep));
                assert_eq!(r.status, 403, "{r:?}");
                assert!(r.error().contains("needs the engineer role"), "{r:?}");
            }
        }
    }
    assert_eq!(b.head(), head, "nothing applied");
    assert_eq!(b.cmd("horn"), Some(Value::Bool(false)));
    // 32 refusals, 16 of them with a wrong PIN, and entry is not locked:
    // the role was refused before any PIN was checked.
    let r = b.apply(logic, "engineer", ENGINEER, Some(r1));
    assert_eq!(r.status, 200, "{r:?}");
    assert_audit_complete(&b);
}

#[test]
fn a_stricter_policy_is_what_the_box_enforces() {
    let policy = Policy::parse(
        "helmsman-policy 1\nparam engineer replay=no\noutput-affecting integrator replay=yes\n",
    )
    .unwrap();
    let b = Bench::with(Setup {
        policy,
        ..Setup::default()
    });
    assert_eq!(get(b.addr, "/api/policy").json()["param"]["role"], "engineer");
    let id = b.param(2.0);
    let r = b.apply(id, "operator", OPERATOR, None);
    assert_eq!(r.status, 403, "{r:?}");
    let horn = MANIFEST.replace("drives   lamp", "drives   lamp horn");
    let (out, _) = b.module(&horn, SOURCE_HORN);
    let rep = b.replay(&format!("/api/proposals/{out}"));
    assert_eq!(b.apply(out, "engineer", ENGINEER, Some(rep)).status, 403);
    let r = b.apply(out, "integrator", INTEGRATOR, Some(rep));
    assert_eq!(r.status, 200, "{r:?}");
    wait("the horn driven", || b.cmd("horn") == Some(Value::Bool(true)));
    assert_eq!(
        b.apply(id, "engineer", ENGINEER, None).status,
        409,
        "drafted against the old head: stale"
    );
    assert_audit_complete(&b);
}

// ---- logic: preview first --------------------------------------------------

#[test]
fn a_logic_change_needs_a_replay_of_exactly_itself_then_an_engineer_pin() {
    let b = Bench::new();
    let (id, _) = b.module(MANIFEST, SOURCE_V2);
    let r = b.apply(id, "engineer", ENGINEER, None);
    assert_eq!(r.status, 428, "{r:?}");
    assert_eq!(r.json()["tier"], "logic");
    assert_eq!(b.apply(id, "engineer", ENGINEER, Some(9999)).status, 428);
    let rep = b.replay(&format!("/api/proposals/{id}"));
    let ev = get(b.addr, &format!("/api/replays/{rep}")).json();
    assert!(ev["window_ms"].as_u64().unwrap() > 0, "{ev}");
    assert_eq!(ev["signals"][0]["signal"], "lamp", "{ev}");
    let r = b.apply(id, "engineer", ENGINEER, Some(rep));
    assert_eq!(r.status, 200, "{r:?}");
    let m = get(b.addr, "/api/modules/conveyor").json();
    assert_eq!(m["source"], SOURCE_V2, "the running program is the new one");
    assert_eq!(m["stats"]["scans"].as_u64().map(|n| n > 0), Some(true), "{m}");
    assert_eq!(b.cmd("lamp"), Some(Value::Bool(true)));
    assert_audit_complete(&b);
}

// ---- output-affecting ------------------------------------------------------

#[test]
fn an_output_affecting_change_disguised_as_a_param_change_is_caught() {
    let b = Bench::new();
    // "Only changing speed": but the manifest also adds a drives line.
    let sneaky = MANIFEST
        .replace("drives   lamp", "drives   lamp horn")
        .replace("real 1.5", "real 2.5");
    let (id, d) = b.module(&sneaky, SOURCE_HORN);
    assert_eq!(d["approval"]["tier"], "output-affecting", "{d}");
    let r = b.apply(id, "operator", OPERATOR, None);
    assert_eq!(r.status, 403, "{r:?}");
    assert_eq!(r.json()["tier"], "output-affecting");
    assert_eq!(b.apply(id, "engineer", ENGINEER, None).status, 428, "no replay");
    // A replay of the harmless param change does not cover it.
    let p = b.param(2.5);
    let harmless = b.replay(&format!("/api/proposals/{p}"));
    let r = b.apply(id, "engineer", ENGINEER, Some(harmless));
    assert_eq!(r.status, 428, "{r:?}");
    assert_eq!(b.cmd("horn"), Some(Value::Bool(false)), "never moved");
    let own = b.replay(&format!("/api/proposals/{id}"));
    let ev = get(b.addr, &format!("/api/replays/{own}")).json();
    let horn = ev["signals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["signal"] == "horn")
        .unwrap_or_else(|| panic!("the replay shows the horn: {ev}"));
    assert_eq!(horn["identical"], false, "{horn}");
    let r = b.apply(id, "engineer", ENGINEER, Some(own));
    assert_eq!(r.status, 200, "{r:?}");
    wait("the horn on", || b.cmd("horn") == Some(Value::Bool(true)));
    assert_audit_complete(&b);
}

#[test]
fn a_replay_of_other_content_or_of_an_older_box_is_refused() {
    let b = Bench::new();
    let (pa, _) = b.module(MANIFEST, SOURCE_V2);
    let other = SOURCE_V2.replace("0.25", "0.75");
    let (pc, _) = b.module(MANIFEST, &other);
    let ra = b.replay(&format!("/api/proposals/{pa}"));
    // a's evidence cited for c: different content hash.
    let r = b.apply(pc, "engineer", ENGINEER, Some(ra));
    assert_eq!(r.status, 428, "{r:?}");
    assert!(r.error().contains("different content"), "{r:?}");
    // Rollback evidence made now, then the box changes under it.
    let first = b.head().unwrap();
    let r = b.apply(pa, "engineer", ENGINEER, Some(ra));
    assert_eq!(r.status, 200, "{r:?}");
    let v2 = b.head().unwrap();
    let back = b.replay(&format!("/api/snapshots/{first}"));
    let p = b.param(3.0);
    assert_eq!(b.apply(p, "operator", OPERATOR, None).status, 200);
    // Same target content, but the evidence was made against v2.
    let r = post(
        b.addr,
        &format!("/api/snapshots/{first}/rollback"),
        &approve_body("engineer", ENGINEER, Some(back)),
    );
    assert_eq!(r.status, 428, "{r:?}");
    assert_ne!(b.head().unwrap(), first);
    assert_ne!(b.head().unwrap(), v2);
    // c was drafted against the first head: stale whatever it cites.
    let rc = b.replay(&format!("/api/proposals/{pc}"));
    assert_eq!(b.apply(pc, "engineer", ENGINEER, Some(rc)).status, 409);
    assert_audit_complete(&b);
}

// ---- rollback: same policy -------------------------------------------------

#[test]
fn rollback_goes_through_the_same_policy() {
    let b = Bench::new();
    let first = b.head().unwrap();
    let (id, _) = b.module(MANIFEST, SOURCE_V2);
    let rep = b.replay(&format!("/api/proposals/{id}"));
    assert_eq!(b.apply(id, "engineer", ENGINEER, Some(rep)).status, 200);
    let path = format!("/api/snapshots/{first}/rollback");
    let r = post(b.addr, &path, &approve_body("operator", OPERATOR, None));
    assert_eq!(r.status, 403, "rolling logic back is a logic change: {r:?}");
    assert_eq!(r.json()["tier"], "logic");
    let r = post(b.addr, &path, &approve_body("engineer", ENGINEER, None));
    assert_eq!(r.status, 428, "{r:?}");
    let back = b.replay(&format!("/api/snapshots/{first}"));
    let r = post(b.addr, &path, &approve_body("engineer", ENGINEER, Some(back)));
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(b.head().unwrap(), first);
    assert_eq!(get(b.addr, "/api/modules/conveyor").json()["source"], SOURCE);
    let entry = &Store::open(b.store.as_ref().unwrap()).unwrap().list().unwrap()[0];
    assert_eq!(entry.meta.approver.as_deref(), Some("engineer (PIN)"));
    assert!(matches!(entry.kind, helmsman_plc::store::Kind::Rollback { .. }));
    assert_audit_complete(&b);
}

// ---- PINs ------------------------------------------------------------------

#[test]
fn pin_brute_force_over_the_api_locks_out() {
    let b = Bench::new();
    let id = b.param(2.0);
    for n in 0..5 {
        let r = b.apply(id, "operator", &format!("guess-{n:04}"), None);
        assert_eq!(r.status, 403, "{r:?}");
        assert_eq!(r.error(), "wrong PIN");
    }
    // The right PIN, any role: not even checked while locked.
    for (role, pin) in [("operator", OPERATOR), ("integrator", INTEGRATOR)] {
        let r = b.apply(id, role, pin, None);
        assert_eq!(r.status, 429, "{r:?}");
        assert!(r.head.contains("Retry-After: 30"), "{r:?}");
    }
    // Hammering from many connections changes nothing.
    let hammer: Vec<_> = (0..12)
        .map(|k| {
            let addr = b.addr;
            thread::spawn(move || {
                post(
                    addr,
                    &format!("/api/proposals/{id}/apply"),
                    &approve_body("engineer", &format!("guess-x{k:03}"), None),
                )
                .status
            })
        })
        .collect();
    for h in hammer {
        assert_eq!(h.join().unwrap(), 429);
    }
    assert_eq!(b.speed(), 1.5, "nothing applied");
    b.clock.store(31, Ordering::SeqCst);
    let r = b.apply(id, "operator", OPERATOR, None);
    assert_eq!(r.status, 200, "{r:?}");
    assert_audit_complete(&b);
    let locked = audit(b.addr).iter().filter(|e| e["status"] == 429).count();
    assert_eq!(locked, 14, "every locked attempt is audited");
}

#[test]
fn malformed_approvals_are_refused_and_never_echo_the_pin() {
    let b = Bench::new();
    let id = b.param(2.0);
    let path = format!("/api/proposals/{id}/apply");
    for body in [
        String::new(),
        "{}".into(),
        json!({ "role": "operator" }).to_string(),
        json!({ "role": "operator", "pin": 246_810 }).to_string(),
        json!({ "role": "operator", "pin": OPERATOR, "extra": 1 }).to_string(),
        json!({ "role": "root", "pin": OPERATOR }).to_string(),
        json!({ "role": "Operator", "pin": OPERATOR }).to_string(),
        format!("{{\"role\":\"operator\",\"pin\":\"{OPERATOR}\""),
    ] {
        let r = request(b.addr, "POST", &path, true, &body);
        assert_eq!(r.status, 400, "{body}: {r:?}");
        assert!(!r.body.contains(OPERATOR) && !r.body.contains("246810"), "{r:?}");
    }
    // No token: refused before anything, PIN or not.
    let r = request(
        b.addr,
        "POST",
        &path,
        false,
        &approve_body("operator", OPERATOR, None).to_string(),
    );
    assert_eq!(r.status, 401);
    // Unknown targets.
    for p in [
        "/api/proposals/999/apply".to_string(),
        "/api/proposals/..%2f1/apply".into(),
        format!("/api/snapshots/{}/rollback", "0".repeat(64)),
    ] {
        assert_eq!(
            post(b.addr, &p, &approve_body("operator", OPERATOR, None)).status,
            404,
            "{p}"
        );
    }
    assert_eq!(b.speed(), 1.5);
    assert_audit_complete(&b);
}

// ---- races and failures ----------------------------------------------------

#[test]
fn concurrent_conflicting_applies_one_wins_and_nothing_is_half_applied() {
    let b = Bench::new();
    for round in 0..4 {
        let before = b.entries();
        let ids: Vec<u64> = (0..6)
            .map(|k| b.param(2.0 + f64::from(k) + f64::from(round) / 10.0))
            .collect();
        let go = Arc::new(std::sync::Barrier::new(ids.len()));
        let handles: Vec<_> = ids
            .iter()
            .map(|&id| {
                let (addr, go) = (b.addr, Arc::clone(&go));
                thread::spawn(move || {
                    go.wait();
                    let r = post(
                        addr,
                        &format!("/api/proposals/{id}/apply"),
                        &approve_body("operator", OPERATOR, None),
                    );
                    (id, r.status, r.body)
                })
            })
            .collect();
        let results: Vec<(u64, u16, String)> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let won: Vec<_> = results.iter().filter(|r| r.1 == 200).collect();
        assert_eq!(won.len(), 1, "{results:?}");
        assert!(results.iter().all(|r| r.1 == 200 || r.1 == 409), "{results:?}");
        assert_eq!(b.entries(), before + 1, "exactly one commit");
        let winner = get(b.addr, &format!("/api/proposals/{}", won[0].0)).json();
        assert_eq!(b.speed(), winner["to"], "what runs is the winner, whole");
        assert_eq!(
            get(b.addr, "/api/snapshots").json()["running"],
            json!(b.head().unwrap())
        );
    }
    assert_audit_complete(&b);
}

#[test]
fn a_swap_the_scan_thread_never_takes_changes_nothing_and_is_recorded() {
    let b = Bench::with(Setup {
        swap_wait: Duration::from_millis(300),
        ..Setup::default()
    });
    let head = b.head();
    let id = b.param(0.5);
    b.pause.store(true, Ordering::SeqCst);
    let r = b.apply(id, "operator", OPERATOR, None);
    assert_eq!(r.status, 409, "{r:?}");
    assert_eq!(r.json()["changed"], false);
    b.pause.store(false, Ordering::SeqCst);
    thread::sleep(Duration::from_millis(100));
    assert_eq!(b.head(), head, "nothing committed");
    assert_eq!(b.speed(), 1.5);
    assert_eq!(
        b.cmd("lamp"),
        Some(Value::Bool(true)),
        "the withdrawn job never landed"
    );
    let log = audit(b.addr);
    assert!(
        log.iter()
            .any(|e| e["status"] == 409 && e["detail"].as_str().unwrap().contains("did not take it")),
        "{log:?}"
    );
    // And the same proposal applies once the scan thread is back.
    assert_eq!(b.apply(id, "operator", OPERATOR, None).status, 200);
    assert_audit_complete(&b);
}

#[test]
fn a_change_that_does_not_fit_the_running_box_is_refused_whole() {
    let b = Bench::new();
    // Compiles and checks as a draft, but its %Q window is past the image.
    let wide = MANIFEST.replace("drives   lamp", "drives   lamp\nq        40+1");
    let (id, d) = b.module(&wide, SOURCE);
    assert_eq!(d["approval"]["tier"], "output-affecting");
    let r = post(b.addr, &format!("/api/proposals/{id}/replay"), &json!({}));
    assert_eq!(r.status, 422, "the replay refuses it as the runtime would: {r:?}");
    assert_eq!(b.apply(id, "engineer", ENGINEER, None).status, 428);
    assert_eq!(b.entries(), 1);
    assert_audit_complete(&b);
}

#[test]
fn without_a_store_nothing_is_applied() {
    let b = Bench::with(Setup {
        store: false,
        ..Setup::default()
    });
    let id = b.param(0.5);
    let r = b.apply(id, "operator", OPERATOR, None);
    assert_eq!(r.status, 409, "{r:?}");
    assert!(r.error().contains("no store"), "{r:?}");
    assert_eq!(b.speed(), 1.5);
}

// ---- WASM modules ---------------------------------------------------------------

/// `conveyor` compiled to WASM: the lamp lights at a lower speed than
/// [`SOURCE`] (a logic change), and with `horn` it drives the horn too.
/// A trailing custom section of every byte value: no UTF-8 anywhere.
fn conveyor_wasm(horn: bool) -> Vec<u8> {
    let (import, call) = if horn {
        (
            r#"(import "helmsman/v1/out" "horn" (func $horn (param i32)))"#,
            "(call $horn (call $start))",
        )
    } else {
        ("", "")
    };
    let mut b = wat::parse_str(format!(
        r#"(module
             (import "helmsman/v1/in" "start" (func $start (result i32)))
             (import "helmsman/v1/in" "speed" (func $speed (result f64)))
             (import "helmsman/v1/out" "lamp" (func $lamp (param i32)))
             {import}
             (func (export "scan")
               (call $lamp (i32.and (call $start) (f64.gt (call $speed) (f64.const 0.25))))
               {call}))"#
    ))
    .unwrap();
    b.extend_from_slice(&[0, 0x82, 0x02, 1, b'x']);
    b.extend(0..=255u8);
    b
}

fn b64(b: &[u8]) -> String {
    helmsman_plc::api::photo::base64(b)
}

fn stored(b: &Bench, id: &str) -> ChangeSet {
    Store::open(b.store.as_ref().unwrap())
        .unwrap()
        .get(&helmsman_plc::store::SnapshotId::parse(id).unwrap())
        .unwrap()
}

#[test]
fn a_wasm_module_is_proposed_approved_rolled_back_and_restored_byte_identical() {
    let b = Bench::new();
    let first = b.head().unwrap();
    let wasm = conveyor_wasm(false);
    let (id, d) = b.propose(&json!({ "kind": "module", "manifest": MANIFEST, "wasm": b64(&wasm) }));
    // The API shows what the binary imports, never a source.
    assert_eq!(d["source"], Json::Null, "{d}");
    assert_eq!(d["wasm"]["bytes"], wasm.len(), "{d}");
    assert_eq!(d["wasm"]["abi"], "helmsman/v1", "{d}");
    let imports: Vec<String> = d["wasm"]["imports"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            format!(
                "{} {}",
                i["module"].as_str().unwrap(),
                i["name"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(
        imports,
        [
            "helmsman/v1/in start",
            "helmsman/v1/in speed",
            "helmsman/v1/out lamp"
        ],
        "{d}"
    );
    assert_eq!(d["diff"]["modules"][0]["source"], true, "{d}");
    // The same policy as an ST program change: logic, a replay first.
    assert_eq!(d["approval"]["tier"], "logic", "{d}");
    assert_eq!(b.apply(id, "operator", OPERATOR, None).status, 403);
    assert_eq!(b.apply(id, "engineer", ENGINEER, None).status, 428);
    let rep = b.replay(&format!("/api/proposals/{id}"));
    let r = b.apply(id, "engineer", ENGINEER, Some(rep));
    assert_eq!(r.status, 200, "{r:?}");
    let applied = b.head().unwrap();
    assert_eq!(r.json()["snapshot"], json!(applied));
    assert_eq!(
        stored(&b, &applied).modules["conveyor"].code.wasm(),
        Some(wasm.as_slice())
    );
    let module = get(b.addr, "/api/modules/conveyor").json();
    let note = module["source"].as_str().unwrap();
    assert!(note.starts_with("(* WASM module (helmsman/v1)"), "{module}");
    assert_eq!(module["ports"].as_array().unwrap().len(), 3, "{module}");
    assert_eq!(
        module["stats"]["scans"].as_u64().map(|n| n > 0),
        Some(true),
        "{module}"
    );
    wait("the WASM module lighting the lamp", || {
        b.cmd("lamp") == Some(Value::Bool(true))
    });
    // Its snapshot as JSON: imports, not bytes.
    let snap = get(b.addr, &format!("/api/snapshots/{applied}")).json();
    assert_eq!(snap["modules"]["conveyor"]["source"], Json::Null, "{snap}");
    assert_eq!(
        snap["modules"]["conveyor"]["wasm"]["imports"][2]["type"],
        "(i32) -> ()"
    );

    // Back to ST, through the same policy.
    let path = format!("/api/snapshots/{first}/rollback");
    assert_eq!(
        post(b.addr, &path, &approve_body("operator", OPERATOR, None)).status,
        403
    );
    let back = b.replay(&format!("/api/snapshots/{first}"));
    let r = post(b.addr, &path, &approve_body("engineer", ENGINEER, Some(back)));
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(b.head().unwrap(), first);
    assert_eq!(get(b.addr, "/api/modules/conveyor").json()["source"], SOURCE);

    // And forward to the WASM snapshot again: the same bytes.
    let path = format!("/api/snapshots/{applied}/rollback");
    let again = b.replay(&format!("/api/snapshots/{applied}"));
    let r = post(b.addr, &path, &approve_body("engineer", ENGINEER, Some(again)));
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(b.head().unwrap(), applied);
    assert_eq!(
        stored(&b, &applied).modules["conveyor"].code.wasm(),
        Some(wasm.as_slice())
    );
    assert!(
        get(b.addr, "/api/modules/conveyor").json()["source"]
            .as_str()
            .unwrap()
            .starts_with("(* WASM module")
    );
    // A param proposal on a WASM module keeps its binary.
    let param = b.param(0.5);
    let r = b.apply(param, "operator", OPERATOR, None);
    assert_eq!(r.status, 200, "{r:?}");
    let now = stored(&b, r.json()["snapshot"].as_str().unwrap());
    assert_eq!(now.modules["conveyor"].code.wasm(), Some(wasm.as_slice()));
    assert_eq!(b.entries(), 5, "initial, apply, rollback, rollback, param");
    assert_audit_complete(&b);
}

#[test]
fn a_wasm_module_cannot_understate_what_it_drives() {
    let b = Bench::new();
    let head = b.head();
    let refused = |body: Json| {
        let r = post(b.addr, "/api/proposals", &body);
        assert_eq!(r.status, 422, "{r:?}");
        r.json()
    };
    // The binary drives the horn; the manifest says only the lamp.
    let horn = b64(&conveyor_wasm(true));
    let j = refused(json!({ "kind": "module", "manifest": MANIFEST, "wasm": horn }));
    assert_eq!(j["error"], "manifest does not match the program", "{j}");
    assert!(j["problems"].to_string().to_lowercase().contains("horn"), "{j}");
    // Declared, it is output-affecting.
    let declared = MANIFEST.replace("drives   lamp", "drives   lamp horn");
    let (_, d) = b.propose(&json!({ "kind": "module", "manifest": declared, "wasm": horn }));
    assert_eq!(d["approval"]["tier"], "output-affecting", "{d}");
    // A watch module drives nothing, whatever it is written in.
    let j = refused(json!({ "kind": "module", "manifest": declared, "wasm": horn, "watch": true }));
    assert!(j["error"].as_str().unwrap().contains("watch"), "{j}");
    // Windows would hide a %Q a WASM module cannot have.
    let windowed = format!("{MANIFEST}q 0+1\n");
    let j = refused(json!({ "kind": "module", "manifest": windowed, "wasm": b64(&conveyor_wasm(false)) }));
    assert!(j["problems"].to_string().contains("no i, q or m windows"), "{j}");
    // Not WASM, or WASM that reaches past the interface.
    let j = refused(
        json!({ "kind": "module", "manifest": MANIFEST, "wasm": b64(b"PROGRAM conveyor END_PROGRAM") }),
    );
    assert_eq!(j["error"], "wasm does not load", "{j}");
    let wasi = wat::parse_str(
        r#"(module (import "wasi_snapshot_preview1" "fd_write" (func (param i32 i32 i32 i32) (result i32)))
                   (func (export "scan")))"#,
    )
    .unwrap();
    let j = refused(json!({ "kind": "module", "manifest": MANIFEST, "wasm": b64(&wasi) }));
    assert!(
        j["problems"]
            .to_string()
            .contains("not part of the helmsman/v1 interface"),
        "{j}"
    );
    assert_eq!(b.head(), head, "nothing applied");
}

#[test]
fn malformed_wasm_proposals_are_refused() {
    let b = Bench::new();
    let wasm = b64(&conveyor_wasm(false));
    let bad = |body: Json, status: u16| {
        let r = post(b.addr, "/api/proposals", &body);
        assert_eq!(r.status, status, "{body}: {r:?}");
        r.json()
    };
    let j = bad(
        json!({ "kind": "module", "manifest": MANIFEST, "source": SOURCE, "wasm": wasm }),
        400,
    );
    assert!(j["error"].as_str().unwrap().contains("not both"), "{j}");
    bad(json!({ "kind": "module", "manifest": MANIFEST }), 400);
    bad(json!({ "kind": "module", "wasm": wasm }), 400);
    // One binary, one spelling: unpadded, URL-safe, wrapped or
    // whitespace-padded base64 is refused, as is anything else.
    for spelled in [
        wasm.trim_end_matches('=').to_string(),
        wasm.replace('+', "-").replace('/', "_"),
        format!("{}\n{}", &wasm[..8], &wasm[8..]),
        format!(" {wasm}"),
        format!("{wasm}===="),
        "!!!!".to_string(),
    ] {
        if spelled == wasm {
            continue;
        }
        let j = bad(
            json!({ "kind": "module", "manifest": MANIFEST, "wasm": spelled }),
            400,
        );
        assert!(j["error"].as_str().unwrap().contains("base64"), "{j}");
    }
    bad(
        json!({ "kind": "param", "module": "conveyor", "param": "speed", "value": 2.0, "wasm": wasm }),
        400,
    );
    bad(json!({ "kind": "io_map", "text": MAP, "wasm": wasm }), 400);
    assert_eq!(get(b.addr, "/api/proposals").json(), json!([]), "nothing stored");
}

#[test]
fn a_wasm_proposal_over_the_file_cap_is_refused_whatever_the_body_limit() {
    // Over HTTP the default body limit (1 MiB) refuses it first; the
    // proposal check holds the cap on its own, for any limit.
    let cap = usize::try_from(helmsman_plc::manifest::MAX_FILE).unwrap();
    let check = |len: usize| {
        let new: helmsman_plc::api::proposal::New = serde_json::from_value(
            json!({ "kind": "module", "manifest": MANIFEST, "wasm": b64(&vec![0; len]) }),
        )
        .unwrap();
        helmsman_plc::api::proposal::check(&new, &Catalog::default(), 16, Some(&running()))
    };
    let (status, j) = check(cap + 1).unwrap_err();
    assert_eq!(status, 422);
    assert_eq!(
        j["error"], "manifest or wasm larger than a module file may be",
        "{j}"
    );
    // At the cap it is size-checked through, and refused as not WASM.
    let (status, j) = check(cap).unwrap_err();
    assert_eq!(
        (status, j["error"].as_str()),
        (422, Some("wasm does not load")),
        "{j}"
    );
}
