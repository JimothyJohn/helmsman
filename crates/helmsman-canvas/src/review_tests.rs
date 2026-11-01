//! The review screen against the real box: a runtime scanning on its own
//! thread with the recorder on, a real snapshot store, the floor policy,
//! the PIN file and API token in a temporary state directory, and the box
//! API over them. Every request is the [`Request`] the screen made,
//! carried out over a socket exactly as the browser's `fetch` would; only
//! the agent's draft is posted by the test (the canvas never drafts).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use helmsman_diagram::diff::{Change, Line};
use helmsman_iec::{ImageSizes, compile};
use helmsman_io::Value;
use helmsman_plc::api::{
    Api, Approval, Catalog, Config, Devices, Limits, Live, MemAudit, MemProposals, ModuleInfo, Plant,
    Publisher, Shared, swap, token,
};
use helmsman_plc::manifest::ModuleDir;
use helmsman_plc::policy::account::{Accounts, Logins};
use helmsman_plc::policy::pin::{Gate, Pins};
use helmsman_plc::policy::{FLOOR, Role};
use helmsman_plc::record::{RecordConfig, Recorder};
use helmsman_plc::spec::Defaults;
use helmsman_plc::store::{ChangeSet, Meta, ModuleFiles, Store};
use helmsman_plc::{Runtime, SignalTable, Window};
use serde_json::{Value as Json, json};

use crate::review::{Answer, Call, Delta, Method, Phase, Replay, Request, Review, Target, Why};

#[test]
fn explain_rides_the_same_cookie_and_is_refused_without_it() {
    let b = Bench::new();
    // `POST /api/explain` (the canvas no longer asks it; the box still
    // answers it): the body only, no token.
    let ask = Request {
        call: Call::List,
        method: Method::Post,
        url: "/api/explain".into(),
        body: Some(r#"{"module":"conveyor","path":"LAMP"}"#.into()),
    };
    let a = b.send(&ask);
    assert_eq!(a.status, 401, "not signed in: {a:?}");
    let mut r = Review::new();
    settle(&b, &mut r, 0.0);
    r.login_pin("operator", OPERATOR.into());
    settle(&b, &mut r, 0.0);
    assert!(b.jar.session().is_some());
    let a = b.send(&ask);
    assert_eq!(
        a.status, 503,
        "past the session check, to the agent (none here): {a:?}"
    );
    assert!(a.body.contains("no relay"), "{a:?}");
}

const OPERATOR: &str = "op-246810";
const ENGINEER: &str = "eng-135797";

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
VAR n : UDINT; blink : TON; END_VAR
  n := n + 1;
  lamp := start AND speed > 1.0;
END_PROGRAM";

/// Drives the horn too, blinking: output-affecting.
const MANIFEST_HORN: &str = "\
name     conveyor
author   Agent <agent@example.com>
prompt   Also sound the horn in pulses while start is held.
scan_ms  2
reads    start
drives   lamp horn
param    speed real 1.5 min=0 max=10
";

const SOURCE_HORN: &str = "
PROGRAM conveyor
VAR_INPUT start : BOOL; speed : REAL; END_VAR
VAR_OUTPUT lamp : BOOL; horn : BOOL; END_VAR
VAR n : UDINT; blink : TON; END_VAR
  n := n + 1;
  lamp := start AND speed > 1.0;
  blink(IN := NOT blink.Q, PT := T#20ms);
  IF blink.Q THEN horn := NOT horn; END_IF;
  horn := horn AND start;
END_PROGRAM";

const MAP: &str = "\
in  start bool box di:0
out lamp  bool box do:0 safe=false
out horn  bool box do:1 safe=false
";

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
    }
}

