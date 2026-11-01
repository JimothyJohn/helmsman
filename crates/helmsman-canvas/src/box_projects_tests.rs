//! The Editor's projects and forcing against the real box: helmsman-plc's
//! API with projects, forces and accounts (alice an engineer, bob an
//! operator) over a runtime scanning on its own thread and a real store.
//! Every request is the [`PReq`] the page made, carried out over a socket
//! the way the browser's `fetch` does (same origin, the session cookie,
//! `X-Helmsman-Request`); the clicks go through egui, headless.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use eframe::egui::{self, Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use helmsman_iec::{ImageSizes, compile};
use helmsman_plc::api::{
    Api, Approval, Catalog, Config, Devices, Extras, Limits, Live, MemAudit, MemProposals, ModuleInfo, More,
    Plant, Publisher, Shared, swap,
};
use helmsman_plc::force::{self, Forces};
use helmsman_plc::manifest::ModuleDir;
use helmsman_plc::policy::account::{Accounts, Logins};
use helmsman_plc::policy::pin::{Gate, Pins};
use helmsman_plc::policy::{FLOOR, Role};
use helmsman_plc::project::Projects;
use helmsman_plc::record::{RecordConfig, Recorder};
use helmsman_plc::spec::Defaults;
use helmsman_plc::store::{ChangeSet, Meta, ModuleFiles, Store};
use helmsman_plc::{Runtime, SignalTable, Window};
use serde_json::Value as Json;

use crate::app::App;
use crate::box_projects::{Answer, BoxProjects, Deploy, PReq};
use crate::box_tests::sse;
use crate::box_view::BoxView;
use crate::projects::Workspace;
use crate::remote::{Cmd, Msg, Stream};
use crate::review::Session;
use crate::review_view::ReviewView;

const ALICE: &str = "alice-engineer-pw";
const BOB: &str = "bob-operator-pw";

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

/// A function block in one file, used by the PROGRAM in another.
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

fn running() -> ChangeSet {
    ChangeSet {
        modules: BTreeMap::from([("conveyor".to_string(), ModuleFiles::st(MANIFEST, SOURCE))]),
        io_map: Some(MAP.into()),
        calendar: None,
    }
}

