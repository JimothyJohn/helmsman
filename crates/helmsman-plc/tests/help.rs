//! `POST /api/help` end to end: a real box API with accounts (bob, an
//! operator: anyone signed in may ask for help), a device link that is
//! down, and a fake relay on a real socket that replays recorded model
//! turns in the relay protocol's shape and records every request (the
//! approach of tests/requirements.rs). The relay does not serve the help
//! profile yet (card `help-mode-relay`): this is the fixture it is built
//! against.
//!
//! The contract: the person's words and the page's messages go in; the
//! box attaches its build, the project it runs and its devices with their
//! errors; nothing that is a secret (the box token, the relay token, a
//! session token, a PIN) reaches the relay or the answer; the answer is a
//! diagnosis and a Markdown issue to copy, and nothing is filed. Without
//! the agent (no relay, offline, a profile the relay does not serve) the
//! box still writes the issue.
#![allow(clippy::too_many_lines)] // each test walks one whole flow

use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use helmsman_agent::http::{Timeouts, Url};
use helmsman_agent::{Retry, relay};
use helmsman_plc::api::{
    Api, Approval, Catalog, Config, Devices, Extras, Limits, Live, MemAudit, MemProposals, More, Plant,
    Shared, chat, help, swap,
};
use helmsman_plc::policy::account::{Accounts, Logins};
use helmsman_plc::policy::pin::{Gate, Pins};
use helmsman_plc::policy::{FLOOR, Role};
use helmsman_plc::spec::Defaults;
use helmsman_plc::{SignalTable, Window};
use serde_json::{Value as Json, json};

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const RELAY_TOKEN: &str = "hmr1_00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
const BOB: &str = "bob-operator-pw";

const QUESTION: &str = "The conveyor stopped and the lamp on the panel is red.\nIt worked yesterday.";

// ---------------------------------------------------------------- relay

#[derive(Clone)]
enum Reply {
    Stream(String),
    /// A plain JSON error: status and body.
    Status(u16, Json),
}

#[derive(Debug, Clone)]
struct Seen {
    head: String,
    body: Json,
}

struct FakeRelay {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl FakeRelay {
    fn start(script: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (seen, stop) = (Arc::clone(&seen), Arc::clone(&stop));
            thread::spawn(move || {
                let mut script = script.into_iter();
                while !stop.load(Ordering::Acquire) {
                    let Ok((mut s, _)) = listener.accept() else {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    };
                    s.set_nonblocking(false).unwrap();
                    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                    let Some(req) = read_request(&mut s) else { continue };
                    if req.head.starts_with("GET /v1/health ") {
                        let _ = write!(
                            s,
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
                        );
                        continue;
                    }
                    lock(&seen).push(req);
                    let (status, body) = match script.next() {
                        Some(Reply::Stream(body)) => {
                            let _ = write!(
                                s,
                                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nx-request-id: r\r\nConnection: close\r\n\r\n{body}"
                            );
                            continue;
                        }
                        Some(Reply::Status(status, body)) => (status, body),
                        None => (
                            400,
                            json!({"type":"error","error":{"type":"invalid_request","message":"script exhausted","retryable":false}}),
                        ),
                    };
                    let b = body.to_string();
                    let _ = write!(
                        s,
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nx-request-id: r\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}",
                        b.len()
                    );
                }
            })
        };
        Self {
            addr,
            seen,
            stop,
            thread: Some(thread),
        }
    }

    fn seen(&self) -> Vec<Seen> {
        lock(&self.seen).clone()
    }

    fn url(&self) -> Url {
        Url::parse(&format!("http://{}", self.addr)).unwrap()
    }
}

impl Drop for FakeRelay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn read_request(s: &mut TcpStream) -> Option<Seen> {
    let mut r = BufReader::new(s.try_clone().ok()?);
    let mut head = String::new();
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).ok()? == 0 {
            return None;
        }
        if line == "\r\n" {
            break;
        }
        head.push_str(&line);
    }
    let len = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    let mut body = vec![0; len];
    r.read_exact(&mut body).ok()?;
    Some(Seen {
        head,
        body: serde_json::from_slice(&body).unwrap_or(Json::Null),
    })
}

fn ev(out: &mut String, data: &Json) {
    let _ = write!(out, "event: {}\ndata: {data}\n\n", data["type"].as_str().unwrap());
}

