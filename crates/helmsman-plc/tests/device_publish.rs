//! Publishing a saved device profile to the library (card
//! `device-profile-publish`): `POST /api/device-profile/publish` on a real
//! box API with real accounts and a real projects folder, and a scripted
//! relay on a real socket that records what the box sends it and answers
//! as the relay's contract says (the relay's own tests, in its repository,
//! run the real tool against a scripted GitHub).
//!
//! The contract: only an engineer's session publishes; what is sent is
//! the project's saved file, read again by `Profile::parse`, never text
//! from the request; the box's relay token goes to the relay and nowhere
//! else, and no GitHub credential exists on the box; every relay refusal
//! comes back in words with a status the Devices tab can show, a link is
//! only ever a library pull request's page, nothing of the profile reaches
//! the audit log or the box's log, and no route of the agent reaches it.
#![allow(clippy::too_many_lines)] // each test walks one whole flow
#![allow(clippy::many_single_char_names)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use helmsman_agent::http::{Timeouts, Url};
use helmsman_agent::relay;
use helmsman_plc::SignalTable;
use helmsman_plc::Window;
use helmsman_plc::api::{
    Api, Approval, Catalog, Config, Devices, Extras, Limits, Live, MemAudit, MemProposals, More, Plant,
    Shared, chat, swap,
};
use helmsman_plc::policy::account::{Accounts, Logins};
use helmsman_plc::policy::pin::{Gate, Pins};
use helmsman_plc::policy::{FLOOR, Role};
use helmsman_plc::project::Projects;
use helmsman_plc::spec::Defaults;
use serde_json::{Value as Json, json};

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const RELAY_TOKEN: &str = "hmr1_00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
const ALICE: &str = "alice-engineer-pw";
const BOB: &str = "bob-operator-pw";

/// A real profile (hand-made from the E1213's manual and a capture).
const E1213: &str = include_str!("../../../docs/devices/moxa-iologik-e1213.device");
/// A word that appears only in a comment of the profile text, to look for
/// in the places the text must never reach.
const MARK: &str = "zz-vendor-text-marker-7";
const PR_URL: &str = "https://github.com/JimothyJohn/helmsman-devices/pull/12";

