//! A device profile the agent proposes (card `device-profile-propose-box`):
//! the tool `propose_device_profile` over the chat agent's path
//! (`BoxApi`) and MCP, the pending proposal, the user's rename, and the
//! save of `NAME.device` into a project, against a real box API with real
//! accounts and a real projects folder. The profiles are the repo's own:
//! docs/devices/moxa-iologik-e1213.device, and the E1213's EDS facts as
//! the vendor's file the agent reads.
//!
//! The contract: a proposal is held, never a file; only an engineer's
//! session accepts, renames and saves, and what is saved is the whole text
//! read again by `Profile::parse`; anything that does not read, collides,
//! or is not a plain file name leaves the project and the proposal as they
//! were; nothing of the vendor's text reaches the audit log or the box's
//! log. The chat agent's turn, through a scripted relay, is in
//! tests/dropins.rs.
#![allow(clippy::too_many_lines)] // each test walks one whole flow
#![allow(clippy::many_single_char_names)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use helmsman_agent::tools::{self, BoxApi, Drafts};
use helmsman_io::profile::Profile;
use helmsman_plc::SignalTable;
use helmsman_plc::Window;
use helmsman_plc::api::{
    Api, Approval, Catalog, Config, Devices, Extras, Limits, Live, MemAudit, MemProposals, More, Plant,
    Shared, swap,
};
use helmsman_plc::policy::account::{Accounts, Logins};
use helmsman_plc::policy::pin::{Gate, Pins};
use helmsman_plc::policy::{FLOOR, Role};
use helmsman_plc::project::Projects;
use helmsman_plc::spec::Defaults;
use serde_json::{Value as Json, json};

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const ALICE: &str = "alice-engineer-pw";
const BOB: &str = "bob-operator-pw";

/// A real profile (hand-made from the E1213's manual and a capture).
const E1213: &str = include_str!("../../../docs/devices/moxa-iologik-e1213.device");
/// The vendor's file the agent reads: the E1213's EDS facts.
const EDS: &str = include_str!("../../helmsman-proto/tests/fixtures/e1213-facts.eds");
/// A word that appears only in a comment of the profile text, to look for
/// in the places the text must never reach.
const MARK: &str = "zz-vendor-text-marker-7";

fn tmp(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "devprofile-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct Bench {
    api: Option<Api>,
    addr: SocketAddr,
    state: PathBuf,
    log: Arc<Mutex<Vec<String>>>,
}

