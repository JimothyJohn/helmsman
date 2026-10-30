//! The box API over real sockets, against a real runtime scanning on its
//! own thread: what it serves, and everything it refuses. The contract:
//! reads are open, anything else needs the token and is audited, nothing
//! can write a signal, an output, a param or a module, and no client,
//! however slow or hostile, holds up the scan.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use helmsman_iec::{ImageSizes, compile};
use helmsman_io::Value;
use helmsman_plc::api::{
    Api, Catalog, Config, Devices, Extras, Limits, Live, MemAudit, MemProposals, ModuleInfo, Publisher, Screen, Shared, http,
    token,
};
use helmsman_plc::manifest::ModuleDir;
use helmsman_plc::spec::Defaults;
use helmsman_plc::store::{ChangeSet, Meta, ModuleFiles, Store};
use helmsman_plc::{Runtime, SignalTable, Window};
use proptest::prelude::*;
use serde_json::{Value as Json, json};

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

const MANIFEST: &str = "\
name     conveyor
author   Test <test@example.com>
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

const MAP: &str = "\
in  start bool box di:0
out lamp  bool box do:0 safe=false
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
                source: SOURCE.into(),
            },
        )]),
        io_map: Some(MAP.into()),
        calendar: None,
    }
}

fn tmp(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "api-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// What a bench is built with.
struct Setup {
    limits: Limits,
    stream_every: Duration,
    running: Option<ChangeSet>,
    store: Option<PathBuf>,
    screen: Option<Screen>,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            limits: Limits {
                deadline: Duration::from_secs(2),
                ..Limits::default()
            },
            stream_every: Duration::from_millis(20),
            running: None,
            store: None,
            screen: None,
        }
    }
}

/// A runtime scanning on its own thread, the way the binary runs it, and
/// the API over it.
struct Bench {
    api: Option<Api>,
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    scan: Option<JoinHandle<()>>,
    ticks: Arc<AtomicU64>,
    table: Arc<Mutex<SignalTable>>,
    lamp: usize,
    logs: Arc<Mutex<Vec<String>>>,
}