fn tmp(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "helmsman-canvas-projects-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
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

/// The box, as `helmsman-plc --projects --api` runs it with accounts.
pub struct Bench {
    api: Option<Api>,
    pub addr: SocketAddr,
    stop: Arc<AtomicBool>,
    scan: Option<JoinHandle<()>>,
    dirs: Vec<PathBuf>,
}

impl Drop for Bench {
    fn drop(&mut self) {
        self.api.take();
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.scan.take() {
            let _ = t.join();
        }
        for d in &self.dirs {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

/// A browser: its session cookie.
#[derive(Default)]
pub struct Jar(Mutex<Option<String>>);

impl Jar {
    fn get(&self) -> Option<String> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn set(&self, v: Option<String>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = v;
    }
}

impl Bench {
    #[allow(clippy::too_many_lines)]
    pub fn new() -> Self {
        let d = Defaults {
            period: Duration::from_millis(2),
            i: Window::new(0, 0),
            q: Window::new(0, 0),
            m: 16,
        };
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
        let meta = Meta {
            author: "Test".into(),
            prompt: String::new(),
            approver: None,
            time_ms: 1,
        };
        let store = tmp("store");
        Store::open(&store).unwrap().commit(&running(), &meta).unwrap();
        let state = tmp("state");
        let mut a = Accounts::default();
        a.add("alice", Role::Engineer, ALICE, 1, [1; 16]).unwrap();
        a.add("bob", Role::Operator, BOB, 1, [2; 16]).unwrap();
        a.save(&state).unwrap();
        let mut recorder = Recorder::new(&table, RecordConfig::default(), None);
        let (handle, swapper) = swap::channel(Duration::from_secs(5));
        let t0 = Instant::now();
        let approval =
            Approval {
                policy: FLOOR,
                gate: Gate::new(Pins::default(), FLOOR.lockout, Arc::new(Instant::now)).with_logins(
                    Logins::new(state.clone(), 1, FLOOR.lockout, Arc::new(Instant::now)),
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
        let cfg = Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            token: "0".repeat(64),
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
            ..More::default()
        };
        let api = Api::start_box(cfg, shared, Extras::default(), None, more).unwrap();
        let addr = api.addr();
        let stop = Arc::new(AtomicBool::new(false));
        let ticks = Arc::new(AtomicU64::new(0));
        let scan = {
            let (stop, table, ticks) = (Arc::clone(&stop), Arc::clone(&table), Arc::clone(&ticks));
            let mut publisher = Publisher::new(live, Duration::from_millis(5));
            let mut reader = force::Reader::default();
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    {
                        let mut t = table.lock().unwrap_or_else(PoisonError::into_inner);
                        // The I/O read: start is held on.
                        t.set_input(start, Some(helmsman_io::Value::Bool(true)));
                        let mut inputs = vec![0u8; 32];
                        swapper.serve(&mut rt, &mut t);
                        reader.apply(&forces, &mut inputs, &mut t);
                        let now = t0.elapsed();
                        recorder.observe(now, &t);
                        rt.tick_with(now, &inputs, &mut t);
                        publisher.publish(now, &rt, &t);
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
            dirs: vec![store, state],
        }
    }

    /// Sends `head` and `body`; the status, `Set-Cookie`s and body.
    fn raw(&self, head: &str, body: &[u8]) -> (u16, Vec<String>, Vec<u8>) {
        let mut s = TcpStream::connect(self.addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        s.write_all(head.as_bytes()).unwrap();
        s.write_all(body).unwrap();
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        let split = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or(out.len());
        let head = String::from_utf8_lossy(&out[..split]).into_owned();
        let body = out.get(split + 4..).unwrap_or_default().to_vec();
        let cookies = head
            .lines()
            .filter_map(|l| l.strip_prefix("Set-Cookie: "))
            .map(str::to_string)
            .collect();
        (
            head.get(9..12).and_then(|s| s.parse().ok()).unwrap_or(0),
            cookies,
            body,
        )
    }

    /// Carries out `req` the way `web::ProjHttp` does in the browser whose
    /// cookies are `jar`.
    pub fn send(&self, jar: &Jar, req: &PReq) -> Answer {
        let body = req.body.clone().unwrap_or_default();
        let mut head = format!("{} {} HTTP/1.1\r\nHost: box\r\n", req.method, req.url);
        if req.method != "GET" {
            head.push_str("X-Helmsman-Request: 1\r\nOrigin: http://box\r\n");
        }
        if let Some(c) = jar.get() {
            let _ = write!(head, "Cookie: helmsman_session={c}\r\n");
        }
        if req.body.is_some() {
            let _ = write!(
                head,
                "Content-Type: {}\r\nContent-Length: {}\r\n",
                req.ctype,
                body.len()
            );
        }
        head.push_str("\r\n");
        let (status, cookies, body) = self.raw(&head, &body);
        for c in cookies {
            let v = c
                .strip_prefix("helmsman_session=")
                .and_then(|r| r.split(';').next())
                .unwrap_or_default();
            jar.set((!c.contains("Max-Age=0") && !v.is_empty()).then(|| v.to_string()));
        }
        Answer {
            status,
            retry_after: None,
            body: String::from_utf8_lossy(&body).into_owned(),
        }
    }

    /// Signs `user` in, as the header's panel does: the cookie lands in
    /// `jar`; the session as the page knows it.
    pub fn login(&self, jar: &Jar, user: &str, password: &str) -> Session {
        let body = serde_json::json!({ "user": user, "password": password }).to_string();
        let head = format!(
            "POST /api/login HTTP/1.1\r\nHost: box\r\nX-Helmsman-Request: 1\r\nOrigin: http://box\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let (status, cookies, out) = self.raw(&head, body.as_bytes());
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&out));
        let c = cookies.iter().find_map(|c| {
            c.strip_prefix("helmsman_session=")
                .and_then(|r| r.split(';').next())
        });
        jar.set(c.map(str::to_string));
        let j: Json = serde_json::from_slice(&out).unwrap();
        Session {
            user: j["user"].as_str().map(str::to_string),
            role: j["role"].as_str().unwrap().to_string(),
        }
    }

    /// `GET path` with no cookie: status and body bytes.
    pub fn get(&self, path: &str) -> (u16, Vec<u8>) {
        let (s, _, b) = self.raw(&format!("GET {path} HTTP/1.1\r\nHost: box\r\n\r\n"), b"");
        (s, b)
    }

    pub fn json(&self, path: &str) -> Json {
        serde_json::from_slice(&self.get(path).1).unwrap()
    }

    /// The first `n` events of the stream at `path`.
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
            assert!(Instant::now() < deadline, "{n} events from {path}");
            if let Ok(k) = s.read(&mut chunk) {
                assert!(k > 0, "{path} closed");
                buf.push_str(&String::from_utf8_lossy(&chunk[..k]));
            }
        }
    }
}

/// Sends everything `p` has queued, and what that queues, until quiet.
pub fn settle(b: &Bench, jar: &Jar, p: &mut BoxProjects) {
    for _ in 0..40 {
        let reqs = p.poll();
        if reqs.is_empty() {
            return;
        }
        for q in reqs {
            let a = b.send(jar, &q);
            p.answer(q.call, &a);
        }
    }
    panic!("the projects never went quiet");
}

/// A page with projects open, signed in as `who` (`None`: signed out).
fn page(b: &Bench, jar: &Jar, who: Option<(&str, &str)>) -> BoxView {
    let mut v = BoxView::new();
    v.session = who.map(|(u, p)| b.login(jar, u, p));
    settle(b, jar, &mut v.projects);
    v
}

/// One request queue of `v` (state and projects) carried out, as the
/// browser's transports would, at page time `now`.
fn pump(b: &Bench, jar: &Jar, v: &mut BoxView, ctx: &egui::Context, now: f64) {
    let cmds = std::mem::take(&mut v.sent);
    let reqs = std::mem::take(&mut v.psent);
    let mut msgs = Vec::new();
    for c in cmds {
        match c {
            Cmd::Get(what, url) => {
                let (s, body) = b.get(&url);
                let body = String::from_utf8_lossy(&body).into_owned();
                msgs.push(Msg::Got(
                    what,
                    if s == 200 {
                        Ok(body)
                    } else {
                        Err(format!("HTTP {s}"))
                    },
                ));
            }
            Cmd::Listen(stream, url) => {
                msgs.push(Msg::Opened(stream));
                for (name, data) in b.events(&url, 1) {
                    msgs.push(Msg::Event(stream, name, data));
                }
            }
            Cmd::Close(_) => {}
        }
    }
    let answers = reqs
        .into_iter()
        .map(|q| {
            let a = b.send(jar, &q);
            (q.call, a)
        })
        .collect();
    v.step_all(ctx, msgs, answers, now);
}

/// A fresh values event from the box, into `v`.
fn values(b: &Bench, v: &mut BoxView, now: f64) {
    for (name, data) in b.events("/api/events", 1) {
        v.state.apply(Msg::Event(Stream::Values, name, data), now);
    }
}

fn files(p: &BoxProjects) -> Vec<String> {
    p.open.as_ref().unwrap().bufs.keys().cloned().collect()
}

/// Makes project `name` with `files` through the page, as an engineer.
fn make(b: &Bench, jar: &Jar, p: &mut BoxProjects, name: &str, with: &[(&str, &str)]) {
    p.create(name, None);
    settle(b, jar, p);
    assert_eq!(
        p.open.as_ref().map(|o| o.name.as_str()),
        Some(name),
        "{:?}",
        p.notice
    );
    for (f, text) in with {
        p.new_file(f);
        settle(b, jar, p);
        p.open_file(f);
        p.current_mut().unwrap().text = (*text).to_string();
    }
    assert!(p.dirty() || with.is_empty());
    p.save();
    settle(b, jar, p);
    assert!(!p.dirty(), "{:?}", p.notice);
}

// ---- the headless app ----

struct Harness {
    ctx: egui::Context,
    app: App,
    time: f64,
}

impl Harness {
    fn new(bv: BoxView, session: Option<Session>) -> Self {
        let mut app = App::boxed();
        let mut r = ReviewView::new();
        r.state.session = session;
        app.review = Some(r);
        app.boxv = Some(bv);
        let mut h = Self {
            ctx: egui::Context::default(),
            app,
            time: 0.0,
        };
        h.frame(vec![]);
        h.frame(vec![]);
        h
    }

    fn frame(&mut self, events: Vec<Event>) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 860.0))),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        self.time += 1.0 / 60.0;
        let Self { ctx, app, .. } = self;
        let mut out = ctx.run_ui(input, |ui| app.show(ui));
        out.textures_delta.clear();
    }

    fn click(&mut self, at: Pos2) {
        let button = |pressed| Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        self.frame(vec![Event::PointerMoved(at)]);
        self.frame(vec![button(true)]);
        self.frame(vec![button(false)]);
    }

    fn rect(&self, key: &str) -> Rect {
        self.app.chrome.rect(key).unwrap_or_else(|| {
            let keys: Vec<&String> = self.app.chrome.hits.iter().map(|(k, _)| k).collect();
            panic!("no {key} on screen: {keys:?}")
        })
    }

    fn press(&mut self, key: &str) {
        let at = self.rect(key).center();
        self.click(at);
    }

    fn bv(&mut self) -> &mut BoxView {
        self.app.boxv.as_mut().unwrap()
    }

    /// What the page sent: a click queues, the next frame sends.
    fn sent(&mut self) -> Vec<PReq> {
        self.frame(vec![]);
        std::mem::take(&mut self.bv().psent)
    }

    /// Frames for `secs` of page time (panels slide open).
    fn wait(&mut self, secs: f64) {
        let end = self.time + secs;
        while self.time < end {
            self.frame(vec![]);
        }
    }
}

