//! Projects on the box (`project`, `api/projects.rs`) over real sockets,
//! a real snapshot store, real accounts and a runtime scanning on its own
//! thread: the folders and their rules, the engineer gate on every write,
//! ZIP download and upload against hostile archives, deploy through the
//! approval path (compile errors with file and line, nothing changed on
//! failure, rollback after), concurrent saves and a deploy racing them,
//! `PLCopen` import, and the binary booting the deployed project.
#![allow(clippy::many_single_char_names)] // bench, session, response, file
#![allow(clippy::too_many_lines)] // each test walks one whole flow
#![allow(clippy::option_option)] // a save: no version, null (new only), or one
#![allow(clippy::type_complexity)] // tables of (files, file, line)

use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use helmsman_iec::{ImageSizes, compile};
use helmsman_io::Value;
use helmsman_plc::api::{
    Api, Approval, Catalog, Config, Devices, Extras, Limits, Live, MemAudit, MemProposals, ModuleInfo, More,
    Plant, Publisher, Shared, swap,
};
use helmsman_plc::force::{self, Forces};
use helmsman_plc::manifest::ModuleDir;
use helmsman_plc::policy::account::{self, Accounts, Logins};
use helmsman_plc::policy::pin::{Gate, Pins};
use helmsman_plc::policy::{FLOOR, Role};
use helmsman_plc::project::build::{self, Shape};
use helmsman_plc::project::{self, Files, Projects};
use helmsman_plc::record::{RecordConfig, Recorder};
use helmsman_plc::spec::Defaults;
use helmsman_plc::store::{ChangeSet, Meta, ModuleFiles, Store};
use helmsman_plc::{Runtime, SignalTable, Window};
use serde_json::{Value as Json, json};

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const ALICE: &str = "alice-engineer-pw";
const BOB: &str = "bob-operator-pw";
const CAROL: &str = "carol-integrator-pw";

const MANIFEST: &str = "\
name     conveyor
author   Test
scan_ms  2
reads    start
drives   lamp
";

const SOURCE: &str = "
PROGRAM conveyor
VAR_INPUT start : BOOL; END_VAR
VAR_OUTPUT lamp : BOOL; END_VAR
  lamp := start;
END_PROGRAM";

const MAP: &str = "\
in  start bool box di:0
out lamp  bool box do:0 safe=false
out horn  bool box do:1 safe=false
";

/// A function block in one file, used by a PROGRAM in another.
const BLOCKS: &str = "\
FUNCTION_BLOCK Latch
VAR_INPUT set : BOOL; reset : BOOL; END_VAR
VAR_OUTPUT q : BOOL; END_VAR
  q := (q OR set) AND NOT reset;
END_FUNCTION_BLOCK
";

const MAIN: &str = "\
PROGRAM conveyor
VAR_INPUT start : BOOL; END_VAR
VAR_OUTPUT lamp : BOOL; END_VAR
VAR l : Latch; END_VAR
  l(set := start, reset := FALSE);
  lamp := l.q;
END_PROGRAM
";

const COUNTER: &str = "\
PROGRAM counter
VAR n : UDINT; END_VAR
  n := n + 1;
END_PROGRAM
";

const SETTINGS: &str = "\
# how the programs run
program conveyor scan_ms=2
program counter  scan_ms=5
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
        modules: BTreeMap::from([("conveyor".to_string(), ModuleFiles::st(MANIFEST, SOURCE))]),
        io_map: Some(MAP.into()),
        calendar: None,
        drivers: BTreeMap::new(),
    }
}

fn tmp(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "projects-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn meta() -> Meta {
    Meta {
        author: "Test".into(),
        prompt: String::new(),
        approver: None,
        time_ms: 1,
    }
}

/// The box: a runtime scanning on its own thread, the API with projects
/// and accounts (alice engineer, bob operator, carol integrator) over it,
/// a store whose head is [`running`].
struct Bench {
    api: Option<Api>,
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    scan: Option<JoinHandle<()>>,
    store: PathBuf,
    state: PathBuf,
    /// Seconds added to the login clock.
    clock: Arc<AtomicU64>,
    table: Arc<Mutex<SignalTable>>,
    /// The input image the last scan ran on, forces applied.
    image: Arc<Mutex<Vec<u8>>>,
    ticks: Arc<AtomicU64>,
}

impl Bench {
    #[allow(clippy::too_many_lines)]
    fn new() -> Self {
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
        let store = tmp("store");
        Store::open(&store).unwrap().commit(&running(), &meta()).unwrap();
        let state = tmp("state");
        let mut a = Accounts::default();
        a.add("alice", Role::Engineer, ALICE, 1, [1; 16]).unwrap();
        a.add("bob", Role::Operator, BOB, 1, [2; 16]).unwrap();
        a.add("carol", Role::Integrator, CAROL, 1, [3; 16]).unwrap();
        a.save(&state).unwrap();
        let mut recorder = Recorder::new(&table, RecordConfig::default(), None);
        let (handle, swapper) = swap::channel(Duration::from_secs(5));
        let t0 = Instant::now();
        let clock = Arc::new(AtomicU64::new(0));
        let c = Arc::clone(&clock);
        let now: helmsman_plc::policy::pin::Clock =
            Arc::new(move || t0 + Duration::from_secs(c.load(Ordering::SeqCst)));
        let approval = Approval {
            policy: FLOOR,
            gate: Gate::new(Pins::default(), FLOOR.lockout, Arc::clone(&now)).with_logins(Logins::new(
                state.clone(),
                1,
                FLOOR.lockout,
                now,
            )),
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
        let cfg = Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            token: TOKEN.into(),
            limits: Limits::default(),
            stream_every: Duration::from_millis(20),
            memory: 16,
            log: Arc::new(|_: &str| {}),
        };
        let shared = Shared {
            catalog,
            live: Arc::clone(&live),
            devices: Arc::new(Devices::new(["box".to_string()])),
            audit: Box::new(MemAudit::new(100_000)),
            proposals: Box::new(MemProposals::new(1000)),
            running: Some(running()),
            snapshots: Some(Mutex::new(Store::open(&store).unwrap())),
            approval: Some(approval),
            chat: None,
        };
        let forces = Arc::new(Forces::default());
        let more = More {
            projects: Some(Projects::in_state_dir(&state).unwrap()),
            forces: Some(Arc::clone(&forces)),
        };
        let image = Arc::new(Mutex::new(vec![0u8; 32]));
        let api = Api::start_box(cfg, shared, Extras::default(), None, more).unwrap();
        let addr = api.addr();
        let stop = Arc::new(AtomicBool::new(false));
        let ticks = Arc::new(AtomicU64::new(0));
        let scan = {
            let (stop, table, ticks) = (Arc::clone(&stop), Arc::clone(&table), Arc::clone(&ticks));
            let (forces, image) = (Arc::clone(&forces), Arc::clone(&image));
            let mut publisher = Publisher::new(live, Duration::from_millis(5));
            let mut reader = force::Reader::default();
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    {
                        let mut t = table.lock().unwrap_or_else(PoisonError::into_inner);
                        // The I/O read: start is held on, the image zero.
                        t.set_input(start, Some(Value::Bool(true)));
                        let mut inputs = vec![0u8; 32];
                        swapper.serve(&mut rt, &mut t);
                        reader.apply(&forces, &mut inputs, &mut t);
                        let now = t0.elapsed();
                        recorder.observe(now, &t);
                        rt.tick_with(now, &inputs, &mut t);
                        publisher.publish(now, &rt, &t);
                        image
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .clone_from(&inputs);
                    }
                    ticks.fetch_add(1, Ordering::Relaxed);
                    thread::sleep(Duration::from_millis(1));
                }
            })
        };
        wait("a recorded window", || ticks.load(Ordering::Relaxed) > 20);
        Self {
            api: Some(api),
            addr,
            stop,
            scan: Some(scan),
            store,
            state,
            clock,
            table,
            image,
            ticks,
        }
    }

    fn cmd(&self, signal: &str) -> Option<Value> {
        let t = self.table.lock().unwrap_or_else(PoisonError::into_inner);
        t.command(t.index(signal).unwrap())
    }

    fn forces(&self) -> Vec<Json> {
        get(self.addr, "/api/forces").json()["forces"]
            .as_array()
            .unwrap()
            .clone()
    }

    fn login(&self, user: &str, password: &str) -> String {
        let r = send(
            self.addr,
            "POST",
            "/api/login",
            None,
            "application/json",
            json!({ "user": user, "password": password })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(r.status, 200, "{r:?}");
        r.json()["token"].as_str().unwrap().to_string()
    }

    fn alice(&self) -> String {
        self.login("alice", ALICE)
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

    fn modules(&self) -> Vec<String> {
        get(self.addr, "/api/modules")
            .json()
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["name"].as_str().unwrap().to_string())
            .collect()
    }

    fn projects_root(&self) -> PathBuf {
        self.state.join("projects")
    }

    /// A project with the given files, made through the API.
    fn project(&self, s: &str, name: &str, files: &[(&str, &str)]) {
        let r = post_json(self.addr, "/api/projects", Some(s), &json!({ "name": name }));
        assert_eq!(r.status, 201, "{r:?}");
        for (f, text) in files {
            let r = put_file(self.addr, s, name, f, text, None);
            assert_eq!(r.status, 200, "{r:?}");
        }
    }

    /// Replays then deploys `name` with session `s`.
    fn deploy(&self, s: &str, name: &str) -> Resp {
        let r = post_json(
            self.addr,
            &format!("/api/projects/{name}/replay"),
            Some(s),
            &json!({}),
        );
        if r.status != 201 {
            return r;
        }
        let n = r.json()["replay"].as_u64().unwrap();
        post_json(
            self.addr,
            &format!("/api/projects/{name}/deploy"),
            Some(s),
            &json!({ "replay": n }),
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
    }
}

fn wait(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
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
}

fn send(addr: SocketAddr, method: &str, path: &str, bearer: Option<&str>, ctype: &str, body: &[u8]) -> Resp {
    let auth = bearer.map_or_else(String::new, |t| format!("Authorization: Bearer {t}\r\n"));
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
    let body = out.get(split + 4..).unwrap_or_default().to_vec();
    Resp {
        status: head.get(9..12).and_then(|s| s.parse().ok()).unwrap_or(0),
        head,
        body,
    }
}

fn get(addr: SocketAddr, path: &str) -> Resp {
    send(addr, "GET", path, None, "application/json", b"")
}

fn post_json(addr: SocketAddr, path: &str, bearer: Option<&str>, body: &Json) -> Resp {
    send(
        addr,
        "POST",
        path,
        bearer,
        "application/json",
        body.to_string().as_bytes(),
    )
}

fn put_file(addr: SocketAddr, s: &str, p: &str, f: &str, text: &str, version: Option<Option<&str>>) -> Resp {
    let body = match version {
        None => json!({ "text": text }),
        Some(v) => json!({ "text": text, "version": v }),
    };
    send(
        addr,
        "PUT",
        &format!("/api/projects/{p}/files/{f}"),
        Some(s),
        "application/json",
        body.to_string().as_bytes(),
    )
}

fn upload(addr: SocketAddr, s: &str, p: &str, zip: &[u8], replace: bool) -> Resp {
    let q = if replace { "?replace=true" } else { "" };
    send(
        addr,
        "POST",
        &format!("/api/projects/{p}/zip{q}"),
        Some(s),
        "application/zip",
        zip,
    )
}

fn file(addr: SocketAddr, p: &str, f: &str) -> Resp {
    get(addr, &format!("/api/projects/{p}/files/{f}"))
}

/// Every file under `dir`, recursively, as relative paths.
fn tree(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            if e.file_type().unwrap().is_dir() {
                stack.push(p.clone());
            }
            out.push(p.strip_prefix(dir).unwrap().display().to_string());
        }
    }
    out.sort();
    out
}