impl Bench {
    fn new() -> Self {
        Self::with(Setup::default())
    }

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
        let lamp = table.index("lamp").unwrap();
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
        let table = Arc::new(Mutex::new(table));
        let live = Arc::new(Live::default());
        let logs = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&logs);
        let cfg = Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            token: TOKEN.into(),
            limits: setup.limits,
            stream_every: setup.stream_every,
            memory: 16,
            log: Arc::new(move |l: &str| sink.lock().unwrap_or_else(PoisonError::into_inner).push(l.into())),
        };
        let shared = Shared {
            catalog,
            live: Arc::clone(&live),
            devices: Arc::new(Devices::new(["box".to_string()])),
            audit: Box::new(MemAudit::new(1000)),
            proposals: Box::new(MemProposals::new(1000)),
            running: setup.running,
            snapshots: setup.store.map(|d| Mutex::new(Store::open(&d).unwrap())),
            approval: None,
        };
        let extras = Extras {
            ui: None,
            screen: setup.screen,
        };
        let api = Api::start_with(cfg, shared, extras).unwrap();
        let addr = api.addr();
        let stop = Arc::new(AtomicBool::new(false));
        let ticks = Arc::new(AtomicU64::new(0));
        let scan = {
            let (stop, ticks, table) = (Arc::clone(&stop), Arc::clone(&ticks), Arc::clone(&table));
            let mut publisher = Publisher::new(live, Duration::from_millis(5));
            thread::spawn(move || {
                let t0 = Instant::now();
                while !stop.load(Ordering::Relaxed) {
                    {
                        let mut t = table.lock().unwrap_or_else(PoisonError::into_inner);
                        t.set_input(start, Some(Value::Bool(true)));
                        let now = t0.elapsed();
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
            ticks,
            table,
            lamp,
            logs,
        };
        Bench::wait("the first scans", || {
            let m = get(b.addr, "/api/modules").json();
            m[0]["stats"]["scans"].as_u64().unwrap_or(0) > 2
        });
        b
    }

    fn wait(what: &str, mut ok: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if ok() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for {what}");
    }

    fn lamp(&self) -> Option<Value> {
        self.table
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .command(self.lamp)
    }

    fn logs(&self) -> Vec<String> {
        self.logs.lock().unwrap_or_else(PoisonError::into_inner).clone()
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

    fn header(&self, name: &str) -> Option<String> {
        self.head.split("\r\n").skip(1).find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case(name).then(|| v.trim().to_string())
        })
    }
}

fn parse(raw: &[u8]) -> Resp {
    let text = String::from_utf8_lossy(raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head.get(9..12).and_then(|s| s.parse().ok()).unwrap_or(0);
    Resp {
        status,
        head: head.into(),
        body: body.into(),
    }
}

/// Sends `raw` as is and reads the answer to EOF.
fn send(addr: SocketAddr, raw: &[u8]) -> Resp {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(raw).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    parse(&out)
}

fn request(addr: SocketAddr, method: &str, path: &str, auth: Option<&str>, ctype: &str, body: &str) -> Resp {
    let auth = auth
        .map(|a| format!("Authorization: {a}\r\n"))
        .unwrap_or_default();
    let ctype = if ctype.is_empty() {
        String::new()
    } else {
        format!("Content-Type: {ctype}\r\n")
    };
    let raw = format!(
        "{method} {path} HTTP/1.1\r\nHost: box\r\n{auth}{ctype}Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    send(addr, raw.as_bytes())
}

fn get(addr: SocketAddr, path: &str) -> Resp {
    request(addr, "GET", path, None, "", "")
}

fn bearer() -> String {
    format!("Bearer {TOKEN}")
}

fn post(addr: SocketAddr, path: &str, body: &Json) -> Resp {
    request(
        addr,
        "POST",
        path,
        Some(&bearer()),
        "application/json",
        &body.to_string(),
    )
}

fn audit(addr: SocketAddr) -> Vec<Json> {
    get(addr, "/api/audit?limit=1000")
        .json()
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn module_draft(manifest: &str, source: &str) -> Json {
    json!({ "kind": "module", "manifest": manifest, "source": source })
}

// ---- reads ----------------------------------------------------------------

#[test]
fn reads_are_open_and_describe_the_box() {
    let b = Bench::new();
    let index = get(b.addr, "/api");
    assert_eq!(index.status, 200, "{index:?}");
    assert_eq!(index.header("Content-Type").as_deref(), Some("application/json"));
    assert_eq!(index.header("X-Content-Type-Options").as_deref(), Some("nosniff"));

    Bench::wait("lamp on", || {
        get(b.addr, "/api/signals/lamp").json()["value"] == json!(true)
    });
    let sigs = get(b.addr, "/api/signals").json();
    let names: Vec<&str> = sigs
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["start", "lamp"]);
    assert_eq!(sigs[0]["dir"], "in");
    assert_eq!(sigs[0]["type"], "bool");
    assert_eq!(sigs[0]["binding"], json!({"kind": "box", "channel": "di:0"}));
    assert_eq!(sigs[0]["quality"], "good");
    assert_eq!(sigs[1]["writer"], "conveyor");
    assert_eq!(sigs[1]["quality"], "commanded");
    assert_eq!(
        get(b.addr, "/api/signals/LAMP").status,
        200,
        "ST names match in any case"
    );

    let m = get(b.addr, "/api/modules/conveyor").json();
    assert_eq!(m["status"]["state"], "running", "{m}");
    assert_eq!(m["scan_ms"], 2);
    assert!(m["source"].as_str().unwrap().contains("PROGRAM conveyor"));
    assert_eq!(m["manifest"]["params"][0]["name"], "speed");
    assert_eq!(m["manifest"]["params"][0]["value"], 1.5);

    let out = get(b.addr, "/api/outputs").json();
    assert_eq!(out["signals"][0]["name"], "lamp");
    assert_eq!(out["signals"][0]["command"], true);
    assert_eq!(out["signals"][0]["safe"], false);
    assert_eq!(out["image"].as_array().unwrap().len(), 32);

    assert_eq!(
        get(b.addr, "/api/devices").json(),
        json!([{"name": "box", "state": "unknown"}])
    );
    assert_eq!(get(b.addr, "/api/proposals").json(), json!([]));
    assert_eq!(get(b.addr, "/api/audit").json(), json!([]));
    assert_eq!(get(b.addr, "/api/nope").status, 404);
    assert_eq!(get(b.addr, "/").status, 404);
}

#[test]
fn a_trace_is_requested_then_served_and_streamed() {
    let b = Bench::new();
    let mut first = get(b.addr, "/api/modules/conveyor/trace");
    assert!(matches!(first.status, 200 | 202), "{first:?}");
    Bench::wait("a traced scan", || {
        first = get(b.addr, "/api/modules/conveyor/trace");
        first.status == 200
    });
    let t = get(b.addr, "/api/modules/conveyor/trace").json();
    assert_eq!(t["module"], "conveyor");
    assert!(t["scan"].as_u64().unwrap() > 0);
    assert_eq!(t["values"]["LAMP"], true, "{t}");
    assert!(t["values"]["N"].as_u64().unwrap() > 0, "{t}");
    let stmts = t["stmts"].as_array().unwrap();
    assert!(
        stmts.iter().any(|s| s["shape"] == "assign" && s["hits"] == 1),
        "{t}"
    );

    let mut s = TcpStream::connect(b.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(b"GET /api/modules/conveyor/trace/events HTTP/1.1\r\n\r\n")
        .unwrap();
    let mut head = [0u8; 50];
    s.read_exact(&mut head).unwrap();
    assert_eq!(&head, b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n");
    let t = read_event(&mut s, "trace");
    assert_eq!(t["module"], "conveyor");
    assert!(t["stmts"].as_array().is_some_and(|a| !a.is_empty()), "{t}");
}

/// Reads an event stream until an event called `name` has arrived whole;
/// returns its data.
fn read_event(s: &mut TcpStream, name: &str) -> Json {
    let start = format!("event: {name}\ndata: ");
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let text = String::from_utf8_lossy(&buf).into_owned();
        if let Some(at) = text.find(&start) {
            let rest = &text[at + start.len()..];
            if let Some(end) = rest.find('\n') {
                return serde_json::from_str(&rest[..end])
                    .unwrap_or_else(|e| panic!("{e}: {}", &rest[..end]));
            }
        }
        assert!(Instant::now() < deadline, "no {name} event in {text:.500}");
        match s.read(&mut chunk) {
            Ok(0) => panic!("stream closed: {text:.500}"),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => {}
        }
    }
}

#[test]
fn the_event_stream_carries_live_values() {
    let b = Bench::new();
    let mut s = TcpStream::connect(b.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(b"GET /api/events HTTP/1.1\r\n\r\n").unwrap();
    let v = read_event(&mut s, "values");
    assert!(v["seq"].as_u64().unwrap() > 0, "{v}");
    assert_eq!(v["modules"][0]["name"], "conveyor");
    assert_eq!(v["signals"][1]["name"], "lamp");
    let next = read_event(&mut s, "values");
    assert!(next["seq"].as_u64() > v["seq"].as_u64(), "each event is newer");
}

// ---- auth -----------------------------------------------------------------

const WRITE_PATHS: &[&str] = &[
    "/api/proposals",
    "/api/proposals/1/apply",
    "/api/outputs",
    "/api/outputs/lamp",
    "/api/signals/lamp",
    "/api/signals/start",
    "/api/modules/conveyor",
    "/api/modules/conveyor/params/speed",
    "/api/snapshots/0000/rollback",
    "/api/audit",
    "/api/nope",
];

const METHODS: &[&str] = &["POST", "PUT", "PATCH", "DELETE", "OPTIONS", "HEAD"];

#[test]
fn every_mutating_call_without_the_right_token_is_refused_and_does_nothing() {
    let b = Bench::new();
    let body = json!({"kind": "param", "module": "conveyor", "param": "speed", "value": 0.5}).to_string();
    let wrong = format!("Bearer {}", "f".repeat(64));
    let short = format!("Bearer {}", &TOKEN[..63]);
    let long = format!("Bearer {TOKEN}0");
    let basic = format!("Basic {TOKEN}");
    let lower = format!("bearer {}", TOKEN.to_uppercase());
    for method in METHODS {
        for path in WRITE_PATHS {
            for auth in [
                None,
                Some(wrong.as_str()),
                Some(short.as_str()),
                Some(long.as_str()),
                Some(basic.as_str()),
                Some(lower.as_str()),
                Some(TOKEN),
            ] {
                let r = request(b.addr, method, path, auth, "application/json", &body);
                assert_eq!(r.status, 401, "{method} {path} {auth:?}: {r:?}");
                assert_eq!(r.header("WWW-Authenticate").as_deref(), Some("Bearer"));
            }
            for q in ["token", "access_token", "TOKEN", "api_key"] {
                let r = request(
                    b.addr,
                    method,
                    &format!("{path}?{q}={TOKEN}"),
                    None,
                    "application/json",
                    &body,
                );
                assert_eq!(r.status, 400, "{method} {path}?{q}=: {r:?}");
            }
        }
    }
    assert_eq!(
        get(b.addr, &format!("/api/signals?token={TOKEN}")).status,
        400,
        "not even on a read"
    );
    assert_eq!(
        audit(b.addr),
        Vec::<Json>::new(),
        "refused calls take no audit space"
    );
    assert_eq!(get(b.addr, "/api/proposals").json(), json!([]));
    assert_eq!(b.lamp(), Some(Value::Bool(true)));
    let refused = b.logs().iter().filter(|l| l.contains("refused")).count();
    assert!(refused > 0, "refusals are logged");
    assert!(
        b.logs().iter().all(|l| !l.contains(TOKEN)),
        "the token is never logged"
    );
}

#[test]
fn no_endpoint_with_the_token_can_change_an_output_a_signal_or_a_module() {
    let b = Bench::new();
    Bench::wait("lamp on", || b.lamp() == Some(Value::Bool(true)));
    let bodies = [
        json!({"value": false}),
        json!({"command": false, "lamp": false}),
        json!({"kind": "param", "module": "conveyor", "param": "speed", "value": 0.5}),
        json!({"kind": "output", "signal": "lamp", "value": false}),
        json!({"kind": "signal", "signal": "start", "value": false}),
    ];
    for method in METHODS {
        for path in WRITE_PATHS {
            for body in &bodies {
                let r = post_as(b.addr, method, path, body);
                let created = *method == "POST" && *path == "/api/proposals" && r.status == 201;
                assert!(
                    created || !(200..300).contains(&r.status),
                    "{method} {path} {body}: {r:?}"
                );
                if created {
                    assert_eq!(r.json()["status"], "draft");
                }
            }
        }
    }
    // A draft that would turn the lamp off, stored, then applied: refused.
    let p = post(b.addr, "/api/proposals", &bodies[2]);
    assert_eq!(p.status, 201, "{p:?}");
    let id = p.json()["id"].as_u64().unwrap();
    let a = post(b.addr, &format!("/api/proposals/{id}/apply"), &json!({}));
    assert_eq!(a.status, 501, "{a:?}");
    assert!(
        a.json()["error"].as_str().unwrap().contains("approval-policy"),
        "{a:?}"
    );
    assert_eq!(
        post(b.addr, "/api/proposals/999999/apply", &json!({})).status,
        404
    );
    thread::sleep(Duration::from_millis(50));
    assert_eq!(b.lamp(), Some(Value::Bool(true)), "the output never moved");
    let m = get(b.addr, "/api/modules/conveyor").json();
    assert_eq!(m["manifest"]["params"][0]["value"], 1.5, "the param never moved");
    let log = audit(b.addr);
    assert!(
        log.iter().any(|e| e["status"] == 501),
        "apply attempts are audited"
    );
    assert!(log.iter().all(|e| e["status"] != 200), "{log:?}");
}

fn post_as(addr: SocketAddr, method: &str, path: &str, body: &Json) -> Resp {
    request(
        addr,
        method,
        path,
        Some(&bearer()),
        "application/json",
        &body.to_string(),
    )
}

// ---- proposals ------------------------------------------------------------

#[test]
fn a_module_draft_is_checked_stored_and_audited_but_not_applied() {
    let b = Bench::with(Setup {
        running: Some(running()),
        ..Setup::default()
    });
    let manifest = MANIFEST.replace("speed real 1.5", "speed real 3.0");
    let r = post(b.addr, "/api/proposals", &module_draft(&manifest, SOURCE));
    assert_eq!(r.status, 201, "{r:?}");
    let p = r.json();
    let id = p["id"].as_u64().unwrap();
    assert_eq!(r.header("Location"), Some(format!("/api/proposals/{id}")));
    assert_eq!(p["kind"], "module");
    assert_eq!(p["status"], "draft");
    assert_eq!(p["replaces"], true);
    assert_eq!(p["touches_outputs"], true);
    assert_eq!(p["base"], json!(running().id().to_string()));
    let mut after = running();
    after.modules.get_mut("conveyor").unwrap().manifest = manifest;
    assert_eq!(p["snapshot"], json!(after.id().to_string()));
    assert_eq!(
        p["diff"]["modules"][0]["params"][0],
        json!({"name": "speed", "from": 1.5, "to": 3.0})
    );
    assert_eq!(get(b.addr, &format!("/api/proposals/{id}")).json(), p);
    assert_eq!(get(b.addr, "/api/proposals").json(), json!([p]));

    let log = audit(b.addr);
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!(log[0]["status"], 201);
    assert_eq!(log[0]["method"], "POST");
    assert_eq!(log[0]["snapshot"], p["snapshot"], "the audit names the snapshot");
    assert!(
        log[0]["detail"].as_str().unwrap().contains("module conveyor"),
        "{log:?}"
    );
    assert_eq!(
        get(b.addr, "/api/modules/conveyor").json()["manifest"]["params"][0]["value"],
        1.5
    );
}

#[test]
fn a_param_draft_carries_the_change_set_it_would_commit() {
    let b = Bench::with(Setup {
        running: Some(running()),
        ..Setup::default()
    });
    let r = post(
        b.addr,
        "/api/proposals",
        &json!({"kind": "param", "module": "conveyor", "param": "SPEED", "value": 7.0, "note": "faster"}),
    );
    assert_eq!(r.status, 201, "{r:?}");
    let p = r.json();
    assert_eq!(
        (p["param"].clone(), p["from"].clone(), p["to"].clone()),
        (json!("speed"), json!(1.5), json!(7.0))
    );
    assert_eq!(p["note"], "faster");
    assert_eq!(p["diff"]["modules"][0]["source"], false);
    assert_eq!(p["diff"]["modules"][0]["params"][0]["to"], 7.0);
    assert_ne!(p["snapshot"], p["base"]);
}

#[test]
fn a_draft_without_a_running_change_set_has_no_snapshot() {
    let b = Bench::new();
    let r = post(b.addr, "/api/proposals", &module_draft(MANIFEST, SOURCE));
    assert_eq!(r.status, 201, "{r:?}");
    assert_eq!(r.json()["snapshot"], Json::Null);
    assert_eq!(audit(b.addr)[0]["snapshot"], Json::Null);
}

#[test]
fn bad_drafts_are_refused_with_reasons_and_audited() {
    let b = Bench::new();
    let bad_source = SOURCE.replace("lamp := start", "lamp := nope");
    let unknown = MANIFEST.replace("reads    start", "reads    start ghost");
    let unknown_src = SOURCE.replace("start : BOOL;", "start : BOOL; ghost : BOOL;");
    let traversal = MANIFEST.replace("name     conveyor", "name     ../../etc");
    let cases: Vec<(String, &str, u16)> = vec![
        ("{".into(), "application/json", 400),
        ("[]".into(), "application/json", 400),
        ("null".into(), "application/json", 400),
        (json!({"kind": "module"}).to_string(), "application/json", 400),
        (json!({"kind": "rm -rf"}).to_string(), "application/json", 400),
        (json!({"kind": "module", "manifest": MANIFEST, "source": SOURCE, "extra": 1}).to_string(), "application/json", 400),
        (json!({"kind": "module", "manifest": MANIFEST, "source": SOURCE, "value": 1}).to_string(), "application/json", 400),
        (module_draft(MANIFEST, SOURCE).to_string(), "text/plain", 415),
        (module_draft(MANIFEST, SOURCE).to_string(), "", 415),
        (module_draft(MANIFEST, SOURCE).to_string(), "application/x-www-form-urlencoded", 415),
        (module_draft("name x\n", SOURCE).to_string(), "application/json", 422),
        (module_draft(&traversal, SOURCE).to_string(), "application/json", 422),
        (module_draft(MANIFEST, &bad_source).to_string(), "application/json", 422),
        (module_draft(MANIFEST, "PROGRAM conveyor END_PROGRAM").to_string(), "application/json", 422),
        (module_draft(&unknown, &unknown_src).to_string(), "application/json", 422),
        (json!({"kind": "param", "module": "conveyor", "param": "speed", "value": 11.0}).to_string(), "application/json", 422),
        (json!({"kind": "param", "module": "conveyor", "param": "speed", "value": -0.5}).to_string(), "application/json", 422),
        (json!({"kind": "param", "module": "conveyor", "param": "speed", "value": "1.0"}).to_string(), "application/json", 422),
        (json!({"kind": "param", "module": "conveyor", "param": "speed", "value": true}).to_string(), "application/json", 422),
        (json!({"kind": "param", "module": "conveyor", "param": "nope", "value": 1.0}).to_string(), "application/json", 422),
        (json!({"kind": "param", "module": "../conveyor", "param": "speed", "value": 1.0}).to_string(), "application/json", 422),
        (json!({"kind": "param", "module": "conveyor", "param": "speed", "value": 1.0, "note": "ok\u{202e}evil"}).to_string(), "application/json", 422),
        (json!({"kind": "param", "module": "conveyor", "param": "speed", "value": 1.0, "note": "line\nbreak"}).to_string(), "application/json", 422),
        (json!({"kind": "param", "module": "conveyor", "param": "speed", "value": 1.0, "note": "x".repeat(2001)}).to_string(), "application/json", 422),
    ];
    for (body, ctype, want) in &cases {
        let r = request(b.addr, "POST", "/api/proposals", Some(&bearer()), ctype, body);
        assert_eq!(r.status, *want, "{body:.200}: {r:?}");
        let j = r.json();
        assert!(j["error"].is_string(), "{j}");
        if *want == 422 {
            assert!(j["problems"].is_array(), "{j}");
        }
    }
    let r = post(b.addr, "/api/proposals", &module_draft(MANIFEST, &bad_source));
    let problems = r.json()["problems"].to_string();
    assert!(
        problems.contains("module.st:"),
        "compile errors name the file and line: {problems}"
    );
    assert_eq!(
        get(b.addr, "/api/proposals").json(),
        json!([]),
        "nothing refused was stored"
    );
    let log = audit(b.addr);
    assert_eq!(log.len(), cases.len() + 1, "every refusal is audited");
    let seqs: Vec<u64> = log.iter().map(|e| e["seq"].as_u64().unwrap()).collect();
    assert_eq!(
        seqs,
        (1..=log.len() as u64).collect::<Vec<_>>(),
        "append-only, in order"
    );
}

#[test]
fn the_audit_log_pages_and_refuses_bad_paging() {
    let b = Bench::new();
    for _ in 0..5 {
        post(b.addr, "/api/proposals", &module_draft(MANIFEST, SOURCE));
    }
    let page = get(b.addr, "/api/audit?after=2&limit=2").json();
    let seqs: Vec<u64> = page
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["seq"].as_u64().unwrap())
        .collect();
    assert_eq!(seqs, [3, 4]);
    for q in [
        "after=-1",
        "limit=x",
        "after=1e3",
        "limit=99999999999999999999999",
    ] {
        assert_eq!(get(b.addr, &format!("/api/audit?{q}")).status, 400, "{q}");
    }
    assert_eq!(get(b.addr, "/api/proposals/abc").status, 404);
    assert_eq!(get(b.addr, "/api/proposals/99999999999999999999999").status, 404);
    assert_eq!(get(b.addr, "/api/proposals/-1").status, 404);
}

// ---- hostile input ----------------------------------------------------------

#[test]
fn module_names_cannot_traverse_or_alias() {
    let b = Bench::new();
    for name in [
        "..",
        "../../etc/passwd",
        "..%2F..%2Fetc%2Fpasswd",
        "%2e%2e%2f%2e%2e%2fetc%2fpasswd",
        "conveyor%2F..",
        "conveyor%00",
        "%00",
        "conveyor%20",
        "CONVEYOR",
        "%EF%BD%83onveyor",
        "conve%CC%81yor",
        "%E2%80%AEroyevnoc",
        "conveyor;rm",
        "conveyor/..",
        "./conveyor",
    ] {
        let r = get(b.addr, &format!("/api/modules/{name}"));
        assert!(matches!(r.status, 400 | 404), "{name}: {r:?}");
        assert!(!r.body.contains("root:"), "{name}");
        let r = get(b.addr, &format!("/api/modules/{name}/trace"));
        assert!(matches!(r.status, 400 | 404), "{name}/trace: {r:?}");
    }
    for bad in ["%", "%zz", "%c0%ae%c0%ae", "%ff", "%e2%82"] {
        assert_eq!(get(b.addr, &format!("/api/modules/{bad}")).status, 400, "{bad}");
    }
    assert_eq!(get(b.addr, "/api/modules/conveyor").status, 200);
}

#[test]
fn malformed_requests_are_refused_with_the_right_status() {
    let b = Bench::new();
    let big_head = format!("GET /api HTTP/1.1\r\nX-Pad: {}\r\n\r\n", "a".repeat(20_000));
    let many = format!("GET /api HTTP/1.1\r\n{}\r\n", "X-A: b\r\n".repeat(100));
    let cases: Vec<(Vec<u8>, u16)> = vec![
        (
            b"GET /api HTTP/1.1\r\nContent-Length: 99999999\r\n\r\n".to_vec(),
            413,
        ),
        (
            b"POST /api/proposals HTTP/1.1\r\nContent-Length: 1048577\r\n\r\n".to_vec(),
            413,
        ),
        (big_head.into_bytes(), 431),
        (many.into_bytes(), 431),
        (
            b"POST /api/proposals HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n".to_vec(),
            501,
        ),
        (
            b"GET /api HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 1\r\n\r\nx".to_vec(),
            400,
        ),
        (b"GET /api HTTP/1.1\r\nContent-Length: +1\r\n\r\nx".to_vec(), 400),
        (b"GET /api HTTP/1.1\r\nContent-Length: 0x1\r\n\r\n".to_vec(), 400),
        (b"GET /api HTTP/1.1\r\nContent-Length: -1\r\n\r\n".to_vec(), 400),
        (
            b"GET /api HTTP/1.1\r\nAuthorization: a\r\nAuthorization: b\r\n\r\n".to_vec(),
            400,
        ),
        (b"GET /api HTTP/1.1\r\nX-A: b\r\n folded\r\n\r\n".to_vec(), 400),
        (b"GET /api HTTP/1.1\r\nX-A: b\nInjected: c\r\n\r\n".to_vec(), 400),
        (b"GET /api HTTP/1.1\r\nBad Name: x\r\n\r\n".to_vec(), 400),
        (b"GET /api HTTP/1.1\r\nNoColon\r\n\r\n".to_vec(), 400),
        (b"GET /api HTTP/2.0\r\n\r\n".to_vec(), 505),
        (b"GET /api\r\n\r\n".to_vec(), 400),
        (b"GET  /api HTTP/1.1\r\n\r\n".to_vec(), 400),
        (b"get /api HTTP/1.1\r\n\r\n".to_vec(), 400),
        (b"GET api HTTP/1.1\r\n\r\n".to_vec(), 400),
        (b"GET http://evil/api HTTP/1.1\r\n\r\n".to_vec(), 400),
        (b"GET /api#x HTTP/1.1\r\n\r\n".to_vec(), 400),
        (b"GET /api\x7f HTTP/1.1\r\n\r\n".to_vec(), 400),
        (b"GET /\xff\xfe HTTP/1.1\r\n\r\n".to_vec(), 400),
        (b"GET /api HTTP/1.1\r\nX: \xc3\xa9\r\n\r\n".to_vec(), 400),
        (b"\r\n\r\n".to_vec(), 400),
    ];
    for (raw, want) in cases {
        let r = send(b.addr, &raw);
        assert_eq!(
            r.status,
            want,
            "{:?}: {r:?}",
            String::from_utf8_lossy(&raw[..raw.len().min(80)])
        );
    }
    // And the server is still fine.
    assert_eq!(get(b.addr, "/api").status, 200);
}

#[test]
fn nothing_from_the_request_reaches_a_response_header() {
    let b = Bench::new();
    for path in [
        "/api/modules/a%0d%0aSet-Cookie:%20x=1",
        "/api/modules/a%0aLocation:%20http://evil",
        "/api/proposals/1%0d%0aX-Evil:%201",
    ] {
        let r = get(b.addr, path);
        assert!(matches!(r.status, 400 | 404), "{path}: {r:?}");
        let lower = r.head.to_ascii_lowercase();
        assert!(
            !lower.contains("set-cookie") && !lower.contains("x-evil") && !lower.contains("location"),
            "{path}: {r:?}"
        );
    }
    let r = post(b.addr, "/api/proposals", &module_draft(MANIFEST, SOURCE));
    let loc = r.header("Location").unwrap();
    assert!(
        loc.strip_prefix("/api/proposals/")
            .unwrap()
            .bytes()
            .all(|c| c.is_ascii_digit()),
        "{loc}"
    );
}

#[test]
fn nothing_from_the_request_breaks_a_log_line() {
    let b = Bench::new();
    let paths = [
        "/api/x%0Ahelmsman-plc:%20STOP%20faked",
        "/api/x%0D%0Ainjected",
        "/api/x%1B%5B31mred",
        "/api/x%E2%80%AEreversed",
    ];
    for path in paths {
        request(b.addr, "POST", path, None, "", "");
        request(b.addr, "DELETE", path, Some(&bearer()), "", "");
    }
    // Raw (undecoded) control bytes never get as far as a log.
    send(b.addr, b"POST /api/x\x1b[31m HTTP/1.1\r\n\r\n");
    let logs = b.logs();
    assert!(logs.len() >= paths.len() * 2, "{logs:?}");
    for l in &logs {
        assert!(
            !l.chars().any(char::is_control),
            "control character in log line {l:?}"
        );
        assert!(l.chars().count() < 400, "log line not bounded: {l:?}");
    }
    let audit = audit(b.addr);
    assert_eq!(
        audit.len(),
        paths.len(),
        "authenticated calls audited, refused ones not"
    );
    assert!(
        audit.iter().all(|e| e["path"].as_str().unwrap().is_ascii()),
        "paths are stored as sent, encoded"
    );
    assert!(
        get(b.addr, &"/api/".repeat(500)).status != 0,
        "long paths are bounded too"
    );
}

// ---- resource limits --------------------------------------------------------

#[test]
fn a_slowloris_client_is_cut_off_at_the_deadline_and_cannot_starve_others() {
    let deadline = Duration::from_millis(600);
    let b = Bench::with(Setup {
        limits: Limits {
            deadline,
            workers: 4,
            ..Limits::default()
        },
        ..Setup::default()
    });
    let started = Instant::now();
    let dribblers: Vec<JoinHandle<(u16, Duration)>> = (0..4)
        .map(|_| {
            let addr = b.addr;
            thread::spawn(move || {
                let mut s = TcpStream::connect(addr).unwrap();
                s.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
                let t0 = Instant::now();
                let mut out = Vec::new();
                for byte in b"GET /api HTTP/1.1\r\nX-Slow: "
                    .iter()
                    .chain(std::iter::repeat(&b'a'))
                {
                    if s.write_all(&[*byte]).is_err() {
                        break;
                    }
                    let mut buf = [0u8; 512];
                    match s.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            out.extend_from_slice(&buf[..n]);
                            break;
                        }
                        Err(_) => {}
                    }
                    if t0.elapsed() > Duration::from_secs(10) {
                        break;
                    }
                }
                let _ = s.read_to_end(&mut out);
                (parse(&out).status, t0.elapsed())
            })
        })
        .collect();
    // While all four workers are taken, the next client is told so at once.
    thread::sleep(Duration::from_millis(150));
    let t0 = Instant::now();
    let busy = get(b.addr, "/api");
    assert!(matches!(busy.status, 503 | 200), "{busy:?}");
    assert!(
        t0.elapsed() < Duration::from_secs(1),
        "a busy box answers at once"
    );
    for d in dribblers {
        let (status, took) = d.join().unwrap();
        assert_eq!(status, 408, "dribbler cut off with 408");
        assert!(
            took < deadline + Duration::from_secs(2),
            "held a worker for {took:?}"
        );
    }
    assert!(started.elapsed() < Duration::from_secs(5));
    Bench::wait("a free worker", || get(b.addr, "/api").status == 200);
}

#[test]
fn a_client_that_never_reads_its_event_stream_is_dropped() {
    let b = Bench::with(Setup {
        limits: Limits {
            streams: 2,
            write_timeout: Duration::from_millis(200),
            stream_life: Duration::from_secs(1),
            ..Limits::default()
        },
        stream_every: Duration::from_millis(1),
        ..Setup::default()
    });
    let stalled: Vec<TcpStream> = (0..2)
        .map(|_| {
            let mut s = TcpStream::connect(b.addr).unwrap();
            s.write_all(b"GET /api/events HTTP/1.1\r\n\r\n").unwrap();
            s
        })
        .collect();
    thread::sleep(Duration::from_millis(100));
    let third = get(b.addr, "/api/events");
    assert_eq!(third.status, 503, "streams are capped: {third:?}");
    // Reads keep working while the streams are stuck.
    assert_eq!(get(b.addr, "/api/signals").status, 200);
    // The stuck streams end: at the write timeout once the kernel's
    // buffers for them are full (fast on macOS loopback, many megabytes on
    // Linux), and at their lifetime whatever the buffers.
    Bench::wait("a stream slot freed", || {
        let mut s = TcpStream::connect(b.addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        s.write_all(b"GET /api/events HTTP/1.1\r\n\r\n").unwrap();
        let mut buf = [0u8; 12];
        s.read_exact(&mut buf).is_ok() && &buf == b"HTTP/1.1 200"
    });
    let before = b.ticks.load(Ordering::Relaxed);
    thread::sleep(Duration::from_millis(100));
    assert!(b.ticks.load(Ordering::Relaxed) > before, "the scan kept going");
    for s in stalled {
        let _ = s.shutdown(Shutdown::Both);
    }
}

#[test]
fn hammering_the_api_does_not_hold_up_the_scan() {
    let b = Bench::with(Setup {
        limits: Limits {
            write_timeout: Duration::from_millis(200),
            ..Limits::default()
        },
        stream_every: Duration::from_millis(1),
        ..Setup::default()
    });
    let window = Duration::from_millis(1000);
    let quiet = {
        let t = b.ticks.load(Ordering::Relaxed);
        thread::sleep(window);
        b.ticks.load(Ordering::Relaxed) - t
    };
    let stop = Arc::new(AtomicBool::new(false));
    let served = Arc::new(AtomicU64::new(0));
    let mut clients: Vec<JoinHandle<()>> = (0..24)
        .map(|k| {
            let (stop, served, addr) = (Arc::clone(&stop), Arc::clone(&served), b.addr);
            thread::spawn(move || {
                let paths = [
                    "/api/signals",
                    "/api/modules",
                    "/api/outputs",
                    "/api/modules/conveyor/trace",
                    "/api/audit",
                ];
                let mut i = k;
                while !stop.load(Ordering::Relaxed) {
                    if get(addr, paths[i % paths.len()]).status != 0 {
                        served.fetch_add(1, Ordering::Relaxed);
                    }
                    i += 1;
                }
            })
        })
        .collect();
    // And streams nobody reads, which stall in write.
    let stalled: Vec<TcpStream> = (0..4)
        .map(|_| {
            let mut s = TcpStream::connect(b.addr).unwrap();
            s.write_all(b"GET /api/events HTTP/1.1\r\n\r\n").unwrap();
            s
        })
        .collect();
    clients.push({
        let (stop, addr) = (Arc::clone(&stop), b.addr);
        thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                post(addr, "/api/proposals", &module_draft(MANIFEST, SOURCE));
            }
        })
    });
    let scans_before = get(b.addr, "/api/modules").json()[0]["stats"]["scans"]
        .as_u64()
        .unwrap();
    let busy = {
        let t = b.ticks.load(Ordering::Relaxed);
        thread::sleep(window);
        b.ticks.load(Ordering::Relaxed) - t
    };
    stop.store(true, Ordering::Relaxed);
    for c in clients {
        c.join().unwrap();
    }
    drop(stalled);
    let scans_after = get(b.addr, "/api/modules").json()[0]["stats"]["scans"]
        .as_u64()
        .unwrap();
    assert!(served.load(Ordering::Relaxed) > 50, "the hammer hammered");
    assert!(
        busy * 4 >= quiet,
        "scan ticks fell from {quiet} to {busy} per {window:?} under load"
    );
    assert!(
        scans_after > scans_before + 50,
        "the module kept scanning: {scans_before} -> {scans_after}"
    );
}