impl Bench {
    fn new() -> Self {
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
            chat: None,
        };
        let more = More {
            projects: Some(Projects::in_state_dir(&state).unwrap()),
            ..More::default()
        };
        let api = Api::start_box(cfg, shared, Extras::default(), None, more).unwrap();
        Self {
            addr: api.addr(),
            api: Some(api),
            state,
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

    fn pending(&self) -> Resp {
        self.read("/api/device-profile/proposal")
    }

    fn project(&self, s: &str, name: &str) {
        let r = self.json("POST", "/api/projects", Some(s), &json!({ "name": name }));
        assert_eq!(r.status, 201, "{r:?}");
    }

    fn accept(&self, s: &str, body: &Json) -> Resp {
        self.json("POST", "/api/device-profile/proposal/accept", Some(s), body)
    }

    /// Every file under the projects folder with its bytes: what is on disk.
    fn disk(&self) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        walk(&self.state.join("projects"), &mut out);
        out
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

    /// The tool as the chat agent runs it, over loopback with the box token.
    fn tool(&self, name: &str, input: &Json) -> tools::Outcome {
        let api = BoxApi::local(self.addr, TOKEN.into());
        tools::run(&api, &mut Drafts::default(), name, input)
    }
}

impl Drop for Bench {
    fn drop(&mut self) {
        self.api.take();
    }
}

fn walk(dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.insert(
                p.to_string_lossy().into_owned(),
                std::fs::read(&p).unwrap_or_default(),
            );
        }
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

/// The E1213's profile with a comment line carrying [`MARK`].
fn e1213() -> String {
    E1213.replacen("kind   remote-io\n", &format!("kind   remote-io\n# {MARK}\n"), 1)
}

fn names(channels: &Json) -> Vec<String> {
    channels
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect()
}

/// A profile of `n` bools on Modbus TCP.
fn many(n: usize) -> String {
    let mut t = String::from(
        "helmsman-device 1\nvendor Acme\nmodel Many\nkind remote-io\ntransport modbus-tcp unit=1\n",
    );
    for i in 0..n {
        let _ = writeln!(t, "in CH-{i:04} bool modbus-tcp=di:{i}");
    }
    t
}

// ---- the whole path ----------------------------------------------------------

#[test]
fn a_proposal_is_held_renamed_and_saved_into_a_project_by_an_engineer() {
    let b = Bench::new();
    let alice = b.alice();
    b.project(&alice, "line3");
    let before = b.disk();

    // The agent's tool, over the chat path: the channel list comes back.
    let o = b.tool(
        "propose_device_profile",
        &json!({ "name": "moxa-copy", "text": e1213(), "summary": "from the manual, p. 12" }),
    );
    assert!(o.ok, "{} {}", o.summary, o.data);
    let want = Profile::parse(&e1213()).unwrap();
    assert_eq!(o.data["channels"].as_array().unwrap().len(), want.channels.len());
    assert_eq!(o.data["channels"][0]["name"], "DI-00");
    assert_eq!(o.data["channels"][0]["points"]["modbus-tcp"], "di:0");
    assert_eq!(o.data["transports"][0]["driver"], "enip-io");
    assert_eq!(o.data["held"], true);
    assert_eq!(o.proposals, Vec::<u64>::new());
    assert_eq!(b.disk(), before, "a proposal is not a file");

    // What the user reads: the proposal, its text and the agent's summary.
    let p = b.pending();
    assert_eq!(p.status, 200, "{p:?}");
    let p = p.json();
    assert_eq!(p["name"], "moxa-copy");
    assert_eq!(p["text"], e1213());
    assert_eq!(p["summary"], "from the manual, p. 12");
    assert_eq!(p["vendor"], "Moxa");
    assert_eq!(names(&p["channels"]).len(), want.channels.len());
    let id = p["id"].as_u64().unwrap();

    // The user renames two channels and saves.
    let r = b.accept(
        &alice,
        &json!({ "id": id, "project": "line3", "channels": { "DI-00": "start_button", "DO-00": "lamp" } }),
    );
    assert_eq!(r.status, 201, "{r:?}");
    assert_eq!(r.json()["file"], "moxa-copy.device");
    let saved = b.read("/api/projects/line3/files/moxa-copy.device").json();
    let text = saved["text"].as_str().unwrap();
    let got = Profile::parse(text).expect("the saved file reads");
    let got_names: Vec<&str> = got.channels.iter().map(|c| c.name.as_str()).collect();
    assert!(
        got_names.contains(&"start_button") && got_names.contains(&"lamp"),
        "{got_names:?}"
    );
    assert!(!got_names.contains(&"DI-00") && !got_names.contains(&"DO-00"));
    assert_eq!(got.channels.len(), want.channels.len());
    assert!(
        text.contains("# digital inputs"),
        "the rest of the file is as written"
    );
    assert!(text.contains(MARK));
    let start = got.channels.iter().find(|c| c.name == "start_button").unwrap();
    assert_eq!(
        start.points["modbus-tcp"], "di:0",
        "the point is the one DI-00 had"
    );
    let history = b.read("/api/projects/line3/history").json();
    assert!(
        history["revisions"].to_string().contains("moxa-copy.device"),
        "the project's history records it: {history}"
    );

    // Taken once: the proposal is gone and a second accept finds nothing.
    assert_eq!(b.pending().status, 404);
    let again = b.accept(&alice, &json!({ "id": id, "project": "line3" }));
    assert_eq!(again.status, 404, "{again:?}");

    // The audit log names the person and the file, and none of the text.
    let all = b.everything();
    assert!(all.contains("moxa-copy"), "{all}");
    assert!(all.contains("alice"), "{all}");
    assert!(
        !all.contains(MARK),
        "the vendor's text reached the audit log or the box's log"
    );
    assert!(!all.contains("digital inputs"));
}

#[test]
fn the_same_proposal_over_mcp_is_held_and_audited_as_mcp() {
    let b = Bench::new();
    let body = json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {
        "name": "propose_device_profile",
        "arguments": { "name": "via-mcp", "text": e1213(), "summary": "from the EDS" },
        "_meta": {
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientInfo": { "name": "test", "version": "1" },
            "io.modelcontextprotocol/clientCapabilities": {},
        },
    }});
    let head = format!(
        "POST /mcp HTTP/1.1\r\nHost: box\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMCP-Protocol-Version: 2026-07-28\r\nMcp-Method: tools/call\r\nMcp-Name: propose_device_profile\r\nContent-Length: {}\r\n\r\n",
        body.to_string().len()
    );
    let mut s = TcpStream::connect(b.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    s.write_all(head.as_bytes()).unwrap();
    s.write_all(body.to_string().as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let split = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let reply: Json = serde_json::from_slice(&out[split + 4..]).unwrap();
    assert_eq!(reply["result"]["isError"], false, "{reply}");
    let text = reply["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("<box-data tool=\"propose_device_profile\">"),
        "{text}"
    );
    assert!(text.contains("DI-00"), "{text}");
    assert_eq!(b.pending().json()["name"], "via-mcp");
    let audit = b.read("/api/audit?limit=100").json().to_string();
    assert!(
        audit.contains("\"source\":\"mcp\"") && audit.contains("via-mcp"),
        "{audit}"
    );
}

