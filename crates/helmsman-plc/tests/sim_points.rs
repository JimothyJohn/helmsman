//! Placeholder (`sim`) I/O points on the box (docs/FEATURES.md, "Sim
//! points"): the map line, `PUT /api/signals/{name}` and what the running
//! program sees, against the real binary on real sockets, with real
//! accounts, a real snapshot store and a project deployed through the
//! approval path. Nothing is mocked. Boxes listen on free ports and keep
//! their state in directories of their own, so copies of this suite can
//! run side by side.
#![allow(clippy::too_many_lines)] // each test walks one whole flow
#![allow(clippy::type_complexity)] // a table of (who, name, type, body, status, reason)

use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use helmsman_plc::Window;
use helmsman_plc::policy::Role;
use helmsman_plc::policy::account::Accounts;
use helmsman_plc::policy::pin::MIN_ITERATIONS;
use helmsman_plc::project::build::{self, Shape};
use helmsman_plc::project::{self, Files, Projects};
use helmsman_plc::spec::Defaults;
use helmsman_plc::store::{Meta, Store};
use serde_json::{Value as Json, json};

/// The box exits when its stdin closes: it dies with the test.
const EXIT_WITH_STDIN: &str = "HELMSMAN_EXIT_WITH_STDIN";
const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const ALICE: &str = "alice-engineer-pw";
const BOB: &str = "bob-operator-pw";

const PROGRAM: &str = "\
PROGRAM line
VAR_INPUT sw : BOOL; level : INT; rate : REAL; END_VAR
VAR_OUTPUT lamp : BOOL; total : INT; half : REAL; END_VAR
  lamp := sw;
  total := level * 2;
  half := rate / 2.0;
END_PROGRAM
";

/// `ext` is a device-side input (the box's own channel, in memory).
const MAP: &str = "\
# placeholders: no device is wired to these
in  sw    bool sim
in  level int  sim unit=mm
in  rate  real sim
out lamp  bool sim
out total int  sim unit=mm
out half  real sim
in  ext   bool box di:0
";

/// The same, plus a second sim input and output (a change a replay judges).
const PROGRAM_V2: &str = "\
PROGRAM line
VAR_INPUT sw : BOOL; level : INT; rate : REAL; mode : INT; END_VAR
VAR_OUTPUT lamp : BOOL; total : INT; half : REAL; busy : BOOL; END_VAR
  lamp := sw;
  total := level * 2;
  half := rate / 2.0;
  busy := mode > 0;
END_PROGRAM
";

const MAP_V2: &str = "\
in  sw    bool sim
in  level int  sim unit=mm
in  rate  real sim
in  mode  int  sim
out lamp  bool sim
out total int  sim unit=mm
out half  real sim
out busy  bool sim
in  ext   bool box di:0
";

fn tmp(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "sim-points-{name}-{}-{}",
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
    let _ = s.write_all(body);
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let split = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or(out.len());
    Resp {
        status: String::from_utf8_lossy(&out[..split])
            .get(9..12)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0),
        head: String::from_utf8_lossy(&out[..split]).into_owned(),
        body: out.get(split + 4..).unwrap_or_default().to_vec(),
    }
}

fn get(addr: SocketAddr, path: &str) -> Resp {
    send(addr, "GET", path, Some(TOKEN), "application/json", b"")
}

fn post(addr: SocketAddr, path: &str, bearer: &str, body: &Json) -> Resp {
    send(
        addr,
        "POST",
        path,
        Some(bearer),
        "application/json",
        body.to_string().as_bytes(),
    )
}

/// `PUT /api/signals/{name}` with `body` verbatim.
fn put(addr: SocketAddr, name: &str, bearer: Option<&str>, ctype: &str, body: &str) -> Resp {
    send(
        addr,
        "PUT",
        &format!("/api/signals/{name}"),
        bearer,
        ctype,
        body.as_bytes(),
    )
}

fn set(addr: SocketAddr, name: &str, bearer: &str, body: &Json) -> Resp {
    put(addr, name, Some(bearer), "application/json", &body.to_string())
}

struct Plc {
    child: Child,
    addr: SocketAddr,
    state: PathBuf,
}