fn tmp(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "devpublish-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------- relay

/// What the relay answers to a publish.
#[derive(Clone)]
enum Reply {
    /// A status and a JSON body.
    Json(u16, Json),
    /// Read the request and say nothing at all.
    Mute,
    /// Bytes that are not JSON.
    Junk,
}

/// One request the relay got.
#[derive(Debug, Clone)]
struct Seen {
    head: String,
    body: Json,
}

/// A relay that answers health probes and records every other request,
/// answering each with the one scripted reply.
struct FakeRelay {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
    reply: Arc<Mutex<Reply>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FakeRelay {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let reply = Arc::new(Mutex::new(Reply::Json(500, json!({}))));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (seen, reply, stop) = (Arc::clone(&seen), Arc::clone(&reply), Arc::clone(&stop));
            thread::spawn(move || {
                let mut held = Vec::new();
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
                    let scripted = lock(&reply).clone();
                    match scripted {
                        Reply::Json(code, body) => {
                            let b = body.to_string();
                            let _ = write!(
                                s,
                                "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}",
                                b.len()
                            );
                        }
                        Reply::Junk => {
                            let _ = write!(
                                s,
                                "HTTP/1.1 200 OK\r\nContent-Length: 9\r\nConnection: close\r\n\r\nnot json!!"
                            );
                        }
                        Reply::Mute => held.push(s),
                    }
                }
            })
        };
        Self {
            addr,
            seen,
            reply,
            stop,
            thread: Some(thread),
        }
    }

    fn says(&self, r: Reply) {
        *lock(&self.reply) = r;
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

// ---------------------------------------------------------------- the box

struct Bench {
    api: Option<Api>,
    addr: SocketAddr,
    log: Arc<Mutex<Vec<String>>>,
}

impl Bench {
    fn new(chat: Option<chat::Setup>) -> Self {
        let parsed = helmsman_io::text::parse("in start bool box di:0\n").unwrap();
        let table = SignalTable::new(&parsed.map, &parsed.safe).unwrap();
        let state = tmp("state");
        let mut a = Accounts::default();
        a.add("alice", Role::Engineer, ALICE, 1, [1; 16]).unwrap();
        a.add("bob", Role::Operator, BOB, 1, [2; 16]).unwrap();
        a.save(&state).unwrap();
        let (handle, _swapper) = swap::channel(Duration::from_secs(5));
        let t0 = Instant::now();
        let now: helmsman_plc::policy::pin::Clock = Arc::new(move || t0);
        let policy = FLOOR;
        let approval = Approval {
            policy,
            gate: Gate::new(Pins::default(), policy.lockout, Arc::clone(&now)).with_logins(Logins::new(
                state.clone(),
                1,
                policy.lockout,
                now,
            )),
            plant: Plant {
                table,
                i_len: 32,
                q_len: 32,
                defaults: Defaults {
                    period: Duration::from_millis(2),
                    i: Window::new(0, 0),
                    q: Window::new(0, 0),
                    m: 16,
                },
                record: None,
            },
            swap: handle,
        };
        let log = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&log);
        let cfg = Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            token: TOKEN.into(),
            limits: Limits::default(),
            stream_every: Duration::from_millis(20),
            memory: 16,
            log: Arc::new(move |l: &str| sink.lock().unwrap_or_else(PoisonError::into_inner).push(l.into())),
        };
        let shared = Shared {
            catalog: Catalog::default(),
            live: Arc::new(Live::default()),
            devices: Arc::new(Devices::new(["box".to_string()])),
            audit: Box::new(MemAudit::new(100_000)),
            proposals: Box::new(MemProposals::new(1000)),
            running: None,
            snapshots: None,
            approval: Some(approval),
            chat,
        };
        let more = More {
            projects: Some(Projects::in_state_dir(&state).unwrap()),
            ..More::default()
        };
        let api = Api::start_box(cfg, shared, Extras::default(), None, more).unwrap();
        Self {
            addr: api.addr(),
            api: Some(api),

            log,
        }
    }

    fn call(&self, method: &str, path: &str, bearer: Option<&str>, ctype: &str, body: &[u8]) -> Resp {
        send(self.addr, method, path, bearer, ctype, body)
    }

    fn json(&self, method: &str, path: &str, bearer: Option<&str>, body: &Json) -> Resp {
        self.call(
            method,
            path,
            bearer,
            "application/json",
            body.to_string().as_bytes(),
        )
    }

    fn login(&self, user: &str, password: &str) -> String {
        let r = self.json(
            "POST",
            "/api/login",
            None,
            &json!({ "user": user, "password": password }),
        );
        assert_eq!(r.status, 200, "{r:?}");
        r.json()["token"].as_str().unwrap().to_string()
    }

    fn alice(&self) -> String {
        self.login("alice", ALICE)
    }

    fn bob(&self) -> String {
        self.login("bob", BOB)
    }

    fn read(&self, path: &str) -> Resp {
        self.call("GET", path, Some(TOKEN), "application/json", b"")
    }

    /// `propose_device_profile` as the agent's token sends it.
    fn propose(&self, name: &str, text: &str) -> Resp {
        self.json(
            "POST",
            "/api/device-profile/proposal",
            Some(TOKEN),
            &json!({ "name": name, "text": text, "summary": "from the EDS and the manual" }),
        )
    }

    fn project(&self, s: &str, name: &str) {
        let r = self.json("POST", "/api/projects", Some(s), &json!({ "name": name }));
        assert_eq!(r.status, 201, "{r:?}");
    }

    fn accept(&self, s: &str, body: &Json) -> Resp {
        self.json("POST", "/api/device-profile/proposal/accept", Some(s), body)
    }

    /// A profile as a project file, saved directly.
    fn save(&self, s: &str, project: &str, file: &str, text: &str) {
        let r = self.json(
            "PUT",
            &format!("/api/projects/{project}/files/{file}"),
            Some(s),
            &json!({ "text": text }),
        );
        assert_eq!(r.status, 200, "{r:?}");
    }

    fn publish(&self, bearer: Option<&str>, body: &Json) -> Resp {
        self.json("POST", "/api/device-profile/publish", bearer, body)
    }

    /// The audit log and the box's own log, to look for what must not be there.
    fn everything(&self) -> String {
        let mut all = String::from_utf8_lossy(&self.read("/api/audit?limit=5000").body).into_owned();
        for l in self.log.lock().unwrap_or_else(PoisonError::into_inner).iter() {
            all.push_str(l);
            all.push('\n');
        }
        all
    }
}