/// One model turn in the relay's shape: text blocks and tool calls, the
/// tool input in fragments, then `relay_usage`.
fn turn(text: Option<&str>, calls: &[(&str, &str, Json)]) -> Reply {
    let mut out = String::new();
    ev(
        &mut out,
        &json!({"type":"message_start","message":{"id":"msg_test","type":"message","role":"assistant","content":[],"model":"claude-sonnet-5-5","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":100,"output_tokens":1}}}),
    );
    let mut i = 0;
    if let Some(t) = text {
        ev(
            &mut out,
            &json!({"type":"content_block_start","index":i,"content_block":{"type":"text","text":""}}),
        );
        ev(
            &mut out,
            &json!({"type":"content_block_delta","index":i,"delta":{"type":"text_delta","text":t}}),
        );
        ev(&mut out, &json!({"type":"content_block_stop","index":i}));
        i += 1;
    }
    for (id, name, input) in calls {
        ev(
            &mut out,
            &json!({"type":"content_block_start","index":i,"content_block":{"type":"tool_use","id":id,"name":name,"input":{}}}),
        );
        let text = input.to_string();
        let chars: Vec<char> = text.chars().collect();
        for chunk in chars.chunks(23) {
            let part: String = chunk.iter().collect();
            ev(
                &mut out,
                &json!({"type":"content_block_delta","index":i,"delta":{"type":"input_json_delta","partial_json":part}}),
            );
        }
        ev(&mut out, &json!({"type":"content_block_stop","index":i}));
        i += 1;
    }
    let stop = if calls.is_empty() { "end_turn" } else { "tool_use" };
    ev(
        &mut out,
        &json!({"type":"message_delta","delta":{"stop_reason":stop,"stop_sequence":null},"usage":{"output_tokens":50}}),
    );
    ev(&mut out, &json!({"type":"message_stop"}));
    let _ = write!(
        out,
        "event: relay_usage\ndata: {}\n\n",
        json!({"type":"relay_usage","request_id":"r","agent_version":help::PROFILE,"model":"claude-sonnet-5-5","input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":0,"cache_read_input_tokens":0})
    );
    Reply::Stream(out)
}

fn text(t: &str) -> Reply {
    turn(Some(t), &[])
}

const DIAGNOSIS: &str = "The remote I/O module rio is not answering, so the conveyor's inputs went to their safe values and the program stopped it. Check the module's power and its network cable, then that it still has address 192.168.3.60.";

fn issue(files: &[&str]) -> Json {
    json!({
        "diagnosis": DIAGNOSIS,
        "title": "Conveyor stops when the rio link drops",
        "what_happened": "The conveyor stopped and the panel lamp went red; the rio link is down.",
        "expected": "The conveyor keeps running, or the box says plainly which device is lost.",
        "steps": ["Run the line project", "Unplug rio's network cable"],
        "suspected_files": files,
    })
}

fn submit(id: &str, input: Json) -> Reply {
    turn(
        Some("Looking at the box's devices."),
        &[(id, help::SUBMIT, input)],
    )
}

// ---------------------------------------------------------------- bench