// ---- snapshots ----------------------------------------------------------------

fn meta() -> Meta {
    Meta {
        author: "test".into(),
        prompt: String::new(),
        approver: None,
        time_ms: 1_700_000_000_000,
    }
}

#[test]
fn snapshots_are_readable_and_rollback_needs_the_approval_policy() {
    let dir = tmp("store");
    let first = running();
    let mut second = running();
    second.modules.get_mut("conveyor").unwrap().manifest = MANIFEST.replace("1.5", "2.5");
    {
        let mut s = Store::open(&dir).unwrap();
        s.commit(&first, &meta()).unwrap();
        s.commit(&second, &meta()).unwrap();
    }
    let b = Bench::with(Setup {
        running: Some(second.clone()),
        store: Some(dir.clone()),
        ..Setup::default()
    });
    let (old, new) = (first.id().to_string(), second.id().to_string());
    let list = get(b.addr, "/api/snapshots").json();
    assert_eq!(list["head"], json!(new), "{list}");
    assert_eq!(list["running"], json!(new));
    assert_eq!(list["log"].as_array().unwrap().len(), 2);
    assert_eq!(
        (list["log"][0]["id"].clone(), list["log"][1]["id"].clone()),
        (json!(new), json!(old)),
        "newest first"
    );
    assert_eq!(get(b.addr, "/api/snapshots/head").json()["id"], json!(new));
    let snap = get(b.addr, &format!("/api/snapshots/{old}")).json();
    assert_eq!(snap["id"], json!(old));
    assert_eq!(snap["modules"]["conveyor"]["manifest"], MANIFEST);
    assert_eq!(snap["io_map"], MAP);
    let diff = get(b.addr, &format!("/api/snapshots/{old}/diff/{new}")).json();
    assert_eq!(
        diff["modules"][0]["params"][0],
        json!({"name": "speed", "from": 1.5, "to": 2.5})
    );
    for bad in [
        "0".repeat(64),
        "zz".into(),
        "../snapshots.db".into(),
        "%2e%2e%2fsnapshots.db".into(),
        old.to_uppercase(),
    ] {
        assert_eq!(get(b.addr, &format!("/api/snapshots/{bad}")).status, 404, "{bad}");
    }
    let rb = post(b.addr, &format!("/api/snapshots/{old}/rollback"), &json!({}));
    assert_eq!(rb.status, 501, "{rb:?}");
    assert_eq!(
        post(
            b.addr,
            &format!("/api/snapshots/{}/rollback", "0".repeat(64)),
            &json!({})
        )
        .status,
        404
    );
    let unauth = request(
        b.addr,
        "POST",
        &format!("/api/snapshots/{old}/rollback"),
        None,
        "",
        "",
    );
    assert_eq!(unauth.status, 401);
    let log = audit(b.addr);
    assert_eq!(
        log[0]["snapshot"],
        json!(old),
        "the audit names the rollback target: {log:?}"
    );
    assert_eq!(
        get(b.addr, "/api/snapshots").json()["head"],
        json!(new),
        "nothing was rolled back"
    );
    let s = Store::open(&dir).unwrap();
    assert_eq!(s.list().unwrap().len(), 2, "the store is untouched");
}