impl Drop for Plc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn shape() -> Shape {
    Shape {
        defaults: Defaults {
            period: Duration::from_millis(10),
            i: Window::new(0, 32),
            q: Window::new(0, 32),
            m: 1024,
        },
        i_len: 32,
        q_len: 32,
    }
}

fn files(program: &str, map: &str) -> Files {
    Files(BTreeMap::from([
        ("main.st".to_string(), program.to_string()),
        ("io.map".to_string(), map.to_string()),
    ]))
}

/// A box on `--projects` with alice (engineer) and bob (operator), already
/// running project `sim` built from `PROGRAM` and `MAP`.
fn start() -> Plc {
    let state = tmp("state");
    std::fs::write(state.join("api-token"), TOKEN).unwrap();
    let mut accounts = Accounts::default();
    accounts
        .add("alice", Role::Engineer, ALICE, MIN_ITERATIONS, [1; 16])
        .unwrap();
    accounts
        .add("bob", Role::Operator, BOB, MIN_ITERATIONS, [2; 16])
        .unwrap();
    accounts.save(&state).unwrap();
    let f = files(PROGRAM, MAP);
    let built = build::build("sim", &f, None, &shape()).unwrap();
    let meta = Meta {
        author: "Test".into(),
        prompt: String::new(),
        approver: None,
        time_ms: 1,
    };
    let id = Store::open(&state.join("store"))
        .unwrap()
        .commit(&built.change, &meta)
        .unwrap();
    let projects = Projects::in_state_dir(&state).unwrap();
    projects.install("sim", &f, false).unwrap();
    projects
        .record(&project::Running {
            name: "sim".into(),
            snapshot: id.to_string(),
        })
        .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_helmsman-plc"))
        .args(["--opcua", "off", "--projects", "--https", "off"])
        .args(["--bind", "127.0.0.1", "--encap-port", "0", "--io-port", "0"])
        .args(["--api", "127.0.0.1:0", "--scan-ms", "2", "--state-dir"])
        .arg(&state)
        .env(EXIT_WITH_STDIN, "1")
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut addr = None;
    while addr.is_none() {
        let mut line = String::new();
        assert!(stderr.read_line(&mut line).unwrap() > 0, "PLC exited");
        addr = line
            .strip_prefix("helmsman-plc: API on http://")
            .and_then(|r| r.split('/').next())
            .and_then(|a| a.parse().ok());
    }
    thread::spawn(move || {
        let mut sink = String::new();
        let _ = stderr.read_to_string(&mut sink);
    });
    let plc = Plc {
        child,
        addr: addr.unwrap(),
        state,
    };
    wait("the module running", || {
        get(plc.addr, "/api/modules").json()[0]["status"]["state"] == "running"
    });
    plc
}