// ---- the tests ----

#[test]
fn an_engineer_lists_opens_edits_saves_and_deploys_a_project_on_the_box() {
    let b = Bench::new();
    let jar = Jar::default();
    let mut v = page(&b, &jar, Some(("alice", ALICE)));
    let p = &mut v.projects;
    assert_eq!(p.available, Some(true));
    assert!(p.projects.is_empty() && p.open.is_none(), "{:?}", p.projects);

    make(
        &b,
        &jar,
        p,
        "line",
        &[("main.st", MAIN), ("blocks.st", BLOCKS), ("io.map", MAP)],
    );
    assert_eq!(files(p), ["blocks.st", "io.map", "main.st"]);
    assert_eq!(
        p.projects.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        ["line"]
    );
    // Saved on the box, as typed.
    let on_box = b.json("/api/projects/line/files/main.st");
    assert_eq!(on_box["text"], MAIN);

    // Another page lists it and opens it with every file.
    let mut other = BoxProjects::new();
    settle(&b, &Jar::default(), &mut other);
    assert_eq!(other.open.as_ref().unwrap().name, "line");
    assert_eq!(files(&other), ["blocks.st", "io.map", "main.st"]);
    assert_eq!(other.current().unwrap().text, BLOCKS, "the first program file");
    assert!(!other.dirty());

    // An edit is dirty until saved, and saving it carries the version read.
    p.open_file("main.st");
    let edited = MAIN.replace("reset := FALSE", "reset := NOT start");
    p.current_mut().unwrap().text.clone_from(&edited);
    assert!(p.dirty());
    p.save();
    settle(&b, &jar, p);
    assert!(!p.dirty());
    assert_eq!(
        b.json("/api/projects/line/files/main.st")["text"],
        edited.as_str()
    );

    // Deploy: check, the replay the floor wants, deploy; what runs is said.
    p.deploy();
    settle(&b, &jar, p);
    let Deploy::Done(what) = &p.deploy else {
        panic!("not deployed: {:?} {:?}", p.deploy, p.diags);
    };
    assert!(what.starts_with("line is running: conveyor"), "{what}");
    assert_eq!(p.running.as_deref(), Some("line"));
    assert!(p.projects[0].running);
    let running = b.json("/api/running");
    assert_eq!(
        (running["project"].as_str(), running["current"].as_bool()),
        (Some("line"), Some(true))
    );
    v.step_all(&egui::Context::default(), vec![], vec![], 0.0);
    assert!(
        v.sent
            .iter()
            .any(|c| matches!(c, Cmd::Get(_, u) if u == "/api/modules")),
        "the module list is read again after a deploy: {:?}",
        v.sent
    );

    // Unsaved edits are saved first when deploying.
    let p = &mut v.projects;
    p.current_mut().unwrap().text = MAIN.to_string();
    p.deploy();
    assert_eq!(p.deploy, Deploy::Saving);
    settle(&b, &jar, p);
    assert!(matches!(p.deploy, Deploy::Done(_)), "{:?}", p.deploy);
    assert_eq!(b.json("/api/projects/line/files/main.st")["text"], MAIN);

    // Files: new, renamed, deleted; projects: copied, renamed, deleted.
    p.new_file("extra.st");
    settle(&b, &jar, p);
    p.current_mut().unwrap().text = "FUNCTION_BLOCK x\nEND_FUNCTION_BLOCK\n".into();
    p.save();
    settle(&b, &jar, p);
    p.rename_file("extra.st", "more.st");
    settle(&b, &jar, p);
    assert!(files(p).contains(&"more.st".to_string()) && !files(p).contains(&"extra.st".to_string()));
    assert_eq!(
        b.json("/api/projects/line/files/more.st")["text"],
        "FUNCTION_BLOCK x\nEND_FUNCTION_BLOCK\n"
    );
    assert_eq!(b.get("/api/projects/line/files/extra.st").0, 404);
    p.delete_file("more.st");
    settle(&b, &jar, p);
    assert_eq!(files(p), ["blocks.st", "io.map", "main.st"]);
    p.create("line-b", Some("line"));
    settle(&b, &jar, p);
    assert_eq!(p.open.as_ref().unwrap().name, "line-b");
    assert_eq!(files(p), ["blocks.st", "io.map", "main.st"]);
    p.rename("line-b", "line-c");
    settle(&b, &jar, p);
    assert_eq!(p.open.as_ref().unwrap().name, "line-c");
    p.delete("line");
    settle(&b, &jar, p);
    let (msg, err) = p.notice.clone().unwrap();
    assert!(
        err && msg.contains("not deleted"),
        "the running project stays: {msg}"
    );
    p.delete("line-c");
    settle(&b, &jar, p);
    let names: Vec<&str> = p.projects.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["line"]);
    assert_eq!(p.open.as_ref().unwrap().name, "line", "back to the one running");

    // Download is the box's own zip; upload it back as another project.
    p.download();
    assert_eq!(p.downloads, ["/api/projects/line/zip"]);
    let (status, zip) = b.get("/api/projects/line/zip");
    assert_eq!(status, 200);
    p.upload("Line Copy.zip", zip.clone(), false);
    settle(&b, &jar, p);
    assert_eq!(p.open.as_ref().unwrap().name, "line-copy", "{:?}", p.notice);
    assert_eq!(files(p), ["blocks.st", "io.map", "main.st"]);
    assert_eq!(b.get("/api/projects/line-copy/zip").1, zip, "byte for byte");
    // Again: taken, so the page offers to replace it.
    p.upload("line copy.zip", zip, false);
    settle(&b, &jar, p);
    assert!(p.pending.is_some(), "{:?}", p.notice);
    p.replace();
    settle(&b, &jar, p);
    assert!(
        p.pending.is_none() && !p.notice.as_ref().unwrap().1,
        "{:?}",
        p.notice
    );
    // A single .st goes into the open project; an L5X the box cannot
    // convert is refused, in the box's words.
    p.upload(
        "Extra.st",
        b"FUNCTION f : INT\n  f := 1;\nEND_FUNCTION\n".to_vec(),
        false,
    );
    settle(&b, &jar, p);
    assert!(files(p).contains(&"Extra.st".to_string()), "{:?}", p.notice);
    assert_eq!(
        p.current().unwrap().text,
        "FUNCTION f : INT\n  f := 1;\nEND_FUNCTION\n"
    );
    p.upload("plant.L5X", b"<RSLogix5000Content/>".to_vec(), false);
    settle(&b, &jar, p);
    let (msg, err) = p.notice.clone().unwrap();
    assert!(err && msg.contains("plant.L5X is not converted"), "{msg}");
}

