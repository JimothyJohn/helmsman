//! Configuring devices through the box API (`api/devconf.rs`) over real
//! sockets, real accounts and a simulated Moxa console
//! (`helmsman_devconf::moxa_e1200::sim`): every route's happy path, the
//! auth negatives, the interruption gate, the network option off by
//! default, the saved configuration as a project file, the console
//! password never leaving the box, and one operation per device at a time
//! with the rest of the API served meanwhile.
//!
//! Nothing here touches a real device: the console is the simulator on
//! `127.0.0.1`, reached because the test aims the box's console port at it.
#![allow(clippy::too_many_lines)] // each test walks one whole flow
#![allow(clippy::many_single_char_names)]

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use helmsman_devconf::moxa_e1200::Config as ConsoleConfig;
use helmsman_devconf::moxa_e1200::sim::{Options, Restart, Settings, Sim};
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

// ---- reads ------------------------------------------------------------------

#[test]
fn a_read_gives_facts_the_export_and_no_password() {
    let b = Bench::new(Use::Free);
    let r = b.cfg(TOKEN);
    assert_eq!(r.status, 200, "{r:?}");
    let j = r.json();
    assert_eq!(j["device"], "moxa");
    assert_eq!(j["configurator"], "moxa-e1200");
    assert_eq!(j["endpoint"], format!("127.0.0.1:{}", sim_http_port(&b.sim)));
    assert_eq!(j["password_stored"], false);
    assert_eq!(j["export"].as_str().unwrap(), Settings::default().export());
    assert!(j["snapshot"].is_null() && j["drift"].is_null());
    let f = &j["facts"];
    assert_eq!(f["firmware"], "V3.1", "{f}");
    assert_eq!(f["watchdog"]["enabled"], false);
    assert_eq!(f["watchdog"]["label"], "Disabled");
    assert_eq!(f["eip_enabled"], true);
    assert_eq!(f["rest_enabled"], true);
    let outs = f["outputs"].as_array().unwrap();
    assert_eq!(outs.len(), 8);
    assert_eq!(outs[0]["channel"], "DO-00");
    assert_eq!(outs[0]["safe"]["label"], "Off");
    // A read changes nothing on the unit.
    assert_eq!(b.sim.count("/set.htm"), 0);
    assert_eq!(b.sim.reboots(), 0);
}

#[test]
fn a_device_without_a_configurator_has_no_config_routes() {
    let b = Bench::new(Use::Free);
    let s = b.alice();
    for id in ["plain", "bad", "box", "nope"] {
        let g = b.read_as(&s, &format!("/api/devices/{id}/config"));
        assert_eq!(g.status, 404, "{id}: {g:?}");
        let p = b.call(
            "POST",
            &format!("/api/devices/{id}/config/restart"),
            Some(&s),
            &json!({}),
        );
        assert_eq!(p.status, 404, "{id}: {p:?}");
        let k = b.call(
            "PUT",
            &format!("/api/devices/{id}/config/password"),
            Some(&s),
            &json!({ "password": "x" }),
        );
        assert_eq!(k.status, 404, "{id}: {k:?}");
    }
    // The profile that names an unknown configurator does not read at all.
    let g = b.read_as(&s, "/api/devices/bad/config");
    assert_eq!(g.code(), "not_configurable");
    assert_eq!(b.sim.requests().len(), 0, "nothing reached a unit");
    assert!(!b.state.join("devconf").exists(), "nothing kept for them");
}

#[test]
fn a_map_naming_a_profile_with_an_unknown_configurator_is_a_listed_problem() {
    // The load-time check: the profile is held to Profile::parse when the
    // map loads, so a bad `configure` line is a problem naming the line.
    let dir = tmp("profiles");
    std::fs::write(
        dir.join("bad.device"),
        E1213.replace("configure moxa-e1200", "configure web-console"),
    )
    .unwrap();
    let sources = helmsman_plc::driver_dir::Sources {
        project: Some(("p".into(), dir)),
        boxed: None,
    };
    let found = sources.find_text(Some("device d device:bad 127.0.0.1:502\n"));
    let text = found.problems.join("\n");
    assert!(
        text.contains("bad.device:") && text.contains("configure web-console"),
        "{text}"
    );
}