fn tmp(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "help-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

enum Agent {
    None,
    Relay(Url),
}

struct Bench {
    api: Option<Api>,
    addr: SocketAddr,
    _swapper: swap::Swapper,
}

impl Drop for Bench {
    fn drop(&mut self) {
        self.api.take();
    }
}

impl Bench {
    /// A box with one device link, `rio`, down with `error`.
    fn new(agent: &Agent, error: &str) -> Self {
        let state = tmp("state");
        let mut a = Accounts::default();
        a.add("bob", Role::Operator, BOB, 1, [2; 16]).unwrap();
        a.save(&state).unwrap();
        let t0 = Instant::now();
        let now: helmsman_plc::policy::pin::Clock = Arc::new(move || t0);
        let (handle, swapper) = swap::channel(Duration::from_secs(5));
        let approval = Approval {
            policy: FLOOR,
            gate: Gate::new(Pins::default(), FLOOR.lockout, Arc::clone(&now)).with_logins(Logins::new(
                state,
                1,
                FLOOR.lockout,
                now,
            )),
            plant: Plant {
                table: SignalTable::default(),
                i_len: 32,
                q_len: 32,
                defaults: Defaults {
                    period: Duration::from_millis(10),
                    i: Window::new(0, 0),
                    q: Window::new(0, 0),
                    m: 256,
                },
                record: None,
            },
            swap: handle,
        };
        let chat = match agent {
            Agent::None => None,
            Agent::Relay(url) => {
                let mut rc = relay::Config::new(url.clone(), RELAY_TOKEN.into(), "v7".into());
                rc.timeouts = Timeouts {
                    connect: Duration::from_secs(2),
                    idle: Duration::from_secs(10),
                    total: Duration::from_secs(20),
                };
                let mut s = chat::Setup::new(rc);
                s.probe_every = Duration::from_millis(50);
                s.retry = Retry {
                    attempts: 0,
                    first: Duration::from_millis(10),
                    cap: Duration::from_millis(40),
                };
                Some(s)
            }
        };
        let cfg = Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            token: TOKEN.into(),
            limits: Limits::default(),
            stream_every: Duration::from_millis(20),
            memory: 16,
            log: Arc::new(|_: &str| {}),
        };
        let devices = Arc::new(Devices::new(["rio".to_string()]));
        devices.set_with("rio", Some(error.to_string()), Some("192.168.3.60:502".into()));
        let shared = Shared {
            catalog: Catalog::default(),
            live: Arc::new(Live::default()),
            devices,
            audit: Box::new(MemAudit::new(100_000)),
            proposals: Box::new(MemProposals::new(1000)),
            running: None,
            snapshots: None,
            approval: Some(approval),
            chat,
        };
        let api = Api::start_box(cfg, shared, Extras::default(), None, More::default()).unwrap();
        let addr = api.addr();
        Self {
            api: Some(api),
            addr,
            _swapper: swapper,
        }
    }

    fn bob(&self) -> String {
        let r = send(
            self.addr,
            "POST",
            "/api/login",
            &[],
            &json!({ "user": "bob", "password": BOB }).to_string(),
        );
        assert_eq!(r.status, 200, "{r:?}");
        r.json()["token"].as_str().unwrap().to_string()
    }

    /// One help request: the status, the raw body, and every event.
    fn help(&self, headers: &[(String, String)], body: &Json) -> (u16, String, Vec<(String, Json)>) {
        let r = send(self.addr, "POST", "/api/help", headers, &body.to_string());
        if r.status != 200 {
            let j = r.json();
            return (r.status, r.body, vec![("body".into(), j)]);
        }
        let evs = events(&r.body);
        (r.status, r.body, evs)
    }

    fn audit(&self) -> Vec<Json> {
        send(self.addr, "GET", "/api/audit?limit=1000", &[], "")
            .json()
            .as_array()
            .unwrap()
            .clone()
    }
}

fn bearer(t: &str) -> (String, String) {
    ("Authorization".into(), format!("Bearer {t}"))
}

#[derive(Debug)]
struct Resp {
    status: u16,
    body: String,
}

impl Resp {
    fn json(&self) -> Json {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|e| panic!("not JSON ({e}): {} {}", self.status, self.body))
    }
}

fn send(addr: SocketAddr, method: &str, path: &str, headers: &[(String, String)], body: &str) -> Resp {
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: box\r\nContent-Type: application/json\r\n");
    for (n, v) in headers {
        let _ = write!(head, "{n}: {v}\r\n");
    }
    let _ = write!(head, "Content-Length: {}\r\n\r\n", body.len());
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    s.write_all(head.as_bytes()).unwrap();
    let _ = s.write_all(body.as_bytes());
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let text = String::from_utf8_lossy(&out).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    Resp {
        status: head.get(9..12).and_then(|s| s.parse().ok()).unwrap_or(0),
        body: body.into(),
    }
}

/// The events of a whole event-stream body.
fn events(body: &str) -> Vec<(String, Json)> {
    body.split("\n\n")
        .filter(|b| !b.trim().is_empty())
        .map(|b| {
            let name = b.lines().find_map(|l| l.strip_prefix("event: ")).unwrap();
            let data = b.lines().find_map(|l| l.strip_prefix("data: ")).unwrap();
            (name.to_string(), serde_json::from_str(data).unwrap())
        })
        .collect()
}

fn names(evs: &[(String, Json)]) -> Vec<&str> {
    evs.iter().map(|(n, _)| n.as_str()).collect()
}

fn last<'a>(evs: &'a [(String, Json)], name: &str) -> &'a Json {
    &evs.iter()
        .rev()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("no {name} in {evs:?}"))
        .1
}

/// The data inside a block's envelope.
fn data(text: &str) -> Json {
    let open = text.find("\">\n").unwrap() + 3;
    let close = text.rfind("\n</box-data>").unwrap();
    serde_json::from_str(&text[open..close]).unwrap()
}