#[test]
fn a_stale_save_is_a_409_and_reload_brings_the_boxs_text() {
    let b = Bench::new();
    let (ja, jb) = (Jar::default(), Jar::default());
    let mut a = page(&b, &ja, Some(("alice", ALICE)));
    make(&b, &ja, &mut a.projects, "line", &[("main.st", MAIN)]);
    // A second engineer's page, the same project open.
    let mut c = page(&b, &jb, Some(("alice", ALICE)));
    assert_eq!(c.projects.current().unwrap().text, MAIN);

    a.projects.current_mut().unwrap().text = format!("(* first *)\n{MAIN}");
    a.projects.save();
    settle(&b, &ja, &mut a.projects);
    assert!(!a.projects.dirty());

    c.projects.current_mut().unwrap().text = format!("(* second *)\n{MAIN}");
    c.projects.save();
    settle(&b, &jb, &mut c.projects);
    assert_eq!(c.projects.conflict.as_deref(), Some("main.st"));
    let (msg, err) = c.projects.notice.clone().unwrap();
    assert!(err && msg.contains("Someone else saved main.st"), "{msg}");
    assert!(c.projects.dirty(), "the edits stay until reloaded");
    assert_eq!(
        b.json("/api/projects/line/files/main.st")["text"],
        format!("(* first *)\n{MAIN}"),
        "the first save stands"
    );
    // The toolbar offers Reload; it brings the box's text.
    let mut h = Harness::new(c, None);
    h.press("reload");
    let reqs = h.sent();
    for q in reqs {
        let ans = b.send(&jb, &q);
        h.bv().projects.answer(q.call, &ans);
    }
    let p = &h.bv().projects;
    assert_eq!(p.conflict, None);
    assert_eq!(p.current().unwrap().text, format!("(* first *)\n{MAIN}"));
    assert!(!p.dirty());
    // Saving now carries the new version and lands.
    let p = &mut h.bv().projects;
    p.current_mut().unwrap().text = format!("(* second *)\n{MAIN}");
    p.save();
    settle(&b, &jb, p);
    assert_eq!(p.conflict, None, "{:?}", p.notice);
    assert_eq!(
        b.json("/api/projects/line/files/main.st")["text"],
        format!("(* second *)\n{MAIN}")
    );
}