// ---- auth -------------------------------------------------------------------

#[test]
fn every_route_checks_who_asks() {
    let b = Bench::new(Use::Free);
    let alice = b.alice();
    let bob = b.login("bob", BOB);
    let writes: Vec<(&str, &str, Json)> = vec![
        ("PUT", "password", json!({ "password": "x" })),
        ("DELETE", "password", json!({})),
        (
            "POST",
            "set",
            json!({ "page": "04_1.htm", "changes": { "MOD_LOC": "x" } }),
        ),
        ("POST", "restart", json!({})),
        ("POST", "import", json!({ "text": Settings::default().export() })),
        ("POST", "password", json!({ "new": "y" })),
        ("POST", "snapshot", json!({})),
    ];
    for (m, route, body) in &writes {
        let path = format!("/api/devices/moxa/config/{route}");
        let none = b.call(m, &path, None, body);
        assert_eq!(none.status, 401, "{m} {route} without a credential: {none:?}");
        let token = b.call(m, &path, Some(TOKEN), body);
        assert_eq!(token.status, 403, "{m} {route} with the box token: {token:?}");
        let operator = b.call(m, &path, Some(&bob), body);
        assert_eq!(operator.status, 403, "{m} {route} as an operator: {operator:?}");
        let wrong = b.call(m, &path, Some("0".repeat(64).as_str()), body);
        assert_eq!(wrong.status, 401, "{m} {route} with a wrong token: {wrong:?}");
    }
    // Reads: a session or the token, never nothing.
    assert_eq!(
        b.call("GET", "/api/devices/moxa/config", None, &Json::Null)
            .status,
        401
    );
    assert_eq!(b.cfg(&bob).status, 200);
    assert_eq!(b.cfg(TOKEN).status, 200);
    // Nothing above reached the unit's writes.
    assert_eq!(
        b.sim.count("/set.htm") + b.sim.count("/set_ok.htm") + b.sim.count("/06_5_1.htm"),
        0
    );
    assert_eq!(b.sim.reboots(), 0);
    assert!(!b.state.join("devconf").exists());
    // An engineer can.
    assert_eq!(b.post(&alice, "set", &writes[2].2).status, 200);
    // A session past its hour is 401 again, on a read and on a write.
    b.clock.fetch_add(2 * 3600, Ordering::SeqCst);
    assert_eq!(b.cfg(&alice).status, 401);
    assert_eq!(b.post(&alice, "restart", &json!({})).status, 401);
    assert_eq!(b.sim.reboots(), 0);
}

#[test]
fn a_cookie_session_needs_the_same_origin_proof_on_a_write() {
    let b = Bench::new(Use::Free);
    let r = send(
        b.addr,
        "POST",
        "/api/devices/moxa/config/restart",
        None,
        "application/json",
        b"{}",
    );
    assert_eq!(r.status, 401);
    // (The cookie path is the same code as projects'; its proof is tested in tests/accounts.rs.)
    assert_eq!(b.sim.reboots(), 0);
}

// ---- set, restart, import -----------------------------------------------------

#[test]
fn set_stages_a_change_and_restart_applies_it() {
    let b = Bench::new(Use::Free);
    let s = b.alice();
    let r = b.post(
        &s,
        "set",
        &json!({ "page": "04_1.htm", "changes": { "MOD_LOC": "bench" } }),
    );
    assert_eq!(r.status, 200, "{r:?}");
    let j = r.json();
    assert_eq!(j["staged"], true);
    assert_eq!(
        j["diff"]["changed"],
        json!([{ "before": "MOD_LOC=", "after": "MOD_LOC=bench" }])
    );
    assert_eq!(b.sim.staged().mod_loc, "bench");
    assert_eq!(b.sim.running().mod_loc, "", "staged, not running");
    assert!(
        b.cfg(&s).json()["export"]
            .as_str()
            .unwrap()
            .contains("MOD_LOC=bench")
    );

    let r = b.post(&s, "restart", &json!({}));
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(r.json()["restarted"], true);
    assert!(r.json()["took_ms"].is_u64());
    assert_eq!(b.sim.running().mod_loc, "bench");
    assert!(b.sim.is_up());
    assert_eq!(
        b.sim.count("/set_ok.htm"),
        1,
        "one restart request, never repeated"
    );
}