#[test]
fn without_a_store_snapshots_are_not_found() {
    let b = Bench::new();
    assert_eq!(get(b.addr, "/api/snapshots").status, 404);
    assert_eq!(get(b.addr, "/api/snapshots/head").status, 404);
}

#[test]
fn a_point_error_shows_on_its_signal_until_it_clears() {
    let b = Bench::new();
    assert_eq!(get(b.addr, "/api/signals/lamp").json()["point_error"], Json::Null);
    let why = "exception 2 \"illegal address\"\nsecond line";
    b.table
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .set_point_error(b.lamp, Some(why.into()));
    Bench::wait("the error published", || {
        get(b.addr, "/api/signals/lamp").json()["point_error"] == json!(why)
    });
    let out = get(b.addr, "/api/outputs").json();
    assert_eq!(out["signals"][0]["point_error"], json!(why), "{out}");
    assert_eq!(
        get(b.addr, "/api/signals/start").json()["point_error"],
        Json::Null
    );
    b.table
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .set_point_error(b.lamp, None);
    Bench::wait("the error cleared", || {
        get(b.addr, "/api/signals/lamp").json()["point_error"] == Json::Null
    });
}

// ---- the token ----------------------------------------------------------------

#[cfg(unix)]
fn mode(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[test]
fn the_token_is_made_once_private_and_then_reused() {
    let dir = tmp("token");
    let t = token::load_or_create(&dir).unwrap();
    assert!(t.new);
    assert_eq!(t.token.len(), token::LEN);
    assert!(t.token.bytes().all(|b| b.is_ascii_hexdigit()));
    #[cfg(unix)]
    assert_eq!(mode(&t.path), 0o600);
    let again = token::load_or_create(&dir).unwrap();
    assert!(!again.new);
    assert_eq!(again.token, t.token);
    let other = token::load_or_create(&tmp("token")).unwrap();
    assert_ne!(other.token, t.token, "tokens are random");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&t.path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(token::load_or_create(&dir).unwrap().token, t.token);
        assert_eq!(mode(&t.path), 0o600, "made private again");
    }
    std::fs::write(&t.path, "short\n").unwrap();
    let fresh = token::load_or_create(&dir).unwrap();
    assert!(fresh.new, "a file without a token is replaced");
    assert_ne!(fresh.token, t.token);
}