// ---- hostile ZIP archives -----------------------------------------------------

/// One entry as a hostile archive may say it: any name bytes, a method,
/// declared sizes that may lie, a Unix mode.
struct Raw {
    name: Vec<u8>,
    data: Vec<u8>,
    deflate: bool,
    /// Declared uncompressed size; `None`: the real one.
    size: Option<u32>,
    mode: u32,
    utf8: bool,
}

impl Raw {
    fn file(name: &str, data: &[u8]) -> Self {
        Self {
            name: name.as_bytes().to_vec(),
            data: data.to_vec(),
            deflate: false,
            size: None,
            mode: 0o100_644,
            utf8: true,
        }
    }
}

fn crc(data: &[u8]) -> u32 {
    project::zip::crc32(data)
}

/// A ZIP written by hand, whatever it says.
fn zip_raw(entries: &[Raw]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    let p16 = |v: &mut Vec<u8>, x: u16| v.extend_from_slice(&x.to_le_bytes());
    let p32 = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());
    for e in entries {
        let offset = u32::try_from(out.len()).unwrap();
        let stored = if e.deflate {
            miniz_oxide::deflate::compress_to_vec(&e.data, 6)
        } else {
            e.data.clone()
        };
        let method = if e.deflate { 8 } else { 0 };
        let flags = if e.utf8 { 1 << 11 } else { 0 };
        let size = e.size.unwrap_or(u32::try_from(e.data.len()).unwrap());
        let csize = u32::try_from(stored.len()).unwrap();
        let nlen = u16::try_from(e.name.len()).unwrap();
        p32(&mut out, 0x0403_4b50);
        p16(&mut out, 20);
        p16(&mut out, flags);
        p16(&mut out, method);
        p16(&mut out, 0);
        p16(&mut out, 33);
        p32(&mut out, crc(&e.data));
        p32(&mut out, csize);
        p32(&mut out, size);
        p16(&mut out, nlen);
        p16(&mut out, 0);
        out.extend_from_slice(&e.name);
        out.extend_from_slice(&stored);
        p32(&mut central, 0x0201_4b50);
        p16(&mut central, (3 << 8) | 0x14);
        p16(&mut central, 20);
        p16(&mut central, flags);
        p16(&mut central, method);
        p16(&mut central, 0);
        p16(&mut central, 33);
        p32(&mut central, crc(&e.data));
        p32(&mut central, csize);
        p32(&mut central, size);
        p16(&mut central, nlen);
        p16(&mut central, 0);
        p16(&mut central, 0);
        p16(&mut central, 0);
        p16(&mut central, 0);
        p32(&mut central, e.mode << 16);
        p32(&mut central, offset);
        central.extend_from_slice(&e.name);
    }
    let cd_offset = u32::try_from(out.len()).unwrap();
    let cd_size = u32::try_from(central.len()).unwrap();
    out.extend_from_slice(&central);
    let n = u16::try_from(entries.len()).unwrap();
    p32(&mut out, 0x0605_4b50);
    p16(&mut out, 0);
    p16(&mut out, 0);
    p16(&mut out, n);
    p16(&mut out, n);
    p32(&mut out, cd_size);
    p32(&mut out, cd_offset);
    p16(&mut out, 0);
    out
}

// ---- tests ---------------------------------------------------------------------

