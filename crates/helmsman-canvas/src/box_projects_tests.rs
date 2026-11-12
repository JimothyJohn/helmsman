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
use helmsman_io::local::Channels;
use helmsman_plc::api::swap::Scene;
use helmsman_plc::api::{
    Api, Approval, Catalog, Config, Devices, Extras, Limits, Live, MemAudit, MemProposals, ModuleInfo, More,
    Plant, Publisher, Shared, swap,
};
use helmsman_plc::force::{self, Forces};
use helmsman_plc::link::LinkSet;
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
use crate::box_projects::{Answer, BoxProjects, Buf, Deploy, PReq, Pending};
use crate::box_tests::sse;
use crate::box_view::BoxView;
use crate::projects::{Act, Workspace};
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

const SOURCE: &str = "PROGRAM conveyor
VAR_INPUT
  start : BOOL;
END_VAR
VAR_OUTPUT
  lamp : BOOL;
END_VAR
lamp := start;
END_PROGRAM
";

const MAP: &str = "\
in  start bool box di:0
out lamp  bool box do:0 safe=false
out horn  bool box do:1 safe=false
";

/// A function block in one file, used by the PROGRAM in another.
const BLOCKS: &str = "\
FUNCTION_BLOCK Latch
VAR_INPUT
  set : BOOL;
  reset : BOOL;
END_VAR
VAR_OUTPUT
  q : BOOL;
END_VAR
q := (q OR set) AND NOT reset;
END_FUNCTION_BLOCK
";

const MAIN: &str = "\
PROGRAM conveyor
VAR_INPUT
  start : BOOL;
END_VAR
VAR_OUTPUT
  lamp : BOOL;
END_VAR
VAR
  l : Latch;