fn login(addr: SocketAddr, user: &str, password: &str) -> String {
    let r = send(
        addr,
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

fn value(addr: SocketAddr, name: &str) -> Json {
    get(addr, &format!("/api/signals/{name}")).json()["value"].clone()
}

fn waits_for(addr: SocketAddr, name: &str, want: &Json) {
    wait(&format!("{name} = {want}"), || value(addr, name) == *want);
}

#[test]
fn a_sim_input_set_through_the_api_is_read_by_the_program_and_its_outputs_follow() {
    let plc = start();
    let a = plc.addr;
    let alice = login(a, "alice", ALICE);

    // Everything starts at off / zero, and the reads say which is which.
    waits_for(a, "lamp", &json!(false));
    let all = get(a, "/api/signals").json();
    let sim_of = |n: &str| all.as_array().unwrap().iter().find(|s| s["name"] == n).unwrap()["sim"].clone();
    for n in ["sw", "level", "rate", "lamp", "total", "half"] {
        assert_eq!(sim_of(n), json!(true), "{n}: {all}");
    }
    assert_eq!(sim_of("ext"), json!(false), "{all}");
    let one = get(a, "/api/signals/level").json();
    assert_eq!(
        (one["sim"].clone(), one["unit"].clone(), one["quality"].clone()),
        (json!(true), json!("mm"), json!("good"))
    );
    assert_eq!(one["binding"]["kind"], "sim", "{one}");
    assert_eq!(value(a, "sw"), json!(false), "a sim input is never missing");
    assert_eq!(value(a, "total"), json!(0));

    // An engineer session sets each kind; the program sees it at a later scan.
    let r = set(a, "sw", &alice, &json!({ "value": true }));
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(r.json(), json!({ "name": "sw", "value": true }));
    waits_for(a, "lamp", &json!(true));
    let r = set(a, "level", &alice, &json!({ "value": 21 }));
    assert_eq!(
        (r.status, r.json()),
        (200, json!({ "name": "level", "value": 21 }))
    );
    waits_for(a, "total", &json!(42));
    // A whole number is a fine real; the name matches in any case.
    let r = set(a, "RATE", &alice, &json!({ "value": 3 }));
    assert_eq!(
        (r.status, r.json()),
        (200, json!({ "name": "rate", "value": 3.0 })),
        "{r:?}"
    );
    waits_for(a, "half", &json!(1.5));
    let r = set(a, "rate", &alice, &json!({ "value": -3.5 }));
    assert_eq!(r.status, 200, "{r:?}");
    waits_for(a, "half", &json!(-1.75));
    // The box's own token sets one too, and the value stays set.
    let r = set(a, "sw", TOKEN, &json!({ "value": false }));
    assert_eq!(r.status, 200, "{r:?}");
    waits_for(a, "lamp", &json!(false));
    waits_for(a, "total", &json!(42));

    // Every set is in the audit log, with who made it.
    let audit = get(a, "/api/audit?limit=1000").json();
    let has = |needle: &str, role: Option<&str>| {
        audit.as_array().unwrap().iter().any(|e| {
            e["detail"].as_str().unwrap_or_default().contains(needle)
                && e["method"] == "PUT"
                && e["status"] == 200
                && role.is_none_or(|r| e["role"] == r)
        })
    };
    assert!(has("sim set: sw := true", Some("engineer")), "{audit}");
    assert!(has("sim set: level := 21", Some("engineer")), "{audit}");
    assert!(has("sim set: sw := false", None), "{audit}");

    // A stopped module zeroes its sim outputs like any output with no safe=;
    // its sim inputs keep what was set.
    set(a, "sw", &alice, &json!({ "value": true }));
    waits_for(a, "lamp", &json!(true));
    let r = post(a, "/api/modules/line/stop", &alice, &json!({}));
    assert_eq!(r.status, 200, "{r:?}");
    waits_for(a, "lamp", &json!(false));
    waits_for(a, "total", &json!(0));
    assert_eq!(value(a, "sw"), json!(true), "an input is memory");
    let r = post(a, "/api/modules/line/run", &alice, &json!({}));
    assert_eq!(r.status, 200, "{r:?}");
    waits_for(a, "lamp", &json!(true));
    waits_for(a, "total", &json!(42));
}

#[test]
fn a_refused_set_changes_nothing_and_says_why() {
    let plc = start();
    let a = plc.addr;
    let alice = login(a, "alice", ALICE);
    let bob = login(a, "bob", BOB);
    assert_eq!(set(a, "sw", &alice, &json!({ "value": true })).status, 200);
    assert_eq!(set(a, "level", &alice, &json!({ "value": 5 })).status, 200);
    assert_eq!(set(a, "rate", &alice, &json!({ "value": 2.5 })).status, 200);
    waits_for(a, "total", &json!(10));
    waits_for(a, "half", &json!(1.25));

    let j = "application/json";
    // (who, name, content type, body, status, a word the reason has)
    let cases: Vec<(Option<&str>, &str, &str, String, u16, &str)> = vec![
        (None, "sw", j, r#"{"value":false}"#.into(), 401, "Authorization"),
        (
            Some("f".repeat(64).leak()),
            "sw",
            j,
            r#"{"value":false}"#.into(),
            401,
            "token",
        ),
        (
            Some(bob.as_str()),
            "sw",
            j,
            r#"{"value":false}"#.into(),
            403,
            "engineer",
        ),
        (
            Some(bob.as_str()),
            "nosuch",
            j,
            r#"{"value":false}"#.into(),
            403,
            "engineer",
        ),
        (
            Some(alice.as_str()),
            "nosuch",
            j,
            r#"{"value":false}"#.into(),
            404,
            "no such signal",
        ),
        (
            Some(alice.as_str()),
            "ext",
            j,
            r#"{"value":false}"#.into(),
            409,
            "only a sim input",
        ),
        (
            Some(alice.as_str()),
            "lamp",
            j,
            r#"{"value":false}"#.into(),
            409,
            "only a sim input",
        ),
        (
            Some(alice.as_str()),
            "total",
            j,
            r#"{"value":1}"#.into(),
            409,
            "only a sim input",
        ),
        (
            Some(TOKEN),
            "half",
            j,
            r#"{"value":1.5}"#.into(),
            409,
            "only a sim input",
        ),
        // The kind of value.
        (
            Some(alice.as_str()),
            "sw",
            j,
            r#"{"value":1}"#.into(),
            400,
            "true or false",
        ),
        (
            Some(alice.as_str()),
            "sw",
            j,
            r#"{"value":"true"}"#.into(),
            400,
            "true or false",
        ),
        (
            Some(alice.as_str()),
            "sw",
            j,
            r#"{"value":null}"#.into(),
            400,
            "true or false",
        ),
        (
            Some(alice.as_str()),
            "level",
            j,
            r#"{"value":true}"#.into(),
            400,
            "integer",
        ),
        (
            Some(alice.as_str()),
            "level",
            j,
            r#"{"value":1.5}"#.into(),
            400,
            "integer",
        ),
        (
            Some(alice.as_str()),
            "level",
            j,
            r#"{"value":"7"}"#.into(),
            400,
            "integer",
        ),
        (
            Some(alice.as_str()),
            "level",
            j,
            r#"{"value":[7]}"#.into(),
            400,
            "integer",
        ),
        (
            Some(alice.as_str()),
            "rate",
            j,
            r#"{"value":true}"#.into(),
            400,
            "number",
        ),
        (
            Some(alice.as_str()),
            "rate",
            j,
            r#"{"value":"NaN"}"#.into(),
            400,
            "number",
        ),
        (
            Some(alice.as_str()),
            "rate",
            j,
            r#"{"value":"inf"}"#.into(),
            400,
            "number",
        ),
        (
            Some(alice.as_str()),
            "rate",
            j,
            r#"{"value":null}"#.into(),
            400,
            "number",
        ),
        // Out of range: for 64 bits, for the INT port that reads it, for a REAL port.
        (
            Some(alice.as_str()),
            "level",
            j,
            r#"{"value":9223372036854775808}"#.into(),
            400,
            "64 bits",
        ),
        (
            Some(alice.as_str()),
            "level",
            j,
            r#"{"value":-9223372036854775809}"#.into(),
            400,
            "64 bits",
        ),
        (
            Some(alice.as_str()),
            "level",
            j,
            r#"{"value":40000}"#.into(),
            400,
            "out of range",
        ),
        (
            Some(alice.as_str()),
            "level",
            j,
            r#"{"value":-32769}"#.into(),
            400,
            "out of range",
        ),
        (
            Some(alice.as_str()),
            "rate",
            j,
            r#"{"value":1e300}"#.into(),
            400,
            "out of range",
        ),
        // Numbers JSON cannot say, past what f64 holds.
        (
            Some(alice.as_str()),
            "rate",
            j,
            r#"{"value":1e999}"#.into(),
            400,
            "value",
        ),
        (
            Some(alice.as_str()),
            "rate",
            j,
            format!(r#"{{"value":1{}}}"#, "0".repeat(400)),
            400,
            "value",
        ),
        // The shape of the body.
        (
            Some(alice.as_str()),
            "sw",
            j,
            r#"{"value":true,"extra":1}"#.into(),
            400,
            "exactly",
        ),
        (
            Some(alice.as_str()),
            "sw",
            j,
            r#"{"value":true,"value":false,"x":0}"#.into(),
            400,
            "exactly",
        ),
        (Some(alice.as_str()), "sw", j, r"{}".into(), 400, "exactly"),
        (
            Some(alice.as_str()),
            "sw",
            j,
            r#"{"Value":true}"#.into(),
            400,
            "exactly",
        ),
        (Some(alice.as_str()), "sw", j, r"[true]".into(), 400, "exactly"),
        (Some(alice.as_str()), "sw", j, "true".into(), 400, "exactly"),
        (Some(alice.as_str()), "sw", j, "value=true".into(), 400, "exactly"),
        (
            Some(alice.as_str()),
            "sw",
            j,
            "{\"value\":tru".into(),
            400,
            "exactly",
        ),
        (Some(alice.as_str()), "sw", j, String::new(), 400, "exactly"),
        (
            Some(alice.as_str()),
            "sw",
            "text/plain",
            r#"{"value":true}"#.into(),
            415,
            "application/json",
        ),
    ];
    for (who, name, ctype, body, status, why) in cases {
        let r = put(a, name, who, ctype, &body);
        let shown = body.chars().take(60).collect::<String>();
        assert_eq!(r.status, status, "{name} {shown}: {r:?}");
        let said = r.error();
        assert!(said.contains(why), "{name} {shown}: wanted {why:?} in {said:?}");
    }
    // The route's other methods say what it takes.
    let r = send(a, "POST", "/api/signals/sw", Some(&alice), j, b"{}");
    assert_eq!(r.status, 405, "{r:?}");
    assert!(r.head.contains("Allow: GET, PUT"), "{}", r.head);
    // A force cannot pin a sim input: setting it is the call for that.
    let r = post(
        a,
        "/api/forces",
        &alice,
        &json!({ "signal": "sw", "value": false }),
    );
    assert_eq!(r.status, 409, "{r:?}");
    assert!(r.error().contains("PUT /api/signals/sw"), "{}", r.error());

    // None of it moved anything: the values are what the last good set left.
    thread::sleep(Duration::from_millis(100));
    assert_eq!(value(a, "sw"), json!(true));
    assert_eq!(value(a, "level"), json!(5));
    assert_eq!(value(a, "rate"), json!(2.5));
    assert_eq!(value(a, "total"), json!(10));
    assert_eq!(value(a, "half"), json!(1.25));
    // Refusals are in the audit log too.
    let audit = get(a, "/api/audit?limit=1000").json();
    let refused = |status: u64| {
        audit.as_array().unwrap().iter().any(|e| {
            e["method"] == "PUT"
                && e["status"] == status
                && e["detail"]
                    .as_str()
                    .unwrap_or_default()
                    .starts_with("sim set: refused")
        })
    };
    assert!(
        refused(403) && refused(404) && refused(409) && refused(400),
        "{audit}"
    );
    // The module never faulted on any of it.
    assert_eq!(get(a, "/api/modules/line").json()["status"]["state"], "running");
}

#[test]
fn a_map_that_gives_a_sim_point_a_device_or_an_option_is_refused_with_its_line() {
    let prog = tmp("prog").join("p.st");
    std::fs::write(
        &prog,
        "PROGRAM p VAR_INPUT start : BOOL; END_VAR VAR_OUTPUT lamp : BOOL; END_VAR lamp := start; END_PROGRAM",
    )
    .unwrap();
    for (text, want) in [
        (
            "in start bool sim\nout lamp bool sim poll_ms=5\n",
            "sim.map:2: poll_ms",
        ),
        (
            "in start bool sim wired=ifm/if6123\nout lamp bool sim\n",
            "sim.map:1: wired",
        ),
        (
            "in start bool sim\nout lamp bool sim safe=true\n",
            "sim.map:2: safe",
        ),
        (
            "in start bool sim di:0\nout lamp bool sim\n",
            "sim.map:1: \"di:0\"",
        ),
        (
            "device sim modbus-tcp 127.0.0.1:502\nin start bool sim\nout lamp bool sim\n",
            "sim.map:1: device id",
        ),
    ] {
        let map = tmp("map").join("sim.map");
        std::fs::write(&map, text).unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_helmsman-plc"))
            .args(["--opcua", "off", "--api", "127.0.0.1:0"])
            .arg(&prog)
            .arg("--io-map")
            .arg(&map)
            .args(["--encap-port", "0", "--io-port", "0"])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{text}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(want), "{text}: wanted {want:?} in {err}");
        assert!(
            !err.contains("CIP on"),
            "refused before going on the network: {err}"
        );
    }
}

#[test]
fn a_box_runs_a_map_of_sim_points_alone_and_a_project_check_reads_the_lines() {
    let plc = start();
    let a = plc.addr;
    let alice = login(a, "alice", ALICE);
    let r = post(a, "/api/projects", &alice, &json!({ "name": "bad" }));
    assert_eq!(r.status, 201, "{r:?}");
    let save = |f: &str, text: &str| {
        send(
            a,
            "PUT",
            &format!("/api/projects/bad/files/{f}"),
            Some(&alice),
            "application/json",
            json!({ "text": text }).to_string().as_bytes(),
        )
    };
    assert_eq!(save("main.st", PROGRAM).status, 200);
    let bad = "in sw bool sim\nin level int sim poll_ms=5\nin rate real sim\n\
               out lamp bool sim\nout total int sim\nout half real sim\n";
    assert_eq!(save("io.map", bad).status, 200);
    let r = post(a, "/api/projects/bad/check", &alice, &json!({}));
    let text = String::from_utf8_lossy(&r.body).into_owned();
    assert!(
        text.contains("io.map") && text.contains("poll_ms"),
        "{} {text}",
        r.status
    );
    assert!(text.contains(r#""line":2"#), "{text}");
    // The same project with the line fixed checks clean.
    assert_eq!(
        save("io.map", &MAP.replace("in  ext   bool box di:0\n", "")).status,
        200
    );
    let r = post(a, "/api/projects/bad/check", &alice, &json!({}));
    assert_eq!(r.status, 200, "{r:?}");
    assert!(!String::from_utf8_lossy(&r.body).contains("poll_ms"), "{r:?}");
}

#[test]
fn a_project_with_sim_ports_replays_and_deploys_and_keeps_the_values_set() {
    let plc = start();
    let a = plc.addr;
    let alice = login(a, "alice", ALICE);
    assert_eq!(set(a, "sw", &alice, &json!({ "value": true })).status, 200);
    assert_eq!(set(a, "level", &alice, &json!({ "value": 4 })).status, 200);
    waits_for(a, "total", &json!(8));

    let r = post(a, "/api/projects", &alice, &json!({ "name": "sim2" }));
    assert_eq!(r.status, 201, "{r:?}");
    for (f, text) in [("main.st", PROGRAM_V2), ("io.map", MAP_V2)] {
        let r = send(
            a,
            "PUT",
            &format!("/api/projects/sim2/files/{f}"),
            Some(&alice),
            "application/json",
            json!({ "text": text }).to_string().as_bytes(),
        );
        assert_eq!(r.status, 200, "{r:?}");
    }
    // The replay needs a recorded window of the running box; ask until it has one.
    let mut replay = None;
    wait("a replay of the new project", || {
        let r = post(a, "/api/projects/sim2/replay", &alice, &json!({}));
        if r.status == 201 {
            replay = r.json()["replay"].as_u64();
            true
        } else {
            assert_eq!(r.status, 409, "{r:?}");
            false
        }
    });
    let r = post(
        a,
        "/api/projects/sim2/deploy",
        &alice,
        &json!({ "replay": replay.unwrap() }),
    );
    assert!(r.status == 200 || r.status == 201, "{r:?}");

    // The new program runs against the new map; what was set is still set.
    wait("the new sim point", || get(a, "/api/signals/mode").status == 200);
    assert_eq!(get(a, "/api/signals/mode").json()["sim"], true);
    assert_eq!(value(a, "sw"), json!(true), "a deploy keeps a sim input");
    assert_eq!(value(a, "level"), json!(4));
    waits_for(a, "lamp", &json!(true));
    waits_for(a, "total", &json!(8));
    waits_for(a, "busy", &json!(false));
    assert_eq!(set(a, "mode", &alice, &json!({ "value": 3 })).status, 200);
    waits_for(a, "busy", &json!(true));
    // The deploy is the box's, on the box's disk: a project, not a leftover.
    assert!(Path::new(&plc.state).join("projects").join("sim2").exists());
}