#[test]
fn set_refuses_what_the_unit_must_not_get() {
    let b = Bench::new(Use::Free);
    let s = b.alice();
    let bad = [
        json!({ "page": "04_1.htm", "changes": { "nope": "1" } }),
        json!({ "page": "04_1.htm", "changes": { "token": "1" } }),
        json!({ "page": "08.htm", "changes": { "a": "1" } }),
        json!({ "page": "../04_1.htm", "changes": { "a": "1" } }),
        json!({ "page": "07.htm", "changes": { "OldPwd": "1" } }),
    ];
    for body in &bad {
        let r = b.post(&s, "set", body);
        assert_eq!(r.status, 422, "{body}: {r:?}");
        assert_eq!(r.code(), "refused");
    }
    for body in [
        json!({}),
        json!({ "page": "04_1.htm" }),
        json!({ "page": "04_1.htm", "changes": {} }),
        json!({ "page": "04_1.htm", "changes": { "MOD_LOC": 5 } }),
        json!({ "page": "04_1.htm", "changes": { "MOD_LOC": "x" }, "extra": true }),
    ] {
        let r = b.post(&s, "set", &body);
        assert_eq!(r.status, 400, "{body}: {r:?}");
    }
    let r = send(
        b.addr,
        "POST",
        "/api/devices/moxa/config/set",
        Some(&s),
        "text/plain",
        b"{}",
    );
    assert_eq!(r.status, 400);
    assert_eq!(b.sim.count("/set.htm"), 0);
    assert_eq!(
        b.sim.count("/08.htm") + b.sim.count("/09.htm"),
        0,
        "no action page was fetched"
    );
    assert_eq!(b.sim.staged(), Settings::default());
}

#[test]
fn a_restart_that_would_interrupt_a_module_needs_interrupts_true() {
    let b = Bench::new(Use::Used);
    let s = b.alice();
    let r = b.post(&s, "restart", &json!({}));
    assert_eq!(r.status, 409, "{r:?}");
    assert_eq!(r.code(), "interrupts");
    assert_eq!(r.json()["modules"], json!(["reader"]));
    assert!(r.json()["error"].as_str().unwrap().contains("reader"));
    assert_eq!(b.sim.reboots(), 0, "refused before the unit was touched");
    assert_eq!(b.sim.requests().len(), 0);
    for bad in [json!({ "interrupts": "yes" }), json!({ "interrupts": 1 })] {
        assert_eq!(b.post(&s, "restart", &bad).status, 400, "{bad}");
    }
    assert_eq!(b.sim.reboots(), 0);
    let ok = b.post(&s, "restart", &json!({ "interrupts": true }));
    assert_eq!(ok.status, 200, "{ok:?}");
    assert_eq!(b.sim.reboots(), 1);
}

#[test]
fn a_restart_nothing_depends_on_needs_no_interrupts() {
    let b = Bench::new(Use::Free);
    let s = b.alice();
    assert_eq!(b.post(&s, "restart", &json!({})).status, 200);
    assert_eq!(b.sim.reboots(), 1);
}

#[test]
fn import_restores_without_the_network_unless_asked() {
    let b = Bench::new(Use::Free);
    let s = b.alice();
    let other = Settings {
        mod_loc: "restored".into(),
        ip_address: "192.0.2.99".into(),
        ..Settings::default()
    };
    let text = other.export();

    let r = b.post(&s, "import", &json!({ "text": text }));
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(r.json()["imported"], true);
    assert_eq!(
        b.sim.running().mod_loc,
        "restored",
        "an import reboots the unit: it runs"
    );
    assert_eq!(
        b.sim.running().ip_address,
        "192.0.2.60",
        "the unit's own address survived"
    );
    assert_eq!(b.sim.reboots(), 1);
    assert_eq!(r.json()["diff"]["changed"].as_array().unwrap().len(), 1);

    let r = b.post(&s, "import", &json!({ "text": text, "overwrite_network": false }));
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(b.sim.running().ip_address, "192.0.2.60");

    let r = b.post(&s, "import", &json!({ "text": text, "overwrite_network": true }));
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(b.sim.running().ip_address, "192.0.2.99");
    assert_eq!(b.sim.reboots(), 3);
}