// ---- who may do what ---------------------------------------------------------

#[test]
fn every_route_wants_a_credential_and_saving_wants_an_engineer() {
    let b = Bench::new();
    let alice = b.alice();
    let bob = b.bob();
    b.project(&alice, "line3");
    assert_eq!(b.propose("moxa-copy", &e1213()).status, 201);
    let id = b.pending().json()["id"].as_u64().unwrap();
    let before = b.disk();
    let accept = json!({ "id": id, "project": "line3" });

    // No credential, a wrong token: 401 on every route, whatever the verb.
    for bearer in [None, Some("not-the-token")] {
        for (m, p, body) in [
            ("GET", "/api/device-profile/proposal", json!({})),
            (
                "POST",
                "/api/device-profile/proposal",
                json!({ "name": "a", "text": e1213(), "summary": "s" }),
            ),
            ("POST", "/api/device-profile/proposal/accept", accept.clone()),
            ("DELETE", "/api/device-profile/proposal", json!({})),
        ] {
            let r = b.json(m, p, bearer, &body);
            assert_eq!(r.status, 401, "{m} {p} {bearer:?}: {r:?}");
        }
    }
    // The cookie alone, with no same-origin proof, saves nothing either.
    let r = send(
        b.addr,
        "POST",
        "/api/device-profile/proposal/accept",
        None,
        "application/json",
        accept.to_string().as_bytes(),
    );
    assert_eq!(r.status, 401);

    // The box token is the agent's: it proposes and reads, never accepts or drops.
    for (m, p, body) in [
        ("POST", "/api/device-profile/proposal/accept", accept.clone()),
        ("DELETE", "/api/device-profile/proposal", json!({})),
    ] {
        let r = b.json(m, p, Some(TOKEN), &body);
        assert_eq!(r.status, 403, "{m} {p}: {r:?}");
        assert!(r.error().contains("engineer"), "{}", r.error());
    }
    // An operator's session reads and may propose, but cannot accept or drop.
    assert_eq!(
        b.json("GET", "/api/device-profile/proposal", Some(&bob), &json!({}))
            .status,
        200
    );
    for (m, p, body) in [
        ("POST", "/api/device-profile/proposal/accept", accept.clone()),
        ("DELETE", "/api/device-profile/proposal", json!({})),
    ] {
        let r = b.json(m, p, Some(&bob), &body);
        assert_eq!(r.status, 403, "{m} {p}: {r:?}");
    }
    assert_eq!(b.disk(), before, "nothing was written by a refused call");
    assert_eq!(b.pending().json()["id"], id, "the proposal is still there");

    // Unknown paths and verbs under the route are refused, not routed.
    let r = b.json("PUT", "/api/device-profile/proposal", Some(&alice), &json!({}));
    assert_eq!(r.status, 405, "{r:?}");
    let r = b.json("POST", "/api/device-profile/other", Some(&alice), &json!({}));
    assert_eq!(r.status, 404, "{r:?}");
    let r = b.json(
        "POST",
        "/api/device-profile/proposal/accept/extra",
        Some(&alice),
        &accept,
    );
    assert_eq!(r.status, 404, "{r:?}");

    // Every refusal above is in the audit log.
    let audit = b.read("/api/audit?limit=5000").json().to_string();
    assert!(
        audit.contains("device profile accept: refused, not an engineer"),
        "{audit}"
    );
}