#[test]
fn compile_errors_name_file_and_line_and_a_click_opens_it_there() {
    let b = Bench::new();
    let jar = Jar::default();
    let mut v = page(&b, &jar, Some(("alice", ALICE)));
    let broken = MAIN.replace("lamp := l.q;", "lamp := l.q +;");
    make(
        &b,
        &jar,
        &mut v.projects,
        "line",
        &[("blocks.st", BLOCKS), ("main.st", &broken), ("io.map", MAP)],
    );
    let before = b.json("/api/running");
    v.projects.deploy();
    settle(&b, &jar, &mut v.projects);
    let Deploy::Failed(why) = &v.projects.deploy else {
        panic!("{:?}", v.projects.deploy);
    };
    assert!(why.contains("does not build"), "{why}");
    let d = v.projects.diags.first().expect("a diagnostic").clone();
    assert_eq!(
        (d.file.as_str(), d.line),
        ("main.st", 6),
        "{:?}",
        v.projects.diags
    );
    assert_eq!(b.json("/api/running"), before, "nothing changed");

    // In the app: the error under the editor, a click opens main.st at 6.
    let session = v.session.clone();
    v.projects.open_file("blocks.st");
    let mut h = Harness::new(v, session);
    h.app.chrome.st = true;
    h.wait(0.5);
    assert_eq!(
        h.bv().projects.open.as_ref().unwrap().current.as_deref(),
        Some("blocks.st")
    );
    h.press("diag 0");
    assert_eq!(
        h.bv().projects.open.as_ref().unwrap().current.as_deref(),
        Some("main.st")
    );
    assert_eq!(h.app.chrome.line, Some(6));
    assert!(h.app.chrome.rect("deploy failed").is_some());
}