/// A session token of the shape the box makes, never this box's own.
/// A session token the page held (not one this box issued).
const PAGE_SESSION: &str = "c0ffee00c0ffee00c0ffee00c0ffee00c0ffee00c0ffee00c0ffee00c0ffee00";
const OTHER_SESSION: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

/// Page messages carrying every secret a page could hold.
fn leaky_messages(session: &str) -> String {
    format!(
        "     1.0s info    Connected to the box.\n     2.0s error   Deploy failed: Authorization: Bearer {session}\n     3.0s warning relay token {RELAY_TOKEN} refused\n     4.0s error   login with pin=4321 failed; cookie helmsman_session={OTHER_SESSION}\n     5.0s error   The box is unreachable.\n"
    )
}

/// Every secret of this bench is absent from `text`.
fn no_secrets(text: &str, session: &str, what: &str) {
    for s in [TOKEN, RELAY_TOKEN, session, OTHER_SESSION, "pin=4321", "hmr1_"] {
        assert!(!text.contains(s), "{what} carries {s}: {text}");
    }
    // Nor any part of one long enough to matter.
    for s in [TOKEN, session, OTHER_SESSION] {
        assert!(!text.contains(&s[..24]), "{what} carries part of {s}: {text}");
    }
}

// ---------------------------------------------------------------- tests

#[test]
fn help_answers_with_a_diagnosis_and_an_issue_carrying_the_box_context() {
    let relay = FakeRelay::start(vec![
        submit(
            "t1",
            issue(&[
                "crates/helmsman-plc/src/link.rs:120",
                "docs/devices/moxa-iologik-e1213.md",
            ]),
        ),
        text("I wrote the issue."),
    ]);
    // The device's own error text carries the box token: it never leaves.
    let b = Bench::new(
        &Agent::Relay(relay.url()),
        &format!("connection refused (token={TOKEN})"),
    );
    let session = b.bob();
    let (s, raw, evs) = b.help(
        &[bearer(&session)],
        &json!({ "question": QUESTION, "messages": leaky_messages(&session) }),
    );
    assert_eq!(s, 200, "{evs:?}");
    assert_eq!(
        names(&evs),
        [
            "help",
            "text",
            "tool_call",
            "tool_result",
            "text",
            "issue",
            "done"
        ],
        "{evs:?}"
    );
    // The context the box attached, as the page shows it.
    let c = &evs[0].1["context"];
    assert_eq!(c["build"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(c["build"]["os"], std::env::consts::OS);
    let commit = c["build"]["commit"].as_str().unwrap();
    assert!(
        commit == "unknown" || (commit.len() == 12 && commit.bytes().all(|b| b.is_ascii_hexdigit())),
        "{commit}"
    );
    assert_eq!(c["running"]["project"], Json::Null);
    assert_eq!(c["devices"][0]["name"], "rio");
    assert_eq!(c["devices"][0]["state"], "down");
    assert_eq!(c["devices"][0]["address"], "192.168.3.60:502");
    assert!(
        c["devices"][0]["error"]
            .as_str()
            .unwrap()
            .starts_with("connection refused"),
        "{c}"
    );
    assert!(
        c["messages"]
            .as_str()
            .unwrap()
            .contains("The box is unreachable."),
        "{c}"
    );
    assert_eq!(last(&evs, "tool_result")["ok"], true, "{evs:?}");
    // The issue: the agent's, with the box's context after it.
    let i = last(&evs, "issue");
    assert_eq!(i["by"], "agent");
    assert_eq!(i["title"], "Conveyor stops when the rio link drops");
    assert_eq!(i["diagnosis"], DIAGNOSIS);
    let md = i["markdown"].as_str().unwrap();
    let order = [
        "# Conveyor stops when the rio link drops\n",
        "### What happened\n",
        "### What I expected\n",
        "### Steps to reproduce\n\n1. Run the line project\n2. Unplug rio's network cable\n",
        "### Diagnosis\n",
        "### Suspected files\n\n- [crates/helmsman-plc/src/link.rs:120](https://github.com/JimothyJohn/helmsman/blob/dev/crates/helmsman-plc/src/link.rs#L120)\n- [docs/devices/moxa-iologik-e1213.md](https://github.com/JimothyJohn/helmsman/blob/dev/docs/devices/moxa-iologik-e1213.md)\n",
        "### Context from the box\n",
        &format!(
            "- Build: helmsman-plc {}, commit {commit}, ",
            env!("CARGO_PKG_VERSION")
        ),
        "- Project: none deployed\n",
        "  - `rio`: down: connection refused",
        "(at 192.168.3.60:502)",
        "- Recent messages:\n\n```text\n",
        "The box is unreachable.",
    ];
    let mut at = 0;
    for part in order {
        let k = md[at..]
            .find(part)
            .unwrap_or_else(|| panic!("{part:?} missing or out of order in:\n{md}"));
        at += k + part.len();
    }
    assert_eq!(last(&evs, "done")["stop"], "end_turn");
    assert_eq!(last(&evs, "done")["why"], Json::Null);
    // The relay was asked for the help profile, with fixed words outside
    // the envelope and the person's words and context inside.
    let seen = relay.seen();
    assert_eq!(seen.len(), 2);
    assert!(
        seen[0]
            .head
            .starts_with("POST /v1/agents/helmsman%2Fhelp-v1/messages "),
        "{}",
        seen[0].head
    );
    let blocks = seen[0].body["messages"][0]["content"].as_array().unwrap().clone();
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0]["text"].as_str().unwrap(), help::ASK);
    let d = data(blocks[1]["text"].as_str().unwrap());
    assert_eq!(d["question"], QUESTION);
    assert_eq!(d["devices"], c["devices"]);
    // No secret anywhere: not to the relay, not in the answer.
    for (k, s) in seen.iter().enumerate() {
        no_secrets(&s.body.to_string(), &session, &format!("relay request {k}"));
    }
    no_secrets(&raw, &session, "the answer");
    // Nothing is filed: the box only wrote text. Audited with the person.
    assert!(
        b.audit()
            .iter()
            .any(|e| e["path"] == "/api/help" && e["status"] == 200 && e["user"] == "bob"),
        "audited"
    );
}