END_VAR
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
        Self::build(false)
    }

    /// `links`: run device links, so a deploy may change the map.
    pub fn build(links: bool) -> Self {
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
        let recorder = Recorder::new(&table, RecordConfig::default(), None);
        let plant_table = table.clone();
        let table = Arc::new(Mutex::new(table));
        let devices = Arc::new(Devices::new(["box".to_string()]));
        let (handle, swapper) = swap::channel(Duration::from_secs(5));
        // As the real binary does: a deploy may change the map, and the
        // device links follow. Off otherwise, so `start` stays as held below.
        let handle = if links {
            let dv = Arc::clone(&devices);
            let mut set = LinkSet::new(
                Arc::clone(&table),
                Arc::new(helmsman_io::open),
                Arc::new(|| Box::new(Channels::new())),
                Arc::new(
                    move |name: &str, err: Option<&helmsman_io::Error>, _address: Option<String>| {
                        dv.set(name, err.map(ToString::to_string));
                    },
                ),
            );
            set.start(&parsed.map).unwrap();
            handle.with_links(Arc::new(Mutex::new(set)))
        } else {
            handle
        };
        let t0 = Instant::now();
        let approval =
            Approval {
                policy: FLOOR,
                gate: Gate::new(Pins::default(), FLOOR.lockout, Arc::new(Instant::now)).with_logins(
                    Logins::new(state.clone(), 1, FLOOR.lockout, Arc::new(Instant::now)),
                ),
                plant: Plant {
                    table: plant_table,
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
            token: crate::box_tests::TOKEN.into(),
            limits: Limits::default(),
            stream_every: Duration::from_millis(20),
            memory: 16,
            log: Arc::new(|_: &str| {}),
        };
        let shared = Shared {
            catalog,
            live: Arc::clone(&live),
            devices,
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
            let mut scene = Scene {
                calendar: None,
                recorder: Some(recorder),
                wall: None,
            };
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    {
                        let mut t = table.lock().unwrap_or_else(PoisonError::into_inner);
                        // The I/O read: start is held on.
                        if !links {
                            t.set_input(start, Some(helmsman_io::Value::Bool(true)));
                        }
                        let mut inputs = vec![0u8; 32];
                        swapper.serve_scene(&mut rt, &mut t, Some(&mut scene));
                        reader.apply(&forces, &mut inputs, &mut t);
                        let now = t0.elapsed();
                        if let Some(r) = &mut scene.recorder {
                            r.observe(now, &t);
                        }
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

    /// `GET path` as the page in `jar` reads: its session cookie and
    /// nothing else (every read needs a session, card read-lock: signed
    /// out, this is a 401).
    pub fn get_in(&self, jar: &Jar, path: &str) -> (u16, Vec<u8>) {
        let cookie = jar
            .get()
            .map(|s| format!("Cookie: helmsman_session={s}\r\n"))
            .unwrap_or_default();
        let (s, _, b) = self.raw(&format!("GET {path} HTTP/1.1\r\nHost: box\r\n{cookie}\r\n"), b"");
        (s, b)
    }

    /// The first `n` events of the stream at `path`, as the page in `jar`
    /// opens it (its cookie alone, as an `EventSource` sends it).
    pub fn events_in(&self, jar: &Jar, path: &str, n: usize) -> Vec<(String, String)> {
        let cookie = jar
            .get()
            .map(|s| format!("Cookie: helmsman_session={s}\r\n"))
            .unwrap_or_default();
        self.events_with(path, n, &cookie)
    }

    /// `GET path` as the test itself looks at the box: the box token.
    pub fn get(&self, path: &str) -> (u16, Vec<u8>) {
        let (s, _, b) = self.raw(
            &format!(
                "GET {path} HTTP/1.1\r\nHost: box\r\nAuthorization: Bearer {}\r\n\r\n",
                crate::box_tests::TOKEN
            ),
            b"",
        );
        (s, b)
    }

    pub fn json(&self, path: &str) -> Json {
        serde_json::from_slice(&self.get(path).1).unwrap()
    }

    /// The first `n` events of the stream at `path`, with the box token.
    pub fn events(&self, path: &str, n: usize) -> Vec<(String, String)> {
        let auth = format!("Authorization: Bearer {}\r\n", crate::box_tests::TOKEN);
        self.events_with(path, n, &auth)
    }

    fn events_with(&self, path: &str, n: usize, headers: &str) -> Vec<(String, String)> {
        let mut s = TcpStream::connect(self.addr).unwrap();
        s.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
        write!(s, "GET {path} HTTP/1.1\r\nHost: box\r\n{headers}\r\n").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut buf = String::new();
        let mut chunk = [0u8; 8192];
        loop {
            // A refused stream (401 signed out) is no events at all.
            if let Some(head) = buf.split_once("\r\n\r\n").map(|(h, _)| h)
                && !head.starts_with("HTTP/1.1 200")
            {
                return Vec::new();
            }
            let events = sse(&buf);
            if events.len() >= n {
                return events.into_iter().take(n).collect();
            }
            assert!(Instant::now() < deadline, "{n} events from {path}");
            if let Ok(k) = s.read(&mut chunk) {
                if k == 0 {
                    return Vec::new();
                }
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
                let (s, body) = b.get_in(jar, &url);
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
                // Signed out the box refuses the stream (401): the browser
                // would report it failed.
                let events = b.events_in(jar, &url, 1);
                if events.is_empty() {
                    msgs.push(Msg::Failed(stream));
                    continue;
                }
                msgs.push(Msg::Opened(stream));
                for (name, data) in events {
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

    /// A right click at `at` (opens a context menu).
    fn right_click(&mut self, at: Pos2) {
        let press = |pressed| Event::PointerButton {
            pos: at,
            button: PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        self.frame(vec![Event::PointerMoved(at)]);
        self.frame(vec![press(true)]);
        self.frame(vec![press(false)]);
        self.frame(vec![]);
    }

    fn key(&mut self, key: egui::Key) {
        self.chord(Modifiers::NONE, key);
    }

    fn chord(&mut self, modifiers: Modifiers, key: egui::Key) {
        for pressed in [true, false] {
            self.frame(vec![Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers,
            }]);
        }
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

/// Nick 2026-11-01 (project-history): the page lists the box's revisions
/// after each save, reads one whole, and restores it as an engineer.
#[test]
fn the_page_lists_reads_and_restores_the_boxs_revisions() {
    let b = Bench::new();
    let jar = Jar::default();
    let mut v = page(&b, &jar, Some(("alice", ALICE)));
    let p = &mut v.projects;
    make(&b, &jar, p, "line", &[("main.st", MAIN)]);
    settle(&b, &jar, p);
    let base = p.history.len();
    assert!(base >= 1, "{:?}", p.history);
    assert_eq!(p.history[0].what, "saved main.st");
    assert_eq!(p.history[0].by, "alice (engineer)");
    p.open_file("main.st");
    let edited = MAIN.replace("reset := FALSE", "reset := NOT start");
    p.current_mut().unwrap().text.clone_from(&edited);
    p.save();
    settle(&b, &jar, p);
    assert_eq!(p.history.len(), base + 1, "a save is a revision");
    // The last revision `make` wrote holds MAIN (its first is the empty file).
    let last = u32::try_from(base).unwrap();
    p.read_revision("line", last);
    settle(&b, &jar, p);
    let view = p.revision.clone().expect("the revision on view");
    assert_eq!(view.row.n, last);
    assert_eq!(view.files["main.st"], MAIN);
    assert_eq!(view.page, None);
    p.restore("line", last);
    settle(&b, &jar, p);
    assert!(p.revision.is_none(), "shut after the restore");
    assert_eq!(p.current().unwrap().text, MAIN, "the file came back");
    assert_eq!(p.history.len(), base + 2);
    assert_eq!(p.history[base + 1].what, format!("restored revision {last}"));
    assert_eq!(b.json("/api/projects/line/files/main.st")["text"], MAIN);
    // An operator sees the history and cannot restore.
    let jar2 = Jar::default();
    let mut w = page(&b, &jar2, Some(("bob", BOB)));
    let q = &mut w.projects;
    q.open_project("line");
    settle(&b, &jar2, q);
    assert_eq!(q.history.len(), base + 2);
    q.restore("line", 2);
    settle(&b, &jar2, q);
    assert_eq!(q.history.len(), base + 2, "nothing restored");
    assert!(
        q.notice
            .as_ref()
            .is_some_and(|(n, err)| *err && n.contains("not restored")),
        "{:?}",
        q.notice
    );
}

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

    // Another page (signed in: every read needs a session, card read-lock)
    // lists it and opens it with every file.
    let mut other = BoxProjects::new();
    let jar2 = Jar::default();
    b.login(&jar2, "bob", BOB);
    settle(&b, &jar2, &mut other);
    assert_eq!(other.open.as_ref().unwrap().name, "line");
    assert_eq!(files(&other), ["blocks.st", "io.map", "main.st"]);
    assert_eq!(other.current().unwrap().text, MAIN, "main.st first");
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
    h.press("bottom messages");
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

/// A requirement test that wants the lamp off while start is held: red
/// against MAIN (the latch turns the lamp on).
const TEST_RED: &str = "\
(* The lamp stays off while start is held. *)
PROGRAM test_lamp
VAR_INPUT
  lamp : BOOL;
END_VAR
VAR_OUTPUT
  start : BOOL;
  done : BOOL;
END_VAR
VAR
  t : TON;
  lamp_stays_off : ASSERT;
END_VAR
start := TRUE;
t(IN := TRUE, PT := T#50ms);
IF t.Q THEN
  lamp_stays_off(COND := NOT lamp);
  done := TRUE;
END_IF;
END_PROGRAM
";

/// The same test, wanting the lamp on: green against MAIN.
const TEST_GREEN: &str = "\
(* The lamp is on within 50 ms of start. *)
PROGRAM test_lamp
VAR_INPUT
  lamp : BOOL;
END_VAR
VAR_OUTPUT
  start : BOOL;
  done : BOOL;
END_VAR
VAR
  t : TON;
  lamp_follows_start : ASSERT;
END_VAR
start := TRUE;
t(IN := TRUE, PT := T#50ms);
IF t.Q THEN
  lamp_follows_start(COND := lamp);
  done := TRUE;
END_IF;
END_PROGRAM
";

/// Requirement tests on a box (TODO.md spec-tests): opening a project runs
/// its `test_*.st` files on the box, red or green with why; a red one
/// refuses the deploy, saying which; a save runs them again, and green,
/// the deploy goes through. (The tab that listed them is archived,
/// 2026-11-08; the gate stays.)
#[test]
fn requirement_tests_show_on_the_box_and_a_red_one_refuses_the_deploy() {
    let b = Bench::new();
    let jar = Jar::default();
    let mut v = page(&b, &jar, Some(("alice", ALICE)));
    let p = &mut v.projects;
    make(
        &b,
        &jar,
        p,
        "line",
        &[
            ("main.st", MAIN),
            ("blocks.st", BLOCKS),
            ("io.map", MAP),
            ("test_lamp.st", TEST_RED),
        ],
    );
    settle(&b, &jar, p);
    assert!(p.tests.ran, "opening the project ran its tests on the box");
    assert_eq!(p.tests.rows.len(), 1, "{:?}", p.tests);
    assert!(!p.tests.rows[0].passed());
    assert_eq!(
        p.tests.rows[0].requirement,
        "The lamp stays off while start is held."
    );
    assert!(
        p.tests.rows[0].why.starts_with("lamp stays off failed at "),
        "{:?}",
        p.tests
    );
    assert_eq!(p.tests.red().len(), 1);

    // Deploy: refused by the box, naming the test; nothing runs from it.
    p.deploy();
    settle(&b, &jar, p);
    let Deploy::Failed(why) = &p.deploy else {
        panic!("deployed past a red test: {:?}", p.deploy);
    };
    assert!(
        why.contains("requirement test") && why.contains("test_lamp"),
        "{why}"
    );
    assert!(p.running.is_none());

    // Fixed and saved: the box runs them again, green, and deploys.
    let p = &mut v.projects;
    p.open_file("test_lamp.st");
    p.current_mut().unwrap().text = TEST_GREEN.to_string();
    p.save();
    settle(&b, &jar, p);
    assert!(p.tests.rows[0].passed(), "{:?}", p.tests);
    p.deploy();
    settle(&b, &jar, p);
    assert!(
        matches!(p.deploy, Deploy::Done(_)),
        "{:?} {:?}",
        p.deploy,
        p.tests
    );
    assert_eq!(p.running.as_deref(), Some("line"));
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
    h.app.chrome.show_code();
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
    h.press("bottom messages");
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
            None => {
                assert!(why.contains("Sign in as an engineer"), "{why}");
                // Signed out, the box answers no read (card read-lock):
                // nothing is listed or opened until a sign-in.
                assert!(v.projects.open.is_none(), "nothing listed or opened");
                continue;
            }
            Some(_) => assert!(
                why.contains("bob (operator)") && why.contains("only an engineer"),
                "{why}"
            ),
        }
        let session = v.session.clone();
        let mut h = Harness::new(v, session);
        h.app.chrome.show_code();
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
        // The sidebar's buttons are on the Files tab; Rename and Delete
        // are its right-click menu's (disabled here) and the Delete key's
        // (nothing happens without write access).
        h.app.chrome.show_files();
        h.wait(0.3);
        for k in ["save", "new file"] {
            h.press(k);
        }
        let row = h.rect("project line").center();
        h.right_click(row);
        h.press("menu rename project");
        h.right_click(row);
        h.press("menu delete project");
        h.key(egui::Key::Backspace);
        assert_eq!(h.bv().files_ui.confirm, None, "no delete asked for");
        // New project is the File menu's (disabled here; pressing it is nothing).
        h.press("bar file");
        h.press("file new");
        // Deploy is the File menu's (disabled here; pressing it is nothing).
        h.press("bar file");
        h.press("file deploy");
        // ... and a file's right-click menu's (disabled here too).
        h.right_click(h.rect("file main.st").center());
        h.press("menu deploy");
        // ... and the arrow by the Structured Text header (disabled here).
        h.app.chrome.show_code();
        h.wait(0.3);
        h.press("code deploy");
        // Reads (the Trend's pins) go; no write does.
        let sent: Vec<PReq> = h.sent().into_iter().filter(|q| q.method != "GET").collect();
        assert!(sent.is_empty(), "{sent:?}");
        assert_eq!(h.app.chrome.rect("name"), None, "no name asked for");
        // A force asked for in code is not sent either; the box would
        // refuse it anyway.
        h.bv().force("start", helmsman_iec::Value::Bool(false));
        let sent: Vec<PReq> = h.sent().into_iter().filter(|q| q.method != "GET").collect();
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
    // Right-click a file › Deploy starts the deploy, the check first
    // (Nick, 2026-11-05).
    h.app.chrome.show_files();
    h.wait(0.3);
    h.right_click(h.rect("file main.st").center());
    h.press("menu deploy");
    let sent = h.sent();
    assert!(
        sent.iter()
            .any(|q| q.method == "POST" && q.url.ends_with("/check")),
        "{sent:?}"
    );
    h.bv().projects.deploy = Deploy::Idle;
    h.bv().projects.current_mut().unwrap().text.push_str("(* mine *)");
    h.frame(vec![]);
    assert!(h.app.chrome.rect("dirty").is_some(), "the header marks it");
    h.press("save");
    let sent = h.sent();
    assert!(sent.iter().any(|q| q.method == "PUT"), "{sent:?}");
    // The arrow right of the Structured Text header deploys too (Nick,
    // 2026-11-08). That save is still on its way (the arrow waits while
    // one is); done, the edit is still dirty, so the deploy saves first.
    h.bv().projects.deploy = Deploy::Idle;
    for b in h.bv().projects.open.as_mut().unwrap().bufs.values_mut() {
        b.saving = None;
    }
    h.app.chrome.show_code();
    h.wait(0.3);
    h.press("code deploy");
    assert_eq!(
        h.bv().projects.deploy,
        Deploy::Saving,
        "saves, then checks and deploys"
    );
    let sent = h.sent();
    assert!(sent.iter().any(|q| q.method == "PUT"), "{sent:?}");
}

/// Alice's page on the bench box, watched until the running module is
/// drawn with a scan.
fn drawn(b: &Bench, jar: &Jar) -> BoxView {
    let ctx = egui::Context::default();
    let mut v = page(b, jar, Some(("alice", ALICE)));
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut now = 0.0;
    while v.view.is_none() || v.state.scan.is_none() {
        assert!(Instant::now() < deadline, "never drawn: {:?}", v.state.problem);
        v.step_all(&ctx, vec![], vec![], now);
        pump(b, jar, &mut v, &ctx, now);
        if let Some(name) = v.state.selected.clone() {
            for (ev, data) in b.events(&crate::remote::trace_url(&name), 1) {
                v.state.apply(Msg::Event(Stream::Trace, ev, data), now);
            }
        }
        now += 0.1;
    }
    v
}

#[test]
fn an_engineer_forces_an_input_from_the_diagram_and_releases_all() {
    let b = Bench::new();
    let jar = Jar::default();
    let v = drawn(&b, &jar);
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
fn ctrl_shift_f_forces_the_picked_input_and_ctrl_shift_u_releases_all() {
    let b = Bench::new();
    let jar = Jar::default();
    let mut v = drawn(&b, &jar);
    let view = v.view.as_mut().expect("drawn");
    let read = view
        .diagram()
        .nodes
        .iter()
        .find(|n| matches!(&n.kind, helmsman_diagram::NodeKind::Read { path } if path.eq_ignore_ascii_case("start")))
        .expect("a read of start")
        .id;
    view.pick = Some(crate::st_view::Pick::Node(read));
    let session = v.session.clone();
    let mut h = Harness::new(v, session);
    h.frame(vec![]);
    h.chord(Modifiers::ALT.plus(Modifiers::SHIFT), egui::Key::F);
    let reqs = h.sent();
    let post = reqs
        .iter()
        .find(|q| q.url == "/api/forces" && q.method == "POST")
        .unwrap_or_else(|| panic!("{reqs:?}"));
    let body = String::from_utf8_lossy(post.body.as_deref().unwrap_or_default()).into_owned();
    assert!(body.contains("false"), "start (on) flips to FALSE: {body}");
    for q in reqs {
        let a = b.send(&jar, &q);
        h.bv().projects.answer(q.call, &a);
    }
    settle(&b, &jar, &mut h.bv().projects);
    h.frame(vec![]);
    assert_eq!(h.bv().state.forces.len(), 1);
    h.chord(Modifiers::ALT.plus(Modifiers::SHIFT), egui::Key::U);
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
    assert_eq!(b.json("/api/forces")["forces"], serde_json::json!([]));
}

#[test]
fn ctrl_shift_f_sends_nothing_without_an_engineer_signed_in() {
    let b = Bench::new();
    let jar = Jar::default();
    let mut v = drawn(&b, &jar);
    let view = v.view.as_mut().expect("drawn");
    let read = view
        .diagram()
        .nodes
        .iter()
        .find(|n| matches!(&n.kind, helmsman_diagram::NodeKind::Read { path } if path.eq_ignore_ascii_case("start")))
        .expect("a read of start")
        .id;
    view.pick = Some(crate::st_view::Pick::Node(read));
    v.session = None;
    let mut h = Harness::new(v, None);
    h.frame(vec![]);
    h.sent();
    h.chord(Modifiers::ALT.plus(Modifiers::SHIFT), egui::Key::F);
    h.chord(Modifiers::ALT.plus(Modifiers::SHIFT), egui::Key::U);
    let reqs = h.sent();
    assert!(!reqs.iter().any(|q| q.url.starts_with("/api/forces")), "{reqs:?}");
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
    make(
        &b,
        &jar,
        &mut v.projects,
        "line",
        &[("main.st", MAIN), ("blocks.st", BLOCKS), ("io.map", MAP)],
    );
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

/// A browser's storage two page loads share (`localStorage`).
#[derive(Clone, Default)]
struct Slot(std::rc::Rc<std::cell::RefCell<Option<String>>>);

impl crate::sim_projects::Storage for Slot {
    fn load(&self) -> Result<Option<String>, String> {
        Ok(self.0.borrow().clone())
    }

    fn save(&mut self, text: &str) -> Result<(), String> {
        *self.0.borrow_mut() = Some(text.to_string());
        Ok(())
    }
}

/// A trend page: signed in as `who`, its browser's pins in `slot`, run
/// until its requests are answered.
fn trend_page(b: &Bench, jar: &Jar, who: Option<(&str, &str)>, slot: &Slot) -> BoxView {
    let mut v = BoxView::with_pins(Box::new(slot.clone()));
    v.session = who.map(|(u, p)| b.login(jar, u, p));
    let ctx = egui::Context::default();
    for k in 0..6 {
        pump(b, jar, &mut v, &ctx, f64::from(k));
    }
    v
}

/// Card `box-trend-source`: pins follow the signed-in person (or stay in
/// the browser signed out), and a Trend opens with the box's history.
#[test]
fn the_trend_on_a_box_opens_with_history_and_pins_follow_the_person() {
    let b = Bench::new();
    let ctx = egui::Context::default();
    let jar = Jar::default();
    let mut a = trend_page(&b, &jar, Some(("alice", ALICE)), &Slot::default());
    assert_eq!(a.trend.pins, Vec::<String>::new());
    let session = a.session.clone();
    a.trend.toggle("lamp", session.as_ref(), 10.0);
    for k in 10..14 {
        pump(&b, &jar, &mut a, &ctx, f64::from(k));
    }
    assert_eq!(a.trend.notice, None);
    assert_eq!(b.json("/api/trends?signals=lamp")["signals"][0]["name"], "lamp");

    // Alice on another screen: her pins, and the history before any
    // values event of that page.
    let jar2 = Jar::default();
    let mut v = BoxView::with_pins(Box::new(Slot::default()));
    v.session = Some(b.login(&jar2, "alice", ALICE));
    // Only the page's own requests are answered: no values stream.
    v.step_all(&ctx, vec![], vec![], 0.0);
    for _ in 0..4 {
        let answers: Vec<_> = std::mem::take(&mut v.psent)
            .into_iter()
            .map(|q| {
                let a = b.send(&jar2, &q);
                (q.call, a)
            })
            .collect();
        v.step_all(&ctx, vec![], answers, 1.0);
    }
    assert_eq!(v.trend.pins, ["lamp"], "her pins follow her");
    wait("lamp's history on open", || {
        let r = b.send(
            &jar2,
            &PReq::get(
                crate::box_projects::PCall::Trend,
                "/api/trends?signals=lamp".into(),
            ),
        );
        v.trend.answer(&crate::box_projects::PCall::Trend, &r, 2.0);
        v.trend
            .series
            .get("lamp")
            .is_some_and(|s| s.iter().any(|p| (p.1 - 1.0).abs() < 1e-9))
    });
    assert_eq!(v.state.seq, None, "no values event yet: it is the box's history");

    // Bob's pins are his own.
    let bob = trend_page(&b, &Jar::default(), Some(("bob", BOB)), &Slot::default());
    assert_eq!(bob.trend.pins, Vec::<String>::new());

    // Signed out: this browser keeps the pins, across a reload.
    let slot = Slot::default();
    let jar3 = Jar::default();
    let mut anon = trend_page(&b, &jar3, None, &slot);
    anon.trend.toggle("start", None, 20.0);
    assert!(anon.psent.is_empty() || anon.psent.iter().all(|q| q.method == "GET"));
    let anon = trend_page(&b, &jar3, None, &slot);
    assert_eq!(anon.trend.pins, ["start"]);

    // Hostile pin lists: too many, unknown signals, no session.
    let send = |jar: &Jar, body: Json| {
        b.send(
            jar,
            &PReq::json(
                crate::box_projects::PCall::SavePins,
                "PUT",
                "/api/trends/pins".into(),
                &body,
            ),
        )
    };
    let many: Vec<String> = (0..17).map(|k| format!("s{k}")).collect();
    assert_eq!(send(&jar, serde_json::json!({ "signals": many })).status, 400);
    let r = send(&jar, serde_json::json!({ "signals": ["lamp", "nosuch"] }));
    assert_eq!(r.status, 422, "{r:?}");
    assert!(r.body.contains("nosuch"));
    assert_eq!(send(&jar, serde_json::json!({ "signals": "lamp" })).status, 400);
    assert_eq!(
        send(&Jar::default(), serde_json::json!({ "signals": ["lamp"] })).status,
        401
    );
    assert!(b.json("/api/trends/pins")["error"].is_string());
}

/// Card `arrangement-persist`: an engineer saves the diagram's arrangement
/// as the running project's default (its `diagram.layout`, so it travels
/// in the `.zip`); every page reads it; an operator cannot save one.
#[test]
fn the_shared_layout_round_trips_through_the_running_project_for_an_engineer_only() {
    let b = Bench::new();
    let jar = Jar::default();
    let mut eng = page(&b, &jar, Some(("alice", ALICE)));
    make(
        &b,
        &jar,
        &mut eng.projects,
        "line",
        &[("main.st", MAIN), ("blocks.st", BLOCKS), ("io.map", MAP)],
    );
    // Nothing deployed from a project yet: no project to keep it in.
    assert!(eng.projects.running.is_none());
    eng.projects.save_layout("helmsman-layout 1\n".into());
    assert!(eng.projects.poll().is_empty() && eng.projects.notice.as_ref().unwrap().1);
    eng.projects.deploy();
    settle(&b, &jar, &mut eng.projects);
    assert_eq!(eng.projects.running.as_deref(), Some("line"));
    let none = eng.projects.layout.clone().expect("read after the deploy");
    assert_eq!((none.text.as_str(), none.version), ("", None), "no default yet");

    let text = "helmsman-layout 1\nmove\tconveyor\t0\t0\t120\tLAMP :=\tLAMP := L.Q\nmove\tLatch\t0\t40\t0\tQ :=\tQ := (Q OR SET) AND NOT RESET\n";
    eng.projects.save_layout(text.into());
    settle(&b, &jar, &mut eng.projects);
    assert_eq!(
        eng.projects.layout_saved.as_deref(),
        Some(text),
        "{:?}",
        eng.projects.notice
    );
    assert_eq!(eng.projects.shared_layout(), text);
    assert_eq!(
        b.json("/api/projects/line/files/diagram.layout")["text"],
        text,
        "kept on the box"
    );
    let (_, zip) = b.get("/api/projects/line/zip");
    let files = helmsman_project::read_zip(&zip).unwrap();
    assert_eq!(files.get("diagram.layout"), Some(text), "in the .zip");
    // The layout is no program change: what runs is still the project.
    assert_eq!(b.json("/api/running")["current"], true);

    // Another page, any signed-in role, starts from it; signed out, the
    // box answers no read (card read-lock), so there is nothing to start
    // from.
    for who in [None, Some(("bob", BOB))] {
        let jar = Jar::default();
        let mut v = page(&b, &jar, who);
        if who.is_none() {
            // Nothing read, so nothing to start from and nothing to save.
            assert_eq!(v.projects.shared_layout(), "", "nothing read signed out");
            continue;
        }
        assert_eq!(v.projects.shared_layout(), text);
        // An operator's page offers no Save as default and sends nothing.
        v.save_layout();
        assert_eq!(v.projects.poll(), Vec::<PReq>::new());
        // Asked for anyway, the box refuses it and keeps the engineer's.
        v.projects.save_layout("helmsman-layout 1\n".into());
        settle(&b, &jar, &mut v.projects);
        let (msg, err) = v.projects.notice.clone().unwrap();
        // Signed out, a 401 (sign in again); an operator, a 403.
        let want = if who.is_some() {
            "not saved"
        } else {
            "Sign in again"
        };
        assert!(err && msg.contains(want), "{msg}");
        assert!(v.projects.layout_saved.is_none());
        assert_eq!(b.json("/api/projects/line/files/diagram.layout")["text"], text);
    }

    // A stale save (someone else saved since) is a 409, read again.
    let jar2 = Jar::default();
    let mut other = page(&b, &jar2, Some(("alice", ALICE)));
    let newer = "helmsman-layout 1\nmove\tconveyor\t0\t0\t300\tLAMP :=\tLAMP := L.Q\n";
    other.projects.save_layout(newer.into());
    settle(&b, &jar2, &mut other.projects);
    eng.projects.save_layout("helmsman-layout 1\n".into());
    settle(&b, &jar, &mut eng.projects);
    let (msg, err) = eng.projects.notice.clone().unwrap();
    assert!(err && msg.contains("Someone else saved"), "{msg}");
    assert_eq!(eng.projects.shared_layout(), newer, "read again");
    // A layout that does not read is refused by the box.
    let bad = PReq {
        call: crate::box_projects::PCall::SaveLayout("line".into()),
        method: "PUT",
        url: "/api/projects/line/files/diagram.layout".into(),
        body: Some(
            serde_json::json!({ "text": "move\tx\n", "version": null })
                .to_string()
                .into_bytes(),
        ),
        ctype: "application/json",
    };
    assert_eq!(b.send(&jar, &bad).status, 422);
}

/// Every example arrives on the box as a project of its own, named as asked,
/// and deploys there against the real API as it stands.
#[test]
fn a_new_project_from_any_example_deploys_on_the_box() {
    let b = Bench::build(true);
    let jar = Jar::default();
    let mut v = page(&b, &jar, Some(("alice", ALICE)));
    let p = &mut v.projects;
    for e in &crate::playground::EXAMPLES {
        let (from, file) = crate::sim_projects::example_project(e.src);
        let name = format!("my-{from}").replace('_', "-");
        p.create_from(&name, &from);
        settle(&b, &jar, p);
        assert_eq!(p.open.as_ref().unwrap().name, name, "{:?}", p.notice);
        let mut want = vec![file.as_str(), "io.map"];
        want.sort_unstable();
        assert_eq!(files(p), want);
        assert_eq!(p.current().unwrap().text, e.src, "{from}");
        p.deploy();
        settle(&b, &jar, p);
        assert!(
            matches!(p.deploy, Deploy::Done(_)),
            "{from}: {:?} {:?}",
            p.deploy,
            p.diags
        );
        assert_eq!(p.running.as_deref(), Some(name.as_str()));
        // It runs there, with nothing wired: not held for a missing input.
        wait("the program scanning", || {
            b.events("/api/events", 1).iter().any(|(_, data)| {
                let j: Json = serde_json::from_str(data).unwrap_or_default();
                j["modules"].as_array().is_some_and(|m| {
                    m.iter().any(|m| {
                        m["status"]["state"] == "running" && m["stats"]["scans"].as_u64().unwrap_or(0) > 0
                    })
                })
            })
        });
    }
    // A name that is taken is not overwritten without asking.
    p.create_from("my-blink", "counter");
    settle(&b, &jar, p);
    let (msg, err) = p.notice.clone().unwrap();
    assert!(err && msg.contains("Replace it?"), "{msg}");
    assert!(
        b.json("/api/projects/my-blink/files/blink.st")["text"]
            .as_str()
            .unwrap()
            .contains("PROGRAM blink"),
        "still blink"
    );
}

/// The list is an engineer's: anyone else sees it disabled and sends nothing.
#[test]
fn only_an_engineer_can_start_a_project_from_the_list() {
    let b = Bench::new();
    let jar = Jar::default();
    let v = page(&b, &jar, Some(("alice", ALICE)));
    let session = v.session.clone();
    let mut h = Harness::new(v, session);
    h.wait(0.2);
    h.press("starter latch");
    h.wait(0.1);
    assert!(h.app.chrome.rect("name").is_some(), "the name is asked for");
    h.press("name ok");
    let sent = h.sent();
    let up = sent
        .iter()
        .find(|q| q.method == "POST")
        .unwrap_or_else(|| panic!("nothing sent: {sent:?}"));
    assert_eq!(up.url, "/api/projects/latch/zip");
    assert_eq!(up.ctype, "application/zip");
    for q in sent {
        let a = b.send(&jar, &q);
        h.bv().projects.answer(q.call, &a);
    }
    assert_eq!(b.get("/api/projects/latch/files/latch.st").0, 200);
    assert_eq!(b.get("/api/projects/latch/files/io.map").0, 200);

    let jar = Jar::default();
    let v = page(&b, &jar, Some(("bob", BOB)));
    let session = v.session.clone();
    let mut h = Harness::new(v, session);
    h.wait(0.2);
    h.press("starter latch");
    h.wait(0.1);
    assert_eq!(h.app.chrome.rect("name"), None, "disabled for an operator");
    let writes: Vec<PReq> = h.sent().into_iter().filter(|q| q.method != "GET").collect();
    assert!(writes.is_empty(), "{writes:?}");
}

/// Nick, 2026-11-07: "the interface and inputs need to be more
/// persistent so they don't just disappear during a refresh". A page
/// reloaded on the same browser comes back on the project and file it
/// showed, with its unsaved edit and its half-typed inputs; an edit made
/// against a file someone has saved since is refused as stale on save
/// rather than overwriting their change.
#[test]
fn a_reload_puts_back_the_project_the_file_the_edits_and_the_inputs() {
    use crate::desk::{Desk, MAX_BYTES};
    use crate::sim_projects::MemStorage;
    let b = Bench::new();
    let (ja, jb) = (Jar::default(), Jar::default());
    let mut a = page(&b, &ja, Some(("alice", ALICE)));
    make(&b, &ja, &mut a.projects, "aaa", &[("main.st", MAIN)]);
    make(
        &b,
        &ja,
        &mut a.projects,
        "line",
        &[("main.st", MAIN), ("blocks.st", BLOCKS)],
    );
    a.projects.open_project("line");
    settle(&b, &ja, &mut a.projects);
    a.projects.open_file("blocks.st");
    let edited = format!("{BLOCKS}\n(* half done *)\n");
    a.projects.current_mut().unwrap().text = edited.clone();
    a.instruct.draft = "make the lamp blink at 2 Hz".into();
    let (kept, dropped) = a.desk().text_within(MAX_BYTES);
    assert_eq!(dropped, Vec::<String>::new());
    assert!(!kept.contains(ALICE), "no password in the desk");

    // The reload: a new page on the same browser.
    let mut r = BoxView::new().with_desk(Box::new(MemStorage(Some(kept.clone()))));
    r.session = Some(b.login(&ja, "alice", ALICE));
    settle(&b, &ja, &mut r.projects);
    let o = r.projects.open.as_ref().expect("a project open");
    assert_eq!(o.name, "line", "the project it showed, not the first");
    assert_eq!(o.current.as_deref(), Some("blocks.st"));
    assert_eq!(o.bufs["blocks.st"].text, edited);
    assert!(r.projects.dirty(), "still an edit to save");
    assert!(
        r.projects
            .notice
            .as_ref()
            .unwrap()
            .0
            .contains("blocks.st are back")
    );
    assert_eq!(r.instruct.draft, "make the lamp blink at 2 Hz");
    // Saved, the draft is gone from the desk.
    r.projects.save();
    settle(&b, &ja, &mut r.projects);
    assert_eq!(
        b.json("/api/projects/line/files/blocks.st")["text"],
        edited.as_str()
    );
    assert!(r.desk().drafts.is_empty(), "{:?}", r.desk().drafts);

    // Someone saves the file while this page's edit waits in the desk.
    let mut stale = Desk::parse(&kept);
    stale
        .drafts
        .get_mut("line")
        .unwrap()
        .get_mut("blocks.st")
        .unwrap()
        .text = format!("{BLOCKS}\n(* mine *)\n");
    let mut other = page(&b, &jb, Some(("alice", ALICE)));
    other.projects.open_project("line");
    settle(&b, &jb, &mut other.projects);
    other.projects.open_file("blocks.st");
    other.projects.current_mut().unwrap().text = format!("{BLOCKS}\n(* theirs *)\n");
    other.projects.save();
    settle(&b, &jb, &mut other.projects);
    let mut r = BoxView::new().with_desk(Box::new(MemStorage(Some(stale.text_within(MAX_BYTES).0))));
    r.session = Some(b.login(&ja, "alice", ALICE));
    settle(&b, &ja, &mut r.projects);
    assert!(r.projects.notice.as_ref().unwrap().0.contains("changed since"));
    r.projects.save();
    settle(&b, &ja, &mut r.projects);
    assert_eq!(
        r.projects.conflict.as_deref(),
        Some("blocks.st"),
        "{:?}",
        r.projects.notice
    );
    assert!(
        b.json("/api/projects/line/files/blocks.st")["text"]
            .as_str()
            .unwrap()
            .contains("theirs"),
        "their save stands"
    );

    // Signed in as an operator (a read needs a session, card read-lock),
    // the edit still comes back; saving it waits for an engineer's sign-in.
    let mut r = BoxView::new().with_desk(Box::new(MemStorage(Some(stale.text_within(MAX_BYTES).0))));
    let jar3 = Jar::default();
    b.login(&jar3, "bob", BOB);
    settle(&b, &jar3, &mut r.projects);
    assert!(r.projects.dirty());
}

/// The screen and the sign-in name come back after a reload too; a
/// password typed into the form is never kept.
#[test]
fn a_reload_comes_back_on_the_monitor_with_the_sign_in_name() {
    use crate::app::Screen;
    use crate::desk::{Desk, MAX_BYTES};
    use crate::sim_projects::MemStorage;
    let d = Desk {
        screen: Some("monitor".into()),
        user: "alice".into(),
        ..Desk::default()
    };
    let bv = BoxView::new().with_desk(Box::new(MemStorage(Some(d.text_within(MAX_BYTES).0))));
    let mut h = Harness::new(bv, None);
    assert_eq!(h.app.screen, Screen::Monitor);
    let r = h.app.review.as_mut().unwrap();
    assert_eq!(r.user_input, "alice");
    r.password_input = "hunter2-secret".into();
    h.app.screen = Screen::Editor;
    h.frame(vec![]);
    let kept = h.bv().desk();
    assert_eq!(kept.screen.as_deref(), Some("editor"));
    assert_eq!(kept.user, "alice");
    assert!(!kept.text_within(MAX_BYTES).0.contains("hunter2"), "no password");
}

/// The box's Agent tab takes the page's path (`crate::instruct`, Nick
/// 2026-11-05): the instruction goes to `POST /api/chat` with the answer
/// format and the whole file in a new conversation; a checked answer is
/// saved on the box with its History entry and its scenario as a test;
/// Undo restores the revision before it; a follow-up carries the earlier
/// instruction, read back from the box's History; a red answer is sent
/// back in the same conversation and, still red, never written.
#[test]
fn the_agent_on_a_box_saves_a_checked_answer_with_its_history_and_undo_restores_it() {
    use crate::box_agent::Incoming;
    use crate::instruct::{BOX_RULES, Kind};
    use crate::instruct_tests::{LAMP, st};
    use serde_json::json;

    /// The agent's turn for the last message: its words, then done.
    fn answer(v: &mut BoxView, text: &str) {
        let seq = v.chat_sent.last().expect("a message").seq;
        let ev = |n: &str, d: Json| format!("event: {n}\ndata: {d}\n\n");
        let body = [
            ev("session", json!({"id": "s1"})),
            ev("text", json!({ "delta": text })),
            ev("done", json!({"stop": "end_turn", "why": null})),
        ]
        .concat();
        v.agent.apply(Incoming::Chunk { seq, text: body });
        v.agent.apply(Incoming::End { seq });
    }
    /// A frame, then the box's answers carried out until quiet.
    fn run(b: &Bench, jar: &Jar, v: &mut BoxView, ctx: &egui::Context) {
        v.tick(ctx);
        for _ in 0..20 {
            pump(b, jar, v, ctx, 1.0);
            if v.psent.is_empty() {
                return;
            }
        }
        panic!("the box never went quiet");
    }
    fn body(v: &BoxView, k: usize) -> Json {
        serde_json::from_str(&v.chat_sent[k].body).unwrap()
    }

    let b = Bench::new();
    let jar = Jar::default();
    let ctx = egui::Context::default();
    let mut v = page(&b, &jar, Some(("alice", ALICE)));
    make(&b, &jar, &mut v.projects, "line", &[("main.st", LAMP)]);
    v.projects.open_file("main.st");
    run(&b, &jar, &mut v, &ctx);
    let before = v.projects.history.last().map(|r| r.n).expect("revisions");

    v.instruct.draft = "comment it".into();
    v.send_instruction(&ctx);
    assert_eq!(v.chat_sent.len(), 1, "{:?}", v.instruct.outcome);
    let m = body(&v, 0);
    assert_eq!(m["session"], Json::Null);
    let message = m["message"].as_str().unwrap();
    assert!(message.starts_with(BOX_RULES));
    assert!(
        message.contains("\n\ncomment it\n\n<current_program>"),
        "{message}"
    );
    assert!(
        message.contains(&format!("text:\n{LAMP}</edit_scope>")),
        "{message}"
    );
    let new = LAMP.replace("lamp := start;", "lamp := start; (* follows start *)");
    answer(&mut v, &st(&new));
    run(&b, &jar, &mut v, &ctx);
    let o = v.instruct.outcome.clone().unwrap();
    assert_eq!(o.kind, Kind::Applied, "{o:?}");
    assert_eq!(o.status, "Updated main.st: 1 scenario passed, kept as a test.");
    assert_eq!(o.why.as_deref(), Some("it says so."));
    assert_eq!(o.undo.as_ref().map(|u| u.revision), Some(before));
    assert_eq!(b.json("/api/projects/line/files/main.st")["text"], new.as_str());
    assert!(
        b.json("/api/projects/line/files/test_lamp_follows.st")["text"]
            .as_str()
            .is_some_and(|t| t.starts_with("(* The lamp follows start. *)")),
        "the scenario is a test on the box"
    );
    let saved = v
        .projects
        .history
        .iter()
        .find(|r| r.what == "took the Agent's answer into main.st")
        .cloned()
        .expect("the change's revision");
    assert!(saved.page);
    let page = b.json(&format!("/api/projects/line/history/{}", saved.n));
    assert!(
        page["page"]
            .as_str()
            .is_some_and(|p| p.starts_with("Instruction: comment it\nFile: main.st\nWhy: it says so.\n")),
        "{page}"
    );
    assert_eq!(v.projects.current().unwrap().text, new, "the editor shows it");

    // Undo: the box restores the revision before it.
    v.undo_change();
    run(&b, &jar, &mut v, &ctx);
    assert_eq!(v.instruct.outcome.as_ref().map(|o| o.kind), Some(Kind::Undone));
    assert_eq!(b.json("/api/projects/line/files/main.st")["text"], LAMP);
    assert_eq!(
        v.projects.history.last().map(|r| r.what.clone()),
        Some(format!("restored revision {before}"))
    );

    // A follow-up: the earlier instruction comes from the box's History;
    // a red answer goes back in the same conversation, then is refused.
    run(&b, &jar, &mut v, &ctx);
    v.instruct.draft = "now invert it".into();
    v.send_instruction(&ctx);
    let m = body(&v, 1);
    assert_eq!(m["session"], Json::Null, "a new conversation per instruction");
    assert!(
        m["message"]
            .as_str()
            .unwrap()
            .contains("<earlier_instructions>\n- comment it\n</earlier_instructions>"),
        "{m}"
    );
    let red = st(&LAMP.replace("lamp := start;", "lamp := NOT start;"));
    answer(&mut v, &red);
    run(&b, &jar, &mut v, &ctx);
    assert_eq!(v.chat_sent.len(), 3);
    let m = body(&v, 2);
    assert_eq!(m["session"], "s1", "the retry continues the conversation");
    assert!(
        m["message"]
            .as_str()
            .unwrap()
            .starts_with("The page checked your answer before showing it"),
        "{m}"
    );
    answer(&mut v, &red);
    run(&b, &jar, &mut v, &ctx);
    answer(&mut v, &red);
    run(&b, &jar, &mut v, &ctx);
    let o = v.instruct.outcome.clone().unwrap();
    assert_eq!(o.kind, Kind::Refused, "{o:?}");
    assert!(o.problems.iter().any(|p| p.contains("lamp on failed")), "{o:?}");
    assert_eq!(
        b.json("/api/projects/line/files/main.st")["text"],
        LAMP,
        "never written"
    );
}

/// A program file of realistic size (60 KiB) goes to the box's agent once,
/// with the answer format and its tests, inside what the box takes; before,
/// the format and two copies of the file went over 8000 characters at
/// about 2.5 KB (review 2026-11-08).
#[test]
fn a_realistic_file_reaches_the_box_agent_once() {
    use crate::box_agent::MAX_MESSAGE;
    use crate::instruct::BOX_RULES;
    use crate::instruct_tests::{FOLLOWS, LAMP};
    let b = Bench::new();
    let jar = Jar::default();
    let ctx = egui::Context::default();
    let mut v = page(&b, &jar, Some(("alice", ALICE)));
    let big = format!("{}{LAMP}", "(* plant notes for the next shift *)\n".repeat(1_600));
    assert!(big.len() > 58_000, "{}", big.len());
    make(
        &b,
        &jar,
        &mut v.projects,
        "big",
        &[("main.st", big.as_str()), ("test_lamp.st", FOLLOWS)],
    );
    v.projects.open_file("main.st");
    v.tick(&ctx);
    v.instruct.draft = "comment it".into();
    v.send_instruction(&ctx);
    assert_eq!(v.chat_sent.len(), 1, "{:?}", v.instruct.outcome);
    let m: Json = serde_json::from_str(&v.chat_sent[0].body).unwrap();
    let message = m["message"].as_str().unwrap();
    assert!(message.chars().count() < MAX_MESSAGE / 2, "{}", message.len());
    assert!(message.starts_with(BOX_RULES));
    assert_eq!(message.matches("lamp := start;").count(), 1, "the file goes once");
    assert!(message.contains("<requirement_tests>"));
}

/// The Agent checks an answer against the box's own image sizes and scan
/// period, not the page's simulation (8 bytes of I and Q, 64 of M, 10 ms):
/// a valid program that reads an input at byte 20 (the page has 8) failed
/// to compile there and the model was told to fix it (review 2026-11-08).
#[test]
fn the_agent_on_a_box_checks_against_the_boxs_sizes_and_period() {
    use crate::box_agent::Incoming;
    use crate::instruct::Kind;
    use crate::instruct_tests::LAMP;
    use serde_json::json;
    let big = "PROGRAM lamp\nVAR_INPUT\n  start : BOOL;\nEND_VAR\nVAR_OUTPUT\n  lamp : BOOL;\n  kept : BOOL;\nEND_VAR\nVAR\n  sensor AT %IX20.0 : BOOL;\nEND_VAR\nlamp := start;\nkept := start OR sensor;\nEND_PROGRAM\n";
    let scenario = "` ` ` st scenario\n(* The kept flag follows start. *)\nPROGRAM test_kept\nVAR_INPUT\n  kept : BOOL;\nEND_VAR\nVAR_OUTPUT\n  start : BOOL;\n  done : BOOL;\nEND_VAR\nVAR\n  n : INT;\n  kept_on : ASSERT;\nEND_VAR\nstart := TRUE;\nn := n + 1;\nIF n > 3 THEN\n  kept_on(COND := kept);\n  done := TRUE;\nEND_IF;\nEND_PROGRAM\n` ` `\n";
    // The page's simulation cannot hold it; the box can.
    assert!(helmsman_iec::compile(big, crate::playground::SIZES).is_err());
    let b = Bench::new();
    let jar = Jar::default();
    let ctx = egui::Context::default();
    let mut v = page(&b, &jar, Some(("alice", ALICE)));
    make(&b, &jar, &mut v.projects, "line", &[("main.st", LAMP)]);
    v.projects.open_file("main.st");
    v.tick(&ctx);
    let runs = v
        .projects
        .open
        .as_ref()
        .and_then(|o| o.runs)
        .expect("the box reported its sizes");
    assert_eq!((runs.sizes.i, runs.sizes.q), (32, 32), "{runs:?}");
    assert_eq!(
        runs.period,
        Duration::from_millis(2),
        "the bench box scans at 2 ms"
    );
    v.instruct.draft = "keep a counter word".into();
    v.send_instruction(&ctx);
    let seq = v.chat_sent.last().expect("a message").seq;
    let answer = format!("Why: it keeps a word.\n\n```st\n{big}```\n\n{scenario}");
    let ev = |n: &str, d: Json| format!("event: {n}\ndata: {d}\n\n");
    let body = [
        ev("session", json!({"id": "s1"})),
        ev("text", json!({ "delta": answer })),
        ev("done", json!({"stop": "end_turn", "why": null})),
    ]
    .concat();
    v.agent.apply(Incoming::Chunk { seq, text: body });
    v.agent.apply(Incoming::End { seq });
    v.tick(&ctx);
    let o = v.instruct.outcome.clone().expect("an outcome");
    assert_eq!(o.kind, Kind::Applied, "{o:?}");
}

/// An instruction belongs to the project it was typed in: opening another
/// while the box's agent answers ends the run, and a failing first answer
/// is not sent back with the other project's files.
#[test]
fn opening_another_project_ends_the_boxs_run_and_sends_no_retry() {
    use crate::box_agent::Incoming;
    use crate::instruct::Kind;
    use crate::instruct_tests::LAMP;
    use serde_json::json;
    let b = Bench::new();
    let jar = Jar::default();
    let ctx = egui::Context::default();
    let mut v = page(&b, &jar, Some(("alice", ALICE)));
    make(&b, &jar, &mut v.projects, "line", &[("main.st", LAMP)]);
    make(&b, &jar, &mut v.projects, "second", &[("main.st", LAMP)]);
    v.projects.open_project("line");
    for _ in 0..20 {
        pump(&b, &jar, &mut v, &ctx, 1.0);
        if v.psent.is_empty() {
            break;
        }
    }
    v.projects.open_file("main.st");
    v.instruct.draft = "comment it".into();
    v.send_instruction(&ctx);
    assert_eq!(v.chat_sent.len(), 1, "{:?}", v.instruct.outcome);
    let seq = v.chat_sent[0].seq;
    v.projects.open_project("second");
    v.tick(&ctx);
    let ev = |n: &str, d: Json| format!("event: {n}\ndata: {d}\n\n");
    let body = [
        ev("session", json!({"id": "s1"})),
        ev(
            "text",
            json!({ "delta": "Why: x.\n\n```st\nPROGRAM broken\n```\n" }),
        ),
        ev("done", json!({"stop": "end_turn", "why": null})),
    ]
    .concat();
    v.agent.apply(Incoming::Chunk { seq, text: body });
    v.agent.apply(Incoming::End { seq });
    v.tick(&ctx);
    assert_eq!(v.chat_sent.len(), 1, "no retry from the other project");
    let o = v.instruct.outcome.clone().expect("an outcome");
    assert_eq!(o.kind, Kind::Failed, "{o:?}");
    assert!(o.status.contains("project changed"), "{}", o.status);
}

/// A turn the box ended early (`budget`) with a whole-looking answer in
/// it is cut off, not complete: nothing is written (review 2026-11-08).
#[test]
fn an_answer_the_box_cut_off_is_never_written() {
    use crate::box_agent::Incoming;
    use crate::instruct::Kind;
    use crate::instruct_tests::{LAMP, st};
    use serde_json::json;
    let b = Bench::new();
    let jar = Jar::default();
    let ctx = egui::Context::default();
    let mut v = page(&b, &jar, Some(("alice", ALICE)));
    make(&b, &jar, &mut v.projects, "line", &[("main.st", LAMP)]);
    v.projects.open_file("main.st");
    v.tick(&ctx);
    v.instruct.draft = "comment it".into();
    v.send_instruction(&ctx);
    let seq = v.chat_sent.last().expect("a message").seq;
    let new = LAMP.replace("lamp := start;", "lamp := start; (* follows start *)");
    let ev = |n: &str, d: Json| format!("event: {n}\ndata: {d}\n\n");
    let body = [
        ev("session", json!({"id": "s1"})),
        ev("text", json!({ "delta": st(&new) })),
        ev("done", json!({"stop": "budget", "why": null})),
    ]
    .concat();
    v.agent.apply(Incoming::Chunk { seq, text: body });
    v.agent.apply(Incoming::End { seq });
    v.tick(&ctx);
    let o = v.instruct.outcome.clone().expect("an outcome");
    assert_eq!(o.kind, Kind::Failed, "{o:?}");
    assert!(o.status.contains("cut off"), "{}", o.status);
    assert_eq!(
        b.json("/api/projects/line/files/main.st")["text"],
        LAMP,
        "never written"
    );
}

// ---- the Monitor's Stop, Run and Restart on a box (card monitor-box-run-stop) ----

/// The page on the Monitor, its requests carried out against the box until
/// the conveyor's status tile is up.
fn monitor(b: &Bench, jar: &Jar, who: Option<(&str, &str)>) -> Harness {
    let v = page(b, jar, who);
    let session = v.session.clone();
    let mut h = Harness::new(v, session);
    h.app.open_monitor();
    let deadline = Instant::now() + Duration::from_secs(20);
    while h.app.chrome.rect("status conveyor").is_none() {
        assert!(
            Instant::now() < deadline,
            "no status tile: {:?}",
            h.app.chrome.hits
        );
        monitor_pump(b, jar, &mut h);
    }
    h
}

/// One frame, then every request the page queued answered by the box.
fn monitor_pump(b: &Bench, jar: &Jar, h: &mut Harness) {
    h.frame(vec![]);
    let Harness { ctx, app, time } = h;
    pump(b, jar, app.boxv.as_mut().unwrap(), ctx, *time);
    values(b, app.boxv.as_mut().unwrap(), *time);
    h.frame(vec![]);
}

fn run_of(h: &Harness) -> crate::monitor::Run {
    use crate::monitor::Feed as _;
    let bv = h.app.boxv.as_ref().unwrap();
    bv.state.programs(h.time).remove(0).run
}

#[test]
fn an_engineer_stops_runs_and_restarts_the_box_program_from_the_monitor() {
    use crate::monitor::Run;
    let b = Bench::new();
    let jar = Jar::default();
    let mut h = monitor(&b, &jar, Some(("alice", ALICE)));
    assert!(h.app.chrome.rect("run").is_some(), "Stop/Run for an engineer");
    assert!(h.app.chrome.rect("restart").is_some(), "Restart for an engineer");
    assert!(h.app.chrome.rect("step").is_none(), "a box has no step");
    assert_eq!(run_of(&h), Run::Running);

    // Stop: the box stops the module, the tile says who.
    h.press("run");
    let reqs = h.sent();
    assert_eq!(
        reqs.iter()
            .map(|q| (q.method, q.url.as_str()))
            .collect::<Vec<_>>(),
        [("POST", "/api/modules/conveyor/stop")],
        "{reqs:?}"
    );
    for q in reqs {
        let a = b.send(&jar, &q);
        assert_eq!(a.status, 200, "{a:?}");
        h.bv().projects.answer(q.call, &a);
    }
    wait("the box reports it stopped", || {
        b.json("/api/modules/conveyor")["status"]["state"] == "stopped"
    });
    wait("the tile says stopped, by alice", || {
        monitor_pump(&b, &jar, &mut h);
        run_of(&h) == Run::Stopped(Some("alice (engineer)".into()))
    });
    let (msg, err) = h.bv().projects.notice.clone().unwrap();
    assert!(!err && msg.contains("safe values"), "{msg}");

    // The same button runs it again.
    h.press("run");
    let reqs = h.sent();
    assert_eq!(reqs.len(), 1, "{reqs:?}");
    assert_eq!(reqs[0].url, "/api/modules/conveyor/run");
    for q in reqs {
        let a = b.send(&jar, &q);
        h.bv().projects.answer(q.call, &a);
    }
    wait("running again", || {
        monitor_pump(&b, &jar, &mut h);
        run_of(&h) == Run::Running
    });

    // Restart.
    h.press("restart");
    let reqs = h.sent();
    assert_eq!(reqs.len(), 1, "{reqs:?}");
    assert_eq!(reqs[0].url, "/api/modules/conveyor/restart");
    for q in reqs {
        let a = b.send(&jar, &q);
        assert_eq!(a.status, 200, "{a:?}");
        h.bv().projects.answer(q.call, &a);
    }
    wait("the box reports it running", || {
        b.json("/api/modules/conveyor")["status"]["state"] == "running"
    });
    let audit = b.json("/api/audit?limit=1000");
    for want in [
        "module conveyor stopped",
        "module conveyor run",
        "module conveyor restarted",
    ] {
        assert!(
            audit.as_array().unwrap().iter().any(|e| e["detail"] == want),
            "{want} not audited"
        );
    }
}

#[test]
fn signed_out_or_an_operator_sees_no_stop_and_sends_nothing() {
    let b = Bench::new();
    let jar = Jar::default();
    for who in [None, Some(("bob", BOB))] {
        let mut h = monitor(&b, &jar, who);
        assert!(h.app.chrome.rect("run").is_none(), "{who:?}: no Stop");
        assert!(h.app.chrome.rect("restart").is_none(), "{who:?}: no Restart");
        // The hotkey for Run/Stop on the Monitor does nothing on a box either.
        h.chord(Modifiers::CTRL, egui::Key::Enter);
        let reqs = h.sent();
        assert!(reqs.iter().all(|q| q.method == "GET"), "{who:?}: {reqs:?}");
        assert_eq!(b.json("/api/modules/conveyor")["status"]["state"], "running");
    }
}

#[test]
fn the_editor_has_the_box_run_stop_and_restart_for_an_engineer() {
    let b = Bench::new();
    let jar = Jar::default();
    let v = drawn(&b, &jar);
    let session = v.session.clone();
    let mut h = Harness::new(v, session);
    h.frame(vec![]);
    assert!(h.app.chrome.rect("step").is_none(), "a box has no step");
    // Ctrl+Enter, the simulation's Run/Stop, never stops a machine.
    h.chord(Modifiers::CTRL, egui::Key::Enter);
    let reqs = h.sent();
    assert!(reqs.iter().all(|q| q.method == "GET"), "{reqs:?}");

    h.press("run");
    let reqs = h.sent();
    assert_eq!(
        reqs.iter()
            .map(|q| (q.method, q.url.as_str()))
            .collect::<Vec<_>>(),
        [("POST", "/api/modules/conveyor/stop")],
        "{reqs:?}"
    );
    for q in reqs {
        let a = b.send(&jar, &q);
        assert_eq!(a.status, 200, "{a:?}");
        h.bv().projects.answer(q.call, &a);
    }
    wait("the box reports it stopped", || {
        b.json("/api/modules/conveyor")["status"]["state"] == "stopped"
    });
    let t = h.time;
    wait("the page sees it stopped", || {
        values(&b, h.app.boxv.as_mut().unwrap(), t);
        h.app.boxv.as_ref().unwrap().state.modules[0].state == "stopped"
    });
    h.frame(vec![]);
    h.press("run");
    let reqs = h.sent();
    assert_eq!(reqs[0].url, "/api/modules/conveyor/run", "{reqs:?}");
    for q in reqs {
        let a = b.send(&jar, &q);
        h.bv().projects.answer(q.call, &a);
    }
    wait("the box reports it running", || {
        b.json("/api/modules/conveyor")["status"]["state"] == "running"
    });
    h.press("restart");
    let reqs = h.sent();
    assert_eq!(reqs[0].url, "/api/modules/conveyor/restart", "{reqs:?}");
}

#[test]
fn the_editor_offers_no_box_run_stop_to_anyone_else() {
    let b = Bench::new();
    let jar = Jar::default();
    let v = page(&b, &jar, Some(("bob", BOB)));
    let session = v.session.clone();
    let mut h = Harness::new(v, session);
    h.wait(0.2);
    assert!(h.app.chrome.rect("run").is_none());
    assert!(h.app.chrome.rect("restart").is_none());
    h.chord(Modifiers::CTRL, egui::Key::Enter);
    let reqs = h.sent();
    assert!(reqs.iter().all(|q| q.method == "GET"), "{reqs:?}");
}
/// Card `desk-on-box`: a draft started on one screen (the Studio) is there
/// on another (the Air) once the person signs in, though that browser held
/// an older desk of its own; signed out, the other screen keeps its own
/// and asks the box nothing. A role switch on the same account keeps the
/// desk, and the box never holds a password.
#[test]
fn a_draft_started_on_one_screen_is_there_on_another_after_signing_in() {
    use crate::box_projects::PCall;
    use crate::desk::{Desk, MAX_BYTES};
    use crate::sim_projects::MemStorage;
    let b = Bench::new();
    let ctx = egui::Context::default();
    let (studio_jar, air_jar) = (Jar::default(), Jar::default());
    let mut studio = page(&b, &studio_jar, Some(("alice", ALICE)));
    make(
        &b,
        &studio_jar,
        &mut studio.projects,
        "line",
        &[("main.st", MAIN), ("blocks.st", BLOCKS)],
    );
    studio.projects.open_project("line");
    settle(&b, &studio_jar, &mut studio.projects);
    studio.projects.open_file("blocks.st");
    let edited = format!("{BLOCKS}\n(* started on the Studio *)\n");
    studio.projects.current_mut().unwrap().text = edited.clone();
    studio.instruct.draft = "make the lamp blink at 2 Hz".into();
    let mut now = 0.0;
    let box_desk = |jar: &Jar| -> Json {
        let a = b.send(jar, &PReq::get(PCall::Desk, "/api/desk".into()));
        serde_json::from_str(&a.body).unwrap()
    };
    for _ in 0..20 {
        now += 1.5;
        pump(&b, &studio_jar, &mut studio, &ctx, now);
        if box_desk(&studio_jar)["desk"]["agent"] == "make the lamp blink at 2 Hz" {
            break;
        }
    }
    let held = box_desk(&studio_jar);
    assert_eq!(held["owner"], "user:alice");
    assert_eq!(
        held["desk"]["drafts"]["line"]["blocks.st"]["text"],
        edited.as_str()
    );
    assert!(!held.to_string().contains(ALICE), "no password on the box");

    // The Air: its browser holds an older desk of its own.
    let old = Desk {
        saved_at: 5,
        agent: "yesterday's note".into(),
        ..Desk::default()
    };
    let mut air = BoxView::new().with_desk(Box::new(MemStorage(Some(old.text_within(MAX_BYTES).0))));
    for _ in 0..3 {
        now += 1.5;
        pump(&b, &air_jar, &mut air, &ctx, now);
        assert!(
            !air.psent.iter().any(|q| q.url == "/api/desk"),
            "signed out: the box is not asked"
        );
    }
    assert_eq!(
        air.instruct.draft, "yesterday's note",
        "signed out: this browser's"
    );
    air.session = Some(b.login(&air_jar, "alice", ALICE));
    for _ in 0..20 {
        now += 1.5;
        pump(&b, &air_jar, &mut air, &ctx, now);
        let back = air
            .projects
            .open
            .as_ref()
            .and_then(|o| o.bufs.get("blocks.st"))
            .is_some_and(|f| f.text == edited);
        if back {
            break;
        }
    }
    assert_eq!(air.instruct.draft, "make the lamp blink at 2 Hz");
    let o = air.projects.open.as_ref().expect("the Studio's project open");
    assert_eq!(o.name, "line");
    assert_eq!(o.current.as_deref(), Some("blocks.st"));
    assert_eq!(o.bufs["blocks.st"].text, edited);
    assert!(air.projects.dirty(), "still an edit to save");

    // The same account in another role: the same desk, nothing asked.
    air.session = air.session.take().map(|s| Session {
        role: "operator".into(),
        ..s
    });
    now += 1.5;
    pump(&b, &air_jar, &mut air, &ctx, now);
    assert!(
        !air.psent.iter().any(|q| q.call == PCall::Desk),
        "{:?}",
        air.psent
    );
    assert_eq!(air.instruct.draft, "make the lamp blink at 2 Hz");
    assert_eq!(air.projects.open.as_ref().unwrap().bufs["blocks.st"].text, edited);
}

/// 2026-11-08 review: two screens of one account, each with an unsaved
/// edit to a different file. The screen whose desk the box refuses as older
/// reads the other's and takes it, and its own edit is still there, still
/// unsaved, next to the other screen's.
#[test]
fn taking_a_newer_desk_from_another_screen_keeps_this_screens_unsaved_edit() {
    use crate::box_projects::PCall;
    use crate::sim_projects::wall_ms;
    let b = Bench::new();
    let ctx = egui::Context::default();
    let (ja, js) = (Jar::default(), Jar::default());
    let mut a = page(&b, &ja, Some(("alice", ALICE)));
    make(
        &b,
        &ja,
        &mut a.projects,
        "line",
        &[("main.st", MAIN), ("blocks.st", BLOCKS)],
    );
    let mut s = page(&b, &js, Some(("alice", ALICE)));
    for (v, j) in [(&mut a, &ja), (&mut s, &js)] {
        v.projects.open_project("line");
        settle(&b, j, &mut v.projects);
    }
    let mut now = 0.0;
    for _ in 0..3 {
        now += 1.5;
        pump(&b, &ja, &mut a, &ctx, now);
        pump(&b, &js, &mut s, &ctx, now);
    }
    a.projects.open_file("blocks.st");
    let from_a = format!("{BLOCKS}\n(* from the first screen *)\n");
    a.projects.current_mut().unwrap().text = from_a.clone();
    s.projects.open_file("main.st");
    let from_s = format!("{MAIN}\n(* from the second screen *)\n");
    s.projects.current_mut().unwrap().text = from_s.clone();
    s.instruct.draft = "the second screen's note".into();

    // The second screen dates its desk first but its PUT is slow; the
    // first screen's, dated later, lands before it.
    now += 1.5;
    s.step_all(&ctx, vec![], vec![], now);
    let held = std::mem::take(&mut s.psent);
    let put = held.iter().find(|q| q.call == PCall::SaveDesk).expect("a PUT");
    let dated: Json = serde_json::from_slice(put.body.as_ref().unwrap()).unwrap();
    let t = dated["saved_at"].as_u64().unwrap();
    wait("the clock to move on", || wall_ms() > t);
    a.step_all(&ctx, vec![], vec![], now);
    let first = std::mem::take(&mut a.psent);
    let kept = first
        .iter()
        .find(|q| q.call == PCall::SaveDesk)
        .expect("the first screen's PUT");
    let ans = b.send(&ja, kept);
    assert_eq!(ans.status, 200, "{}", ans.body);
    for q in held {
        let ans = b.send(&js, &q);
        if q.call == PCall::SaveDesk {
            assert_eq!(ans.status, 409, "{}", ans.body);
        }
        s.step_all(&ctx, vec![], vec![(q.call, ans)], now);
    }
    for _ in 0..10 {
        now += 1.5;
        pump(&b, &js, &mut s, &ctx, now);
    }
    let o = s.projects.open.as_ref().expect("the project stays open");
    assert_eq!(o.name, "line");
    assert_eq!(o.current.as_deref(), Some("main.st"), "the file it was on");
    assert_eq!(o.bufs["main.st"].text, from_s, "its own edit is not discarded");
    assert!(o.bufs["main.st"].dirty());
    assert_eq!(
        o.bufs["blocks.st"].text, from_a,
        "the other screen's edit arrived"
    );
    assert_eq!(
        s.instruct.draft, "the second screen's note",
        "the typed input stays"
    );
    let box_desk = |jar: &Jar| -> Json {
        let a = b.send(jar, &PReq::get(PCall::Desk, "/api/desk".into()));
        serde_json::from_str(&a.body).unwrap()
    };
    let held = box_desk(&js);
    assert_eq!(held["desk"]["drafts"]["line"]["main.st"]["text"], from_s.as_str());
    assert_eq!(
        held["desk"]["drafts"]["line"]["blocks.st"]["text"],
        from_a.as_str()
    );
}

/// 2026-11-08 review: another person signing in on the same page does not
/// inherit the last one's open project, unsaved edits and inputs, and the
/// last one's desk does not go over theirs on the box.
#[test]
fn another_person_signing_in_on_the_page_does_not_inherit_the_last_ones_desk() {
    use crate::box_projects::PCall;
    let b = Bench::new();
    let ctx = egui::Context::default();
    let jar = Jar::default();
    let mut v = page(&b, &jar, Some(("alice", ALICE)));
    make(
        &b,
        &jar,
        &mut v.projects,
        "line",
        &[("main.st", MAIN), ("blocks.st", BLOCKS)],
    );
    v.projects.open_project("line");
    settle(&b, &jar, &mut v.projects);
    v.projects.open_file("blocks.st");
    let edited = format!("{BLOCKS}\n(* alice's *)\n");
    v.projects.current_mut().unwrap().text = edited.clone();
    v.instruct.draft = "alice's note".into();
    let box_desk = |jar: &Jar| -> Json {
        let a = b.send(jar, &PReq::get(PCall::Desk, "/api/desk".into()));
        serde_json::from_str(&a.body).unwrap()
    };
    let mut now = 0.0;
    for _ in 0..20 {
        now += 1.5;
        pump(&b, &jar, &mut v, &ctx, now);
        if box_desk(&jar)["desk"]["agent"] == "alice's note" {
            break;
        }
    }
    assert_eq!(box_desk(&jar)["desk"]["agent"], "alice's note");

    // Bob has an older desk of his own on the box, from another screen.
    let jb = Jar::default();
    b.login(&jb, "bob", BOB);
    let ans = b.send(
        &jb,
        &PReq {
            call: PCall::SaveDesk,
            method: "PUT",
            url: "/api/desk".into(),
            body: Some(br#"{"saved_at":5,"agent":"bob's note"}"#.to_vec()),
            ctype: "application/json",
        },
    );
    assert!(ans.status < 300, "{}", ans.body);

    // Alice signs out and Bob signs in on the same page.
    v.session = Some(b.login(&jar, "bob", BOB));
    for _ in 0..8 {
        now += 1.5;
        pump(&b, &jar, &mut v, &ctx, now);
    }
    assert_eq!(v.instruct.draft, "bob's note", "his own desk, not alice's input");
    assert!(!v.projects.dirty(), "alice's unsaved edit is not on his screen");
    assert!(v.desk().drafts.is_empty(), "{:?}", v.desk().drafts);
    assert_eq!(box_desk(&jb)["desk"]["agent"], "bob's note");
    assert_eq!(box_desk(&jb)["desk"]["drafts"], serde_json::json!({}));
    // Alice's desk on the box is hers still.
    let ja = Jar::default();
    b.login(&ja, "alice", ALICE);
    let hers = box_desk(&ja);
    assert_eq!(
        hers["desk"]["drafts"]["line"]["blocks.st"]["text"],
        edited.as_str()
    );
}

/// One browser's `localStorage`: every slot a page opens on it, shared
/// across page loads.
#[derive(Clone, Default)]
struct LocalStore(std::rc::Rc<std::cell::RefCell<BTreeMap<String, String>>>);

struct StoreSlot(LocalStore, String);

impl crate::sim_projects::Storage for StoreSlot {
    fn load(&self) -> Result<Option<String>, String> {
        Ok(self.0.0.borrow().get(&self.1).cloned())
    }

    fn save(&mut self, text: &str) -> Result<(), String> {
        self.0.0.borrow_mut().insert(self.1.clone(), text.to_string());
        Ok(())
    }

    fn keyed(&self, suffix: &str) -> Option<Box<dyn crate::sim_projects::Storage>> {
        Some(Box::new(Self(self.0.clone(), format!("{}/{suffix}", self.1))))
    }
}

impl LocalStore {
    /// A page load on this browser.
    fn page(&self) -> BoxView {
        BoxView::new().with_desk(Box::new(StoreSlot(self.clone(), crate::desk::SLOT.into())))
    }

    /// Everything this browser holds, to look for a person's text in.
    fn everything(&self) -> String {
        format!("{:?}", self.0.borrow())
    }
}

/// `person` on `v`, the box's session cookie in `jar`.
fn sign_in(b: &Bench, jar: &Jar, v: &mut BoxView, person: (&str, &str)) {
    v.session = Some(b.login(jar, person.0, person.1));
}

/// The box's desk for the person `jar` is signed in as.
fn desk_on_box(b: &Bench, jar: &Jar) -> Json {
    let a = b.send(
        jar,
        &PReq::get(crate::box_projects::PCall::Desk, "/api/desk".into()),
    );
    serde_json::from_str(&a.body).unwrap()
}

/// A page whose person is alice, `line` open on `blocks.st` with an unsaved
/// edit and a note typed, on `browser`; `now` is where the clock stands.
fn alice_with_a_draft(b: &Bench, jar: &Jar, browser: &LocalStore, now: &mut f64) -> (BoxView, String) {
    let ctx = egui::Context::default();
    let mut v = browser.page();
    sign_in(b, jar, &mut v, ("alice", ALICE));
    make(
        b,
        jar,
        &mut v.projects,
        "line",
        &[("main.st", MAIN), ("blocks.st", BLOCKS)],
    );
    v.projects.open_project("line");
    settle(b, jar, &mut v.projects);
    v.projects.open_file("blocks.st");
    let edited = format!("{BLOCKS}\n(* alice's draft *)\n");
    v.projects.current_mut().unwrap().text.clone_from(&edited);
    v.instruct.draft = "alice's note".into();
    for _ in 0..20 {
        *now += 1.5;
        pump(b, jar, &mut v, &ctx, *now);
        if desk_on_box(b, jar)["desk"]["agent"] == "alice's note" {
            break;
        }
    }
    assert_eq!(desk_on_box(b, jar)["desk"]["agent"], "alice's note");
    (v, edited)
}

/// Card `desk-person-switch`: alice leaves a draft unsaved and signs out,
/// bob signs in on the same browser with nothing on the box. Bob's screen
/// is empty, his desk on the box never holds alice's text, and alice's
/// draft is back when she signs in again.
#[test]
fn a_second_person_on_the_browser_never_gets_the_firsts_draft() {
    let b = Bench::new();
    let ctx = egui::Context::default();
    let browser = LocalStore::default();
    let mut now = 0.0;
    let ja = Jar::default();
    let (mut v, edited) = alice_with_a_draft(&b, &ja, &browser, &mut now);

    v.session = None;
    for _ in 0..3 {
        now += 1.5;
        pump(&b, &ja, &mut v, &ctx, now);
    }
    let jb = Jar::default();
    sign_in(&b, &jb, &mut v, ("bob", BOB));
    for _ in 0..8 {
        now += 1.5;
        pump(&b, &jb, &mut v, &ctx, now);
    }
    assert_eq!(v.instruct.draft, "", "not alice's note");
    assert!(!v.projects.dirty(), "alice's unsaved edit is not on bob's screen");
    assert!(v.desk().drafts.is_empty(), "{:?}", v.desk().drafts);
    let bobs = desk_on_box(&b, &jb);
    assert!(!bobs.to_string().contains("alice"), "{bobs}");

    // Bob types his own; alice signs in again and has hers back.
    v.instruct.draft = "bob's note".into();
    for _ in 0..8 {
        now += 1.5;
        pump(&b, &jb, &mut v, &ctx, now);
    }
    assert_eq!(desk_on_box(&b, &jb)["desk"]["agent"], "bob's note");
    sign_in(&b, &ja, &mut v, ("alice", ALICE));
    for _ in 0..8 {
        now += 1.5;
        pump(&b, &ja, &mut v, &ctx, now);
    }
    assert_eq!(v.instruct.draft, "alice's note");
    assert_eq!(
        v.desk().drafts["line"]["blocks.st"].text,
        edited,
        "alice's draft is back"
    );
    assert_eq!(desk_on_box(&b, &jb)["desk"]["agent"], "bob's note");
}

/// The box keeping no desk for alice (it did not answer), this browser's
/// is hers: bob signing in on it does not wipe or inherit it, and it is
/// back for alice after a reload.
#[test]
fn each_person_has_their_own_desk_in_the_browser() {
    let b = Bench::new();
    let ctx = egui::Context::default();
    let browser = LocalStore::default();
    let mut now = 0.0;
    let ja = Jar::default();
    let (v, edited) = alice_with_a_draft(&b, &ja, &browser, &mut now);
    drop(v);

    // Bob, on a page loaded after alice left.
    let jb = Jar::default();
    let mut v = browser.page();
    sign_in(&b, &jb, &mut v, ("bob", BOB));
    for _ in 0..8 {
        now += 1.5;
        pump(&b, &jb, &mut v, &ctx, now);
    }
    assert_eq!(v.instruct.draft, "");
    assert!(v.desk().drafts.is_empty(), "{:?}", v.desk().drafts);
    assert!(!desk_on_box(&b, &jb).to_string().contains("alice"));
    v.instruct.draft = "bob's note".into();
    for _ in 0..4 {
        now += 1.5;
        pump(&b, &jb, &mut v, &ctx, now);
    }

    // Alice again after a reload, her box not answering: the browser's.
    let mut v = browser.page();
    sign_in(&b, &ja, &mut v, ("alice", ALICE));
    for _ in 0..4 {
        now += 1.5;
        v.step_all(&ctx, Vec::new(), Vec::new(), now);
        v.psent.clear();
    }
    assert_eq!(v.instruct.draft, "alice's note");
    assert_eq!(v.desk().drafts["line"]["blocks.st"].text, edited);
}

/// Bob has a desk of his own on the box: he gets that one, not alice's
/// from the same browser.
#[test]
fn a_second_person_with_a_desk_on_the_box_gets_that_one() {
    let b = Bench::new();
    let ctx = egui::Context::default();
    let browser = LocalStore::default();
    let mut now = 0.0;
    let ja = Jar::default();
    let (mut v, _) = alice_with_a_draft(&b, &ja, &browser, &mut now);
    let jb = Jar::default();
    b.login(&jb, "bob", BOB);
    let ans = b.send(
        &jb,
        &PReq {
            call: crate::box_projects::PCall::SaveDesk,
            method: "PUT",
            url: "/api/desk".into(),
            body: Some(br#"{"saved_at":5,"agent":"bob's note"}"#.to_vec()),
            ctype: "application/json",
        },
    );
    assert!(ans.status < 300, "{}", ans.body);
    v.session = None;
    pump(&b, &ja, &mut v, &ctx, now + 1.5);
    sign_in(&b, &jb, &mut v, ("bob", BOB));
    for _ in 0..8 {
        now += 1.5;
        pump(&b, &jb, &mut v, &ctx, now);
    }
    assert_eq!(v.instruct.draft, "bob's note");
    assert!(!v.projects.dirty());
    assert!(!desk_on_box(&b, &jb).to_string().contains("alice"));
}

/// Signed out, with no account: the desk is the browser's one slot,
/// `helmsman-canvas/desk`, as before person desks.
#[test]
fn signed_out_the_desk_is_the_one_browser_slot() {
    let b = Bench::new();
    let ctx = egui::Context::default();
    let browser = LocalStore::default();
    let jar = Jar::default();
    let mut v = browser.page();
    v.instruct.draft = "typed before signing in".into();
    let mut now = 0.0;
    for _ in 0..3 {
        now += 1.5;
        pump(&b, &jar, &mut v, &ctx, now);
    }
    assert!(
        browser.0.borrow()["helmsman-canvas/desk"].contains("typed before signing in"),
        "{}",
        browser.everything()
    );
    assert_eq!(browser.page().instruct.draft, "typed before signing in");
}

/// Work typed signed out is taken by the first person to sign in, and
/// leaves the signed-out slot: the next person does not get it.
#[test]
fn signed_out_work_goes_to_the_first_person_to_sign_in_only() {
    let b = Bench::new();
    let ctx = egui::Context::default();
    let browser = LocalStore::default();
    let mut v = browser.page();
    v.instruct.draft = "typed before signing in".into();
    let jar = Jar::default();
    let mut now = 0.0;
    for _ in 0..3 {
        now += 1.5;
        pump(&b, &jar, &mut v, &ctx, now);
    }
    let ja = Jar::default();
    sign_in(&b, &ja, &mut v, ("alice", ALICE));
    for _ in 0..8 {
        now += 1.5;
        pump(&b, &ja, &mut v, &ctx, now);
    }
    assert_eq!(v.instruct.draft, "typed before signing in", "alice's now");
    assert_eq!(desk_on_box(&b, &ja)["desk"]["agent"], "typed before signing in");
    assert!(
        !browser.0.borrow()["helmsman-canvas/desk"].contains("typed before"),
        "{}",
        browser.everything()
    );
    let jb = Jar::default();
    let mut v = browser.page();
    sign_in(&b, &jb, &mut v, ("bob", BOB));
    for _ in 0..8 {
        now += 1.5;
        pump(&b, &jb, &mut v, &ctx, now);
    }
    assert_eq!(v.instruct.draft, "");
    assert!(!desk_on_box(&b, &jb).to_string().contains("typed before"));
}

/// A desk an earlier release left in the one slot, kept for alice: bob
/// does not get it, alice does.
#[test]
fn a_desk_left_in_the_old_slot_goes_only_to_its_owner() {
    use crate::desk::Desk;
    use crate::sim_projects::Storage as _;
    let b = Bench::new();
    let ctx = egui::Context::default();
    let browser = LocalStore::default();
    let old = Desk {
        saved_at: 9,
        agent: "alice's old note".into(),
        owner: Some("user:alice".into()),
        user: "alice".into(),
        ..Desk::default()
    };
    StoreSlot(browser.clone(), crate::desk::SLOT.into())
        .save(&old.text_within(crate::desk::MAX_BYTES).0)
        .unwrap();
    let mut now = 0.0;
    let jb = Jar::default();
    let mut v = browser.page();
    sign_in(&b, &jb, &mut v, ("bob", BOB));
    for _ in 0..8 {
        now += 1.5;
        pump(&b, &jb, &mut v, &ctx, now);
    }
    assert_eq!(v.instruct.draft, "");
    assert!(!desk_on_box(&b, &jb).to_string().contains("alice"));
    let ja = Jar::default();
    let mut v = browser.page();
    sign_in(&b, &ja, &mut v, ("alice", ALICE));
    for _ in 0..8 {
        now += 1.5;
        pump(&b, &ja, &mut v, &ctx, now);
    }
    assert_eq!(v.instruct.draft, "alice's old note");
}

/// An instruction under way on the box's agent, with a proposal stored
/// and the answer still streaming: `v` signed in, `line` open on
/// `main.st`.
fn mid_conversation(b: &Bench, jar: &Jar, ctx: &egui::Context) -> BoxView {
    use crate::box_agent::Incoming;
    let mut v = page(b, jar, Some(("alice", ALICE)));
    make(b, jar, &mut v.projects, "line", &[("main.st", MAIN)]);
    v.projects.open_file("main.st");
    v.instruct.draft = "make the lamp blink at 2 Hz".into();
    v.send_instruction(ctx);
    let seq = v.chat_sent.last().expect("sent").seq;
    v.agent.apply(Incoming::Chunk {
        seq,
        text: "event: session\ndata: {\"id\":\"s1\"}\n\n\
               event: text\ndata: {\"delta\":\"Looking at the lamp\"}\n\n\
               event: proposal\ndata: {\"ids\":[3,4],\"summary\":\"speed to 2\"}\n\n"
            .into(),
    });
    assert!(v.instruct.busy() && v.agent.busy());
    v
}

/// Card `desk-agent-and-forms`: a reload while the Agent answers shows the
/// same turns, the instruction over with nothing written, and the
/// proposals that turn stored still offered for review; one decided since
/// (applied on the Proposals screen) shows as decided and is not offered
/// again.
#[test]
fn a_reload_mid_conversation_shows_the_turns_and_offers_only_the_undecided_proposal() {
    use crate::box_agent::End;
    use crate::desk::{MAX_BYTES, TURN_USER};
    use crate::instruct::Kind;
    use crate::review::Row;
    use crate::sim_projects::MemStorage;
    let b = Bench::new();
    let jar = Jar::default();
    let ctx = egui::Context::default();
    let v = mid_conversation(&b, &jar, &ctx);
    let before = v.agent.turns.clone();
    let (kept, dropped) = v.desk().text_within(MAX_BYTES);
    assert!(dropped.is_empty(), "{dropped:?}");
    assert!(!kept.contains(ALICE), "no password in the desk");

    let r = BoxView::new().with_desk(Box::new(MemStorage(Some(kept))));
    assert_eq!(r.agent.turns.len(), before.len());
    let (was, is) = (&before[0], &r.agent.turns[0]);
    assert!(
        was.user.starts_with(&is.user[..TURN_USER - 8]),
        "the message, cut"
    );
    assert_eq!(is.text, "Looking at the lamp");
    assert_eq!(is.items, was.items);
    assert!(matches!(&is.end, Some(End::Failed(w)) if w.contains("reloaded")));
    assert!(
        !r.instruct.busy() && !r.agent.busy(),
        "nothing under way after a reload"
    );
    let o = r.instruct.outcome.clone().unwrap();
    assert_eq!(o.kind, Kind::Failed);
    assert_eq!(o.review, [3, 4]);
    assert_eq!(
        b.json("/api/projects/line/files/main.st")["text"],
        MAIN,
        "nothing written"
    );

    let session = Some(b.login(&jar, "alice", ALICE));
    let mut h = Harness::new(r, session);
    h.press("tab claude");
    h.frame(vec![]);
    assert!(h.app.chrome.rect("agent result").is_some());
    let reviews = |h: &Harness| {
        h.app
            .chrome
            .hits
            .iter()
            .filter(|(k, _)| k == "agent review")
            .count()
    };
    assert_eq!(reviews(&h), 2, "both offered while the list is not in");
    // Proposal 3 was applied since; 4 still waits.
    h.app.review.as_mut().unwrap().state.list = [(3, "applied"), (4, "draft")]
        .into_iter()
        .map(|(id, status)| Row {
            id,
            title: "speed to 2".into(),
            status: status.into(),
            tier: None,
        })
        .collect();
    h.frame(vec![]);
    h.frame(vec![]);
    assert_eq!(reviews(&h), 1, "the decided one is not offered again");
    assert!(h.app.chrome.rect("agent decided").is_some());
    h.press("agent review");
    h.frame(vec![]);
    let asked: Vec<String> = h
        .app
        .review
        .as_ref()
        .unwrap()
        .sent
        .iter()
        .map(|q| q.url.clone())
        .collect();
    assert!(asked.iter().any(|u| u == "/api/proposals/4"), "{asked:?}");
}

/// The Agent's conversation and a half-filled New device form ride the
/// box's desk: kept by the box for the person (without the credential
/// typed into the form's settings) and there on another screen after
/// signing in. The box reads back every field the page's desk has.
#[test]
fn the_conversation_and_the_device_form_follow_the_person_to_another_screen() {
    use crate::box_projects::PCall;
    use crate::desk::{Desk, MAX_BYTES, form_of};
    use crate::devices::NewDevice;
    let b = Bench::new();
    let ctx = egui::Context::default();
    let (studio_jar, air_jar) = (Jar::default(), Jar::default());
    let mut studio = mid_conversation(&b, &studio_jar, &ctx);
    let mut f = NewDevice::from_eds("e1213.eds", "[Device]\nProdName = \"E1213\";\n");
    f.endpoint = "192.168.1.60".into();
    f.points[2].on = true;
    f.params = format!("{} password=hunter2-device", f.params);
    studio.devices_ui.adding = Some(f.clone());

    // Every field of the page's desk survives the box's reading of it.
    let text = studio.desk().text_within(MAX_BYTES).0;
    let on_box = helmsman_plc::api::desk::Desk::read(text.as_bytes()).expect("the box reads it");
    assert_eq!(
        Desk::parse(&serde_json::to_string(&on_box).unwrap()),
        Desk::parse(&text),
        "the box keeps every field"
    );

    let box_desk = |jar: &Jar| -> Json {
        let a = b.send(jar, &PReq::get(PCall::Desk, "/api/desk".into()));
        serde_json::from_str(&a.body).unwrap()
    };
    let mut now = 0.0;
    for _ in 0..20 {
        now += 1.5;
        pump(&b, &studio_jar, &mut studio, &ctx, now);
        if box_desk(&studio_jar)["desk"]["device"]["endpoint"] == "192.168.1.60" {
            break;
        }
    }
    let held = box_desk(&studio_jar);
    assert_eq!(
        held["desk"]["conversation"]["turns"][0]["text"],
        "Looking at the lamp"
    );
    assert!(!held.to_string().contains("hunter2"), "no credential on the box");
    assert!(!held.to_string().contains(ALICE), "no password on the box");

    let mut air = BoxView::new();
    air.session = Some(b.login(&air_jar, "alice", ALICE));
    for _ in 0..20 {
        now += 1.5;
        pump(&b, &air_jar, &mut air, &ctx, now);
        if air.devices_ui.adding.is_some() {
            break;
        }
    }
    assert_eq!(air.devices_ui.adding, Some(form_of(&f)));
    assert_eq!(air.agent.turns.len(), 1);
    assert_eq!(air.agent.turns[0].text, "Looking at the lamp");
    assert_eq!(air.instruct.outcome.as_ref().unwrap().review, [3, 4]);
    assert_eq!(air.instruct.draft, "", "the instruction went out on the Studio");
}
// ---- answers matched to what was asked ----

/// The text of `file` in the open project's editor.
fn text_of(p: &BoxProjects, file: &str) -> String {
    p.open.as_ref().unwrap().bufs[file].text.clone()
}

/// Sends `reqs` to the box and answers each, in order.
fn deliver(b: &Bench, jar: &Jar, p: &mut BoxProjects, reqs: Vec<PReq>) {
    for q in reqs {
        let a = b.send(jar, &q);
        p.answer(q.call, &a);
    }
}

/// Bug 1: a file made with text (a duplicate, a paste, an imported EDS,
/// the io.map a first device makes) holds that text in the editor, so the
/// next device or keystroke builds on it and Save cannot wipe it.
#[test]
fn a_file_made_with_text_holds_that_text_and_a_save_keeps_it() {
    let b = Bench::new();
    let jar = Jar::default();
    let mut v = page(&b, &jar, Some(("alice", ALICE)));
    let p = &mut v.projects;
    make(&b, &jar, p, "line", &[("main.st", MAIN)]);
    p.new_file_with("io.map", MAP);
    settle(&b, &jar, p);
    let buf = &p.open.as_ref().unwrap().bufs["io.map"];
    assert_eq!((buf.text.as_str(), buf.base.as_str()), (MAP, MAP));
    assert!(!p.dirty());

    p.new_file_with("main-2.st", MAIN);
    settle(&b, &jar, p);
    assert_eq!(text_of(p, "main-2.st"), MAIN, "the duplicate shows the copy");
    p.current_mut().unwrap().text.push_str("(* x *)");
    p.save();
    settle(&b, &jar, p);
    assert_eq!(
        b.json("/api/projects/line/files/main-2.st")["text"],
        format!("{MAIN}(* x *)")
    );
}

/// Bug 2: a rename marks clean only the text it sent; what was typed
/// during the round trip stays an edit.
#[test]
fn a_rename_keeps_what_was_typed_while_it_was_on_its_way() {
    let b = Bench::new();
    let jar = Jar::default();
    let mut v = page(&b, &jar, Some(("alice", ALICE)));
    let p = &mut v.projects;
    make(&b, &jar, p, "line", &[("main.st", MAIN)]);
    p.rename_file("main.st", "moved.st");
    let typed = format!("{MAIN}(* typed meanwhile *)");
    p.current_mut().unwrap().text.clone_from(&typed);
    settle(&b, &jar, p);
    let buf = &p.open.as_ref().unwrap().bufs["moved.st"];
    assert_eq!(buf.base, MAIN);
    assert_eq!(buf.text, typed);
    assert!(p.dirty(), "the typing still has to be saved");
    p.save();
    settle(&b, &jar, p);
    assert_eq!(
        b.json("/api/projects/line/files/moved.st")["text"],
        typed.as_str()
    );
}

/// Bug 2: a file not read yet has no text to move; renaming it would write
/// an empty file under the new name and delete the original.
#[test]
fn a_file_not_read_yet_is_not_renamed() {
    let b = Bench::new();
    let jar = Jar::default();
    let mut v = page(&b, &jar, Some(("alice", ALICE)));
    let p = &mut v.projects;
    make(&b, &jar, p, "line", &[("main.st", MAIN)]);
    p.open
        .as_mut()
        .unwrap()
        .bufs
        .insert("unread.st".into(), Buf::default());
    p.rename_file("unread.st", "other.st");
    assert!(p.poll().is_empty(), "nothing goes to the box");
    assert!(matches!(&p.notice, Some((_, true))), "{:?}", p.notice);
    assert_eq!(
        b.json("/api/projects/line/files/main.st")["text"],
        MAIN,
        "the box is as it was"
    );
}

/// Bug 2: signed out between the write and the delete, the new name is on
/// the box; the note says so rather than "nothing was changed".
#[test]
fn a_rename_cut_short_by_a_sign_out_says_where_the_file_is() {
    let b = Bench::new();
    let jar = Jar::default();
    let mut v = page(&b, &jar, Some(("alice", ALICE)));
    let p = &mut v.projects;
    make(&b, &jar, p, "line", &[("main.st", MAIN)]);
    p.rename_file("main.st", "moved.st");
    let write = p.poll();
    deliver(&b, &jar, p, write);
    let del = p.poll();
    assert_eq!(del.len(), 1, "{del:?}");
    deliver(&b, &Jar::default(), p, del);
    assert!(p.ended);
    let (msg, error) = p.notice.clone().unwrap();
    assert!(error && !msg.contains("nothing was changed"), "{msg}");
    assert!(msg.contains("moved.st") && msg.contains("main.st"), "{msg}");
}

/// Bug 3: a save answered after its project was left must not leave the
/// Deploy waiting for it for ever.
#[test]
fn a_deploy_whose_save_lands_after_a_project_switch_does_not_hang() {
    let b = Bench::new();
    let jar = Jar::default();
    let mut v = page(&b, &jar, Some(("alice", ALICE)));
    let p = &mut v.projects;
    make(&b, &jar, p, "line", &[("main.st", MAIN)]);
    make(&b, &jar, p, "other", &[]);
    p.open_project("line");
    settle(&b, &jar, p);
    p.current_mut().unwrap().text.push_str("(* edit *)");
    p.deploy();
    assert_eq!(p.deploy, Deploy::Saving);
    p.open_project("other");
    settle(&b, &jar, p);
    assert!(!p.deploy.busy(), "{:?}", p.deploy);
    assert!(matches!(p.deploy, Deploy::Failed(_)), "{:?}", p.deploy);
}

/// Bug 3: renaming the open project while a save is on its way would
/// leave that save unanswered for the project's new name; the rename waits.
#[test]
fn a_project_is_not_renamed_while_a_save_is_on_its_way() {
    let b = Bench::new();
    let jar = Jar::default();
    let mut v = page(&b, &jar, Some(("alice", ALICE)));
    let p = &mut v.projects;
    make(&b, &jar, p, "line", &[("main.st", MAIN)]);
    let typed = format!("{MAIN}(* typed *)");
    p.current_mut().unwrap().text.clone_from(&typed);
    p.save();
    let saving = p.poll();
    p.rename("line", "renamed");
    assert!(p.poll().is_empty(), "no rename goes out");
    assert!(matches!(&p.notice, Some((_, true))), "{:?}", p.notice);
    deliver(&b, &jar, p, saving);
    assert!(!p.dirty());
    assert!(p.open.as_ref().unwrap().bufs["main.st"].saving.is_none());
    // Idle, the rename goes through and the files are all there, read.
    p.rename("line", "renamed");
    settle(&b, &jar, p);
    let o = p.open.as_ref().unwrap();
    assert_eq!(o.name, "renamed");
    assert!(o.bufs["main.st"].loaded);
    p.current_mut().unwrap().text.push('\n');
    p.save();
    settle(&b, &jar, p);
    assert!(!p.dirty(), "{:?}", p.notice);
}

/// Bug 4: Save, keep typing, Deploy: the box deploys what was typed last.
#[test]
fn a_deploy_after_a_save_still_on_its_way_saves_the_later_typing_too() {
    let b = Bench::new();
    let jar = Jar::default();
    let mut v = page(&b, &jar, Some(("alice", ALICE)));
    let p = &mut v.projects;
    make(
        &b,
        &jar,
        p,
        "line",
        &[("blocks.st", BLOCKS), ("io.map", MAP), ("main.st", MAIN)],
    );
    p.current_mut().unwrap().text = format!("(* one *)\n{MAIN}");
    p.save();
    let first = p.poll();
    let second = format!("(* two *)\n{MAIN}");
    p.current_mut().unwrap().text.clone_from(&second);
    p.deploy();
    assert_eq!(p.deploy, Deploy::Saving);
    deliver(&b, &jar, p, first);
    settle(&b, &jar, p);
    assert_eq!(
        b.json("/api/projects/line/files/main.st")["text"],
        second.as_str(),
        "the box holds the latest typing"
    );
    assert!(!p.dirty());
    assert!(matches!(p.deploy, Deploy::Done(_)), "{:?}", p.deploy);
}

/// Bug 5: the Replace offer belongs to its own notice: another note, or
/// another project, ends it, and it never writes into the project open now.
#[test]
fn the_replace_offer_ends_with_its_notice_and_stays_with_its_project() {
    let b = Bench::new();
    let jar = Jar::default();
    let mut v = page(&b, &jar, Some(("alice", ALICE)));
    let p = &mut v.projects;
    make(&b, &jar, p, "line", &[("main.st", MAIN)]);
    make(&b, &jar, p, "other", &[("main.st", MAIN)]);
    p.open_project("line");
    settle(&b, &jar, p);
    p.upload("main.st", b"(* replaced *)".to_vec(), false);
    settle(&b, &jar, p);
    assert!(p.replace_offered(), "{:?}", p.notice);

    // A project switch ends the offer.
    p.open_project("other");
    settle(&b, &jar, p);
    assert!(!p.replace_offered());
    p.replace();
    assert!(p.poll().is_empty(), "the project switch dropped the offer");
    assert_eq!(b.json("/api/projects/other/files/main.st")["text"], MAIN);

    // So does any other note.
    p.open_project("line");
    settle(&b, &jar, p);
    p.upload("main.st", b"(* replaced *)".to_vec(), false);
    settle(&b, &jar, p);
    assert!(p.replace_offered());
    p.note("something else failed", true);
    assert!(!p.replace_offered());

    // An offer for a project that is not the open one is not taken.
    p.open_project("other");
    settle(&b, &jar, p);
    p.pending = Some(Pending {
        file: "main.st".into(),
        bytes: b"(* replaced *)".to_vec(),
        project: "line".into(),
        offered: true,
    });
    p.replace();
    assert!(p.poll().is_empty(), "no upload goes to the wrong project");
    assert!(matches!(&p.notice, Some((_, true))), "{:?}", p.notice);
    assert_eq!(b.json("/api/projects/other/files/main.st")["text"], MAIN);
    assert_eq!(b.json("/api/projects/line/files/main.st")["text"], MAIN);
}

/// A zip of one project, as an upload.
fn zip_of(main: &str) -> Vec<u8> {
    helmsman_project::Files(BTreeMap::from([("main.st".to_string(), main.to_string())]))
        .to_zip()
        .unwrap()
}

/// Bug 6: every way of replacing the open project's files keeps an
/// unsaved edit: it comes back when the project is opened again, and the
/// desk holds it meanwhile.
#[test]
fn a_restore_a_new_project_a_copy_and_an_upload_keep_the_unsaved_edit() {
    use crate::desk::Desk;
    let b = Bench::new();
    let jar = Jar::default();
    let mut v = page(&b, &jar, Some(("alice", ALICE)));
    let p = &mut v.projects;
    make(&b, &jar, p, "line", &[("main.st", MAIN)]);
    let mine = format!("{MAIN}(* mine *)");
    let held = |p: &BoxProjects| {
        let mut d = Desk::default();
        p.to_desk(&mut d);
        d.drafts
            .get("line")
            .and_then(|m| m.get("main.st"))
            .map(|d| d.text.clone())
    };

    // Restore: the project is read again, the edit put back over it.
    let n = p.history[0].n;
    p.current_mut().unwrap().text.clone_from(&mine);
    p.restore("line", n);
    settle(&b, &jar, p);
    assert_eq!(text_of(p, "main.st"), mine, "restore: {:?}", p.notice);
    assert!(p.dirty());

    // New project, copy of a project, upload of a zip: each opens another
    // project; the edit waits in the desk and returns with `line`.
    for step in 0..3 {
        match step {
            0 => p.create(&format!("fresh{step}"), None),
            1 => p.create(&format!("copy{step}"), Some("line")),
            _ => p.upload(&format!("zipped{step}.zip"), zip_of(MAIN), false),
        }
        settle(&b, &jar, p);
        assert_ne!(p.open.as_ref().unwrap().name, "line", "step {step}");
        assert_eq!(held(p).as_deref(), Some(mine.as_str()), "step {step}");
        p.open_project("line");
        settle(&b, &jar, p);
        assert_eq!(text_of(p, "main.st"), mine, "step {step}: {:?}", p.notice);
        assert!(p.dirty(), "step {step}");
    }
}

/// Bug 7: the Delete key deletes what was last clicked; once that is gone
/// it does not turn into a delete of the open project.
#[test]
fn the_delete_key_does_not_fall_through_to_the_open_project() {
    let b = Bench::new();
    let jar = Jar::default();
    let mut v = page(&b, &jar, Some(("alice", ALICE)));
    make(
        &b,
        &jar,
        &mut v.projects,
        "line",
        &[("main.st", MAIN), ("extra.st", BLOCKS)],
    );
    let session = Some(Session {
        user: Some("alice".into()),
        role: "engineer".into(),
    });
    let mut h = Harness::new(v, session);
    h.app.chrome.show_files();
    h.wait(0.3);
    h.press("file extra.st");
    h.key(egui::Key::Backspace);
    assert_eq!(h.bv().files_ui.confirm, Some(Act::DeleteFile("extra.st".into())));
    h.press("confirm yes");
    let reqs = h.sent();
    deliver(&b, &jar, &mut h.bv().projects, reqs);
    h.wait(0.2);
    assert!(!files(&h.bv().projects).contains(&"extra.st".to_string()));
    h.key(egui::Key::Backspace);
    assert_eq!(h.bv().files_ui.confirm, None, "nothing is picked any more");
}

/// Bug 8: a restored edit for a file or project the box no longer has is
/// dropped, not kept and written to the desk for ever.
#[test]
fn restored_edits_for_files_and_projects_that_are_gone_are_dropped() {
    use crate::box_projects::Restore;
    use crate::desk::Draft;
    let b = Bench::new();
    let jar = Jar::default();
    let mut v = page(&b, &jar, Some(("alice", ALICE)));
    let p = &mut v.projects;
    make(&b, &jar, p, "line", &[("main.st", MAIN)]);
    let draft = |text: &str| Draft {
        text: text.into(),
        version: None,
    };
    p.adopt(Restore {
        project: Some("line".into()),
        file: None,
        drafts: BTreeMap::from([
            (
                "line".to_string(),
                BTreeMap::from([
                    ("main.st".to_string(), draft("(* kept *)")),
                    ("gone.st".to_string(), draft("(* file gone *)")),
                ]),
            ),
            (
                "deleted".to_string(),
                BTreeMap::from([("main.st".to_string(), draft("(* project gone *)"))]),
            ),
        ]),
    });
    settle(&b, &jar, p);
    assert!(text_of(p, "main.st").contains("kept"));
    assert!(p.restore.drafts.is_empty(), "{:?}", p.restore.drafts);
}

/// Bug 8: a history list read earlier does not replace a later one.
#[test]
fn an_older_history_answer_does_not_replace_a_newer_one() {
    let b = Bench::new();
    let jar = Jar::default();
    let mut v = page(&b, &jar, Some(("alice", ALICE)));
    let p = &mut v.projects;
    make(&b, &jar, p, "line", &[("main.st", MAIN)]);
    p.read_history("line");
    let older = p.poll().pop().unwrap();
    let older_answer = b.send(&jar, &older);
    p.current_mut().unwrap().text.push_str("(* more *)");
    p.save();
    settle(&b, &jar, p);
    let newest = p.history.len();
    p.answer(older.call, &older_answer);
    assert_eq!(p.history.len(), newest, "the older list is ignored");
    settle(&b, &jar, p);
}

/// A function block that declares RETAIN: the checker warns, the project
/// builds.
const RETAIN_BLOCKS: &str = "\
FUNCTION_BLOCK Latch
VAR_INPUT
  set : BOOL;
  reset : BOOL;
END_VAR
VAR_OUTPUT
  q : BOOL;
END_VAR
VAR RETAIN
  kept : INT;
END_VAR
q := (q OR set) AND NOT reset;
END_FUNCTION_BLOCK
";

#[test]
fn the_boxs_warnings_show_under_the_editor_and_in_the_messages_and_never_stop_the_deploy() {
    use crate::messages::Level;
    use crate::projects::Severity;
    let b = Bench::new();
    let jar = Jar::default();
    let mut v = page(&b, &jar, Some(("alice", ALICE)));
    make(
        &b,
        &jar,
        &mut v.projects,
        "line",
        &[("blocks.st", RETAIN_BLOCKS), ("main.st", MAIN), ("io.map", MAP)],
    );
    v.projects.deploy();
    settle(&b, &jar, &mut v.projects);
    let Deploy::Done(what) = &v.projects.deploy else {
        panic!(
            "a warning stopped the deploy: {:?} {:?}",
            v.projects.deploy, v.projects.diags
        );
    };
    assert!(what.starts_with("line is running"), "{what}");
    let d = &v.projects.diags;
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!((d[0].file.as_str(), d[0].line), ("blocks.st", 4), "{d:?}");
    assert_eq!(d[0].severity, Severity::Warning);
    assert!(d[0].message.contains("RETAIN inside a function block"), "{d:?}");

    // In the app: the row under the editor, the warning in the Messages,
    // and a click goes to the line.
    let session = v.session.clone();
    v.projects.open_file("main.st");
    let mut h = Harness::new(v, session);
    h.app.chrome.show_code();
    h.wait(0.5);
    assert!(h.app.chrome.rect("diag 0").is_some(), "the warning row");
    assert!(
        h.bv()
            .log
            .iter()
            .any(|m| m.level == Level::Warning && m.text.contains("do not stop a deploy")),
        "{:?}",
        h.bv().log.iter().collect::<Vec<_>>()
    );
    h.press("diag 0");
    assert_eq!(
        h.bv().projects.open.as_ref().unwrap().current.as_deref(),
        Some("blocks.st")
    );
    assert_eq!(h.app.chrome.line, Some(4));
}

#[test]
fn a_box_error_and_warning_are_both_listed_errors_first_and_only_the_error_fails() {
    let b = Bench::new();
    let jar = Jar::default();
    let mut v = page(&b, &jar, Some(("alice", ALICE)));
    let broken = MAIN.replace("lamp := l.q;", "lamp := nothing_here;");
    make(
        &b,
        &jar,
        &mut v.projects,
        "line",
        &[
            ("blocks.st", RETAIN_BLOCKS),
            ("main.st", &broken),
            ("io.map", MAP),
        ],
    );
    let before = b.json("/api/running");
    v.projects.deploy();
    settle(&b, &jar, &mut v.projects);
    let Deploy::Failed(why) = &v.projects.deploy else {
        panic!("{:?}", v.projects.deploy);
    };
    assert!(
        why.contains("does not build: 1 problem(s)"),
        "only errors are counted: {why}"
    );
    let d = &v.projects.diags;
    assert_eq!(d.len(), 2, "{d:?}");
    assert!(!d[0].is_warning() && d[0].file == "main.st", "{d:?}");
    assert!(d[1].is_warning() && d[1].file == "blocks.st", "{d:?}");
    assert_eq!(b.json("/api/running"), before, "nothing changed");
}

#[test]
fn a_check_answer_without_the_warnings_field_still_reads() {
    use crate::projects::Severity;
    // An older box: errors only, no `warnings`, no `severity`.
    let old = serde_json::json!({
        "error": "the project does not build",
        "diagnostics": [{ "file": "main.st", "line": 6, "col": 3, "message": "expected ;" }],
    });
    let d = crate::box_projects::diags_of(&old);
    assert_eq!(d.len(), 1);
    assert_eq!((d[0].severity, d[0].line), (Severity::Error, 6));
    // An older box's good answer: nothing to show.
    let count = |j: serde_json::Value| crate::box_projects::diags_of(&j).len();
    assert_eq!(count(serde_json::json!({ "ok": true, "diagnostics": [] })), 0);
    assert_eq!(count(serde_json::json!({})), 0);
    // A newer box: warnings, with the optional id and blocking flag the
    // lints will add; junk in them reads as a plain warning.
    let new = serde_json::json!({
        "diagnostics": [],
        "warnings": [
            { "file": "a.st", "line": 2, "col": 1, "message": "m", "severity": "warning",
              "id": "HELMSMAN-L1", "blocking": true },
            { "file": "b.st", "line": "x", "message": 7, "id": 3, "blocking": "yes" },
        ],
    });
    let d = crate::box_projects::diags_of(&new);
    assert_eq!(d.len(), 2);
    assert!(d[0].is_warning() && d[0].id.as_deref() == Some("HELMSMAN-L1") && d[0].blocking);
    assert!(d[1].is_warning() && d[1].id.is_none() && !d[1].blocking && d[1].line == 0);
    assert!(d[0].label().starts_with("warning HELMSMAN-L1  a.st:2:1"));
}

/// A first visit, nobody signed in: the box refuses the page's own reads
/// (401), which is the box asking for a sign-in and no error. The page
/// opened with a red "the box no longer knows this page's session" and
/// "1 error" on the Messages tab (2026-11-10). A session that ends under
/// a signed-in page still says so.
#[test]
fn a_signed_out_first_visit_reports_no_ended_session() {
    use crate::box_projects::PCall;
    let b = Bench::new();
    let jar = Jar::default();
    let mut v = page(&b, &jar, Some(("alice", ALICE)));
    let p = &mut v.projects;
    // The box's answer to the projects list from a page with no session.
    let refused = b.send(&Jar::default(), &PReq::get(PCall::List, "/api/projects".into()));
    assert_eq!(refused.status, 401);

    p.notice = None;
    p.answer_for(PCall::List, &refused, false);
    assert_eq!(p.notice, None, "signed out, nothing to report");
    assert!(p.ended, "the page still goes back to signing in");

    p.answer_for(PCall::List, &refused, true);
    let (msg, error) = p.notice.clone().unwrap();
    assert!(error && msg.contains("Sign in again"), "{msg}");

    // A signed-out page's own act (here a save) is still answered.
    p.notice = None;
    let save = PCall::Save {
        project: "line".into(),
        file: "main.st".into(),
    };
    p.answer_for(save, &refused, false);
    assert!(p.notice.is_some(), "an act that was refused says so");
}