#[test]
fn signed_out_or_below_engineer_everything_is_read_only() {
    let b = Bench::new();
    let jar = Jar::default();
    let mut eng = page(&b, &jar, Some(("alice", ALICE)));
    make(
        &b,
        &jar,
        &mut eng.projects,
        "line",
        &[("main.st", MAIN), ("io.map", MAP)],
    );

    for who in [None, Some(("bob", BOB))] {
        let jar = Jar::default();
        let v = page(&b, &jar, who);
        let why = v.writable().unwrap_err();
        match who {
            None => assert!(why.contains("Sign in as an engineer"), "{why}"),
            Some(_) => assert!(
                why.contains("bob (operator)") && why.contains("only an engineer"),
                "{why}"
            ),
        }
        let session = v.session.clone();
        let mut h = Harness::new(v, session);
        h.app.chrome.st = true;
        h.wait(0.5);
        assert!(h.app.chrome.rect("read-only").is_some(), "the ST says why");
        // The editor takes no typing.
        let st = h.rect("read-only");
        h.click(st.left_bottom() + Vec2::new(60.0, 40.0));
        h.frame(vec![Event::Text("x".into())]);
        assert!(!h.bv().projects.dirty(), "typing changes nothing");
        // Save, Deploy, New, Delete do nothing; nothing is sent.
        h.bv()
            .projects
            .current_mut()
            .unwrap()
            .text
            .push_str("(* not mine *)");
        for k in ["save", "deploy", "new project", "delete project", "new file"] {
            h.press(k);
        }
        let sent = h.sent();
        assert!(sent.is_empty(), "{sent:?}");
        assert_eq!(h.app.chrome.rect("name"), None, "no name asked for");
        // A force asked for in code is not sent either; the box would
        // refuse it anyway.
        h.bv().force("start", helmsman_iec::Value::Bool(false));
        let sent = h.sent();
        assert!(sent.is_empty(), "{sent:?}");
    }
    // As the engineer, the same page writes.
    let mut h = Harness::new(
        eng,
        Some(Session {
            user: Some("alice".into()),
            role: "engineer".into(),
        }),
    );
    h.bv().projects.current_mut().unwrap().text.push_str("(* mine *)");
    h.frame(vec![]);
    assert!(h.app.chrome.rect("dirty").is_some(), "the header marks it");
    h.press("save");
    let sent = h.sent();
    assert!(sent.iter().any(|q| q.method == "PUT"), "{sent:?}");
}