#[test]
fn processes_racing_to_make_the_token_agree_on_one() {
    let dir = tmp("race");
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let dir = dir.clone();
            thread::spawn(move || token::load_or_create(&dir).unwrap())
        })
        .collect();
    let tokens: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(tokens.iter().filter(|t| t.new).count(), 1, "made once");
    assert!(tokens.iter().all(|t| t.token == tokens[0].token));
}

#[test]
fn token_comparison_needs_every_byte() {
    assert!(token::eq(TOKEN, TOKEN));
    assert!(!token::eq(TOKEN, &TOKEN[..63]));
    assert!(!token::eq(TOKEN, ""));
    assert!(!token::eq("", TOKEN));
    let mut flipped = TOKEN.to_string();
    flipped.replace_range(63.., "0");
    assert!(!token::eq(TOKEN, &flipped));
}

// ---- the parser alone ---------------------------------------------------------

proptest! {
    #[test]
    fn the_head_parser_never_panics(raw in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = http::parse_head(&raw, &Limits::default());
    }

    #[test]
    fn a_parsed_head_holds_only_what_it_promised(
        method in "[A-Z]{1,8}",
        segs in proptest::collection::vec("[a-zA-Z0-9._~%-]{0,12}", 1..5),
        len in 0usize..2000,
    ) {
        let path = format!("/{}", segs.join("/"));
        let head = format!("{method} {path} HTTP/1.1\r\nContent-Length: {len}\r\nAuthorization: Bearer abc");
        match http::parse_head(head.as_bytes(), &Limits::default()) {
            Ok((req, n)) => {
                prop_assert_eq!(n, len);
                prop_assert_eq!(req.method, method);
                prop_assert_eq!(req.path, path);
                prop_assert_eq!(req.bearer.as_deref(), Some("abc"));
                prop_assert_eq!(req.segments.len(), segs.len());
            }
            Err(e) => prop_assert!(matches!(e, http::ReadError::Status(400, _)), "{e:?}"),
        }
    }

    #[test]
    fn decoding_round_trips_any_text(s in "\\PC{0,40}") {
        let enc = s.bytes().fold(String::new(), |mut e, b| { let _ = std::fmt::Write::write_fmt(&mut e, format_args!("%{b:02X}")); e });
        prop_assert_eq!(http::decode(&enc).unwrap(), s);
    }
}