impl Drop for Bench {
    fn drop(&mut self) {
        self.api.take();
    }
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
}

fn send(addr: SocketAddr, method: &str, path: &str, bearer: Option<&str>, ctype: &str, body: &[u8]) -> Resp {
    let auth = bearer.map_or_else(String::new, |t| format!("Authorization: Bearer {t}\r\n"));
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: box\r\n{auth}Content-Type: {ctype}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
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

/// The chat route's setup with `relay` as the answer service.
fn setup(relay: &FakeRelay) -> chat::Setup {
    let mut rc = relay::Config::new(relay.url(), RELAY_TOKEN.into(), "v7".into());
    rc.timeouts = Timeouts {
        connect: Duration::from_secs(2),
        idle: Duration::from_secs(3),
        total: Duration::from_secs(5),
    };
    let mut s = chat::Setup::new(rc);
    s.probe_every = Duration::from_millis(50);
    s.publish_wait = (Duration::from_secs(2), Duration::from_secs(4));
    s
}

/// A box wired to a fake relay, one project `line3`, and the E1213's
/// profile saved in it (with [`MARK`] in a comment). Returns the box, the
/// relay and an engineer's session.
fn rig() -> (Bench, FakeRelay, String) {
    let relay = FakeRelay::start();
    let b = Bench::new(Some(setup(&relay)));
    let alice = b.alice();
    b.project(&alice, "line3");
    b.save(&alice, "line3", "moxa-iologik-e1213.device", &e1213());
    (b, relay, alice)
}

fn e1213() -> String {
    E1213.replacen("kind   remote-io\n", &format!("kind   remote-io\n# {MARK}\n"), 1)
}

fn made(created: bool) -> Reply {
    Reply::Json(
        if created { 201 } else { 200 },
        json!({"type": "pull_request", "created": created, "number": 12, "url": PR_URL, "draft": true,
               "branch": "profile/moxa-iologik-e1213-3f9a1c2b7d40",
               "folder": "devices/moxa/iologik-e1213", "file": "moxa-iologik-e1213.device"}),
    )
}

fn refusal(status: u16, kind: &str, message: &str, retryable: bool) -> Reply {
    Reply::Json(
        status,
        json!({"type": "error", "error": {"type": kind, "message": message, "retryable": retryable}, "request_id": "r"}),
    )
}

fn file() -> Json {
    json!({ "project": "line3", "file": "moxa-iologik-e1213.device" })
}

// ---- the whole path ----------------------------------------------------------

#[test]
fn a_saved_profile_is_sent_to_the_relay_and_the_pull_request_comes_back() {
    let (b, relay, alice) = rig();
    relay.says(made(true));
    let r = b.publish(Some(&alice), &file());
    assert_eq!(r.status, 201, "{r:?}");
    let j = r.json();
    assert_eq!(j["number"], 12);
    assert_eq!(j["url"], PR_URL);
    assert_eq!(
        (j["created"].clone(), j["draft"].clone()),
        (json!(true), json!(true))
    );
    assert_eq!(j["folder"], "devices/moxa/iologik-e1213");
    assert_eq!(j["file"], "moxa-iologik-e1213.device");

    // Exactly one request, to the publish agent's route, as the box with its
    // relay token, carrying the saved file's name and exact text.
    let seen = relay.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert!(
        seen[0]
            .head
            .starts_with("POST /v1/agents/helmsman/v13/tools/publish_device_profile HTTP/1.1\r\n"),
        "{}",
        seen[0].head
    );
    assert!(
        seen[0]
            .head
            .contains(&format!("Authorization: Bearer {RELAY_TOKEN}"))
    );
    assert_eq!(
        seen[0].body,
        json!({ "name": "moxa-iologik-e1213", "text": e1213() }),
        "the file as saved, and nothing else"
    );
    assert!(
        !seen[0].head.contains("ghp_") && !seen[0].head.to_lowercase().contains("github"),
        "the box holds no GitHub credential"
    );

    // The audit log names the person, the profile and the pull request, and
    // none of the text; neither it nor the box's log holds the relay token.
    let all = b.everything();
    assert!(
        all.contains("alice") && all.contains("moxa-iologik-e1213"),
        "{all}"
    );
    assert!(all.contains("pull request #12"), "{all}");
    assert!(
        !all.contains(MARK) && !all.contains("digital inputs"),
        "profile text in a log"
    );
    assert!(
        !all.contains(RELAY_TOKEN) && !all.contains(TOKEN),
        "a token in a log"
    );
    assert!(!String::from_utf8_lossy(&r.body).contains(RELAY_TOKEN));
}

#[test]
fn a_profile_saved_from_the_agents_proposal_publishes_like_any_saved_one() {
    let (b, relay, alice) = rig();
    let held = b.propose("moxa-copy", &e1213());
    assert_eq!(held.status, 201, "{held:?}");
    let id = held.json()["id"].as_u64().unwrap();
    let r = b.accept(
        &alice,
        &json!({ "id": id, "project": "line3", "channels": { "DI-00": "start_button" } }),
    );
    assert_eq!(r.status, 201, "{r:?}");
    relay.says(made(true));
    let r = b.publish(
        Some(&alice),
        &json!({ "project": "line3", "file": "moxa-copy.device" }),
    );
    assert_eq!(r.status, 201, "{r:?}");
    let sent = &relay.seen()[0].body;
    assert_eq!(sent["name"], "moxa-copy");
    assert!(
        sent["text"].as_str().unwrap().contains("start_button"),
        "the renamed channel"
    );
    assert!(!sent["text"].as_str().unwrap().contains("in  DI-00 "));
}

#[test]
fn a_profile_that_already_has_a_pull_request_open_says_so() {
    let (b, relay, alice) = rig();
    relay.says(made(false));
    let r = b.publish(Some(&alice), &file());
    assert_eq!(r.status, 200, "{r:?}");
    let j = r.json();
    assert_eq!(
        (j["created"].clone(), j["number"].clone()),
        (json!(false), json!(12))
    );
    assert!(b.everything().contains("already open"));
}

// ---- who may ------------------------------------------------------------------

#[test]
fn only_an_engineers_session_publishes_and_nothing_is_sent_for_anyone_else() {
    let (b, relay, alice) = rig();
    relay.says(made(true));
    let bob = b.bob();
    for (who, bearer, want) in [
        ("nobody", None, 401),
        ("the box token", Some(TOKEN), 403),
        ("an operator", Some(bob.as_str()), 403),
    ] {
        let r = b.publish(bearer, &file());
        assert_eq!(r.status, want, "{who}: {r:?}");
    }
    assert!(
        relay.seen().is_empty(),
        "nothing reached the relay: {:?}",
        relay.seen()
    );
    // The agent's own tools cannot reach the route.
    assert!(!helmsman_plc::api::mcp::reachable(
        "POST",
        "/api/device-profile/publish"
    ));
    assert!(!helmsman_agent::tools::NAMES.iter().any(|n| n.contains("publish")));
    assert_eq!(
        b.publish(Some(&alice), &file()).status,
        201,
        "and an engineer still can"
    );
}

#[test]
fn without_an_agent_service_or_projects_there_is_nothing_to_publish_through() {
    let b = Bench::new(None);
    let alice = b.alice();
    b.project(&alice, "line3");
    let r = b.publish(Some(&alice), &file());
    assert_eq!(r.status, 404, "{r:?}");
    assert!(r.error().contains("answer service"), "{}", r.error());
}

// ---- hostile requests: refused before the relay hears of them -----------------------

#[test]
fn a_request_that_is_not_a_saved_profile_never_reaches_the_relay() {
    let (b, relay, alice) = rig();
    relay.says(made(true));
    b.save(
        &alice,
        "line3",
        "broken.device",
        "helmsman-device 1\nvendor X\nmodel Y\nin A bool nowhere=di:0\n",
    );
    let cases: Vec<(Json, u16, &str)> = vec![
        (
            json!({ "project": "line3", "file": "../moxa-iologik-e1213.device" }),
            422,
            "NAME.device",
        ),
        (
            json!({ "project": "line3", "file": "moxa-iologik-e1213.device/../../x" }),
            422,
            "NAME.device",
        ),
        (
            json!({ "project": "line3", "file": "Moxa.device" }),
            422,
            "NAME.device",
        ),
        (
            json!({ "project": "line3", "file": ".device" }),
            422,
            "NAME.device",
        ),
        (
            json!({ "project": "line3", "file": "notes.txt" }),
            422,
            "NAME.device",
        ),
        (
            json!({ "project": "line3", "file": "missing.device" }),
            404,
            "no such file",
        ),
        (json!({ "project": "nope", "file": "missing.device" }), 404, ""),
        (
            json!({ "project": "../line3", "file": "moxa-iologik-e1213.device" }),
            404,
            "",
        ),
        (
            json!({ "project": "line3", "file": "broken.device" }),
            422,
            "does not check",
        ),
        (json!({ "project": "line3" }), 400, "expected"),
        (
            json!({ "project": "line3", "file": "moxa-iologik-e1213.device", "text": "helmsman-device 1\n" }),
            400,
            "expected",
        ),
        (
            json!({ "project": "line3", "file": "moxa-iologik-e1213.device", "name": "x" }),
            400,
            "expected",
        ),
        (json!({ "project": 1, "file": 2 }), 400, "expected"),
        (json!(["line3"]), 400, "expected"),
    ];
    for (body, status, word) in cases {
        let r = b.publish(Some(&alice), &body);
        assert_eq!(r.status, status, "{body}: {r:?}");
        assert!(r.error().contains(word), "{body}: {}", r.error());
    }
    let r = b.call(
        "POST",
        "/api/device-profile/publish",
        Some(&alice),
        "text/plain",
        b"x",
    );
    assert_eq!(r.status, 415);
    let r = b.call(
        "POST",
        "/api/device-profile/publish",
        Some(&alice),
        "application/json",
        b"{not json",
    );
    assert_eq!(r.status, 400);
    assert!(
        relay.seen().is_empty(),
        "the relay heard of none of them: {:?}",
        relay.seen()
    );
    // And a profile that does not check lists the line that is wrong.
    let r = b.publish(
        Some(&alice),
        &json!({ "project": "line3", "file": "broken.device" }),
    );
    assert!(!r.json()["problems"].as_array().unwrap().is_empty(), "{r:?}");
    // The route takes POST only.
    let r = b.call(
        "GET",
        "/api/device-profile/publish",
        Some(TOKEN),
        "application/json",
        b"",
    );
    assert_ne!(r.status, 200);
    let r = b.call(
        "DELETE",
        "/api/device-profile/publish",
        Some(&alice),
        "application/json",
        b"",
    );
    assert_eq!(r.status, 405);
}

// ---- what the relay says --------------------------------------------------------------

#[test]
fn each_way_the_relay_refuses_comes_back_in_its_words_with_a_status_to_show() {
    let (b, relay, alice) = rig();
    let cases = [
        (
            refusal(
                400,
                "invalid_request",
                "the device library has no entry for Moxa X: ask for it with a New device issue",
                false,
            ),
            422,
            "no entry for Moxa X",
            false,
            "invalid_request",
        ),
        (
            refusal(
                409,
                "conflict",
                "the library already has a profile for this device",
                false,
            ),
            409,
            "already has a profile",
            false,
            "conflict",
        ),
        (
            refusal(
                429,
                "rate_limited",
                "at most 6 publishes an hour from one box",
                true,
            ),
            429,
            "6 publishes",
            true,
            "rate_limited",
        ),
        (
            refusal(
                503,
                "not_set_up",
                "publishing is not set up on this relay: it has no GitHub token",
                false,
            ),
            503,
            "not set up",
            false,
            "not_set_up",
        ),
        (
            refusal(
                502,
                "publish_failed",
                "GitHub is not answering; publish again later",
                true,
            ),
            502,
            "not answering",
            true,
            "publish_failed",
        ),
        (
            refusal(401, "unauthorized", "missing or invalid box token", false),
            502,
            "does not offer publishing",
            false,
            "unauthorized",
        ),
        (
            refusal(403, "forbidden", "x", false),
            502,
            "does not offer publishing",
            false,
            "forbidden",
        ),
        (
            refusal(404, "not_found", "not found", false),
            502,
            "does not offer publishing",
            false,
            "not_found",
        ),
        (
            refusal(500, "relay_error", "relay error; quote the request id", true),
            502,
            "could not publish",
            true,
            "relay_error",
        ),
    ];
    for (reply, status, word, retryable, kind) in cases {
        relay.says(reply);
        let r = b.publish(Some(&alice), &file());
        assert_eq!(r.status, status, "{kind}: {r:?}");
        let j = r.json();
        assert!(j["error"].as_str().unwrap().contains(word), "{kind}: {j}");
        assert_eq!(
            (j["kind"].as_str().unwrap(), j["retryable"].clone()),
            (kind, json!(retryable)),
            "{j}"
        );
    }
    assert!(b.everything().contains("not published: not_set_up"));
}

#[test]
fn an_answer_that_is_not_the_protocol_is_refused_and_a_link_is_only_a_library_page() {
    let (b, relay, alice) = rig();
    let ok = |url: &str, number: u64| {
        Reply::Json(
            201,
            json!({"type": "pull_request", "created": true, "number": number, "url": url, "draft": true, "folder": "devices/x/y", "file": "f.device"}),
        )
    };
    let bad = [
        Reply::Junk,
        Reply::Json(200, json!({})),
        Reply::Json(200, json!([1])),
        ok("https://evil.example/o/r/pull/12", 12),
        ok("javascript:alert(1)", 12),
        ok("https://github.com/o/r/pull/13", 12),
        ok("https://github.com.evil.example/o/r/pull/12", 12),
        ok(PR_URL, 0),
        Reply::Json(201, json!({"number": "12", "url": PR_URL})),
    ];
    for (n, reply) in bad.into_iter().enumerate() {
        relay.says(reply);
        let r = b.publish(Some(&alice), &file());
        assert_eq!(r.status, 502, "{n}: {r:?}");
        assert!(r.error().contains("not understood"), "{n}: {}", r.error());
        assert!(r.json().get("url").is_none(), "{n}");
    }
    // Words from the relay are printable and bounded.
    relay.says(refusal(
        409,
        "conflict",
        &format!("a\u{202e}b{}", "x".repeat(2000)),
        false,
    ));
    let r = b.publish(Some(&alice), &file());
    assert_eq!(r.status, 409);
    assert!(
        r.error().len() <= 410 && !r.error().contains('\u{202e}'),
        "{}",
        r.error()
    );
}

#[test]
fn a_relay_that_does_not_answer_or_is_gone_is_a_502_that_sends_nothing_further() {
    let (b, relay, alice) = rig();
    relay.says(Reply::Mute);
    let t = Instant::now();
    let r = b.publish(Some(&alice), &file());
    assert_eq!(r.status, 502, "{r:?}");
    assert!(
        r.error().contains("could not be reached") || r.error().contains("not understood"),
        "{}",
        r.error()
    );
    assert!(
        t.elapsed() < Duration::from_secs(30),
        "bounded by the box's timeouts"
    );
    drop(relay);
    let r = b.publish(Some(&alice), &file());
    assert_eq!(r.status, 502, "{r:?}");
    assert!(r.error().contains("could not be reached"), "{}", r.error());
    assert_eq!(r.json()["retryable"], true);
    assert!(!b.everything().contains(MARK));
}

#[test]
fn the_publish_agent_can_be_named() {
    let relay = FakeRelay::start();
    let mut s = setup(&relay);
    s.publish_agent = "v13".into();
    let b = Bench::new(Some(s));
    let alice = b.alice();
    b.project(&alice, "line3");
    b.save(&alice, "line3", "moxa-iologik-e1213.device", &e1213());
    relay.says(made(true));
    assert_eq!(b.publish(Some(&alice), &file()).status, 201);
    assert!(
        relay.seen()[0]
            .head
            .starts_with("POST /v1/agents/v13/tools/publish_device_profile ")
    );
}