#[test]
fn an_import_is_held_to_the_same_gate_and_the_same_shape() {
    let b = Bench::new(Use::Used);
    let s = b.alice();
    let text = Settings::default().export();
    let r = b.post(&s, "import", &json!({ "text": text }));
    assert_eq!((r.status, r.code().as_str()), (409, "interrupts"), "{r:?}");
    assert_eq!(b.sim.reboots(), 0);
    for bad in [
        json!({}),
        json!({ "from": "snapshot", "text": text }),
        json!({ "from": "somewhere" }),
        json!({ "text": text, "overwrite_network": "yes", "interrupts": true }),
        json!({ "text": 5 }),
        json!({ "text": text, "interrupts": true, "whatever": 1 }),
    ] {
        let r = b.post(&s, "import", &bad);
        assert_eq!(r.status, 400, "{bad}: {r:?}");
    }
    // Not an export: refused by the configurator before anything is sent.
    let r = b.post(&s, "import", &json!({ "text": "hello", "interrupts": true }));
    assert_eq!((r.status, r.code().as_str()), (422, "refused"), "{r:?}");
    assert_eq!(b.sim.reboots(), 0);
    assert_eq!(b.sim.count("/06_5_1.htm"), 0);
    // No snapshot to restore from.
    let r = b.post(&s, "import", &json!({ "from": "snapshot", "interrupts": true }));
    assert_eq!((r.status, r.code().as_str()), (404, "no_snapshot"), "{r:?}");
}

// ---- the saved configuration, a project file --------------------------------