#[test]
fn projects_and_files_are_made_saved_read_and_deleted_by_an_engineer() {
    let b = Bench::new();
    let s = b.alice();
    b.project(&s, "line3", &[("main.st", MAIN), ("blocks.st", BLOCKS)]);
    let list = get(b.addr, "/api/projects").json();
    assert_eq!(list["projects"][0]["name"], "line3", "{list}");
    let p = get(b.addr, "/api/projects/line3").json();
    let names: Vec<&str> = p["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["blocks.st", "main.st"]);
    let f = file(b.addr, "line3", "main.st").json();
    assert_eq!(f["text"], MAIN);
    let v = f["version"].as_str().unwrap().to_string();
    // A save names the version it read; a stale one is refused.
    let r = put_file(
        b.addr,
        &s,
        "line3",
        "main.st",
        "PROGRAM x END_PROGRAM",
        Some(Some(&v)),
    );
    assert_eq!(r.status, 200, "{r:?}");
    let r = put_file(
        b.addr,
        &s,
        "line3",
        "main.st",
        "PROGRAM y END_PROGRAM",
        Some(Some(&v)),
    );
    assert_eq!(r.status, 409, "{r:?}");
    let r = put_file(b.addr, &s, "line3", "main.st", "x", Some(None));
    assert_eq!(r.status, 409, "null version: only a new file: {r:?}");
    assert_eq!(
        file(b.addr, "line3", "main.st").json()["text"],
        "PROGRAM x END_PROGRAM"
    );
    // Copy, rename, delete a file, delete a project.
    let r = post_json(
        b.addr,
        "/api/projects",
        Some(&s),
        &json!({ "name": "copy", "from": "line3" }),
    );
    assert_eq!(r.status, 201, "{r:?}");
    let r = post_json(b.addr, "/api/projects", Some(&s), &json!({ "name": "copy" }));
    assert_eq!(r.status, 409, "{r:?}");
    let r = post_json(
        b.addr,
        "/api/projects/copy/rename",
        Some(&s),
        &json!({ "to": "line4" }),
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(get(b.addr, "/api/projects/copy").status, 404);
    assert_eq!(file(b.addr, "line4", "blocks.st").json()["text"], BLOCKS);
    let r = send(
        b.addr,
        "DELETE",
        "/api/projects/line4/files/blocks.st",
        Some(&s),
        "application/json",
        b"",
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(file(b.addr, "line4", "blocks.st").status, 404);
    let r = send(
        b.addr,
        "DELETE",
        "/api/projects/line4",
        Some(&s),
        "application/json",
        b"",
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(get(b.addr, "/api/projects/line4").status, 404);
    assert_eq!(
        tree(&b.projects_root()),
        ["line3", "line3/blocks.st", "line3/main.st"]
    );
    // Every write is audited with the person.
    let audit = get(b.addr, "/api/audit?limit=1000").json();
    let mine = audit
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["path"].as_str().unwrap().starts_with("/api/projects"))
        .collect::<Vec<_>>();
    assert!(mine.len() >= 10, "{audit}");
    assert!(mine.iter().all(|e| e["user"] == "alice"), "{audit}");
}

#[test]
fn file_contents_and_sizes_are_held_to_the_rules() {
    let b = Bench::new();
    let s = b.alice();
    b.project(&s, "p", &[]);
    let bad = [
        "a\u{0}b",
        "x := 1; \u{202E}// looks like a comment",
        "\u{1b}[31mred",
        "ok\u{2066}isolate",
    ];
    for text in bad {
        let r = put_file(b.addr, &s, "p", "a.st", text, None);
        assert_eq!(r.status, 422, "{text:?}: {r:?}");
    }
    // Not UTF-8, raw.
    let mut body = br#"{"text": ""#.to_vec();
    body.extend_from_slice(&[0xff, 0xfe]);
    body.extend_from_slice(br#""}"#);
    let r = send(
        b.addr,
        "PUT",
        "/api/projects/p/files/a.st",
        Some(&s),
        "application/json",
        &body,
    );
    assert_eq!(r.status, 400, "{r:?}");
    let big = "x".repeat(project::MAX_FILE + 1);
    let r = put_file(b.addr, &s, "p", "a.st", &big, None);
    assert!(r.status == 422 || r.status == 413, "{r:?}");
    // The project's total is capped across files.
    let chunk = "(* *)".repeat(project::MAX_FILE / 5 - 1);
    let mut refused = false;
    for k in 0..8 {
        let r = put_file(b.addr, &s, "p", &format!("f{k}.st"), &chunk, None);
        if r.status == 413 {
            refused = true;
            break;
        }
        assert_eq!(r.status, 200, "{r:?}");
    }
    assert!(refused, "the total cap held");
    // Case-only twins are refused.
    let r = put_file(b.addr, &s, "p", "F0.st", "x", None);
    assert_eq!(r.status, 409, "{r:?}");
    // Tabs, CRLF and non-ASCII text are fine.
    let r = put_file(b.addr, &s, "p", "f0.st", "(* Grüße *)\r\n\tx := 1;\r\n", None);
    assert_eq!(r.status, 200, "{r:?}");
}

#[test]
fn traversal_in_project_and_file_names_reaches_nothing() {
    let b = Bench::new();
    let s = b.alice();
    b.project(&s, "p", &[("main.st", MAIN)]);
    // A file planted beside the projects folder must stay unread and unwritten.
    std::fs::write(b.state.join("secret.st"), "SECRET").unwrap();
    let projects = [
        "..",
        "%2e%2e",
        "%2E%2E",
        ".",
        "..%2f..",
        "..%5c..",
        "a%2fb",
        "a%5cb",
        "C:",
        "c%3a",
        "%00",
        "p%00",
        ".hidden",
        "P",
        "UPPER",
        "a b",
        "%20p",
        "p%2f..%2f..%2fsecret.st",
        "%c0%ae%c0%ae",
    ];
    for name in projects {
        let r = get(b.addr, &format!("/api/projects/{name}"));
        assert!(r.status == 404 || r.status == 400, "{name}: {r:?}");
        let r = get(b.addr, &format!("/api/projects/{name}/files/secret.st"));
        assert!(r.status == 404 || r.status == 400, "{name}: {r:?}");
        let r = put_file(b.addr, &s, name, "x.st", "x", None);
        assert!(r.status == 404 || r.status == 400, "{name}: {r:?}");
        let r = post_json(b.addr, "/api/projects", Some(&s), &json!({ "name": name }));
        assert_eq!(r.status, 400, "{name}: {r:?}");
        let decoded = name.replace("%2e", ".").replace("%2f", "/").replace("%5c", "\\");
        let r = post_json(b.addr, "/api/projects", Some(&s), &json!({ "name": decoded }));
        assert_eq!(r.status, 400, "{decoded}: {r:?}");
    }
    let files = [
        "..%2fsecret.st",
        "%2e%2e%2f%2e%2e%2fsecret.st",
        "..%5c..%5csecret.st",
        "secret",
        "x.txt",
        "x.ST",
        ".x.st",
        "x.st%00.txt",
        "x.st.st",
        "a%2fb.st",
        "C%3a%5cx.st",
        "con.st%3a",
        "x%20.st",
        "-x.st",
        "_x.st",
        "%2e.st",
    ];
    for f in files {
        let r = get(b.addr, &format!("/api/projects/p/files/{f}"));
        assert!(r.status == 404 || r.status == 400, "{f}: {r:?}");
        let r = put_file(b.addr, &s, "p", f, "x", None);
        assert!(r.status == 400 || r.status == 404, "{f}: {r:?}");
        let r = send(
            b.addr,
            "DELETE",
            &format!("/api/projects/p/files/{f}"),
            Some(&s),
            "application/json",
            b"",
        );
        assert!(r.status == 404 || r.status == 400, "{f}: {r:?}");
    }
    let r = post_json(
        b.addr,
        "/api/projects/p/rename",
        Some(&s),
        &json!({ "to": "../escape" }),
    );
    assert_eq!(r.status, 400, "{r:?}");
    assert_eq!(
        std::fs::read_to_string(b.state.join("secret.st")).unwrap(),
        "SECRET"
    );
    assert_eq!(tree(&b.projects_root()), ["p", "p/main.st"]);
    let mut outside = tree(&b.state);
    outside.retain(|p| !p.starts_with("projects"));
    assert_eq!(outside, ["accounts", "secret.st"]);
}

#[cfg(unix)]
#[test]
fn links_planted_in_the_folder_are_not_followed() {
    let b = Bench::new();
    let s = b.alice();
    b.project(&s, "p", &[("main.st", MAIN)]);
    std::fs::write(b.state.join("secret.st"), "SECRET").unwrap();
    std::os::unix::fs::symlink(b.state.join("secret.st"), b.projects_root().join("p/leak.st")).unwrap();
    std::os::unix::fs::symlink(b.state.clone(), b.projects_root().join("linked")).unwrap();
    let r = file(b.addr, "p", "leak.st");
    assert_ne!(r.status, 200, "{r:?}");
    assert!(!String::from_utf8_lossy(&r.body).contains("SECRET"));
    assert_eq!(get(b.addr, "/api/projects/linked").status, 404);
    let r = get(b.addr, "/api/projects/p/zip");
    assert!(!String::from_utf8_lossy(&r.body).contains("SECRET"), "{r:?}");
}

#[test]
fn every_write_route_needs_an_engineer_session() {
    let b = Bench::new();
    let alice = b.alice();
    b.project(
        &alice,
        "p",
        &[("main.st", MAIN), ("blocks.st", BLOCKS), ("io.map", MAP)],
    );
    let zip = get(b.addr, "/api/projects/p/zip").body;
    let bob = b.login("bob", BOB);
    let expired = b.login("alice", ALICE);
    let routes: Vec<(&str, &str, &str, Vec<u8>)> = vec![
        (
            "POST",
            "/api/projects",
            "application/json",
            br#"{"name":"q"}"#.to_vec(),
        ),
        (
            "POST",
            "/api/projects",
            "application/json",
            br#"{"name":"q","from":"p"}"#.to_vec(),
        ),
        (
            "POST",
            "/api/projects/p/rename",
            "application/json",
            br#"{"to":"q"}"#.to_vec(),
        ),
        ("DELETE", "/api/projects/p", "application/json", Vec::new()),
        (
            "PUT",
            "/api/projects/p/files/main.st",
            "application/json",
            br#"{"text":"x"}"#.to_vec(),
        ),
        (
            "PUT",
            "/api/projects/p/files/new.st",
            "application/json",
            br#"{"text":"x"}"#.to_vec(),
        ),
        (
            "DELETE",
            "/api/projects/p/files/main.st",
            "application/json",
            Vec::new(),
        ),
        (
            "POST",
            "/api/projects/p/zip?replace=true",
            "application/zip",
            zip.clone(),
        ),
        ("POST", "/api/projects/q/zip", "application/zip", zip),
        (
            "POST",
            "/api/projects/p/import?file=x.st",
            "text/plain",
            b"PROGRAM x END_PROGRAM".to_vec(),
        ),
        (
            "POST",
            "/api/projects/p/replay",
            "application/json",
            b"{}".to_vec(),
        ),
        (
            "POST",
            "/api/projects/p/deploy",
            "application/json",
            b"{}".to_vec(),
        ),
    ];
    // Expire one engineer session by idling past the limit (the other
    // sessions are made after, so they stay live).
    b.clock
        .fetch_add(account::SESSION_IDLE.as_secs(), Ordering::SeqCst);
    let bob = {
        let _ = bob;
        b.login("bob", BOB)
    };
    let head = b.head();
    let before = tree(&b.projects_root());
    for (method, path, ctype, body) in &routes {
        let r = send(b.addr, method, path, None, ctype, body);
        assert_eq!(r.status, 401, "{method} {path} without a credential: {r:?}");
        let r = send(b.addr, method, path, Some(TOKEN), ctype, body);
        assert_eq!(r.status, 403, "{method} {path} with the box token: {r:?}");
        assert!(r.error().contains("engineer"), "{r:?}");
        let r = send(b.addr, method, path, Some(&bob), ctype, body);
        assert_eq!(r.status, 403, "{method} {path} as an operator: {r:?}");
        let r = send(b.addr, method, path, Some(&expired), ctype, body);
        assert_eq!(r.status, 401, "{method} {path} with an expired session: {r:?}");
        let r = send(b.addr, method, path, Some("f".repeat(64).as_str()), ctype, body);
        assert_eq!(r.status, 401, "{method} {path} with a made-up session: {r:?}");
    }
    assert_eq!(tree(&b.projects_root()), before, "nothing changed");
    assert_eq!(b.head(), head);
    // Reads stay open; check takes the token.
    assert_eq!(get(b.addr, "/api/projects/p/files/main.st").status, 200);
    let r = post_json(b.addr, "/api/projects/p/check", Some(TOKEN), &json!({}));
    assert_eq!(r.status, 200, "{r:?}");
    // An integrator is above an engineer.
    let carol = b.login("carol", CAROL);
    let r = put_file(b.addr, &carol, "p", "new.st", "(* *)", None);
    assert_eq!(r.status, 200, "{r:?}");
    // The refusals are audited.
    let audit = get(b.addr, "/api/audit?limit=1000").json();
    let refused = audit
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["status"] == 403 && e["path"].as_str().unwrap().starts_with("/api/projects"))
        .count();
    assert_eq!(refused, routes.len() * 2, "{audit}");
}

#[test]
fn download_then_upload_round_trips_byte_for_byte() {
    let b = Bench::new();
    let s = b.alice();
    let files = [
        ("main.st", MAIN),
        ("blocks.st", BLOCKS),
        ("counter.st", COUNTER),
        ("io.map", MAP),
        ("project.settings", SETTINGS),
        (
            "crlf.st",
            "(* Grüße *)\r\n\tFUNCTION f : INT\r\n f := 1;\r\nEND_FUNCTION\r\n",
        ),
    ];
    b.project(&s, "orig", &files);
    let r = get(b.addr, "/api/projects/orig/zip");
    assert_eq!(r.status, 200, "{r:?}");
    assert!(r.head.contains("Content-Type: application/zip"), "{}", r.head);
    assert!(r.head.contains("filename=\"orig.zip\""), "{}", r.head);
    let zip = r.body;
    let r = upload(b.addr, &s, "copy", &zip, false);
    assert_eq!(r.status, 201, "{r:?}");
    for (name, text) in files {
        let f = file(b.addr, "copy", name).json();
        assert_eq!(f["text"], text, "{name}");
        let on_disk = std::fs::read(b.projects_root().join("copy").join(name)).unwrap();
        assert_eq!(on_disk, text.as_bytes(), "{name}");
    }
    assert_eq!(get(b.addr, "/api/projects/copy/zip").body, zip, "byte-identical");
    // An existing project is replaced only when asked.
    let r = upload(b.addr, &s, "copy", &zip, false);
    assert_eq!(r.status, 409, "{r:?}");
    put_file(b.addr, &s, "copy", "extra.st", "(* *)", None);
    let r = upload(b.addr, &s, "copy", &zip, true);
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(file(b.addr, "copy", "extra.st").status, 404, "replaced whole");
    // A desktop zipper's archive: deflated, in a folder, with Mac litter.
    let raw = [
        Raw {
            deflate: true,
            ..Raw::file("line3/main.st", MAIN.as_bytes())
        },
        Raw {
            mode: 0o040_755,
            ..Raw::file("line3/", b"")
        },
        Raw::file("line3/io.map", MAP.as_bytes()),
        Raw::file("__MACOSX/line3/._main.st", b"\x00\x05\x16\x07"),
        Raw::file("line3/.DS_Store", b"\x00\x00\x00\x01Bud1"),
    ];
    let r = upload(b.addr, &s, "desk", &zip_raw(&raw), false);
    assert_eq!(r.status, 201, "{r:?}");
    assert_eq!(tree(&b.projects_root().join("desk")), ["io.map", "main.st"]);
    assert_eq!(file(b.addr, "desk", "main.st").json()["text"], MAIN);
}

#[test]
fn hostile_archives_are_refused_and_write_nothing() {
    let b = Bench::new();
    let s = b.alice();
    let st = b"PROGRAM p END_PROGRAM";
    let symlink = Raw {
        mode: 0o120_777,
        ..Raw::file("link.st", b"/etc/passwd")
    };
    let mut cases: Vec<(&str, Vec<u8>)> = vec![
        ("slip", zip_raw(&[Raw::file("../evil.st", st)])),
        ("slip deep", zip_raw(&[Raw::file("a/../../evil.st", st)])),
        (
            "slip in folder",
            zip_raw(&[Raw::file("p/../../evil.st", st), Raw::file("p/x.st", st)]),
        ),
        ("absolute", zip_raw(&[Raw::file("/tmp/evil.st", st)])),
        ("windows", zip_raw(&[Raw::file("..\\..\\evil.st", st)])),
        ("drive", zip_raw(&[Raw::file("C:\\evil.st", st)])),
        ("drive relative", zip_raw(&[Raw::file("C:evil.st", st)])),
        (
            "nested",
            zip_raw(&[Raw::file("a/b/x.st", st), Raw::file("a/c.st", st)]),
        ),
        ("symlink", zip_raw(&[symlink])),
        ("not a project file", zip_raw(&[Raw::file("run.sh", b"rm -rf /")])),
        ("dot file", zip_raw(&[Raw::file(".evil.st", st)])),
        (
            "duplicate",
            zip_raw(&[Raw::file("a.st", st), Raw::file("a.st", b"(* other *)")]),
        ),
        (
            "case twins",
            zip_raw(&[Raw::file("a.st", st), Raw::file("A.st", st)]),
        ),
        (
            "non-UTF-8 name",
            zip_raw(&[Raw {
                name: b"caf\xe9.st".to_vec(),
                utf8: false,
                ..Raw::file("x", st)
            }]),
        ),
        (
            "non-UTF-8 content",
            zip_raw(&[Raw::file("a.st", b"(* \xff\xfe *)")]),
        ),
        ("NUL in content", zip_raw(&[Raw::file("a.st", b"x\x00y")])),
        ("not a zip", b"PK\x03\x04 this is not an archive".to_vec()),
        ("empty body", Vec::new()),
    ];
    // Bombs: a deflate stream far past its declared size, declared sizes
    // past the cap, too many entries.
    let zeros = vec![0u8; 8 << 20];
    cases.push((
        "bomb lying about its size",
        zip_raw(&[Raw {
            deflate: true,
            size: Some(100),
            ..Raw::file("a.st", &zeros)
        }]),
    ));
    cases.push((
        "bomb declaring its size",
        zip_raw(&[Raw {
            deflate: true,
            ..Raw::file("a.st", &zeros)
        }]),
    ));
    let many: Vec<Raw> = (0..=project::zip::MAX_ENTRIES)
        .map(|k| Raw::file(&format!("f{k}.st"), b"(* *)"))
        .collect();
    cases.push(("too many entries", zip_raw(&many)));
    let files: Vec<Raw> = (0..=project::MAX_FILES)
        .map(|k| Raw::file(&format!("f{k}.st"), b"(* *)"))
        .collect();
    cases.push(("too many files", zip_raw(&files)));
    for (what, zip) in &cases {
        let r = upload(b.addr, &s, "victim", zip, false);
        assert!(r.status == 422 || r.status == 413, "{what}: {r:?}");
        assert_eq!(get(b.addr, "/api/projects/victim").status, 404, "{what}");
    }
    // A project that exists is left whole when a hostile replacement fails.
    b.project(&s, "kept", &[("main.st", MAIN)]);
    for (what, zip) in &cases {
        let r = upload(b.addr, &s, "kept", zip, true);
        assert!(r.status == 422 || r.status == 413, "{what}: {r:?}");
    }
    assert_eq!(tree(&b.projects_root()), ["kept", "kept/main.st"]);
    let mut outside = tree(&b.state);
    outside.retain(|p| !p.starts_with("projects"));
    assert_eq!(outside, ["accounts"], "nothing escaped the projects folder");
    assert!(!PathBuf::from("/tmp/evil.st").exists());
    // Not application/zip: refused before reading.
    let r = send(
        b.addr,
        "POST",
        "/api/projects/x/zip",
        Some(&s),
        "text/plain",
        b"x",
    );
    assert_eq!(r.status, 415, "{r:?}");
    // The body cap: past 2 MiB is refused from the head.
    let huge = vec![b'P'; (2 << 20) + 1];
    let r = upload(b.addr, &s, "huge", &huge, false);
    assert_eq!(r.status, 413, "{r:?}");
}

#[test]
fn a_project_deploys_through_the_approval_path_and_records_what_runs() {
    let b = Bench::new();
    let s = b.alice();
    b.project(
        &s,
        "line3",
        &[
            ("main.st", MAIN),
            ("blocks.st", BLOCKS),
            ("counter.st", COUNTER),
            ("io.map", MAP),
            ("project.settings", SETTINGS),
        ],
    );
    let before = b.head();
    let check = post_json(b.addr, "/api/projects/line3/check", Some(&s), &json!({})).json();
    assert_eq!(check["ok"], true, "{check}");
    let mods: Vec<&str> = check["modules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    assert_eq!(mods, ["conveyor", "counter"], "{check}");
    assert_eq!(check["modules"][0]["drives"], json!(["lamp"]), "{check}");
    assert_eq!(check["modules"][0]["file"], "main.st", "{check}");
    assert_eq!(check["approval"]["tier"], "logic", "{check}");
    // The floor wants a replay for a logic change.
    let r = post_json(b.addr, "/api/projects/line3/deploy", Some(&s), &json!({}));
    assert_eq!(r.status, 428, "{r:?}");
    assert_eq!(b.head(), before);
    let r = b.deploy(&s, "line3");
    assert_eq!(r.status, 200, "{r:?}");
    let j = r.json();
    assert_eq!(j["project"]["name"], "line3", "{j}");
    assert_eq!(j["user"], "alice", "{j}");
    assert_eq!(j["snapshot"], check["snapshot"], "{j}");
    assert_eq!(b.head(), check["snapshot"].as_str().unwrap());
    wait("both modules running", || b.modules() == ["conveyor", "counter"]);
    let running = get(b.addr, "/api/running").json();
    assert_eq!(running["project"], "line3", "{running}");
    assert_eq!(running["current"], true, "{running}");
    assert_eq!(
        get(b.addr, "/api/projects").json()["projects"][0]["running"],
        true
    );
    // The store's history names the project and the person.
    let log = get(b.addr, "/api/snapshots").json();
    let top = &log["log"][0];
    assert!(top["author"].as_str().unwrap().contains("line3"), "{log}");
    assert!(top["approver"].as_str().unwrap().contains("alice"), "{log}");
    // The running project cannot be deleted.
    let r = send(
        b.addr,
        "DELETE",
        "/api/projects/line3",
        Some(&s),
        "application/json",
        b"",
    );
    assert_eq!(r.status, 409, "{r:?}");
    // Editing it is a draft: what runs does not move.
    put_file(
        b.addr,
        &s,
        "line3",
        "counter.st",
        &COUNTER.replace("n + 1", "n + 2"),
        None,
    );
    assert_eq!(b.head(), check["snapshot"].as_str().unwrap());
    // Renaming it carries the record.
    let r = post_json(
        b.addr,
        "/api/projects/line3/rename",
        Some(&s),
        &json!({ "to": "line3b" }),
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(get(b.addr, "/api/running").json()["project"], "line3b");
    // Rollback to what ran before, through the approval path: the record
    // stays, and says it is no longer what runs.
    let n = post_json(
        b.addr,
        &format!("/api/snapshots/{before}/replay"),
        Some(&s),
        &json!({}),
    )
    .json()["replay"]
        .as_u64()
        .unwrap();
    let r = post_json(
        b.addr,
        &format!("/api/snapshots/{before}/rollback"),
        Some(&s),
        &json!({ "replay": n }),
    );
    assert_eq!(r.status, 200, "{r:?}");
    wait("the old module set", || b.modules() == ["conveyor"]);
    assert_eq!(b.head(), before);
    let running = get(b.addr, "/api/running").json();
    assert_eq!(
        (running["project"].as_str(), running["current"].as_bool()),
        (Some("line3b"), Some(false))
    );
}

#[test]
fn a_project_that_does_not_build_names_file_and_line_and_changes_nothing() {
    let b = Bench::new();
    let s = b.alice();
    let broken = "\
PROGRAM counter
VAR n : UDINT; END_VAR
  n := n + undefined_thing;
END_PROGRAM
";
    b.project(
        &s,
        "bad",
        &[
            ("main.st", MAIN),
            ("blocks.st", BLOCKS),
            ("z.st", broken),
            ("io.map", MAP),
        ],
    );
    let head = b.head();
    for path in ["check", "deploy", "replay"] {
        let r = post_json(b.addr, &format!("/api/projects/bad/{path}"), Some(&s), &json!({}));
        assert_eq!(r.status, 422, "{path}: {r:?}");
        let d = &r.json()["diagnostics"];
        assert_eq!(d[0]["file"], "z.st", "{d}");
        assert_eq!(d[0]["line"], 3, "{d}");
        assert!(
            d[0]["message"].as_str().unwrap().contains("UNDEFINED_THING"),
            "{d}"
        );
    }
    assert_eq!(b.head(), head);
    assert_eq!(b.modules(), ["conveyor"]);
    // A syntax error, the FB defined twice, two PROGRAMs of one name, an
    // empty project, a bad settings line: each pointed at.
    let cases: [(&[(&str, &str)], &str, u32); 5] = [
        (
            &[("a.st", "PROGRAM a\nVAR x : INT END_VAR\nEND_PROGRAM\n")],
            "a.st",
            2,
        ),
        (&[("a.st", BLOCKS), ("b.st", BLOCKS), ("c.st", MAIN)], "b.st", 1),
        (
            &[("a.st", COUNTER), ("b.st", &format!("\n\n{COUNTER}"))],
            "b.st",
            3,
        ),
        (&[("a.st", BLOCKS)], "", 0),
        (
            &[
                ("a.st", COUNTER),
                ("project.settings", "\n\nprogram counter scan_ms=0\n"),
            ],
            "project.settings",
            3,
        ),
    ];
    for (k, (files, at, line)) in cases.iter().enumerate() {
        let name = format!("case{k}");
        b.project(&s, &name, files);
        let r = post_json(
            b.addr,
            &format!("/api/projects/{name}/check"),
            Some(&s),
            &json!({}),
        );
        assert_eq!(r.status, 422, "{name}: {r:?}");
        let d = &r.json()["diagnostics"];
        let hit = d
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["file"] == *at && x["line"] == *line);
        assert!(hit, "{name}: want {at}:{line}, got {d}");
    }
    assert_eq!(b.head(), head);
}

#[test]
fn concurrent_saves_never_tear_a_file() {
    let b = Arc::new(Bench::new());
    let s = b.alice();
    b.project(&s, "p", &[("main.st", MAIN)]);
    let texts: Vec<String> = (0..8)
        .map(|k| format!("(* writer {k} *)\n{}", "x".repeat(20_000 + k)))
        .collect();
    let handles: Vec<_> = texts
        .iter()
        .cloned()
        .map(|t| {
            let (b, s) = (Arc::clone(&b), s.clone());
            thread::spawn(move || {
                for _ in 0..5 {
                    let r = put_file(b.addr, &s, "p", "main.st", &t, None);
                    assert_eq!(r.status, 200, "{r:?}");
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let f = file(b.addr, "p", "main.st").json();
    let text = f["text"].as_str().unwrap();
    assert!(texts.iter().any(|t| t == text), "a whole save, not a mix");
    assert_eq!(f["version"], project::version(text.as_bytes()));
    assert_eq!(
        tree(&b.projects_root()),
        ["p", "p/main.st"],
        "no temporaries left"
    );
    // Racing creates of one name: exactly one wins.
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let (b, s) = (Arc::clone(&b), s.clone());
            thread::spawn(move || {
                post_json(b.addr, "/api/projects", Some(&s), &json!({ "name": "race" })).status
            })
        })
        .collect();
    let mut codes: Vec<u16> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    codes.sort_unstable();
    assert_eq!(codes, [201, 409, 409, 409, 409, 409, 409, 409]);
    // Optimistic saves from one version: exactly one lands.
    let v = file(b.addr, "p", "main.st").json()["version"]
        .as_str()
        .unwrap()
        .to_string();
    let handles: Vec<_> = (0..6)
        .map(|k| {
            let (b, s, v) = (Arc::clone(&b), s.clone(), v.clone());
            thread::spawn(move || {
                put_file(b.addr, &s, "p", "main.st", &format!("(* {k} *)"), Some(Some(&v))).status
            })
        })
        .collect();
    let mut codes: Vec<u16> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    codes.sort_unstable();
    assert_eq!(codes, [200, 409, 409, 409, 409, 409]);
}

#[test]
fn a_deploy_racing_saves_runs_one_whole_version() {
    let b = Arc::new(Bench::new());
    let s = b.alice();
    let v1 = COUNTER.to_string();
    let v2 = COUNTER.replace("n + 1", "n + 2");
    b.project(
        &s,
        "p",
        &[
            ("main.st", MAIN),
            ("blocks.st", BLOCKS),
            ("counter.st", &v1),
            ("io.map", MAP),
        ],
    );
    let shape = Shape {
        defaults: defaults(),
        i_len: 32,
        q_len: 32,
    };
    let id_of = |counter: &str| {
        let files = Files(BTreeMap::from([
            ("main.st".to_string(), MAIN.to_string()),
            ("blocks.st".to_string(), BLOCKS.to_string()),
            ("counter.st".to_string(), counter.to_string()),
            ("io.map".to_string(), MAP.to_string()),
        ]));
        build::build("p", &files, Some(&running()), &shape)
            .unwrap()
            .change
            .id()
            .to_string()
    };
    let ids = [id_of(&v1), id_of(&v2)];
    let initial = b.head();
    let stop = Arc::new(AtomicBool::new(false));
    let saver = {
        let (b, s, stop) = (Arc::clone(&b), s.clone(), Arc::clone(&stop));
        let (v1, v2) = (v1.clone(), v2.clone());
        thread::spawn(move || {
            let mut k = 0usize;
            while !stop.load(Ordering::Relaxed) {
                let t = if k.is_multiple_of(2) { &v2 } else { &v1 };
                assert_eq!(put_file(b.addr, &s, "p", "counter.st", t, None).status, 200);
                k += 1;
            }
        })
    };
    let mut landed = 0;
    for _ in 0..6 {
        let r = b.deploy(&s, "p");
        // A save between replay and deploy makes the evidence another
        // content's: refused, nothing changed. Otherwise one whole version.
        assert!(r.status == 200 || r.status == 428, "{r:?}");
        let head = b.head();
        assert!(ids.contains(&head) || head == initial, "torn: {head}");
        if r.status == 200 {
            landed += 1;
            assert!(ids.contains(&r.json()["snapshot"].as_str().unwrap().to_string()));
        }
    }
    stop.store(true, Ordering::Relaxed);
    saver.join().unwrap();
    // Quiet now: a deploy lands.
    if landed == 0 {
        let r = b.deploy(&s, "p");
        assert_eq!(r.status, 200, "{r:?}");
    }
    assert!(ids.contains(&b.head()));
}

#[test]
fn plcopen_and_st_files_import_into_a_project_and_build() {
    let b = Bench::new();
    let s = b.alice();
    b.project(&s, "imp", &[]);
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../helmsman-iec/tests/plcopen");
    let xml = std::fs::read(fixtures.join("motor.xml")).unwrap();
    let r = send(
        b.addr,
        "POST",
        "/api/projects/imp/import?file=motor.xml",
        Some(&s),
        "application/xml",
        &xml,
    );
    assert_eq!(r.status, 201, "{r:?}");
    assert_eq!(r.json()["name"], "motor.st");
    let st = file(b.addr, "imp", "motor.st").json()["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(st.contains("PROGRAM"), "{st}");
    let r = post_json(b.addr, "/api/projects/imp/check", Some(&s), &json!({}));
    assert_eq!(r.status, 200, "the lowered ST builds: {r:?}");
    // Again without replace: it exists.
    let r = send(
        b.addr,
        "POST",
        "/api/projects/imp/import?file=motor.xml",
        Some(&s),
        "application/xml",
        &xml,
    );
    assert_eq!(r.status, 409, "{r:?}");
    let r = send(
        b.addr,
        "POST",
        "/api/projects/imp/import?file=motor.xml&replace=true",
        Some(&s),
        "application/xml",
        &xml,
    );
    assert_eq!(r.status, 201, "{r:?}");
    // Every PLCopen fixture lowers to ST that builds as a project.
    for entry in std::fs::read_dir(&fixtures).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_stem().unwrap().to_str().unwrap().replace('-', "_");
        let pname = format!("x{}", name.to_ascii_lowercase().replace('_', "-"));
        b.project(&s, &pname, &[]);
        let r = send(
            b.addr,
            "POST",
            &format!("/api/projects/{pname}/import?file={name}.xml"),
            Some(&s),
            "application/xml",
            &std::fs::read(&path).unwrap(),
        );
        assert_eq!(r.status, 201, "{}: {r:?}", path.display());
        let r = post_json(
            b.addr,
            &format!("/api/projects/{pname}/check"),
            Some(&s),
            &json!({}),
        );
        assert_eq!(r.status, 200, "{}: {r:?}", path.display());
    }
    // ST imports as it is; hostile or unknown files are refused.
    let r = send(
        b.addr,
        "POST",
        "/api/projects/imp/import?file=extra.st",
        Some(&s),
        "text/plain",
        COUNTER.as_bytes(),
    );
    assert_eq!(r.status, 201, "{r:?}");
    assert_eq!(file(b.addr, "imp", "extra.st").json()["text"], COUNTER);
    let l5x = std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../helmsman-iec/tests/fixtures/l5x/line3.L5X"),
    )
    .unwrap();
    let r = send(
        b.addr,
        "POST",
        "/api/projects/imp/import?file=line3.L5X",
        Some(&s),
        "application/xml",
        &l5x,
    );
    assert_eq!(r.status, 415, "L5X conversion is not built: {r:?}");
    for bad in ["..%2fx.st", "x.exe", "noext", "%2e%2e.st"] {
        let r = send(
            b.addr,
            "POST",
            &format!("/api/projects/imp/import?file={bad}"),
            Some(&s),
            "text/plain",
            b"x",
        );
        assert!(r.status == 400 || r.status == 415, "{bad}: {r:?}");
    }
    let r = send(
        b.addr,
        "POST",
        "/api/projects/imp/import?file=y.xml",
        Some(&s),
        "application/xml",
        b"<!DOCTYPE x [<!ENTITY a \"b\">]><project/>",
    );
    assert_eq!(r.status, 422, "{r:?}");
}

// ---- the binary ------------------------------------------------------------------

struct Plc {
    child: Child,
    addr: SocketAddr,
}

impl Drop for Plc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn plc_projects(state: &Path) -> Plc {
    let mut child = Command::new(env!("CARGO_BIN_EXE_helmsman-plc"))
        .args(["--opcua", "off", "--projects"])
        .args(["--bind", "127.0.0.1", "--encap-port", "0", "--io-port", "0"])
        .args(["--api", "127.0.0.1:0", "--state-dir"])
        .arg(state)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut addr = None;
    while addr.is_none() {
        let mut line = String::new();
        assert!(stderr.read_line(&mut line).unwrap() > 0, "PLC exited");
        if let Some(rest) = line.strip_prefix("helmsman-plc: API on http://") {
            addr = rest.split('/').next().and_then(|a| a.parse().ok());
        }
    }
    // Keep draining so the child never blocks on a full pipe.
    thread::spawn(move || {
        let mut sink = String::new();
        let _ = stderr.read_to_string(&mut sink);
    });
    Plc {
        child,
        addr: addr.unwrap(),
    }
}

#[test]
fn the_box_boots_the_deployed_project_or_nothing_before_one() {
    let state = tmp("boot");
    // Before any deploy: up, running nothing, waiting for one.
    let plc = plc_projects(&state);
    let r = get(plc.addr, "/api/running").json();
    assert_eq!(r["project"], Json::Null, "{r}");
    assert_eq!(get(plc.addr, "/api/modules").json(), json!([]));
    assert_eq!(get(plc.addr, "/api/projects").json()["projects"], json!([]));
    drop(plc);
    // A deployed project: in the store, recorded as running.
    let files = Files(BTreeMap::from([
        ("main.st".to_string(), MAIN.to_string()),
        ("blocks.st".to_string(), BLOCKS.to_string()),
        ("io.map".to_string(), MAP.to_string()),
    ]));
    let shape = Shape {
        defaults: Defaults {
            period: Duration::from_millis(10),
            i: Window::new(0, 32),
            q: Window::new(0, 32),
            m: 1024,
        },
        i_len: 32,
        q_len: 32,
    };
    let built = build::build("line3", &files, None, &shape).unwrap();
    let id = Store::open(&state.join("store"))
        .unwrap()
        .commit(&built.change, &meta())
        .unwrap();
    let projects = Projects::in_state_dir(&state).unwrap();
    projects.install("line3", &files, false).unwrap();
    projects
        .record(&project::Running {
            name: "line3".into(),
            snapshot: id.to_string(),
        })
        .unwrap();
    let plc = plc_projects(&state);
    let r = get(plc.addr, "/api/running").json();
    assert_eq!(
        (r["project"].as_str(), r["current"].as_bool()),
        (Some("line3"), Some(true)),
        "{r}"
    );
    wait("the conveyor running", || {
        get(plc.addr, "/api/modules").json()[0]["name"] == "conveyor"
    });
    let r = get(plc.addr, "/api/modules/conveyor").json();
    assert!(
        r["source"].as_str().unwrap().contains("(* file blocks.st *)"),
        "{r}"
    );
}

#[test]
fn projects_takes_no_other_module_source() {
    let out = Command::new(env!("CARGO_BIN_EXE_helmsman-plc"))
        .args(["--opcua", "off", "--projects", "x.st"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--projects"));
}

// ---- forcing ---------------------------------------------------------------------

fn force(addr: SocketAddr, s: &str, body: &Json) -> Resp {
    post_json(addr, "/api/forces", Some(s), body)
}

fn release(addr: SocketAddr, s: &str, path: &str) -> Resp {
    send(addr, "DELETE", path, Some(s), "application/json", b"")
}

impl Bench {
    /// Waits `scans` more scans, asserting `ok` at every look.
    fn holds(&self, scans: u64, what: &str, mut ok: impl FnMut() -> bool) {
        let from = self.ticks.load(Ordering::Relaxed);
        wait(what, || {
            assert!(ok(), "{what} broke");
            self.ticks.load(Ordering::Relaxed) > from + scans
        });
    }

    fn audited(&self, needle: &str) -> bool {
        get(self.addr, "/api/audit?limit=1000")
            .json()
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["detail"].as_str().unwrap_or_default().contains(needle))
    }
}

#[test]
fn a_force_holds_across_scans_until_released() {
    let b = Bench::new();
    let s = b.alice();
    wait("the lamp on", || b.cmd("lamp") == Some(Value::Bool(true)));
    let r = force(b.addr, &s, &json!({ "signal": "start", "value": false }));
    assert_eq!(r.status, 201, "{r:?}");
    let id = r.json()["id"].as_u64().unwrap();
    wait("the lamp off", || b.cmd("lamp") == Some(Value::Bool(false)));
    // The I/O read sets start every scan; the force wins every scan.
    b.holds(50, "the lamp held off", || {
        b.cmd("lamp") == Some(Value::Bool(false))
    });
    // Reads mark it.
    let start = get(b.addr, "/api/signals/start").json();
    assert_eq!(
        (start["forced"].as_bool(), start["value"].as_bool()),
        (Some(true), Some(false)),
        "{start}"
    );
    let lamp = get(b.addr, "/api/signals/lamp").json();
    assert_eq!(lamp["forced"], false, "{lamp}");
    assert_eq!(b.forces()[0]["signal"], "start");
    assert_eq!(b.forces()[0]["by"], "alice (engineer)");
    let mut trace = get(b.addr, "/api/modules/conveyor/trace");
    wait("a trace", || {
        trace = get(b.addr, "/api/modules/conveyor/trace");
        trace.status == 200
    });
    assert_eq!(trace.json()["forces"][0]["signal"], "start", "{trace:?}");
    // An address in the input image, after the read, before the scan.
    let r = force(b.addr, &s, &json!({ "address": "%IX0.3", "value": true }));
    assert_eq!(r.status, 201, "{r:?}");
    let r = force(b.addr, &s, &json!({ "address": "%IW2", "value": 4660 }));
    assert_eq!(r.status, 201, "{r:?}");
    wait("the image forced", || {
        let img = b.image.lock().unwrap_or_else(PoisonError::into_inner).clone();
        img[0] == 0b1000 && img[2..4] == [0x34, 0x12]
    });
    // Forcing the same point again replaces it.
    let r = force(b.addr, &s, &json!({ "address": "%ix0.3", "value": false }));
    assert_eq!(r.status, 201, "{r:?}");
    assert_eq!(b.forces().len(), 3);
    // What is refused.
    let refused = [
        (json!({ "signal": "lamp", "value": true }), 409),
        (json!({ "signal": "nope", "value": true }), 404),
        (json!({ "signal": "start", "value": 5 }), 422),
        (json!({ "signal": "start" }), 400),
        (json!({ "signal": "start", "value": "true" }), 400),
        (
            json!({ "signal": "start", "address": "%IX0.0", "value": true }),
            400,
        ),
        (json!({ "address": "%QX0.0", "value": true }), 422),
        (json!({ "address": "%IX32.0", "value": true }), 422),
        (json!({ "address": "%IX0.8", "value": true }), 422),
        (json!({ "address": "%IB0", "value": 256 }), 422),
        (json!({ "address": "%IB0", "value": -1 }), 422),
        (json!({ "address": "%IX0.0", "value": 1 }), 422),
        (json!({ "address": "../%IX0.0", "value": true }), 422),
        (json!({ "value": true }), 400),
        (json!({ "signal": "start", "value": true, "extra": 1 }), 400),
    ];
    for (body, status) in refused {
        let r = force(b.addr, &s, &body);
        assert_eq!(r.status, status, "{body}: {r:?}");
    }
    assert_eq!(b.forces().len(), 3);
    // Release one: the lamp follows the read again.
    let r = release(b.addr, &s, &format!("/api/forces/{id}"));
    assert_eq!(r.status, 200, "{r:?}");
    wait("the lamp on", || b.cmd("lamp") == Some(Value::Bool(true)));
    assert_eq!(get(b.addr, "/api/signals/start").json()["forced"], false);
    assert_eq!(release(b.addr, &s, &format!("/api/forces/{id}")).status, 404);
    assert_eq!(release(b.addr, &s, "/api/forces/x1").status, 404);
    // Release all.
    let r = release(b.addr, &s, "/api/forces");
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(b.forces(), Vec::<Json>::new());
    wait("the image free", || {
        b.image
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .all(|x| *x == 0)
    });
    assert!(b.audited(&format!("force {id}: signal start := false")));
    assert!(b.audited(&format!("force {id} released: signal start")));
    assert!(b.audited("forces released: all (2)"));
}

#[test]
fn forcing_needs_an_engineer_session() {
    let b = Bench::new();
    let bob = b.login("bob", BOB);
    let body = json!({ "signal": "start", "value": false }).to_string();
    for (method, path) in [
        ("POST", "/api/forces"),
        ("DELETE", "/api/forces"),
        ("DELETE", "/api/forces/1"),
    ] {
        let r = send(b.addr, method, path, None, "application/json", body.as_bytes());
        assert_eq!(r.status, 401, "{method} {path}: {r:?}");
        let r = send(
            b.addr,
            method,
            path,
            Some(TOKEN),
            "application/json",
            body.as_bytes(),
        );
        assert_eq!(r.status, 403, "{method} {path}: {r:?}");
        let r = send(
            b.addr,
            method,
            path,
            Some(&bob),
            "application/json",
            body.as_bytes(),
        );
        assert_eq!(r.status, 403, "{method} {path}: {r:?}");
    }
    assert_eq!(b.forces(), Vec::<Json>::new());
    b.holds(20, "the lamp on", || b.cmd("lamp") == Some(Value::Bool(true)));
}

#[test]
fn forces_end_on_logout_on_expiry_and_on_deploy() {
    let b = Bench::new();
    let a = b.alice();
    let carol = b.login("carol", CAROL);
    let r = force(b.addr, &a, &json!({ "signal": "start", "value": false }));
    assert_eq!(r.status, 201, "{r:?}");
    let r = force(b.addr, &carol, &json!({ "address": "%IX0.1", "value": true }));
    assert_eq!(r.status, 201, "{r:?}");
    wait("the lamp off", || b.cmd("lamp") == Some(Value::Bool(false)));
    // Alice logs out: her force ends with her session, Carol's stays.
    let r = post_json(b.addr, "/api/logout", Some(&a), &json!({}));
    assert_eq!(r.status, 200, "{r:?}");
    let left = b.forces();
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(left[0]["address"], "%IX0.1");
    wait("the lamp on", || b.cmd("lamp") == Some(Value::Bool(true)));
    assert!(b.audited("released by the box (its session ended): signal start"));
    // Idle past the limit: the sweep ends Carol's.
    b.clock
        .fetch_add(account::SESSION_IDLE.as_secs(), Ordering::SeqCst);
    wait("the expired session's force gone", || b.forces().is_empty());
    assert!(b.audited("released by the box (its session ended): %IX0.1"));
    // A deploy ends every force.
    let a = b.alice();
    b.project(
        &a,
        "p",
        &[("main.st", MAIN), ("blocks.st", BLOCKS), ("io.map", MAP)],
    );
    let r = force(b.addr, &a, &json!({ "signal": "start", "value": false }));
    assert_eq!(r.status, 201, "{r:?}");
    let r = b.deploy(&a, "p");
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(b.forces(), Vec::<Json>::new());
    assert!(b.audited("released by the box (a project was deployed): signal start"));
    wait("the lamp on under the new program", || {
        b.cmd("lamp") == Some(Value::Bool(true))
    });
}

fn seed(state: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_helmsman-plc"))
        .args(["project", "seed", "--state-dir"])
        .arg(state)
        .args(["--scan-ms", "10"])
        .args(args)
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

#[test]
fn a_first_install_seeds_and_deploys_a_project_and_never_again() {
    let state = tmp("seed");
    let line = concat!(env!("CARGO_MANIFEST_DIR"), "/programs/line.st");
    // Nothing named: nothing done (the program-mode unit runs it so).
    let (ok, out) = seed(&state, &[]);
    assert!(ok && out.contains("no project to seed"), "{out}");
    // A first install: made, deployed, recorded.
    let (ok, out) = seed(&state, &["line", line]);
    assert!(ok && out.contains("project line made and deployed"), "{out}");
    let plc = plc_projects(&state);
    let r = get(plc.addr, "/api/running").json();
    assert_eq!(
        (r["project"].as_str(), r["current"].as_bool()),
        (Some("line"), Some(true)),
        "{r}"
    );
    let modules = get(plc.addr, "/api/modules").json();
    assert_eq!(modules[0]["name"], "line", "{modules}");
    let log = get(plc.addr, "/api/snapshots").json();
    assert_eq!(log["log"][0]["approver"], "first install", "{log}");
    drop(plc);
    // Edited since: the next install (every service start) touches nothing.
    let projects = Projects::in_state_dir(&state).unwrap();
    let mine = "(* mine *)\n".to_string() + &std::fs::read_to_string(line).unwrap();
    projects.save("line", "line.st", mine.as_bytes(), None).unwrap();
    let (ok, out) = seed(&state, &["line", line]);
    assert!(ok && out.contains("nothing seeded"), "{out}");
    assert_eq!(projects.file("line", "line.st").unwrap(), mine, "never clobbered");
    // Nor on a box that already ran something, with no project.
    let other = tmp("seed-other");
    Store::open(&other.join("store"))
        .unwrap()
        .commit(&running(), &meta())
        .unwrap();
    let (ok, out) = seed(&other, &["line", line]);
    assert!(ok && out.contains("already deployed"), "{out}");
    assert_eq!(
        Projects::in_state_dir(&other).unwrap().list().unwrap(),
        Vec::<String>::new()
    );
    // A file that does not build: refused, saying where.
    let bad = tmp("seed-bad");
    let src = bad.join("broken.st");
    std::fs::write(&src, "PROGRAM broken\n  x := ;\nEND_PROGRAM\n").unwrap();
    let (ok, out) = seed(&bad, &["broken", src.to_str().unwrap()]);
    assert!(!ok && out.contains("broken.st:2"), "{out}");
}