// ---- hostile names -----------------------------------------------------------

const BAD_NAMES: [&str; 18] = [
    "../x",
    "..",
    ".",
    "/etc/passwd",
    "a/b",
    "a\\b",
    "a\0b",
    "a\nb",
    "",
    " ",
    "a b",
    "UPPER",
    "Io.Map",
    "a.device",
    "caf\u{e9}",
    "-leading-hyphen",
    "_leading-underscore",
    // 49 characters: one over the name limit.
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
];

#[test]
fn a_name_that_is_not_a_plain_file_stem_is_refused_and_nothing_is_held() {
    let b = Bench::new();
    let alice = b.alice();
    b.project(&alice, "line3");
    let before = b.disk();
    for name in BAD_NAMES {
        let r = b.propose(name, &e1213());
        assert_eq!(r.status, 422, "{name:?}: {r:?}");
        assert_eq!(b.pending().status, 404, "{name:?} was held");
    }
    // The longest allowed name (48) is fine.
    assert_eq!(b.propose(&"a".repeat(48), &e1213()).status, 201);
    assert_eq!(b.disk(), before);
    // Over the agent's path too: the same answer, with nothing to fix by retrying.
    let o = b.tool(
        "propose_device_profile",
        &json!({ "name": "../../etc/x", "text": e1213(), "summary": "s" }),
    );
    assert!(!o.ok);
    assert_eq!(o.data["held"], false);
}

#[test]
fn a_bad_name_at_accept_writes_nothing_and_leaves_the_proposal() {
    let b = Bench::new();
    let alice = b.alice();
    b.project(&alice, "line3");
    assert_eq!(b.propose("moxa-copy", &e1213()).status, 201);
    let id = b.pending().json()["id"].as_u64().unwrap();
    let before = b.disk();
    for name in BAD_NAMES {
        let r = b.accept(&alice, &json!({ "id": id, "project": "line3", "name": name }));
        assert!(matches!(r.status, 400 | 422), "{name:?}: {r:?}");
    }
    // A project name that is a path.
    for project in ["../line3", "/tmp", "line3/..", "a\0b", ""] {
        let r = b.accept(&alice, &json!({ "id": id, "project": project }));
        assert!((400..500).contains(&r.status), "{project:?}: {r:?}");
    }
    // A project that does not exist is not made by saving into it.
    let r = b.accept(&alice, &json!({ "id": id, "project": "nowhere" }));
    assert_eq!(r.status, 404, "{r:?}");
    assert_eq!(b.disk(), before);
    assert_eq!(b.pending().json()["id"], id);
    // And the proposal still saves once the request is right.
    let r = b.accept(
        &alice,
        &json!({ "id": id, "project": "line3", "name": "chosen-name" }),
    );
    assert_eq!(r.status, 201, "{r:?}");
    assert_eq!(r.json()["file"], "chosen-name.device");
}