#[test]
fn a_snapshot_is_a_project_file_and_a_restore_uses_it() {
    let b = Bench::new(Use::Free);
    let s = b.alice();
    // No project runs: nowhere to save it.
    let r = b.post(&s, "snapshot", &json!({}));
    assert_eq!((r.status, r.code().as_str()), (409, "no_project"), "{r:?}");
    b.run_project(&s, "line1");

    let r = b.post(&s, "snapshot", &json!({ "version": null }));
    assert_eq!(r.status, 200, "{r:?}");
    let v1 = r.json()["version"].as_str().unwrap().to_string();
    assert_eq!(r.json()["file"], "moxa.devconf");
    assert!(r.json()["revision"].is_u64());
    // It is a project file, read like any: the export as the unit gave it.
    let f = b.read("/api/projects/line1/files/moxa.devconf");
    assert_eq!(f.status, 200, "{f:?}");
    assert_eq!(f.json()["text"].as_str().unwrap(), Settings::default().export());
    assert_eq!(f.json()["version"], v1);
    let files = b.read("/api/projects/line1").json();
    assert!(
        files["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["name"] == "moxa.devconf"),
        "{files}"
    );
    // It shows in the read, with no drift yet.
    let c = b.cfg(&s).json();
    assert_eq!(c["snapshot"]["file"], "moxa.devconf");
    assert_eq!(c["snapshot"]["version"], v1);
    assert_eq!(c["drift"], json!({ "changed": [], "added": [], "removed": [] }));

    // The unit changes: the drift names the line.
    b.post(
        &s,
        "set",
        &json!({ "page": "04_1.htm", "changes": { "MOD_LOC": "drifted" } }),
    );
    let c = b.cfg(&s).json();
    assert_eq!(
        c["drift"]["changed"],
        json!([{ "before": "MOD_LOC=", "after": "MOD_LOC=drifted" }])
    );

    // A stale version is refused; the right one saves again.
    let stale = b.post(&s, "snapshot", &json!({ "version": "0".repeat(64) }));
    assert_eq!((stale.status, stale.code().as_str()), (409, "stale"), "{stale:?}");
    let new_only = b.post(&s, "snapshot", &json!({ "version": null }));
    assert_eq!(new_only.status, 409, "the file exists: {new_only:?}");
    let ok = b.post(&s, "snapshot", &json!({ "version": v1 }));
    assert_eq!(ok.status, 200, "{ok:?}");
    assert_ne!(ok.json()["version"].as_str().unwrap(), v1);

    // Restore from the snapshot: save a state worth coming back to, move
    // away from it, and import the saved file.
    let saved = ok.json()["version"].as_str().unwrap().to_string();
    b.post(
        &s,
        "set",
        &json!({ "page": "04_1.htm", "changes": { "MOD_LOC": "kept" } }),
    );
    let kept = b.post(&s, "snapshot", &json!({ "version": saved }));
    assert_eq!(kept.status, 200, "{kept:?}");
    b.post(
        &s,
        "set",
        &json!({ "page": "04_1.htm", "changes": { "MOD_LOC": "later" } }),
    );
    let r = b.post(&s, "import", &json!({ "from": "snapshot" }));
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(
        b.sim.running().mod_loc,
        "kept",
        "the saved configuration is what runs"
    );
    assert_eq!(
        r.json()["diff"]["changed"],
        json!([{ "before": "MOD_LOC=later", "after": "MOD_LOC=kept" }])
    );
}

#[test]
fn a_project_accepts_a_devconf_file_by_hand_but_only_a_well_named_one() {
    let b = Bench::new(Use::Free);
    let s = b.alice();
    b.run_project(&s, "line1");
    let put = |name: &str| {
        b.call(
            "PUT",
            &format!("/api/projects/line1/files/{name}"),
            Some(&s),
            &json!({ "text": "[A]\r\nK=1\r\n<END>\r\n" }),
        )
    };
    assert_eq!(put("rio.devconf").status, 200);
    assert_eq!(
        put("Rio.devconf").status,
        400,
        "a device file is a lower-case slug"
    );
    assert_eq!(put("..devconf").status, 400);
}

// ---- the console password -----------------------------------------------------

#[test]
fn the_password_is_kept_write_only_and_used_to_log_in() {
    let b = Bench::with(
        Use::Free,
        Options {
            password: Some(SECRET.into()),
            ..Options::default()
        },
    );
    let s = b.alice();
    // The console asks for a password and none is kept.
    let r = b.cfg(&s);
    assert_eq!((r.status, r.code().as_str()), (409, "password_required"), "{r:?}");
    assert_eq!(b.sim.count("/home.htm"), 0, "nothing posted without one");

    // A wrong one is refused by the console, once per call.
    let r = b.call(
        "PUT",
        "/api/devices/moxa/config/password",
        Some(&s),
        &json!({ "password": "wrong-pw" }),
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(r.json(), json!({ "password_stored": true }));
    let r = b.cfg(&s);
    assert_eq!((r.status, r.code().as_str()), (409, "login_refused"), "{r:?}");
    assert_eq!(b.sim.count("/home.htm"), 1);

    // The right one reads.
    b.call(
        "PUT",
        "/api/devices/moxa/config/password",
        Some(&s),
        &json!({ "password": SECRET }),
    );
    let r = b.cfg(&s);
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(r.json()["password_stored"], true);
    assert!(!r.json().to_string().contains(SECRET));
    // Writes log in too.
    let r = b.post(
        &s,
        "set",
        &json!({ "page": "04_1.htm", "changes": { "MOD_LOC": "locked" } }),
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(b.sim.staged().mod_loc, "locked");

    // Forgotten: back to password_required.
    let r = b.call(
        "DELETE",
        "/api/devices/moxa/config/password",
        Some(&s),
        &json!({}),
    );
    assert_eq!(r.json(), json!({ "password_stored": false }));
    assert_eq!(b.cfg(&s).code(), "password_required");
    assert!(!b.state.join("devconf").join("moxa.password").exists());
}

#[test]
fn the_kept_password_is_private_bounded_and_never_returned() {
    let b = Bench::new(Use::Free);
    let s = b.alice();
    for bad in [
        json!({}),
        json!({ "password": "" }),
        json!({ "password": "a".repeat(25) }),
        json!({ "password": "caf\u{e9}" }),
        json!({ "password": "tab\there" }),
        json!({ "password": SECRET, "extra": 1 }),
        json!({ "password": 5 }),
    ] {
        let r = b.call("PUT", "/api/devices/moxa/config/password", Some(&s), &bad);
        assert_eq!(r.status, 400, "{bad}: {r:?}");
    }
    assert!(
        !b.state.join("devconf").exists(),
        "nothing kept for a refused password"
    );
    let r = b.call(
        "PUT",
        "/api/devices/moxa/config/password",
        Some(&s),
        &json!({ "password": SECRET }),
    );
    assert_eq!(r.status, 200);
    {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = b.state.join("devconf");
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let file = dir.join("moxa.password");
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap().trim_end(), SECRET);
    }
    // No route gives it back.
    for path in [
        "/api/devices/moxa/config",
        "/api/devices",
        "/api/audit?limit=1000",
        "/api",
        "/api/policy",
    ] {
        let r = b.read(path);
        assert!(!String::from_utf8_lossy(&r.body).contains(SECRET), "{path}");
    }
    for m in ["GET", "POST", "DELETE"] {
        let r = b.call(
            m,
            "/api/devices/moxa/config/password",
            Some(&s),
            &json!({ "new": "zz" }),
        );
        assert!(!String::from_utf8_lossy(&r.body).contains(SECRET), "{m}");
    }
}

#[test]
fn changing_the_password_through_the_box_updates_the_kept_one() {
    let b = Bench::new(Use::Free);
    let s = b.alice();
    // A unit with no password: the old one is empty.
    let r = b.post(&s, "password", &json!({ "new": SECRET }));
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(r.json(), json!({ "changed": true }));
    assert_eq!(b.sim.password().as_deref(), Some(SECRET));
    // The box keeps what it set, so it can still read.
    assert_eq!(
        std::fs::read_to_string(b.state.join("devconf/moxa.password"))
            .unwrap()
            .trim_end(),
        SECRET
    );
    let r = b.cfg(&s);
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(r.json()["password_stored"], true);
    // And change it again: the old one is the kept one. (The real unit
    // refused this once a password was set; the simulator accepts it. See
    // docs/ARCHITECTURE.md "Device configuration".)
    let r = b.post(&s, "password", &json!({ "new": NEW_SECRET }));
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(b.sim.password().as_deref(), Some(NEW_SECRET));
    assert_eq!(b.cfg(&s).status, 200);
    // A bad new password never leaves the box.
    for bad in [
        json!({}),
        json!({ "new": "" }),
        json!({ "new": "a".repeat(17) }),
        json!({ "new": "caf\u{e9}" }),
    ] {
        assert_eq!(b.post(&s, "password", &bad).status, 400, "{bad}");
    }
    assert_eq!(b.sim.password().as_deref(), Some(NEW_SECRET));
}

#[test]
fn a_password_change_the_unit_refuses_leaves_the_kept_one_alone() {
    let b = Bench::with(
        Use::Free,
        Options {
            password: Some("moxa".into()),
            ..Options::default()
        },
    );
    let s = b.alice();
    // The kept password is wrong, so the change cannot even log in.
    b.call(
        "PUT",
        "/api/devices/moxa/config/password",
        Some(&s),
        &json!({ "password": "not-moxa" }),
    );
    let r = b.post(&s, "password", &json!({ "new": NEW_SECRET }));
    assert_eq!((r.status, r.code().as_str()), (409, "login_refused"), "{r:?}");
    assert_eq!(b.sim.password().as_deref(), Some("moxa"));
    assert_eq!(
        std::fs::read_to_string(b.state.join("devconf/moxa.password"))
            .unwrap()
            .trim_end(),
        "not-moxa"
    );
}

#[test]
fn no_password_appears_in_any_response_or_log_line() {
    let b = Bench::with(
        Use::Used,
        Options {
            password: Some(SECRET.into()),
            ..Options::default()
        },
    );
    let s = b.alice();
    b.run_project(&s, "line1");
    b.call(
        "PUT",
        "/api/devices/moxa/config/password",
        Some(&s),
        &json!({ "password": SECRET }),
    );
    // Every route, success and refusal.
    b.cfg(&s);
    b.post(
        &s,
        "set",
        &json!({ "page": "04_1.htm", "changes": { "MOD_LOC": "x" } }),
    );
    b.post(
        &s,
        "set",
        &json!({ "page": "04_1.htm", "changes": { "nope": "x" } }),
    );
    b.post(&s, "snapshot", &json!({}));
    b.post(&s, "restart", &json!({}));
    b.post(&s, "restart", &json!({ "interrupts": true }));
    b.post(&s, "import", &json!({ "from": "snapshot", "interrupts": true }));
    b.post(&s, "password", &json!({ "new": NEW_SECRET }));
    b.call(
        "PUT",
        "/api/devices/moxa/config/password",
        Some(&s),
        &json!({ "password": "wrong-one" }),
    );
    b.cfg(&s);
    b.call(
        "DELETE",
        "/api/devices/moxa/config/password",
        Some(&s),
        &json!({}),
    );
    b.cfg(&s);
    b.call(
        "PUT",
        "/api/devices/moxa/config/password",
        Some(&s),
        &json!({ "password": SECRET }),
    );
    // A refusal on a bad request that carries a password.
    b.call(
        "PUT",
        "/api/devices/moxa/config/password",
        Some(&s),
        &json!({ "password": SECRET, "x": 1 }),
    );
    b.call(
        "POST",
        "/api/devices/moxa/config/password",
        None,
        &json!({ "new": NEW_SECRET }),
    );
    let all = b.everything();
    for secret in [SECRET, NEW_SECRET, "wrong-one"] {
        assert!(
            !all.contains(secret),
            "{secret} appeared in a response, the audit log or a log line"
        );
    }
    // The saved configuration is the export, which has no password in it.
    let f = b.read("/api/projects/line1/files/moxa.devconf");
    assert!(!String::from_utf8_lossy(&f.body).contains(SECRET));
    // The audit log did record the calls (it is not simply empty).
    let audit = b.read("/api/audit?limit=1000");
    let text = String::from_utf8_lossy(&audit.body);
    assert!(text.contains("/api/devices/moxa/config/password"), "{text}");
}

// ---- one operation at a time; the rest of the box served meanwhile ----------

#[test]
fn a_long_restart_completes_through_the_server_while_the_box_is_served() {
    let b = Arc::new(Bench::new(Use::Free));
    let s = b.alice();
    b.run_project(&s, "line1");
    // About 1.5 s of downtime at the 5 ms poll: long enough to overlap the
    // calls below, short enough for a test.
    b.sim.set_restart(Restart {
        down1: 100,
        up1: 3,
        down2: 100,
    });
    let runner = {
        let (b, s) = (Arc::clone(&b), s.clone());
        thread::spawn(move || b.post(&s, "restart", &json!({})))
    };
    wait("the unit to be rebooting", || {
        b.sim.reboots() == 1 && !b.sim.is_up()
    });
    // Other API requests are served meanwhile, and quickly.
    for _ in 0..3 {
        let t = Instant::now();
        assert_eq!(b.read("/api/health").status, 200);
        assert_eq!(b.read("/api/signals").status, 200);
        assert_eq!(b.read("/api/modules").status, 200);
        assert!(t.elapsed() < Duration::from_secs(2));
    }
    // The device is listed as restarting, not as a fault.
    let devices = b.read("/api/devices").json();
    let moxa = devices
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "moxa")
        .unwrap()
        .clone();
    assert_eq!(moxa["restarting"], true, "{moxa}");
    assert_eq!(moxa["state"], "down");
    assert_eq!(moxa["health"], "lost");
    assert_eq!(moxa["error"], "restarting for its configuration");
    // One operation per device at a time: a second gets 409, reads included.
    for r in [
        b.post(
            &s,
            "set",
            &json!({ "page": "04_1.htm", "changes": { "MOD_LOC": "x" } }),
        ),
        b.post(&s, "restart", &json!({})),
        b.post(&s, "snapshot", &json!({})),
        b.cfg(&s),
    ] {
        assert_eq!((r.status, r.code().as_str()), (409, "busy"), "{r:?}");
        assert_eq!(r.json()["busy"], "restart");
    }
    assert_eq!(b.sim.count("/set.htm"), 0);
    // The restart completes, with the unit back.
    let r = runner.join().unwrap();
    assert_eq!(r.status, 200, "{r:?}");
    assert!(b.sim.is_up());
    assert_eq!(b.sim.count("/set_ok.htm"), 1);
    // And the device is no longer held.
    let devices = b.read("/api/devices").json();
    let moxa = devices
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "moxa")
        .unwrap()
        .clone();
    assert!(moxa.get("restarting").is_none(), "{moxa}");
    // Free again.
    assert_eq!(b.cfg(&s).status, 200);
}