#[test]
fn a_relay_that_does_not_serve_help_leaves_the_box_issue() {
    let relay = FakeRelay::start(vec![Reply::Status(
        404,
        json!({"type":"error","error":{"type":"unknown_agent_version","message":"no such agent: helmsman/help-v1","retryable":false},"request_id":"r"}),
    )]);
    let b = Bench::new(&Agent::Relay(relay.url()), "connection refused");
    let (s, raw, evs) = b.help(
        &[bearer(TOKEN)],
        &json!({ "question": QUESTION, "messages": leaky_messages(PAGE_SESSION) }),
    );
    assert_eq!(s, 200, "{evs:?}");
    assert_eq!(names(&evs), ["help", "issue", "done"], "{evs:?}");
    let i = last(&evs, "issue");
    assert_eq!(i["by"], "box");
    assert_eq!(
        i["title"],
        "The conveyor stopped and the lamp on the panel is red."
    );
    assert_eq!(i["diagnosis"], "");
    let md = i["markdown"].as_str().unwrap();
    assert!(
        md.starts_with("# The conveyor stopped and the lamp on the panel is red.\n"),
        "{md}"
    );
    assert!(md.contains(&format!("### What happened\n\n{QUESTION}\n")), "{md}");
    assert!(md.contains("### Steps to reproduce\n\n(not given)"), "{md}");
    assert!(
        !md.contains("### Diagnosis") && !md.contains("### Suspected files"),
        "{md}"
    );
    assert!(md.contains("  - `rio`: down: connection refused"), "{md}");
    let d = last(&evs, "done");
    assert_eq!(d["stop"], "unavailable");
    let why = d["why"].as_str().unwrap();
    assert!(
        why.contains("not available") && why.contains(help::PROFILE),
        "{why}"
    );
    no_secrets(&raw, PAGE_SESSION, "the answer");
    no_secrets(
        &relay.seen()[0].body.to_string(),
        PAGE_SESSION,
        "the relay request",
    );
}

#[test]
fn a_box_without_a_relay_still_writes_the_issue() {
    let b = Bench::new(&Agent::None, "timed out");
    let (s, raw, evs) = b.help(
        &[bearer(TOKEN)],
        &json!({ "question": "  Nothing works.  ", "messages": leaky_messages(PAGE_SESSION) }),
    );
    assert_eq!(s, 200, "{evs:?}");
    assert_eq!(names(&evs), ["help", "issue", "done"], "{evs:?}");
    assert_eq!(last(&evs, "issue")["by"], "box");
    assert_eq!(last(&evs, "issue")["title"], "Nothing works.");
    let d = last(&evs, "done");
    assert_eq!(d["stop"], "offline");
    assert!(d["why"].as_str().unwrap().contains("no relay configured"), "{d}");
    no_secrets(&raw, PAGE_SESSION, "the answer");
}