fn tmp(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "helmsman-canvas-review-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// The box, as `helmsman-plc` runs it with `--store --state-dir --api`.
pub struct Bench {
    api: Option<Api>,
    pub addr: SocketAddr,
    pub token: String,
    stop: Arc<AtomicBool>,
    scan: Option<JoinHandle<()>>,
    logs: Arc<Mutex<Vec<String>>>,
    dirs: Vec<PathBuf>,
    /// The browser the screen runs in: its cookie jar.
    pub jar: Jar,
}

/// A browser's cookie jar for the box: the session cookie, as the box
/// last set it, and every `Set-Cookie` it ever sent (to check the flags).
#[derive(Default)]
pub struct Jar {
    pub session: Mutex<Option<String>>,
    pub set: Mutex<Vec<String>>,
}

impl Jar {
    pub fn session(&self) -> Option<String> {
        self.session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
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

impl Bench {
    #[allow(clippy::too_many_lines)]
    pub fn new() -> Self {
        let d = Defaults {
            period: Duration::from_millis(2),
            i: Window::new(0, 0),
            q: Window::new(0, 0),
            m: 16,
        };
        // The state directory: the PIN file (set the way --set-pin does,
        // one PBKDF2 iteration to keep the test quick) and the API token.
        let state = tmp("state");
        let mut pins = Pins::default();
        pins.set(Role::Operator, OPERATOR, 1, [1; 16]).unwrap();
        pins.set(Role::Engineer, ENGINEER, 1, [2; 16]).unwrap();
        pins.save(&state).unwrap();
        let pins = Pins::load(&state, 1).unwrap();
        let token = token::load_or_create(&state).unwrap().token;

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
        Store::open(&store)
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
        let mut recorder = Recorder::new(&table, RecordConfig::default(), None);
        let (handle, swapper) = swap::channel(Duration::from_secs(5));
        let t0 = Instant::now();
        let approval = Approval {
            policy: FLOOR,
            // Logins wired as the binary wires them: PINs until an
            // accounts file appears in the state directory.
            gate: Gate::new(pins, FLOOR.lockout, Arc::new(Instant::now)).with_logins(Logins::new(
                state.clone(),
                1,
                FLOOR.lockout,
                Arc::new(Instant::now),
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
        let logs = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&logs);
        let cfg = Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            token: token.clone(),
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
            snapshots: Some(Mutex::new(Store::open(&store).unwrap())),
            approval: Some(approval),
            chat: None,
        };
        let api = Api::start(cfg, shared).unwrap();
        let addr = api.addr();
        let stop = Arc::new(AtomicBool::new(false));
        let ticks = Arc::new(AtomicU64::new(0));
        let scan = {
            let (stop, table, ticks) = (Arc::clone(&stop), Arc::clone(&table), Arc::clone(&ticks));
            let mut publisher = Publisher::new(live, Duration::from_millis(5));
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let mut t = table.lock().unwrap_or_else(PoisonError::into_inner);
                    // start held for 150 ms in every 300: the horn has something to do.
                    let ms = t0.elapsed().as_millis();
                    t.set_input(start, Some(Value::Bool(ms % 300 < 150)));
                    swapper.serve(&mut rt, &mut t);
                    let now = t0.elapsed();
                    recorder.observe(now, &t);
                    rt.tick_with(now, &[], &mut t);
                    publisher.publish(now, &rt, &t);
                    ticks.fetch_add(1, Ordering::Relaxed);
                    drop(t);
                    thread::sleep(Duration::from_millis(1));
                }
            })
        };
        // At least a second recorded, so replays have rises to compare.
        wait("a recorded window", || ticks.load(Ordering::Relaxed) > 400);
        thread::sleep(Duration::from_millis(1100).saturating_sub(t0.elapsed()));
        Self {
            api: Some(api),
            addr,
            token,
            stop,
            scan: Some(scan),
            logs,
            dirs: vec![state, store],
            jar: Jar::default(),
        }
    }

    /// The box's state directory (PINs, token, an accounts file).
    pub fn state_dir(&self) -> &Path {
        &self.dirs[0]
    }

    /// Carries out `req` the way `web::Http` does in the bench's browser
    /// ([`Bench::jar`]).
    pub fn send(&self, req: &Request) -> Answer {
        self.send_in(&self.jar, req)
    }

    /// Carries out `req` the way the browser's `fetch` does for
    /// `web::Http` on the box's page: same origin (`Host: box`, and for a
    /// `POST` its `Origin` and `X-Helmsman-Request: 1`), the jar's session
    /// cookie, a JSON body, no `Authorization`; the `Set-Cookie` it gets
    /// back goes into the jar. The answer's status, `Retry-After` and body
    /// whatever the status.
    pub fn send_in(&self, jar: &Jar, req: &Request) -> Answer {
        let mut head = format!("{} {} HTTP/1.1\r\nHost: box\r\n", req.method.as_str(), req.url);
        if req.method != Method::Get {
            head.push_str("X-Helmsman-Request: 1\r\nOrigin: http://box\r\n");
        }
        if let Some(s) = jar.session() {
            let _ = write!(head, "Cookie: theme=dark; helmsman_session={s}\r\n");
        }
        let a = self.raw(&head, req.body.as_deref());
        for c in &a.1 {
            jar.set
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(c.clone());
            let value = c
                .strip_prefix("helmsman_session=")
                .and_then(|r| r.split(';').next())
                .unwrap_or_default();
            *jar.session.lock().unwrap_or_else(PoisonError::into_inner) =
                (!c.contains("Max-Age=0") && !value.is_empty()).then(|| value.to_string());
        }
        a.0
    }

    /// A machine's call (the agent, a script): the box token as bearer,
    /// no cookie.
    pub fn machine(&self, method: &str, url: &str, body: Option<&str>) -> Answer {
        let head = format!(
            "{method} {url} HTTP/1.1\r\nHost: box\r\nAuthorization: Bearer {}\r\n",
            self.token
        );
        self.raw(&head, body).0
    }

    /// Sends `head` (without its blank line) and `body`; the answer and
    /// its `Set-Cookie` values.
    fn raw(&self, head: &str, body: Option<&str>) -> (Answer, Vec<String>) {
        let mut head = head.to_string();
        let has_body = body.is_some();
        let body = body.unwrap_or_default().to_string();
        if has_body {
            let _ = write!(
                head,
                "Content-Type: application/json\r\nContent-Length: {}\r\n",
                body.len()
            );
        }
        head.push_str("\r\n");
        let mut s = TcpStream::connect(self.addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        s.write_all(head.as_bytes()).unwrap();
        s.write_all(body.as_bytes()).unwrap();
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        let text = String::from_utf8_lossy(&out).into_owned();
        let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let retry_after = head
            .lines()
            .find_map(|l| l.strip_prefix("Retry-After: "))
            .and_then(|v| v.trim().parse().ok());
        let cookies = head
            .lines()
            .filter_map(|l| l.strip_prefix("Set-Cookie: "))
            .map(str::to_string)
            .collect();
        let a = Answer {
            status: head.get(9..12).and_then(|s| s.parse().ok()).unwrap_or(0),
            retry_after,
            body: body.into(),
        };
        (a, cookies)
    }

    /// The agent's side: a draft posted with the token.
    pub fn draft(&self, body: &Json) -> u64 {
        let a = self.machine("POST", "/api/proposals", Some(&body.to_string()));
        assert_eq!(a.status, 201, "{a:?}");
        serde_json::from_str::<Json>(&a.body).unwrap()["id"]
            .as_u64()
            .unwrap()
    }

    pub fn horn(&self) -> u64 {
        self.draft(&json!({ "kind": "module", "manifest": MANIFEST_HORN, "source": SOURCE_HORN }))
    }

    pub fn running_source(&self) -> String {
        let a = self.machine("GET", "/api/modules/conveyor", None);
        serde_json::from_str::<Json>(&a.body).unwrap()["source"]
            .as_str()
            .unwrap()
            .to_string()
    }

    pub fn logs(&self) -> String {
        self.logs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .join("\n")
    }

    pub fn audit(&self) -> String {
        self.machine("GET", "/api/audit?limit=1000", None).body
    }
}

/// Sends everything `r` has queued, and what that queues, until it is quiet.
pub fn settle(b: &Bench, r: &mut Review, now: f64) {
    settle_in(b, &b.jar, r, now);
}

/// [`settle`], in the browser whose cookies are `jar`.
pub fn settle_in(b: &Bench, jar: &Jar, r: &mut Review, now: f64) {
    for _ in 0..20 {
        let reqs = r.poll();
        if reqs.is_empty() {
            return;
        }
        for q in reqs {
            let a = b.send_in(jar, &q);
            r.answer(q.call, &a, now);
        }
    }
    panic!("the review never went quiet");
}

fn refused(p: &Phase) -> (Why, String) {
    match p {
        Phase::Refused(r) => (r.why, r.error.clone()),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn review_replay_approve_and_undo_against_the_real_box() {
    let b = Bench::new();
    let first = b.draft(&json!({ "kind": "param", "module": "conveyor", "param": "speed", "value": 0.5 }));
    let id = b.horn();
    let mut r = Review::new();
    settle(&b, &mut r, 0.0);
    assert_eq!(
        r.list.iter().map(|x| x.id).collect::<Vec<_>>(),
        [id, first],
        "newest first"
    );
    assert_eq!(r.outputs, ["LAMP", "HORN"]);
    assert!(r.policy.is_some());

    r.open(id);
    settle(&b, &mut r, 0.0);
    let o = r.open.as_ref().unwrap();
    assert_eq!(o.tier(), Some("output-affecting"), "the box's tier");
    assert_eq!(o.needs(), Some(("engineer".into(), true)));
    assert_ne!(o.reasons(), Vec::<String>::new());
    // The source diff: the horn lines added, nothing of the old removed.
    let added: Vec<&str> = o
        .lines
        .iter()
        .filter_map(|l| match l {
            Line::Added { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(added.contains(&"  horn := horn AND start;"), "{added:?}");
    assert!(o.lines.iter().any(
        |l| matches!(l, Line::Removed { text, .. } if text.contains("VAR_OUTPUT lamp : BOOL; END_VAR"))
    ));
    // The diagram diff: the horn's writes added, the lamp untouched.
    let d = o.drawn.as_ref().expect("both sides drawn");
    let added_writes: Vec<String> = d
        .diff
        .nodes
        .iter()
        .filter(|c| c.change == Change::Added)
        .filter_map(|c| c.new.as_ref())
        .filter(|(_, s)| s.key.what.starts_with("write"))
        .map(|(_, s)| s.text.clone())
        .collect();
    assert!(
        added_writes.contains(&"HORN := HORN AND START".to_string()),
        "{added_writes:?}"
    );
    assert!(
        !d.diff.nodes.iter().any(|c| c
            .new
            .as_ref()
            .or(c.old.as_ref())
            .is_some_and(|(_, s)| s.text.starts_with("LAMP"))),
        "the lamp's logic did not change: {:#?}",
        d.diff
    );
    // The manifest: horn is newly driven and stands out.
    let horn = o
        .manifest
        .iter()
        .find(|m| m.field == "drives" && m.name == "horn")
        .unwrap();
    assert_eq!((horn.delta, horn.output), (Delta::Added, true));

    let t = Target::Proposal(id);
    let step = |r: &Review| r.open.as_ref().unwrap().step.clone();
    // Not signed in: nothing is sent.
    r.approve(&t, 1.0);
    assert_eq!(r.poll(), Vec::<Request>::new());
    assert_eq!(refused(&step(&r).phase).0, Why::SignIn);
    assert_eq!(b.jar.session(), None);

    // Signed in by the operator PIN: the box sets the cookie, the page
    // never sees it, and an operator is below the tier (403).
    r.login_pin("operator", OPERATOR.into());
    settle(&b, &mut r, 1.0);
    assert_eq!(r.signed_in().as_deref(), Some("operator (PIN)"));
    let operator = b.jar.session().expect("the browser holds the cookie");
    assert!(!format!("{r:?}").contains(&operator), "the screen never sees it");
    assert_eq!(
        b.jar.set.lock().unwrap().last().map(String::as_str),
        Some(
            format!("helmsman_session={operator}; HttpOnly; SameSite=Strict; Path=/api; Max-Age=3600")
                .as_str()
        )
    );
    r.approve(&t, 1.0);
    settle(&b, &mut r, 1.0);
    let (why, error) = refused(&step(&r).phase);
    assert_eq!(why, Why::Role);
    assert_eq!(
        error,
        "a output-affecting change needs the engineer role or above"
    );

    // Sign out (the box ends it and clears the cookie), then a wrong
    // engineer PIN at sign-in: the box's words.
    r.logout();
    settle(&b, &mut r, 1.5);
    assert_eq!((r.signed_in(), b.jar.session()), (None, None));
    r.login_pin("engineer", "not-the-pin".into());
    settle(&b, &mut r, 1.5);
    assert_eq!(r.login_problem.as_deref(), Some("wrong PIN"));
    assert_eq!(r.signed_in(), None);

    // The engineer, without the replay: 428.
    r.login_pin("engineer", ENGINEER.into());
    settle(&b, &mut r, 2.0);
    assert_eq!(r.signed_in().as_deref(), Some("engineer (PIN)"));
    let engineer = b.jar.session().unwrap();
    assert_ne!(engineer, operator, "a new session, never the old one reused");
    r.approve(&t, 2.0);
    settle(&b, &mut r, 2.0);
    assert_eq!(refused(&step(&r).phase).0, Why::Replay);

    // The replay: the horn would have fired, the lamp is the same.
    r.replay(&t);
    settle(&b, &mut r, 3.0);
    let Replay::Done(chart) = step(&r).replay else {
        panic!("{:?}", step(&r).replay)
    };
    assert!(chart.end_ms - chart.start_ms >= 1000, "{chart:?}");
    let lamp = chart
        .signals
        .iter()
        .find(|s| s.signal.eq_ignore_ascii_case("lamp"))
        .unwrap();
    assert!(lamp.identical, "{lamp:?}");
    let horn = chart
        .signals
        .iter()
        .find(|s| s.signal.eq_ignore_ascii_case("horn"))
        .unwrap();
    assert!(!horn.identical);
    assert!(horn.current.is_none(), "nothing drives the horn today");
    let proposed = horn.proposed.as_ref().unwrap();
    assert_eq!(proposed.ty, "bool");
    assert!(!proposed.truncated);
    assert_eq!(
        proposed.rises().len() as u64,
        horn.rises.1,
        "the marked rises are the box's count"
    );
    assert!(horn.rises.1 > 0, "{horn:?}");
    assert_eq!(horn.extra_rises, i64::try_from(horn.rises.1).unwrap());
    assert!(
        horn.summary()
            .starts_with(&format!("would have fired {} more", horn.rises.1))
    );

    // Approved as the engineer session, citing the replay: applied, with
    // the undo offered.
    let before = b.running_source();
    r.approve(&t, 5.0);
    settle(&b, &mut r, 5.0);
    let Phase::Applied(done) = step(&r).phase else {
        panic!("{:?}", step(&r).phase)
    };
    assert_eq!(done.tier, "output-affecting");
    assert_eq!(b.running_source(), SOURCE_HORN);
    assert_ne!(before, SOURCE_HORN);
    let head = r.head.clone().expect("the new snapshot is shown");
    assert_eq!(head["id"].as_str(), Some(done.snapshot.as_str()));
    assert_eq!(head["approver"].as_str(), Some("engineer (PIN)"));
    assert!(r.list.iter().any(|x| x.id == id && x.status == "applied"));

    // A param draft made against the horn goes stale once it is undone.
    let stale = b.draft(&json!({ "kind": "param", "module": "conveyor", "param": "speed", "value": 2.5 }));
    settle(&b, &mut r, 5.0);

    // Undo goes through the same flow: replay, then approve.
    let ut = Target::Snapshot(done.previous.clone());
    r.approve(&ut, 6.0);
    settle(&b, &mut r, 6.0);
    let undo = |r: &Review| r.open.as_ref().unwrap().undo.clone().unwrap();
    assert_eq!(
        refused(&undo(&r).phase).0,
        Why::Replay,
        "an undo of an output change needs its replay"
    );
    r.replay(&ut);
    settle(&b, &mut r, 7.0);
    assert!(
        matches!(undo(&r).replay, Replay::Done(_)),
        "{:?}",
        undo(&r).replay
    );
    r.approve(&ut, 8.0);
    settle(&b, &mut r, 8.0);
    assert!(
        matches!(undo(&r).phase, Phase::Applied(_)),
        "{:?}",
        undo(&r).phase
    );
    assert_eq!(b.running_source(), SOURCE, "back to what ran");

    // The param draft made while the horn ran is stale: 409, kindly.
    r.open(stale);
    settle(&b, &mut r, 9.0);
    let o = r.open.as_ref().unwrap();
    assert_eq!(o.tier(), Some("param"));
    let p = o.param.as_ref().unwrap();
    assert_eq!((p.from.as_str(), p.to.as_str()), ("1.5", "2.5"));
    assert_eq!((p.min.as_deref(), p.max.as_deref()), (Some("0.0"), Some("10.0")));
    r.approve(&Target::Proposal(stale), 9.0);
    settle(&b, &mut r, 9.0);
    let (why, error) = refused(&step(&r).phase);
    assert_eq!(why, Why::Stale);
    assert!(error.starts_with("stale: the box has changed"), "{error}");

    // A reload: a new screen in the same browser finds the session by its
    // cookie, without signing in again.
    let mut reloaded = Review::new();
    settle(&b, &mut reloaded, 9.5);
    assert_eq!(reloaded.signed_in().as_deref(), Some("engineer (PIN)"));

    // Signed out, wrong PINs at sign-in until the box locks: the screen
    // says when to try again, sets no session, and the box's lockout is
    // the one approvals use.
    r.logout();
    settle(&b, &mut r, 10.0);
    let mut locked = None;
    for k in 0..8 {
        r.login_pin("operator", "guess-000".into());
        settle(&b, &mut r, 10.0 + f64::from(k));
        let p = r.login_problem.clone().unwrap();
        if p.starts_with("too many wrong PINs") {
            locked = Some(p);
            break;
        }
        assert_eq!(p, "wrong PIN");
    }
    let p = locked.expect("the box locks PIN entry");
    assert!(p.contains("Try again in "), "{p}");
    r.login_pin("operator", OPERATOR.into());
    settle(&b, &mut r, 20.0);
    assert_eq!(r.signed_in(), None, "locked: even the right PIN is not taken");
    assert_eq!(b.jar.session(), None);
    let fresh = b.draft(&json!({ "kind": "param", "module": "conveyor", "param": "speed", "value": 3.0 }));
    let a = b.machine(
        "POST",
        &Target::Proposal(fresh).apply_url(),
        Some(&json!({ "role": "operator", "pin": OPERATOR }).to_string()),
    );
    assert_eq!(a.status, 429, "one lockout for sign-in and apply: {a:?}");

    // No PIN, no token and no session ever reached the box's log or its
    // audit.
    let (logs, audit) = (b.logs(), b.audit());
    for secret in [
        OPERATOR,
        ENGINEER,
        "guess-000",
        "not-the-pin",
        b.token.as_str(),
        operator.as_str(),
        engineer.as_str(),
    ] {
        assert!(!logs.contains(secret), "{secret} in the log");
        assert!(!audit.contains(secret), "{secret} in the audit");
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn on_a_box_with_accounts_the_screen_logs_in_and_approves_as_the_person() {
    const ALICE: &str = "alice-pass-31337";
    let b = Bench::new();
    let id = b.horn();
    let mut accounts = Accounts::default();
    accounts.add("alice", Role::Engineer, ALICE, 1, [1; 16]).unwrap();
    accounts
        .add("bob", Role::Operator, "bob-pass-27182", 1, [2; 16])
        .unwrap();
    accounts.save(&b.dirs[0]).unwrap();
    let mut r = Review::new();
    settle(&b, &mut r, 0.0);
    assert!(r.accounts(), "the policy says so: {:?}", r.policy);
    let step = |r: &Review| r.open.as_ref().unwrap().step.clone();

    // Nothing typed: nothing sent.
    r.login("alice", String::new());
    assert_eq!(r.poll(), Vec::<Request>::new());
    assert!(r.login_problem.is_some());
    // A wrong password: the box's words, no session.
    r.login("alice", "not-her-password".into());
    let sent = r.poll();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].url, "/api/login");
    assert!(
        !format!("{sent:?}").contains("not-her-password"),
        "Debug redacts the body"
    );
    let a = b.send(&sent[0]);
    r.answer(sent[0].call.clone(), &a, 0.0);
    assert_eq!(r.login_problem.as_deref(), Some("wrong user or password"));
    assert!(r.session.is_none() && !r.logging_in);

    // Her password: signed in, and no PIN anywhere after.
    r.login("  alice ", ALICE.into());
    settle(&b, &mut r, 0.0);
    assert_eq!(r.signed_in().as_deref(), Some("alice (engineer)"));
    assert!(!format!("{r:?}").contains(ALICE), "{r:?}");
    let alice = b.jar.session().expect("her session is the browser's cookie");
    assert!(!format!("{r:?}").contains(&alice), "the screen never sees it");
    let t = Target::Proposal(id);
    r.open(id);
    settle(&b, &mut r, 0.0);
    r.replay(&t);
    let sent = r.poll();
    assert_eq!(sent.len(), 1);
    for q in sent {
        let a = b.send(&q);
        r.answer(q.call, &a, 1.0);
    }
    assert!(
        matches!(step(&r).replay, Replay::Done(_)),
        "{:?}",
        step(&r).replay
    );
    r.approve(&t, 2.0);
    let sent = r.poll();
    assert_eq!(sent.len(), 1);
    let body: Json = serde_json::from_str(sent[0].body.as_deref().unwrap()).unwrap();
    assert!(body.get("pin").is_none() && body.get("role").is_none(), "{body}");
    for q in sent {
        let a = b.send(&q);
        r.answer(q.call, &a, 2.0);
    }
    settle(&b, &mut r, 2.0);
    let Phase::Applied(done) = step(&r).phase else {
        panic!("{:?}", step(&r).phase)
    };
    assert_eq!(
        (done.user.as_deref(), done.role.as_str()),
        (Some("alice"), "engineer")
    );
    assert_eq!(r.head.as_ref().unwrap()["approver"], "alice (engineer)");
    let audit = b.audit();
    assert!(
        audit.contains("\"user\":\"alice\""),
        "the audit names her: {audit}"
    );

    // A PIN, even the right one, is refused by the box: by the token at an
    // apply, and at sign-in from another browser, which stays signed out.
    let p = b.draft(&json!({ "kind": "param", "module": "conveyor", "param": "speed", "value": 0.5 }));
    let a = b.machine(
        "POST",
        &Target::Proposal(p).apply_url(),
        Some(&json!({ "role": "engineer", "pin": ENGINEER }).to_string()),
    );
    assert_eq!(a.status, 403, "{a:?}");
    assert!(a.body.contains("role PINs are off"), "{a:?}");
    let other = Jar::default();
    let mut by_pin = Review::new();
    settle_in(&b, &other, &mut by_pin, 3.0);
    by_pin.login_pin("engineer", ENGINEER.into());
    settle_in(&b, &other, &mut by_pin, 3.0);
    assert!(
        by_pin
            .login_problem
            .as_deref()
            .unwrap()
            .contains("role PINs are off"),
        "{:?}",
        by_pin.login_problem
    );
    assert_eq!((by_pin.signed_in(), other.session()), (None, None));
    by_pin.open(p);
    settle_in(&b, &other, &mut by_pin, 3.0);
    by_pin.approve(&Target::Proposal(p), 3.0);
    assert_eq!(by_pin.poll(), Vec::<Request>::new());
    assert_eq!(refused(&by_pin.open.as_ref().unwrap().step.phase).0, Why::SignIn);

    // The account disabled while she is signed in: the next call is 401,
    // the box clears the cookie, and the screen goes back to signing in.
    let undo = r.open.as_ref().unwrap().undo.as_ref().unwrap().target.clone();
    accounts.set_disabled("alice", true).unwrap();
    accounts.save(&b.dirs[0]).unwrap();
    r.replay(&undo);
    settle(&b, &mut r, 4.0);
    assert!(r.session.is_none(), "a refused session is forgotten");
    assert!(r.login_problem.as_deref().unwrap().contains("Sign in again"));
    assert_eq!(b.jar.session(), None, "and the browser dropped the cookie");
    accounts.set_disabled("alice", false).unwrap();
    accounts.save(&b.dirs[0]).unwrap();

    // Sign out: gone here at once, then on the box (an old copy of the
    // cookie is refused there).
    r.login("alice", ALICE.into());
    settle(&b, &mut r, 5.0);
    assert!(r.session.is_some());
    let cookie = b.jar.session().unwrap();
    assert_ne!(cookie, alice, "each sign-in is a new session");
    r.logout();
    assert!(r.session.is_none());
    let sent = r.poll();
    assert_eq!(sent.len(), 1);
    let a = b.send(&sent[0]);
    assert_eq!(a.status, 200, "{a:?}");
    assert_eq!(b.jar.session(), None, "the box cleared the cookie");
    let kept = Jar::default();
    *kept.session.lock().unwrap() = Some(cookie.clone());
    let again = b.send_in(&kept, &sent[0]);
    assert_eq!(again.status, 401, "the box ended it too: {again:?}");
    r.approve(&undo, 6.0);
    assert_eq!(r.poll(), Vec::<Request>::new(), "signed out: nothing sent");

    let (logs, audit) = (b.logs(), b.audit());
    for secret in [
        ALICE,
        "bob-pass-27182",
        "not-her-password",
        alice.as_str(),
        cookie.as_str(),
    ] {
        assert!(!logs.contains(secret), "{secret} in the log");
        assert!(!audit.contains(secret), "{secret} in the audit");
    }
}