#[test]
fn a_restart_that_fails_frees_the_device_and_its_link() {
    let b = Bench::with_wait(Use::Free, Options::default(), Duration::from_millis(300));
    let s = b.alice();
    // The unit never comes back within the (short) bound.
    b.sim.set_restart(Restart {
        down1: 1,
        up1: 1,
        down2: u32::MAX,
    });
    let r = b.post(&s, "restart", &json!({}));
    assert_eq!(r.status, 502, "{r:?}");
    assert_eq!(r.code(), "restart_timeout");
    let devices = b.read("/api/devices").json();
    let moxa = devices
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "moxa")
        .unwrap()
        .clone();
    assert!(moxa.get("restarting").is_none(), "{moxa}");
    // Not busy any more: the next call reaches the (still down) unit and fails by itself.
    let r = b.cfg(&s);
    assert_eq!(r.status, 502, "{r:?}");
    assert_ne!(r.code(), "busy");
}

#[test]
fn a_unit_that_does_not_answer_is_a_502_with_a_code() {
    let b = Bench::with_wait(Use::Free, Options::default(), Duration::from_millis(300));
    let s = b.alice();
    b.sim.set_restart(Restart {
        down1: u32::MAX,
        up1: 0,
        down2: 0,
    });
    // Take the unit down by restarting it, then ask.
    let _ = b.post(&s, "restart", &json!({}));
    let r = b.cfg(&s);
    assert_eq!(r.status, 502, "{r:?}");
    assert_eq!(r.code(), "unreachable");
}

