//! The agent's read-only device-configuration tools (`list_device_configs`,
//! `read_device_config`) against a real box with a simulated Moxa console
//! (`helmsman_devconf::moxa_e1200::sim`): over MCP (in process) and as the
//! chat agent runs them (`BoxApi` over loopback). The console password a
//! test stores through the box appears in no tool output and no log line,
//! a device without configuration and an unknown one get a plain answer,
//! and the tools have no write path. The bench is the one in
//! tests/devconf.rs, trimmed.
//!
//! Nothing here touches a real device: the console is the simulator on
//! `127.0.0.1`.
#![allow(clippy::too_many_lines)] // each test walks one whole flow
#![allow(clippy::many_single_char_names)]
#![allow(dead_code)] // the bench is shared in shape with tests/devconf.rs

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use helmsman_agent::tools::{self, BoxApi, Drafts};
use helmsman_devconf::moxa_e1200::Config as ConsoleConfig;
use helmsman_devconf::moxa_e1200::sim::{Options, Sim};
use helmsman_iec::{ImageSizes, compile};
use helmsman_plc::api::devconf::Setup;
use helmsman_plc::api::{
    Api, Approval, Catalog, Config, Devices, Extras, Limits, Live, MemAudit, MemProposals, ModuleInfo, More,
    Plant, Shared, swap,
};
use helmsman_plc::manifest::ModuleDir;
use helmsman_plc::policy::account::{Accounts, Logins};
use helmsman_plc::policy::pin::{Gate, Pins};
use helmsman_plc::policy::{FLOOR, Role};
use helmsman_plc::project::{Projects, Running};
use helmsman_plc::spec::Defaults;
use helmsman_plc::store::{ChangeSet, ModuleFiles};
use helmsman_plc::{Runtime, Window};
use serde_json::{Value as Json, json};

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const ALICE: &str = "alice-engineer-pw";
const BOB: &str = "bob-operator-pw";
/// A console password distinct enough to search every output for.
const SECRET: &str = "hunter-two-sec";
const NEW_SECRET: &str = "fresh-pw-77";

const E1213: &str = include_str!("../../../docs/devices/moxa-iologik-e1213.device");

const MANIFEST_USED: &str = "name reader\nauthor Test\nscan_ms 2\nreads moxa_di_00\ndrives lamp\n";
const SOURCE_USED: &str = "PROGRAM reader\nVAR_INPUT moxa_di_00 : BOOL; END_VAR\nVAR_OUTPUT lamp : BOOL; END_VAR\n  lamp := moxa_di_00;\nEND_PROGRAM";
const MANIFEST_FREE: &str = "name reader\nauthor Test\nscan_ms 2\nreads start\ndrives lamp\n";
const SOURCE_FREE: &str = "PROGRAM reader\nVAR_INPUT start : BOOL; END_VAR\nVAR_OUTPUT lamp : BOOL; END_VAR\n  lamp := start;\nEND_PROGRAM";

/// A module reads a signal from the Moxa, or nothing depends on it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Use {
    Used,
    Free,
}

fn map(u: Use, console_port: u16) -> String {
    let bound = match u {
        Use::Used => "in  moxa_di_00 bool moxa ch:DI-00\nin  start bool box di:0\n",
        Use::Free => "in  start bool box di:0\n",
    };
    format!(
        "device moxa device:moxa-e1200 127.0.0.1:{console_port}\n{bound}out lamp bool box do:0 safe=false\n"
    )
}

fn defaults() -> Defaults {
    Defaults {
        period: Duration::from_millis(2),
        i: Window::new(0, 0),
        q: Window::new(0, 0),
        m: 16,
    }
}