#[test]
fn a_name_that_collides_with_a_file_in_the_project_replaces_nothing() {
    let b = Bench::new();
    let alice = b.alice();
    b.project(&alice, "line3");
    let hand = "helmsman-device 1\nmodel Hand\ntransport modbus-tcp unit=1\nin A bool modbus-tcp=di:0\n";
    let r = b.json(
        "PUT",
        "/api/projects/line3/files/moxa-copy.device",
        Some(&alice),
        &json!({ "text": hand }),
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(b.propose("moxa-copy", &e1213()).status, 201);
    let id = b.pending().json()["id"].as_u64().unwrap();
    let before = b.disk();
    let r = b.accept(&alice, &json!({ "id": id, "project": "line3" }));
    assert_eq!(r.status, 409, "{r:?}");
    assert_eq!(b.disk(), before, "the file that was there is untouched");
    assert_eq!(
        b.pending().status,
        200,
        "the proposal is kept, to save under another name"
    );
    let r = b.accept(
        &alice,
        &json!({ "id": id, "project": "line3", "name": "moxa-copy-2" }),
    );
    assert_eq!(r.status, 201, "{r:?}");
}

// ---- hostile renames ---------------------------------------------------------

#[test]
fn a_rename_that_makes_an_invalid_profile_is_refused_and_leaves_nothing_on_disk() {
    let b = Bench::new();
    let alice = b.alice();
    b.project(&alice, "line3");
    assert_eq!(b.propose("moxa-copy", &e1213()).status, 201);
    let id = b.pending().json()["id"].as_u64().unwrap();
    let before = b.disk();
    let long = "x".repeat(33);
    let cases: Vec<(&str, Json)> = vec![
        ("to a name that starts with a digit", json!({ "DI-00": "1abc" })),
        ("to a name with a space", json!({ "DI-00": "a b" })),
        ("to a path", json!({ "DI-00": "../x" })),
        ("to an absolute path", json!({ "DI-00": "/etc/passwd" })),
        ("to nothing", json!({ "DI-00": "" })),
        ("to a name too long", json!({ "DI-00": long })),
        ("to non-ASCII", json!({ "DI-00": "caf\u{e9}" })),
        (
            "to a name with a line break",
            json!({ "DI-00": "a\nin X bool enip-io=di:0" }),
        ),
        ("to a comment", json!({ "DI-00": "a#b" })),
        ("to another channel's name", json!({ "DI-00": "DI-01" })),
        ("to another's name in another case", json!({ "DI-00": "di-01" })),
        ("to a name that is another's signal", json!({ "DI-00": "DI_01" })),
        (
            "two channels to one name",
            json!({ "DI-00": "same", "DI-01": "same" }),
        ),
        (
            "two channels to one name in two cases",
            json!({ "DI-00": "same", "DI-01": "SAME" }),
        ),
        ("a channel that is not there", json!({ "NOPE": "fine" })),
        ("a channel in another case", json!({ "di-00": "fine" })),
    ];
    for (what, renames) in cases {
        let r = b.accept(
            &alice,
            &json!({ "id": id, "project": "line3", "channels": renames }),
        );
        assert_eq!(r.status, 422, "{what}: {r:?}");
        assert!(
            r.json()["problems"].as_array().is_some_and(|p| !p.is_empty()),
            "{what}: {r:?}"
        );
        assert_eq!(b.disk(), before, "{what}: something was written");
        assert_eq!(b.pending().json()["id"], id, "{what}: the proposal changed");
    }
    // A rename that is not a map of strings is a malformed request.
    for channels in [
        json!(["DI-00"]),
        json!({ "DI-00": 7 }),
        json!("DI-00"),
        json!({ "DI-00": null }),
    ] {
        let r = b.accept(
            &alice,
            &json!({ "id": id, "project": "line3", "channels": channels }),
        );
        assert_eq!(r.status, 400, "{channels}: {r:?}");
    }
    for extra in [json!({ "text": "helmsman-device 1\n" }), json!({ "force": true })] {
        let mut body = json!({ "id": id, "project": "line3" });
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let r = b.accept(&alice, &body);
        assert_eq!(r.status, 400, "{body}: {r:?}");
    }
    assert_eq!(b.disk(), before);

    // Swapping two names is a rename that reads.
    let r = b.accept(
        &alice,
        &json!({ "id": id, "project": "line3", "channels": { "DI-00": "DI-01", "DI-01": "DI-00" } }),
    );
    assert_eq!(r.status, 201, "{r:?}");
    let saved = b.read("/api/projects/line3/files/moxa-copy.device").json();
    let got = Profile::parse(saved["text"].as_str().unwrap()).unwrap();
    assert_eq!(got.point(0, "DI-00"), Some("di:1"));
    assert_eq!(got.point(0, "DI-01"), Some("di:0"));
}

// ---- hostile proposals --------------------------------------------------------

#[test]
fn a_text_that_does_not_read_is_refused_with_its_lines_and_nothing_is_held() {
    let b = Bench::new();
    let good = e1213();
    // The line of `DI-05`'s definition, 1-based.
    let at = good.lines().position(|l| l.starts_with("in  DI-05 ")).unwrap() + 1;
    let word = good.replace("in  DI-05        bool", "in  DI-05        word");
    let r = b.propose("moxa-copy", &word);
    assert_eq!(r.status, 422, "{r:?}");
    let problems = r.json()["problems"].clone();
    assert_eq!(problems[0]["line"], at, "{problems}");
    assert!(
        problems[0]["error"]
            .as_str()
            .unwrap()
            .contains("bool, int or real"),
        "{problems}"
    );
    // The same answer reaches the model, line number and all.
    let o = b.tool(
        "propose_device_profile",
        &json!({ "name": "moxa-copy", "text": word, "summary": "s" }),
    );
    assert!(!o.ok);
    assert_eq!(o.data["problems"][0]["line"], at, "{}", o.data);
    assert_eq!(o.data["held"], false);
    assert!(
        o.data["next"]
            .as_str()
            .unwrap()
            .contains("propose the whole file again")
    );

    let dup = format!("{good}in  DI-00        bool enip-io=di:0 modbus-tcp=di:0\n");
    let dup_line = dup.lines().count();
    let lower = format!("{good}in  di-00        bool enip-io=di:0 modbus-tcp=di:0\n");
    // Two channel names that become one signal (`dev_di_00`).
    let signal = format!("{good}in  DI_00        bool enip-io=di:0 modbus-tcp=di:0\n");
    let no_point = good.replace(
        "in  DI-01        bool enip-io=di:1 modbus-tcp=di:1",
        "in  DI-01        bool enip-io=di:1",
    );
    let not_a_transport = good.replace("modbus-tcp=di:2\n", "modbus-tcp=di:2 profinet-io=di:2\n");
    let long_name = good.replace("DI-03 ", &format!("{} ", "n".repeat(33)));
    let two_writers = format!("{good}out DO-00-again bool safe=false enip-io=do:0 modbus-tcp=coil:0\n");
    let hostile: Vec<(&str, String, Option<usize>)> = vec![
        ("an EDS, not a profile", EDS.to_string(), Some(1)),
        ("nothing", String::new(), Some(1)),
        ("a header only", "helmsman-device 1\n".into(), None),
        ("a duplicate channel", dup, Some(dup_line)),
        ("a duplicate in another case", lower, None),
        ("two names for one signal", signal, None),
        ("a channel with no point on a transport", no_point, None),
        ("a point on a transport the file lacks", not_a_transport, None),
        ("a channel name over 32", long_name, None),
        ("two outputs on one point", two_writers, None),
        (
            "a bidirectional override in a comment",
            good.replace("kind   remote-io", "kind   remote-io # \u{202E}evil"),
            None,
        ),
        (
            "a NUL in a comment",
            good.replace("kind   remote-io", "kind   remote-io # a\0b"),
            None,
        ),
        (
            "an escape in a comment",
            good.replace("kind   remote-io", "kind   remote-io # \u{1b}[2J"),
            None,
        ),
        (
            "a text over the size limit",
            format!("{good}{}", "# padding\n".repeat(7000)),
            None,
        ),
        (
            "more lines than a profile has",
            format!("{good}{}", "#\n".repeat(1100)),
            None,
        ),
        ("257 channels", many(257), None),
    ];
    for (what, text, line) in hostile {
        let r = b.propose("hostile", &text);
        assert_eq!(r.status, 422, "{what}: {r:?}");
        if let Some(line) = line {
            assert_eq!(r.json()["problems"][0]["line"], line, "{what}: {r:?}");
        }
        assert_eq!(b.pending().status, 404, "{what}: it was held");
    }
    // A profile of exactly 256 channels is the biggest allowed, and reads.
    let r = b.propose("many", &many(256));
    assert_eq!(r.status, 201, "{r:?}");
    assert_eq!(r.json()["channels"].as_array().unwrap().len(), 256);
}

#[test]
fn a_malformed_proposal_request_is_refused_before_it_is_read() {
    let b = Bench::new();
    let body = |v: &Json| v.to_string().into_bytes();
    let ok = json!({ "name": "moxa-copy", "text": e1213(), "summary": "s" });
    let post =
        |ctype: &str, bytes: &[u8]| b.call("POST", "/api/device-profile/proposal", Some(TOKEN), ctype, bytes);
    assert_eq!(post("text/plain", &body(&ok)).status, 415);
    assert_eq!(post("application/json", b"{").status, 400);
    assert_eq!(post("application/json", b"[]").status, 400);
    for bad in [
        json!({ "name": "a", "text": e1213() }),
        json!({ "name": "a", "summary": "s" }),
        json!({ "text": e1213(), "summary": "s" }),
        json!({ "name": 7, "text": e1213(), "summary": "s" }),
        json!({ "name": "a", "text": e1213(), "summary": "s", "publish": true }),
    ] {
        assert_eq!(post("application/json", &body(&bad)).status, 400, "{bad}");
    }
    // A summary of nothing is no summary.
    let r = post(
        "application/json",
        &body(&json!({ "name": "a", "text": e1213(), "summary": " \n\t " })),
    );
    assert_eq!(r.status, 422, "{r:?}");
    // A body past the box's limit is refused whole.
    let huge = json!({ "name": "a", "text": "#".repeat(8 << 20), "summary": "s" });
    let r = post("application/json", &body(&huge));
    assert!(matches!(r.status, 413 | 422), "{r:?}");
    assert_eq!(b.pending().status, 404);
    assert_eq!(post("application/json", &body(&ok)).status, 201);
}

#[test]
fn a_second_proposal_replaces_the_first_and_the_old_id_saves_nothing() {
    let b = Bench::new();
    let alice = b.alice();
    b.project(&alice, "line3");
    let first = b.propose("first", &e1213()).json();
    assert_eq!(first["replaced"], false);
    let old = first["id"].as_u64().unwrap();
    let second = b.propose("second", &e1213()).json();
    assert_eq!(second["replaced"], true);
    let new = second["id"].as_u64().unwrap();
    assert_ne!(old, new);
    let before = b.disk();
    let r = b.accept(&alice, &json!({ "id": old, "project": "line3" }));
    assert_eq!(r.status, 409, "{r:?}");
    assert_eq!(b.disk(), before);
    let r = b.accept(&alice, &json!({ "id": new, "project": "line3" }));
    assert_eq!(r.status, 201, "{r:?}");
    assert_eq!(r.json()["file"], "second.device");
    // An id the box never gave.
    assert_eq!(b.propose("third", &e1213()).status, 201);
    let r = b.accept(&alice, &json!({ "id": 99_999, "project": "line3" }));
    assert_eq!(r.status, 409, "{r:?}");
    let r = b.accept(&alice, &json!({ "project": "line3" }));
    assert_eq!(r.status, 400, "an accept without the id is malformed: {r:?}");
}

#[test]
fn accept_with_nothing_pending_and_discard() {
    let b = Bench::new();
    let alice = b.alice();
    b.project(&alice, "line3");
    let before = b.disk();
    let r = b.accept(&alice, &json!({ "id": 1, "project": "line3" }));
    assert_eq!(r.status, 404, "{r:?}");
    let r = b.json("DELETE", "/api/device-profile/proposal", Some(&alice), &json!({}));
    assert_eq!(r.status, 404, "{r:?}");

    assert_eq!(b.propose("moxa-copy", &e1213()).status, 201);
    let id = b.pending().json()["id"].as_u64().unwrap();
    let r = b.json("DELETE", "/api/device-profile/proposal", Some(&alice), &json!({}));
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(b.pending().status, 404);
    let r = b.accept(&alice, &json!({ "id": id, "project": "line3" }));
    assert_eq!(r.status, 404, "a discarded proposal saves nothing: {r:?}");
    assert_eq!(b.disk(), before);
    let audit = b.read("/api/audit?limit=5000").json().to_string();
    assert!(audit.contains("moxa-copy: discarded"), "{audit}");
}

#[test]
fn two_accepts_at_once_write_the_file_once() {
    let b = Bench::new();
    let alice = b.alice();
    b.project(&alice, "line3");
    assert_eq!(b.propose("moxa-copy", &e1213()).status, 201);
    let id = b.pending().json()["id"].as_u64().unwrap();
    let statuses: Vec<u16> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..6)
            .map(|_| s.spawn(|| b.accept(&alice, &json!({ "id": id, "project": "line3" })).status))
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(statuses.iter().filter(|s| **s == 201).count(), 1, "{statuses:?}");
    assert!(statuses.iter().all(|s| matches!(s, 201 | 404)), "{statuses:?}");
    let files: Vec<String> = b.disk().into_keys().filter(|k| k.ends_with(".device")).collect();
    assert_eq!(files.len(), 1, "{files:?}");
}