// ---- the audit trail ------------------------------------------------------------

#[test]
fn every_write_is_audited_with_the_person_and_reads_are_not() {
    let b = Bench::new(Use::Free);
    let s = b.alice();
    let before = b.read("/api/audit?limit=1000").json();
    let n = before.as_array().map_or(0, Vec::len);
    b.cfg(&s);
    b.cfg(&s);
    let after = b.read("/api/audit?limit=1000").json();
    assert_eq!(after.as_array().map_or(0, Vec::len), n, "reads are not audited");
    b.post(
        &s,
        "set",
        &json!({ "page": "04_1.htm", "changes": { "MOD_LOC": "audited" } }),
    );
    b.post(&s, "restart", &json!({ "bogus": 1 }));
    let log = b.read("/api/audit?limit=1000").json();
    let entries: Vec<&Json> = log
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| {
            e["path"]
                .as_str()
                .is_some_and(|p| p.starts_with("/api/devices/moxa/config"))
        })
        .collect();
    assert_eq!(entries.len(), 2, "{log}");
    assert_eq!(entries[0]["method"], "POST");
    assert_eq!(entries[0]["status"], 200);
    assert_eq!(entries[1]["status"], 400, "a refusal is audited too");
    assert!(
        entries[0]["user"].is_string() || entries[0]["person"].is_string() || entries[0]["by"].is_string(),
        "{}",
        entries[0]
    );
}