#[test]
fn a_bad_issue_is_answered_with_why_and_the_next_one_is_kept() {
    let relay = FakeRelay::start(vec![
        submit("t1", issue(&["../../etc/passwd"])),
        submit("t2", issue(&["crates/helmsman-plc/src/link.rs"])),
        text("Done."),
    ]);
    let b = Bench::new(&Agent::Relay(relay.url()), "connection refused");
    let (s, _, evs) = b.help(&[bearer(TOKEN)], &json!({ "question": QUESTION }));
    assert_eq!(s, 200, "{evs:?}");
    let results: Vec<&Json> = evs
        .iter()
        .filter(|(n, _)| n == "tool_result")
        .map(|(_, d)| d)
        .collect();
    assert_eq!(results.len(), 2, "{evs:?}");
    assert_eq!(results[0]["ok"], false);
    assert!(
        results[0]["summary"]
            .as_str()
            .unwrap()
            .contains("suspected_files"),
        "{}",
        results[0]
    );
    assert_eq!(results[1]["ok"], true);
    let md = last(&evs, "issue")["markdown"].as_str().unwrap().to_string();
    assert!(md.contains("blob/dev/crates/helmsman-plc/src/link.rs)"), "{md}");
    assert!(!md.contains("passwd"), "{md}");
    assert_eq!(last(&evs, "issue")["by"], "agent");
}

#[test]
fn an_agent_that_writes_no_issue_leaves_the_box_issue_and_says_why() {
    let relay = FakeRelay::start(vec![text("I am not sure what is wrong.")]);
    let b = Bench::new(&Agent::Relay(relay.url()), "connection refused");
    let (s, _, evs) = b.help(&[bearer(TOKEN)], &json!({ "question": QUESTION }));
    assert_eq!(s, 200, "{evs:?}");
    assert_eq!(names(&evs), ["help", "text", "issue", "done"], "{evs:?}");
    assert_eq!(last(&evs, "issue")["by"], "box");
    let d = last(&evs, "done");
    assert_eq!(d["stop"], "end_turn");
    assert!(
        d["why"].as_str().unwrap().contains("without writing an issue"),
        "{d}"
    );
}

#[test]
fn help_needs_a_credential_and_a_question_in_words() {
    let b = Bench::new(&Agent::None, "x");
    let (s, _, _) = b.help(&[], &json!({ "question": QUESTION }));
    assert_eq!(s, 401);
    for (body, status) in [
        (json!({ "question": "" }), 422),
        (json!({ "question": "   \n  " }), 422),
        (json!({ "question": "a\u{1b}[31mb" }), 422),
        (json!({ "question": "x".repeat(help::MAX_QUESTION + 1) }), 422),
        (json!({ "question": "ok", "extra": 1 }), 400),
        (json!({ "messages": "m" }), 400),
        (
            json!({ "question": "ok", "messages": "m".repeat(help::MAX_MESSAGES_IN + 1) }),
            413,
        ),
    ] {
        let (s, _, evs) = b.help(&[bearer(TOKEN)], &body);
        assert_eq!(s, status, "{body}: {evs:?}");
        assert!(evs[0].1["error"].as_str().is_some(), "{evs:?}");
    }
    // Each refusal audited; GET is not a way in.
    let refused = b
        .audit()
        .iter()
        .filter(|e| e["path"] == "/api/help" && e["status"] != 200)
        .count();
    assert_eq!(refused, 7);
    let r = send(b.addr, "GET", "/api/help", &[], "");
    assert!(matches!(r.status, 404 | 405), "{r:?}");
}

#[test]
fn the_messages_kept_are_the_newest() {
    let b = Bench::new(&Agent::None, "x");
    let lines: String = (0..1500)
        .map(|k| format!("{k:>8}.0s info    line {k}\n"))
        .collect();
    let (s, _, evs) = b.help(&[bearer(TOKEN)], &json!({ "question": "q", "messages": lines }));
    assert_eq!(s, 200, "{evs:?}");
    let m = evs[0].1["context"]["messages"].as_str().unwrap();
    assert!(m.len() <= help::KEEP_MESSAGES, "{}", m.len());
    assert!(m.ends_with("line 1499\n") && !m.contains("line 10\n"), "{m}");
}