const LAYOUT: &str = "\
helmsman-screen 1
title Hall 2 andon
grid 1 1
code line 0 grey Idle
code line 3 red Down: call maintenance
tile line1 0,0 line line1_state Line 1
";

#[test]
fn the_screen_layout_is_served_read_only() {
    let none = Bench::new();
    let r = get(none.addr, "/api/screen");
    assert_eq!(r.status, 404, "{r:?}");
    assert!(r.body.contains("--screen"), "{r:?}");
    drop(none);

    let b = Bench::with(Setup {
        screen: Some(Screen {
            source: LAYOUT.into(),
            layout: helmsman_plc::screen::parse(LAYOUT).unwrap(),
        }),
        ..Setup::default()
    });
    let r = get(b.addr, "/api/screen");
    assert_eq!(r.status, 200, "{r:?}");
    let j = r.json();
    assert_eq!(j["source"], LAYOUT, "byte for byte, for the canvas to parse");
    assert_eq!(j["layout"]["title"], "Hall 2 andon");
    assert_eq!(j["layout"]["tiles"][0]["state"], "line1_state");
    assert_eq!(j["layout"]["sets"]["line"]["3"]["colour"], "red");
    assert_eq!(j["palette"]["yellow"]["fill"], "#FFC20E", "the brand's safety yellow");
    assert_eq!(j["palette"]["yellow"]["ink"], "#141414");
    assert_eq!(
        helmsman_plc::screen::parse(j["source"].as_str().unwrap()).unwrap(),
        helmsman_plc::screen::parse(LAYOUT).unwrap()
    );
    let index = get(b.addr, "/api").json();
    assert!(index["endpoints"].as_array().unwrap().iter().any(|e| e["path"] == "/api/screen"));

    // No method but GET, with or without the token; refusals are audited.
    for method in ["POST", "PUT", "DELETE", "PATCH"] {
        let r = request(b.addr, method, "/api/screen", Some(&bearer()), "application/json", "{}");
        assert_eq!(r.status, 405, "{method}: {r:?}");
        assert!(r.head.contains("Allow: GET"), "{r:?}");
        let r = request(b.addr, method, "/api/screen", None, "application/json", "{}");
        assert_eq!(r.status, 401, "{method}: {r:?}");
    }
    assert!(audit(b.addr).iter().any(|e| e["path"] == "/api/screen" && e["status"] == 405));
    assert_eq!(get(b.addr, "/api/screen").json()["source"], LAYOUT, "unchanged");
    assert_eq!(get(b.addr, "/api/screen/x").status, 404);
}
