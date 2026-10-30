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
use helmsman_plc::policy::pin::{Gate, Pins};
use helmsman_plc::policy::{FLOOR, Role};
use helmsman_plc::record::{RecordConfig, Recorder};
use helmsman_plc::spec::Defaults;
use helmsman_plc::store::{ChangeSet, Meta, ModuleFiles, Store};
use helmsman_plc::{Runtime, SignalTable, Window};
use serde_json::{Value as Json, json};

use crate::review::{Answer, Call, Delta, Method, Phase, Replay, Request, Review, Target, Why};

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
                source: SOURCE.into(),
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
            gate: Gate::new(pins, FLOOR.lockout, Arc::new(Instant::now)),
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
        }
    }

    /// Carries out `req` the way `web::Http` does: method, path, the
    /// bearer only in its header, a JSON body; the answer's status,
    /// `Retry-After` and body whatever the status.
    pub fn send(&self, req: &Request) -> Answer {
        let mut head = format!("{} {} HTTP/1.1\r\nHost: box\r\n", req.method.as_str(), req.url);
        if let Some(t) = &req.bearer {
            let _ = write!(head, "Authorization: Bearer {t}\r\n");
        }
        let body = req.body.clone().unwrap_or_default();
        if req.body.is_some() {
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
        Answer {
            status: head.get(9..12).and_then(|s| s.parse().ok()).unwrap_or(0),
            retry_after,
            body: body.into(),
        }
    }

    /// The agent's side: a draft posted with the token.
    pub fn draft(&self, body: &Json) -> u64 {
        let a = self.send(&Request {
            call: Call::List,
            method: Method::Post,
            url: "/api/proposals".into(),
            bearer: Some(self.token.clone()),
            body: Some(body.to_string()),
        });
        assert_eq!(a.status, 201, "{a:?}");
        serde_json::from_str::<Json>(&a.body).unwrap()["id"]
            .as_u64()
            .unwrap()
    }

    pub fn horn(&self) -> u64 {
        self.draft(&json!({ "kind": "module", "manifest": MANIFEST_HORN, "source": SOURCE_HORN }))
    }

    pub fn running_source(&self) -> String {
        let a = self.send(&Request {
            call: Call::List,
            method: Method::Get,
            url: "/api/modules/conveyor".into(),
            bearer: None,
            body: None,
        });
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
        self.send(&Request {
            call: Call::List,
            method: Method::Get,
            url: "/api/audit?limit=1000".into(),
            bearer: None,
            body: None,
        })
        .body
    }
}

/// Sends everything `r` has queued, and what that queues, until it is quiet.
pub fn settle(b: &Bench, r: &mut Review, now: f64) {
    for _ in 0..20 {
        let reqs = r.poll();
        if reqs.is_empty() {
            return;
        }
        for q in reqs {
            let a = b.send(&q);
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
    let pin = |r: &mut Review, role: &str, p: &str| {
        let s = &mut r.open.as_mut().unwrap().step;
        s.role = role.into();
        s.pin = p.into();
    };
    // No token: nothing is sent.
    pin(&mut r, "engineer", ENGINEER);
    r.approve(&t, 1.0);
    assert_eq!(r.poll(), Vec::<Request>::new());
    assert_eq!(refused(&step(&r).phase).0, Why::Token);
    assert!(r.set_token(&b.token));

    // An operator is below the tier: 403, no PIN spent.
    pin(&mut r, "operator", OPERATOR);
    r.approve(&t, 1.0);
    settle(&b, &mut r, 1.0);
    let (why, error) = refused(&step(&r).phase);
    assert_eq!(why, Why::Role);
    assert_eq!(
        error,
        "a output-affecting change needs the engineer role or above"
    );
    assert_eq!(step(&r).pin, "", "cleared after the submit");

    // An engineer without the replay: 428.
    pin(&mut r, "engineer", ENGINEER);
    r.approve(&t, 2.0);
    settle(&b, &mut r, 2.0);
    assert_eq!(refused(&step(&r).phase).0, Why::Replay);

    // The replay: the horn would have fired, the lamp is the same.
    r.replay(&t, 3.0);
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

    // A wrong PIN: 403, said as the box says it.
    pin(&mut r, "engineer", "not-the-pin");
    r.approve(&t, 4.0);
    settle(&b, &mut r, 4.0);
    assert_eq!(refused(&step(&r).phase), (Why::Pin, "wrong PIN".into()));

    // The right one, citing the replay: applied, with the undo offered.
    let before = b.running_source();
    pin(&mut r, "engineer", ENGINEER);
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

    // Undo goes through the same flow: replay, then PIN.
    let ut = Target::Snapshot(done.previous.clone());
    {
        let u = r.open.as_mut().unwrap().undo.as_mut().unwrap();
        u.role = "engineer".into();
        u.pin = ENGINEER.into();
    }
    r.approve(&ut, 6.0);
    settle(&b, &mut r, 6.0);
    let undo = |r: &Review| r.open.as_ref().unwrap().undo.clone().unwrap();
    assert_eq!(
        refused(&undo(&r).phase).0,
        Why::Replay,
        "an undo of an output change needs its replay"
    );
    r.replay(&ut, 7.0);
    settle(&b, &mut r, 7.0);
    assert!(
        matches!(undo(&r).replay, Replay::Done(_)),
        "{:?}",
        undo(&r).replay
    );
    r.open.as_mut().unwrap().undo.as_mut().unwrap().pin = ENGINEER.into();
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
    pin(&mut r, "operator", OPERATOR);
    r.approve(&Target::Proposal(stale), 9.0);
    settle(&b, &mut r, 9.0);
    let (why, error) = refused(&step(&r).phase);
    assert_eq!(why, Why::Stale);
    assert!(error.starts_with("stale: the box has changed"), "{error}");

    // Wrong PINs until the box locks: 429 with a countdown, then nothing
    // is sent until it ends.
    let fresh = b.draft(&json!({ "kind": "param", "module": "conveyor", "param": "speed", "value": 3.0 }));
    r.open(fresh);
    settle(&b, &mut r, 10.0);
    let ft = Target::Proposal(fresh);
    let mut locked = None;
    for k in 0..8 {
        pin(&mut r, "operator", "guess-000");
        r.approve(&ft, 10.0 + f64::from(k));
        settle(&b, &mut r, 10.0 + f64::from(k));
        if let Phase::Refused(x) = &step(&r).phase
            && x.why == Why::Locked
        {
            locked = Some((x.clone(), 10.0 + f64::from(k)));
            break;
        }
    }
    let (x, at) = locked.expect("the box locks PIN entry");
    assert_eq!(x.status, 429);
    assert_eq!(x.error, "too many wrong PINs; PIN entry is locked");
    let secs = step(&r).locked_for(at).unwrap();
    assert!((1..=30).contains(&secs), "{secs}");
    pin(&mut r, "operator", OPERATOR);
    r.approve(&ft, at + 0.5);
    assert_eq!(r.poll(), Vec::<Request>::new(), "nothing goes while locked");

    // No PIN and no token ever reached the box's log or its audit.
    let (logs, audit) = (b.logs(), b.audit());
    for secret in [OPERATOR, ENGINEER, "guess-000", "not-the-pin", b.token.as_str()] {
        assert!(!logs.contains(secret), "{secret} in the log");
        assert!(!audit.contains(secret), "{secret} in the audit");
    }
}