#[test]
fn an_engineer_forces_an_input_from_the_diagram_and_releases_all() {
    let b = Bench::new();
    let jar = Jar::default();
    let ctx = egui::Context::default();
    let mut v = page(&b, &jar, Some(("alice", ALICE)));
    // Watch the box until the running module is drawn with a scan.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut now = 0.0;
    while v.view.is_none() || v.state.scan.is_none() {
        assert!(Instant::now() < deadline, "never drawn: {:?}", v.state.problem);
        v.step_all(&ctx, vec![], vec![], now);
        pump(&b, &jar, &mut v, &ctx, now);
        if let Some(name) = v.state.selected.clone() {
            for (ev, data) in b.events(&crate::remote::trace_url(&name), 1) {
                v.state.apply(Msg::Event(Stream::Trace, ev, data), now);
            }
        }
        now += 0.1;
    }
    let start = |v: &BoxView| {
        v.state
            .signals
            .iter()
            .find(|s| s.name == "start")
            .cloned()
            .unwrap()
    };
    assert_eq!(
        start(&v).value,
        Some(helmsman_iec::Value::Bool(true)),
        "held on by the I/O"
    );
    assert!(!start(&v).forced);

    // The click: start's block on the diagram flips it, forced.
    let session = v.session.clone();
    let mut h = Harness::new(v, session);
    h.frame(vec![]);
    h.press("force start");
    let reqs = h.sent();
    assert!(
        reqs.iter().any(|q| q.url == "/api/forces" && q.method == "POST"),
        "{reqs:?}"
    );
    for q in reqs {
        let a = b.send(&jar, &q);
        h.bv().projects.answer(q.call, &a);
    }
    settle(&b, &jar, &mut h.bv().projects);
    h.frame(vec![]);
    let f = h.bv().state.forces.clone();
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0].signal.as_deref(), Some("start"));
    assert_eq!(f[0].value, Some(helmsman_iec::Value::Bool(false)));
    assert_eq!(f[0].by, "alice (engineer)");
    // The box's live data marks it: the values stream says forced, and
    // the input reads FALSE though the I/O holds it on.
    let t = h.time;
    wait("start forced off on the box", || {
        values(&b, h.app.boxv.as_mut().unwrap(), t);
        let s = start(h.app.boxv.as_ref().unwrap());
        s.forced && s.value == Some(helmsman_iec::Value::Bool(false))
    });
    h.frame(vec![]);
    h.frame(vec![]);
    assert!(h.app.chrome.rect("forced start").is_some(), "FORCED on its block");
    assert!(h.app.chrome.rect("forces").is_some(), "the banner");
    // Release all from the banner.
    h.press("release all");
    let reqs = h.sent();
    assert!(
        reqs.iter()
            .any(|q| q.url == "/api/forces" && q.method == "DELETE"),
        "{reqs:?}"
    );
    for q in reqs {
        let a = b.send(&jar, &q);
        h.bv().projects.answer(q.call, &a);
    }
    settle(&b, &jar, &mut h.bv().projects);
    h.frame(vec![]);
    assert_eq!(h.bv().state.forces, []);
    assert_eq!(b.json("/api/forces")["forces"], serde_json::json!([]));
    let t = h.time;
    wait("start back to the I/O", || {
        values(&b, h.app.boxv.as_mut().unwrap(), t);
        let s = start(h.app.boxv.as_ref().unwrap());
        !s.forced && s.value == Some(helmsman_iec::Value::Bool(true))
    });
    h.frame(vec![]);
    assert!(h.app.chrome.rect("forced start").is_none());
    assert!(h.app.chrome.rect("forces").is_none());
}