// ---- the vendor's file, read by the agent ------------------------------------

#[test]
fn the_vendors_eds_is_a_drop_in_the_agent_reads_and_a_file_that_is_not_one_is_refused() {
    let b = Bench::new();
    // The upload that already exists holds an EDS as a text drop-in, within
    // its limits and with the box token.
    let up = |q: &str, ctype: &str, bytes: &[u8], bearer: Option<&str>| {
        b.call("POST", &format!("/api/dropins{q}"), bearer, ctype, bytes)
    };
    assert_eq!(
        up("?name=e1213.eds", "text/plain", EDS.as_bytes(), None).status,
        401
    );
    let r = up("?name=e1213.eds", "text/plain", EDS.as_bytes(), Some(TOKEN));
    assert_eq!(r.status, 201, "{r:?}");
    let id = r.json()["id"].as_u64().unwrap();
    // The agent reads it through its tool, as plant data.
    let o = b.tool("read_dropin", &json!({ "dropin_id": id, "chunk": 0 }));
    assert!(o.ok, "{}", o.summary);
    assert!(o.data.to_string().contains("ioLogik E1200 Series"), "{}", o.data);
    // Too big, a PDF that is not one, a binary, a name with a path.
    let big = vec![b'a'; (1 << 20) + 1];
    assert_eq!(up("?name=big.eds", "text/plain", &big, Some(TOKEN)).status, 413);
    let r = up(
        "?name=fake.pdf&kind=pdf",
        "application/pdf",
        EDS.as_bytes(),
        Some(TOKEN),
    );
    assert!((400..500).contains(&r.status), "{r:?}");
    let r = up(
        "?name=x.eds",
        "text/plain",
        b"[Device]\0\x01\x02 binary",
        Some(TOKEN),
    );
    assert!((400..500).contains(&r.status), "{r:?}");
    let r = up("?name=x.eds", "text/plain", &[0xff, 0xfe, 0xfd], Some(TOKEN));
    assert!((400..500).contains(&r.status), "{r:?}");
}