fn tmp(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "devconf-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Every response body and log line, kept for the leak checks.
#[derive(Default)]
struct Seen {
    bodies: Mutex<Vec<String>>,
    log: Mutex<Vec<String>>,
}

struct Bench {
    api: Option<Api>,
    addr: SocketAddr,
    sim: Sim,
    state: PathBuf,
    clock: Arc<AtomicU64>,
    seen: Arc<Seen>,
}

impl Bench {
    fn new(u: Use) -> Self {
        Self::with(u, Options::default())
    }

    fn with(u: Use, opts: Options) -> Self {
        Self::with_wait(u, opts, Duration::from_secs(20))
    }

    /// `restart_up`: how long the box waits for a unit to come back.
    fn with_wait(u: Use, opts: Options, restart_up: Duration) -> Self {
        let sim = Sim::start_with(opts);
        let (manifest, source) = match u {
            Use::Used => (MANIFEST_USED, SOURCE_USED),
            Use::Free => (MANIFEST_FREE, SOURCE_FREE),
        };
        let io_map = map(u, 502);
        // The profiles the running map was loaded with: the E1213's (which
        // has `configure moxa-e1200`), one without the line, one with a name
        // no box has.
        let plain = E1213.replace("configure moxa-e1200\n", "");
        let drivers: BTreeMap<String, Vec<u8>> = BTreeMap::from([
            ("moxa-e1200.device".to_string(), E1213.as_bytes().to_vec()),
            ("plain.device".to_string(), plain.into_bytes()),
            (
                "bad.device".to_string(),
                E1213
                    .replace("configure moxa-e1200", "configure web-console")
                    .into_bytes(),
            ),
        ]);
        let d = defaults();
        let md = ModuleDir::from_text(Path::new("reader"), manifest, source.into(), &d).unwrap();
        let sizes = ImageSizes {
            i: md.spec.i.len,
            q: md.spec.q.len,
            m: md.spec.m,
        };
        let module = md.module(compile(source, sizes).unwrap()).unwrap();
        let layout = helmsman_plc::layout::Layout::build_with(Some(&io_map), None, &drivers).unwrap();
        let table = layout.table;
        let rt = Runtime::with_signals(32, 32, vec![module], &table).unwrap();
        let modules = vec![
            ModuleInfo::of(
                &rt,
                "reader",
                "reader".into(),
                source.into(),
                Some(md.manifest.clone()),
            )
            .unwrap(),
        ];
        let catalog = Catalog {
            signals: Catalog::signals(&table, layout.map.as_ref(), &modules),
            modules,
        };
        let state = tmp("state");
        let mut a = Accounts::default();
        a.add("alice", Role::Engineer, ALICE, 1, [1; 16]).unwrap();
        a.add("bob", Role::Operator, BOB, 1, [2; 16]).unwrap();
        a.save(&state).unwrap();
        let (handle, _swapper) = swap::channel(Duration::from_secs(5));
        let t0 = Instant::now();
        let clock = Arc::new(AtomicU64::new(0));
        let c = Arc::clone(&clock);
        let now: helmsman_plc::policy::pin::Clock =
            Arc::new(move || t0 + Duration::from_secs(c.load(Ordering::SeqCst)));
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
                table: table.clone(),
                i_len: 32,
                q_len: 32,
                defaults: d,
                record: None,
            },
            swap: handle,
        };
        let seen = Arc::new(Seen::default());
        let log_seen = Arc::clone(&seen);
        let cfg = Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            token: TOKEN.into(),
            limits: Limits::default(),
            stream_every: Duration::from_millis(20),
            memory: 16,
            log: Arc::new(move |l: &str| {
                log_seen
                    .log
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(l.into());
            }),
        };
        let running = ChangeSet {
            modules: BTreeMap::from([("reader".to_string(), ModuleFiles::st(manifest, source))]),
            io_map: Some(io_map),
            calendar: None,
        };
        let running_map_with_others = running.io_map.clone().unwrap()
            + "device plain device:plain 127.0.0.1:502\ndevice bad device:bad 127.0.0.1:502\n";
        let running = ChangeSet {
            io_map: Some(running_map_with_others),
            ..running
        };
        let shared = Shared {
            catalog,
            live: Arc::new(Live::default()),
            devices: Arc::new(Devices::new(["box".to_string(), "moxa".to_string()])),
            audit: Box::new(MemAudit::new(100_000)),
            proposals: Box::new(MemProposals::new(1000)),
            running: Some(running),
            snapshots: None,
            approval: Some(approval),
            chat: None,
        };
        let more = More {
            projects: Some(Projects::in_state_dir(&state).unwrap()),
            running_drivers: drivers,
            devconf: Some(Setup {
                dir: state.join("devconf"),
                http_port: sim_http_port(&sim),
                console: ConsoleConfig {
                    gap: Duration::ZERO,
                    retry_wait: Duration::ZERO,
                    poll: Duration::from_millis(5),
                    request: Duration::from_secs(5),
                    restart_down: Duration::from_secs(5),
                    restart_up,
                    modbus_port: sim.modbus_port(),
                    ..ConsoleConfig::default()
                },
            }),
            ..More::default()
        };
        let api = Api::start_box(cfg, shared, Extras::default(), None, more).unwrap();
        let addr = api.addr();
        Self {
            api: Some(api),
            addr,
            sim,
            state,
            clock,
            seen,
        }
    }

    fn call(&self, method: &str, path: &str, bearer: Option<&str>, body: &Json) -> Resp {
        let r = send(
            self.addr,
            method,
            path,
            bearer,
            "application/json",
            body.to_string().as_bytes(),
        );
        self.seen
            .bodies
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(String::from_utf8_lossy(&r.body).into_owned());
        r
    }

    fn login(&self, user: &str, password: &str) -> String {
        let r = self.call(
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

    fn read(&self, path: &str) -> Resp {
        self.call("GET", path, Some(TOKEN), &Json::Null)
    }

    fn cfg(&self, s: &str) -> Resp {
        self.read_as(s, "/api/devices/moxa/config")
    }

    fn read_as(&self, s: &str, path: &str) -> Resp {
        self.call("GET", path, Some(s), &Json::Null)
    }

    fn post(&self, s: &str, route: &str, body: &Json) -> Resp {
        self.call(
            "POST",
            &format!("/api/devices/moxa/config/{route}"),
            Some(s),
            body,
        )
    }

    /// Records project `name` as the running one.
    fn run_project(&self, s: &str, name: &str) {
        let r = self.call("POST", "/api/projects", Some(s), &json!({ "name": name }));
        assert_eq!(r.status, 201, "{r:?}");
        Projects::in_state_dir(&self.state)
            .unwrap()
            .record(&Running {
                name: name.into(),
                snapshot: "0".repeat(64),
            })
            .unwrap();
    }

    /// Everything the box said or logged, to look for what must not be there.
    fn everything(&self) -> String {
        let audit = self.read("/api/audit?limit=1000");
        let mut all = String::from_utf8_lossy(&audit.body).into_owned();
        for b in self
            .seen
            .bodies
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
        {
            all.push_str(b);
        }
        for l in self
            .seen
            .log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
        {
            all.push_str(l);
        }
        all
    }
}

impl Drop for Bench {
    fn drop(&mut self) {
        self.api.take();
    }
}

/// The simulator's HTTP port: its address is `127.0.0.1:port`.
fn sim_http_port(sim: &Sim) -> u16 {
    sim.addr().rsplit(':').next().unwrap().parse().unwrap()
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

    fn code(&self) -> String {
        self.json()["code"].as_str().unwrap_or_default().to_string()
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

// ---- the tools --------------------------------------------------------------

/// `tools/call` over MCP (2026-07-28), the box token: the tool's result.
fn mcp_tool(b: &Bench, name: &str, arguments: &Json) -> Json {
    let body = json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {
        "name": name, "arguments": arguments,
        "_meta": {
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientInfo": { "name": "test", "version": "1" },
            "io.modelcontextprotocol/clientCapabilities": {},
        },
    }});
    let auth = format!("Bearer {TOKEN}");
    let head = format!(
        "POST /mcp HTTP/1.1\r\nHost: box\r\nAuthorization: {auth}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMCP-Protocol-Version: 2026-07-28\r\nMcp-Method: tools/call\r\nMcp-Name: {name}\r\nContent-Length: {}\r\n\r\n",
        body.to_string().len()
    );
    let mut s = TcpStream::connect(b.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    s.write_all(head.as_bytes()).unwrap();
    s.write_all(body.to_string().as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let split = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let text = String::from_utf8_lossy(&out[split + 4..]).into_owned();
    b.seen
        .bodies
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(text.clone());
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"))
}

/// The text of a tool result, or the JSON-RPC error.
fn result_text(r: &Json) -> String {
    r["result"]["content"][0]["text"]
        .as_str()
        .map_or_else(|| r.to_string(), str::to_string)
}

fn bench_with_password() -> Bench {
    let b = Bench::with(
        Use::Free,
        Options {
            password: Some(SECRET.into()),
            ..Options::default()
        },
    );
    let s = b.alice();
    let r = b.call(
        "PUT",
        "/api/devices/moxa/config/password",
        Some(&s),
        &json!({ "password": SECRET }),
    );
    assert_eq!(r.status, 200, "{r:?}");
    b
}

#[test]
fn the_tools_read_facts_and_never_the_password_over_mcp() {
    let b = bench_with_password();
    let r = mcp_tool(&b, "read_device_config", &json!({ "device": "moxa" }));
    assert_eq!(r["result"]["isError"], false, "{r}");
    let text = result_text(&r);
    assert!(text.contains("<box-data tool=\"read_device_config\">"), "{text}");
    assert!(text.contains("\"password_stored\":true"), "{text}");
    assert!(text.contains("\"eip_enabled\""), "{text}");
    assert!(text.contains("\"outputs\""), "{text}");
    assert!(
        !text.contains("\"export\""),
        "the whole export is left out: {text}"
    );
    let l = mcp_tool(&b, "list_device_configs", &json!({}));
    let list = result_text(&l);
    assert!(list.contains("\"device\":\"moxa\""), "{list}");
    assert!(list.contains("moxa-e1200"), "{list}");
    assert!(
        !list.contains("\"plain\""),
        "no configuration, not listed: {list}"
    );
    // The tool list: descriptions say it is the engineer's, and hold no secret.
    let all_text = format!("{text}{list}{}", b.everything());
    assert!(
        !all_text.contains(SECRET),
        "the password reached a tool result or a log line"
    );
}

#[test]
fn the_chat_agents_path_gives_the_same_and_leaks_nothing() {
    let b = bench_with_password();
    let api = BoxApi::local(b.addr, TOKEN.into());
    let mut drafts = Drafts::default();
    let o = tools::run(
        &api,
        &mut drafts,
        "read_device_config",
        &json!({ "device": "moxa" }),
    );
    assert!(o.ok, "{}", o.data);
    assert_eq!(o.data["password_stored"], true);
    assert_eq!(o.data["configurator"], "moxa-e1200");
    assert!(o.data["facts"]["outputs"].is_array(), "{}", o.data);
    let l = tools::run(&api, &mut drafts, "list_device_configs", &json!({}));
    assert!(l.ok, "{}", l.data);
    let seen = format!(
        "{} {} {} {} {}",
        o.data,
        o.summary,
        l.data,
        tools::envelope("read_device_config", &o.data),
        b.everything()
    );
    assert!(
        !seen.contains(SECRET),
        "the password reached the agent's context or a log line"
    );
}

#[test]
fn a_device_without_configuration_and_an_unknown_one_are_answered_plainly() {
    let b = Bench::new(Use::Free);
    let api = BoxApi::local(b.addr, TOKEN.into());
    let mut drafts = Drafts::default();
    let o = tools::run(
        &api,
        &mut drafts,
        "read_device_config",
        &json!({ "device": "plain" }),
    );
    assert!(o.ok, "{}", o.data);
    assert_eq!(o.data["configured"], false);
    assert!(o.summary.contains("has no configuration"), "{}", o.summary);
    let o = tools::run(
        &api,
        &mut drafts,
        "read_device_config",
        &json!({ "device": "ghost" }),
    );
    assert!(!o.ok);
    assert!(o.summary.contains("no device ghost"), "{}", o.summary);
    // Over MCP the same sentences, not a dump.
    let r = mcp_tool(&b, "read_device_config", &json!({ "device": "plain" }));
    assert!(result_text(&r).contains("has no configuration"), "{r}");
    let r = mcp_tool(&b, "read_device_config", &json!({ "device": "../etc" }));
    assert_eq!(r["result"]["isError"], true, "{r}");
    let r = mcp_tool(&b, "read_device_config", &json!({ "device": "moxa", "set": {} }));
    assert_eq!(r["result"]["isError"], true, "unknown fields are refused: {r}");
}

#[test]
fn the_agent_has_no_write_path_to_device_configuration() {
    let b = bench_with_password();
    // No tool is named for a write, over the chat agent's set or MCP's.
    for n in tools::NAMES {
        for w in [
            "restart", "import", "snapshot", "password", "set_", "write", "apply",
        ] {
            assert!(!n.contains(w), "{n}");
        }
    }
    // A made-up writer is "no such tool" and nothing happens.
    for n in [
        "restart_device",
        "set_device_config",
        "import_device_config",
        "snapshot_device",
        "set_device_password",
    ] {
        let r = mcp_tool(&b, n, &json!({ "device": "moxa" }));
        assert!(r.get("error").is_some(), "{n}: {r}");
        let o = tools::run(
            &BoxApi::local(b.addr, TOKEN.into()),
            &mut Drafts::default(),
            n,
            &json!({ "device": "moxa" }),
        );
        assert!(!o.ok && o.summary.contains("no tool named"), "{n}: {}", o.summary);
    }
    // The box token, which is all the agent holds, cannot write a config
    // route: an engineer's session is needed.
    for (m, p) in [
        ("POST", "/api/devices/moxa/config/restart"),
        ("POST", "/api/devices/moxa/config/set"),
        ("POST", "/api/devices/moxa/config/import"),
        ("POST", "/api/devices/moxa/config/snapshot"),
        ("PUT", "/api/devices/moxa/config/password"),
        ("DELETE", "/api/devices/moxa/config/password"),
    ] {
        let r = b.call(m, p, Some(TOKEN), &json!({}));
        assert_eq!(r.status, 403, "{m} {p}: {r:?}");
    }
    // And the in-process MCP caller does not reach them at all.
    assert!(!helmsman_plc::api::mcp::reachable(
        "POST",
        "/api/devices/moxa/config/restart"
    ));
    assert!(!helmsman_plc::api::mcp::reachable(
        "PUT",
        "/api/devices/moxa/config/password"
    ));
}

#[test]
fn the_tool_descriptions_say_changes_are_the_engineers_and_hold_no_secret() {
    let cat = helmsman_plc::api::mcp::catalog().to_string();
    assert!(
        cat.contains("changes are the engineer's, in the Devices tab"),
        "{cat}"
    );
    assert!(!cat.contains(SECRET));
}