#[test]
fn a_box_without_projects_keeps_the_module_view() {
    let b = crate::box_tests::Bench::new();
    let mut p = BoxProjects::new();
    for q in p.poll() {
        let (status, body) = b.get(&q.url);
        p.answer(
            q.call,
            &Answer {
                status,
                retry_after: None,
                body,
            },
        );
    }
    assert_eq!(p.available, Some(false));
    let mut v = BoxView::new();
    v.projects = p;
    assert!(!v.has_projects());
    assert!(v.writable().is_err());
}

#[test]
fn a_session_the_box_ended_signs_the_header_out() {
    let b = Bench::new();
    let jar = Jar::default();
    let mut v = page(&b, &jar, Some(("alice", ALICE)));
    make(&b, &jar, &mut v.projects, "line", &[("main.st", MAIN)]);
    // Signed out elsewhere: the box forgets the cookie.
    let head = "POST /api/logout HTTP/1.1\r\nHost: box\r\nX-Helmsman-Request: 1\r\nOrigin: http://box\r\n";
    let c = jar.get().unwrap();
    let _ = b.raw(&format!("{head}Cookie: helmsman_session={c}\r\n\r\n"), b"");
    v.projects.current_mut().unwrap().text.push_str("(* x *)");
    v.projects.save();
    settle(&b, &jar, &mut v.projects);
    assert!(v.projects.ended);
    let session = v.session.clone();
    let mut h = Harness::new(v, session);
    h.frame(vec![]);
    assert_eq!(h.app.session(), None, "the header shows Sign in again");
    assert!(h.app.chrome.rect("sign in").is_some());
}
