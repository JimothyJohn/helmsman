//! WASM drivers (`helmsman_io::wasm`): the Modbus/TCP guest against the
//! native driver on the same simulator and the same hostile peers, and
//! hand-written hostile guests against the sandbox.

use std::collections::BTreeMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpListener};
use std::thread;
use std::time::{Duration, Instant};

use helmsman_io::harness::{self, Bench, Outcome};
use helmsman_io::modbus::{self, ModbusDriver, Point};
use helmsman_io::sim::{Sim, State};
use helmsman_io::wasm::{self, Limits, WasmDriver};
use helmsman_io::{Device, Driver, Error, Value};
use proptest::prelude::*;

/// The guest, built by scripts/build-wasm-drivers.sh from guests/modbus-tcp.
const MODBUS: &[u8] = include_bytes!("../guests/modbus-tcp.wasm");

fn device(endpoint: &str, params: &[(&str, &str)]) -> Device {
    Device {
        id: "rio".into(),
        driver: "modbus-tcp".into(),
        endpoint: endpoint.into(),
        params: params
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect(),
    }
}

fn guest(endpoint: &str, timeout_ms: &str) -> WasmDriver {
    WasmDriver::load(
        MODBUS,
        &device(endpoint, &[("timeout_ms", timeout_ms)]),
        Limits::default(),
    )
    .unwrap()
}

fn native(endpoint: &str, timeout_ms: &str) -> ModbusDriver {
    let params = BTreeMap::from([("timeout_ms".to_string(), timeout_ms.to_string())]);
    ModbusDriver::from_params(endpoint, &params).unwrap()
}

fn wat_driver(src: &str, endpoint: &str) -> Result<WasmDriver, Error> {
    let wasm = wat::parse_str(src).unwrap();
    WasmDriver::load(
        &wasm,
        &device(endpoint, &[("timeout_ms", "300")]),
        Limits::default(),
    )
}

/// An endpoint nothing listens on.
fn nowhere() -> String {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .to_string()
}

/// What a call came to, comparable across drivers: values, or the
/// error's kind (and the exception code, and a type error's types).
#[derive(Debug)]
enum Out {
    Values(Vec<Value>),
    Done,
    Failed(String),
}

/// As `==`, but a NaN read equals a NaN read: a register pair holding a
/// NaN pattern reads as NaN on both (the interpreter canonicalises its
/// payload).
impl PartialEq for Out {
    fn eq(&self, other: &Self) -> bool {
        let same = |a: &Value, b: &Value| match (a, b) {
            (Value::Real(x), Value::Real(y)) if x.is_nan() && y.is_nan() => true,
            _ => a == b,
        };
        match (self, other) {
            (Self::Values(a), Self::Values(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same(x, y))
            }
            (Self::Done, Self::Done) => true,
            (Self::Failed(a), Self::Failed(b)) => a == b,
            _ => false,
        }
    }
}

fn kind(e: &Error) -> String {
    match e {
        Error::Address(_) => "address".into(),
        Error::Type { want, got } => format!("type {want:?} {got:?}"),
        Error::Io(_) => "io".into(),
        Error::Protocol(_) => "protocol".into(),
        Error::Exception(c) => format!("exception {c}"),
        Error::ReadOnly(_) => "read-only".into(),
        Error::Point(_) => "point".into(),
        Error::UnknownDriver(_) => "unknown driver".into(),
        Error::Driver(m) => format!("driver {m}"),
    }
}

fn read(d: &mut dyn Driver, addrs: &[&str]) -> Out {
    d.read(addrs).map_or_else(|e| Out::Failed(kind(&e)), Out::Values)
}

fn write(d: &mut dyn Driver, addr: &str, v: Value) -> Out {
    d.write(addr, v)
        .map_or_else(|e| Out::Failed(kind(&e)), |()| Out::Done)
}

// ---- the Modbus guest: the same harness, the same results ----

fn harness_sim() -> Sim {
    let mut s = State::new(64);
    s.wires = (0..8).map(|n| (n, n)).collect();
    s.input[0] = 4242;
    s.holding[20] = 0x3FC0; // 1.5f32, high word first
    Sim::start(s).unwrap()
}

/// The bench tests/harness.rs judges the native driver by.
fn harness_bench() -> Bench {
    Bench {
        points: vec!["di:0".into(), "ir:0".into(), "coil:3".into(), "hr:4:i32".into()],
        expect: vec![
            ("ir:0".into(), Value::Int(4242)),
            ("hr:20:f32".into(), Value::Real(1.5)),
        ],
        loops: vec![("coil:0".into(), "di:0".into()), ("coil:5".into(), "di:5".into())],
        registers: vec![
            "hr:0".into(),
            "hr:1:i16".into(),
            "hr:2:u32sw".into(),
            "hr:10:f32".into(),
        ],
        absent: Some("hr:9999".into()),
        settle: Duration::from_millis(200),
    }
}

#[test]
fn the_modbus_guest_passes_the_harness_the_native_driver_passes() {
    let s = harness_sim();
    let checks = harness::run(&mut guest(&s.endpoint(), "200"), &harness_bench(), Some(&s));
    assert!(
        checks.iter().all(|c| c.outcome == Outcome::Pass),
        "{}",
        harness::render(&checks)
    );
    let n = harness_sim();
    let native_checks = harness::run(&mut native(&n.endpoint(), "200"), &harness_bench(), Some(&n));
    let names = |cs: &[harness::Check]| cs.iter().map(|c| c.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&checks), names(&native_checks), "the same checks ran");
}

/// Runs `ops` on the native driver and the guest, each on its own copy of
/// `state`, and returns both drivers' outcomes and both devices' tables.
fn both(state: &State, ops: &[(&str, Option<Value>)]) -> [(Vec<Out>, State); 2] {
    let run = |d: &mut dyn Driver, s: &Sim| {
        let outs = ops
            .iter()
            .map(|(a, v)| match v {
                Some(v) => write(d, a, *v),
                None => read(d, &a.split(',').collect::<Vec<_>>()),
            })
            .collect();
        let mut end = s.state().clone();
        end.faults.clear();
        (outs, end)
    };
    let (sn, sg) = (
        Sim::start(state.clone()).unwrap(),
        Sim::start(state.clone()).unwrap(),
    );
    let n = run(&mut native(&sn.endpoint(), "2000"), &sn);
    let g = run(&mut guest(&sg.endpoint(), "2000"), &sg);
    [n, g]
}

#[test]
fn the_guest_and_the_native_driver_agree_on_every_table_format_and_refusal() {
    let mut st = State::new(32);
    st.discrete[3] = true;
    st.input[2] = 0xFFFF;
    st.input[4] = 0x8000;
    st.input[5] = 0x0001;
    st.holding[20] = 0x3FC0;
    let wr = |a: &'static str, v: Value| (a, Some(v));
    let rd = |a: &'static str| (a, None);
    let ops = [
        rd("di:3,di:4,ir:2,ir:2:i16"),
        rd("ir:4:u32,ir:4:u32sw,ir:4:i32,ir:4:i32sw,hr:20:f32,hr:20:f32sw"),
        wr("coil:9", Value::Bool(true)),
        wr("coil:9", Value::Bool(false)),
        wr("hr:0:i32", Value::Int(-2)),
        wr("hr:4:f32sw", Value::Real(-0.75)),
        wr("hr:6:u32", Value::Int(0xDEAD_BEEF)),
        wr("hr:8:i16", Value::Int(-12345)),
        wr("hr:9", Value::Int(65535)),
        rd("coil:9,hr:0:i32,hr:4:f32sw,hr:6:u32,hr:8:i16,hr:9"),
        // Refusals that never reach the device.
        wr("di:0", Value::Bool(true)),
        wr("ir:0", Value::Int(1)),
        wr("coil:0", Value::Int(1)),
        wr("hr:0", Value::Int(70000)),
        wr("hr:0:i16", Value::Int(-40000)),
        wr("hr:0", Value::Real(1.0)),
        wr("hr:0:f32", Value::Int(1)),
        wr("hr:0:u32", Value::Int(-1)),
        wr("hr:x", Value::Int(1)),
        rd("hr:1,nope"),
        // Exceptions from the device.
        rd("hr:32"),
        rd("hr:31:u32"),
        rd("coil:40"),
        wr("coil:40", Value::Bool(true)),
        wr("hr:31:i32", Value::Int(1)),
        rd("hr:1"),
    ];
    let [n, g] = both(&st, &ops);
    for (k, (a, b)) in n.0.iter().zip(&g.0).enumerate() {
        assert_eq!(a, b, "op {k} {:?}", ops[k]);
    }
    assert_eq!(n.1, g.1, "the devices end the same, requests counted");
    assert!(matches!(&n.0[1], Out::Values(v) if v.len() == 6));
}

#[test]
fn configure_hands_the_guest_its_unit_and_refuses_one_that_does_not_parse() {
    // A peer that answers only unit 7, and records which unit asked.
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    thread::spawn(move || {
        for conn in l.incoming() {
            let Ok(mut c) = conn else { return };
            let mut head = [0u8; 12];
            while c.read_exact(&mut head).is_ok() {
                // A one-register read; answer 0x0102 as the unit asked.
                let mut out = head[..4].to_vec();
                out.extend([0, 5, head[6], 0x03, 2, 1, 2]);
                if c.write_all(&out).is_err() {
                    break;
                }
            }
        }
    });
    let mut d = WasmDriver::load(
        MODBUS,
        &device(&addr, &[("unit", "+7"), ("timeout_ms", "2000")]),
        Limits::default(),
    )
    .unwrap();
    assert_eq!(d.read(&["hr:0"]).unwrap(), [Value::Int(0x0102)]);
    for bad in ["256", "-1", "", "7x", "+"] {
        let e = WasmDriver::load(MODBUS, &device(&addr, &[("unit", bad)]), Limits::default()).unwrap_err();
        assert!(
            matches!(&e, Error::Address(a) if a == "unit"),
            "unit={bad:?}: {e}"
        );
        assert!(
            matches!(
                ModbusDriver::from_params(&addr, &BTreeMap::from([("unit".into(), bad.into())])),
                Err(Error::Address(_))
            ),
            "the native driver refuses unit={bad:?} too"
        );
    }
    let e = WasmDriver::load(
        MODBUS,
        &device(&addr, &[("timeout_ms", "soon")]),
        Limits::default(),
    )
    .unwrap_err();
    assert!(matches!(&e, Error::Address(a) if a == "timeout_ms"), "{e}");
}

/// A server that answers each request with `answer(request)` and then
/// closes, so a short reply ends in EOF rather than a time-out.
fn hostile(answer: impl Fn(&[u8]) -> Vec<u8> + Send + 'static) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    thread::spawn(move || {
        for conn in l.incoming() {
            let Ok(mut conn) = conn else { return };
            let mut head = [0u8; 7];
            if conn.read_exact(&mut head).is_err() {
                continue;
            }
            let len = usize::from(u16::from_be_bytes([head[4], head[5]]));
            let mut pdu = vec![0u8; len.saturating_sub(1)];
            if conn.read_exact(&mut pdu).is_err() {
                continue;
            }
            let mut req = head.to_vec();
            req.extend(pdu);
            let _ = conn.write_all(&answer(&req));
            let _ = conn.shutdown(Shutdown::Write);
            // Drain until the client hangs up, so it sees our bytes, then EOF.
            let mut sink = [0u8; 64];
            while matches!(conn.read(&mut sink), Ok(n) if n > 0) {}
        }
    });
    addr
}

/// A reply frame to `req`: its transaction id, protocol 0, `pdu`.
fn frame(req: &[u8], pdu: &[u8]) -> Vec<u8> {
    let mut out = req[..4].to_vec();
    out.extend(u16::try_from(pdu.len() + 1).unwrap().to_be_bytes());
    out.push(req[6]);
    out.extend(pdu);
    out
}

fn same_on_both(addr: &str, addrs: &[&str]) -> Out {
    let n = read(&mut native(addr, "2000"), addrs);
    let g = read(&mut guest(addr, "2000"), addrs);
    assert_eq!(n, g, "native vs guest on {addrs:?}");
    g
}

#[test]
fn malformed_replies_fail_the_guest_as_they_fail_the_native_driver() {
    type Answer = fn(&[u8]) -> Vec<u8>;
    let cases: [(&str, Answer, &str); 12] = [
        ("empty pdu", |r| frame(r, &[]), "protocol"),
        ("another function", |r| frame(r, &[0x04, 2, 0, 1]), "protocol"),
        ("exception", |r| frame(r, &[0x83, 2]), "exception 2"),
        ("long exception", |r| frame(r, &[0x83, 2, 9]), "protocol"),
        ("byte count lies", |r| frame(r, &[0x03, 4, 0, 1]), "protocol"),
        ("short data", |r| frame(r, &[0x03, 2, 0]), "protocol"),
        ("extra data", |r| frame(r, &[0x03, 2, 0, 1, 9]), "protocol"),
        (
            "wrong transaction",
            |r| {
                let mut f = frame(r, &[0x03, 2, 0, 1]);
                f[1] ^= 0xFF;
                f
            },
            "protocol",
        ),
        (
            "wrong unit",
            |r| {
                let mut f = frame(r, &[0x03, 2, 0, 1]);
                f[6] ^= 1;
                f
            },
            "protocol",
        ),
        (
            "length field 300",
            |r| {
                let mut f = frame(r, &[0x03, 2, 0, 1]);
                f[4..6].copy_from_slice(&300u16.to_be_bytes());
                f
            },
            "protocol",
        ),
        ("truncated", |r| frame(r, &[0x03, 2, 0, 1])[..9].to_vec(), "io"),
        ("silence", |_| Vec::new(), "io"),
    ];
    for (name, answer, want) in cases {
        let addr = hostile(answer);
        assert_eq!(same_on_both(&addr, &["hr:0"]), Out::Failed(want.into()), "{name}");
    }
    let ok = hostile(|r| frame(r, &[0x03, 2, 0, 1]));
    assert_eq!(same_on_both(&ok, &["hr:0"]), Out::Values(vec![Value::Int(1)]));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Whatever bytes come back, the guest ends as the native driver does.
    #[test]
    fn hostile_replies_end_the_same_on_both(junk in prop::collection::vec(any::<u8>(), 0..64), echo_header in any::<bool>(), table in 0usize..4) {
        let addr = hostile(move |req| {
            let mut out = Vec::new();
            if echo_header {
                out.extend(&req[..4]);
                out.extend(u16::try_from(junk.len() + 1).unwrap().to_be_bytes());
                out.push(req[6]);
            }
            out.extend(&junk);
            out
        });
        let a = ["hr:0:f32", "ir:3", "coil:1", "di:9"][table];
        let t = Instant::now();
        same_on_both(&addr, &[a]);
        prop_assert!(t.elapsed() < Duration::from_secs(5));
    }

    /// Any address parses, or fails, the same on both.
    #[test]
    fn addresses_parse_the_same_on_both(s in "(coil|di|hr|ir|HR|x)?:?[0-9+ x-]{0,7}(:(u16|i16|u32|u32sw|i32|i32sw|f32|f32sw|f64|))?:?", t in "\\PC{0,12}") {
        let g = guest(&nowhere(), "300");
        for a in [s.as_str(), t.as_str()] {
            let n = Point::parse(a).map(Point::info);
            match (n, g.parse(a)) {
                (Ok(x), Ok(y)) => prop_assert_eq!(x, y, "{:?}", a),
                (Err(Error::Address(_)), Err(Error::Address(_))) => {}
                (x, y) => prop_assert!(false, "{a:?}: native {x:?}, guest {y:?}"),
            }
        }
    }

    /// Random reads and writes, good and bad, end the same on both, and
    /// leave both devices the same.
    #[test]
    fn random_operations_end_the_same_on_both(ops in prop::collection::vec((0u8..6, 0u16..36, any::<i32>(), 0usize..9), 1..24)) {
        const FORMATS: [&str; 9] = ["", ":i16", ":u32", ":u32sw", ":i32", ":i32sw", ":f32", ":f32sw", ":u16"];
        let ops: Vec<(String, Option<Value>)> = ops
            .into_iter()
            .map(|(op, addr, x, f)| {
                let fmt = FORMATS[f];
                match op {
                    0 => (format!("coil:{addr}"), Some(Value::Bool(x % 2 == 0))),
                    1 => (format!("hr:{addr}{fmt}"), Some(Value::Int(i64::from(x) >> (x & 31)))),
                    2 => (format!("hr:{addr}{fmt}"), Some(Value::Real(f64::from(x) / 64.0))),
                    3 => (format!("coil:{addr},hr:{addr}{fmt},di:{addr}"), None),
                    4 => (format!("ir:{addr}{fmt},hr:{addr}"), None),
                    _ => (format!("di:{addr}"), Some(Value::Int(1))),
                }
            })
            .collect();
        let mut st = State::new(32);
        st.wires = (0..8).map(|n| (n, n + 8)).collect();
        st.input = (0..32).map(|n| 0x1111u16.wrapping_mul(n)).collect();
        let refs: Vec<(&str, Option<Value>)> = ops.iter().map(|(a, v)| (a.as_str(), *v)).collect();
        let [n, g] = both(&st, &refs);
        prop_assert_eq!(n, g);
    }
}

// ---- the sandbox ----

/// The imports a well-behaved test guest uses.
const IMPORTS: &str = r#"
  (import "helmsman-driver/v1" "send" (func $send (param i32 i32) (result i32)))
  (import "helmsman-driver/v1" "recv" (func $recv (param i32 i32) (result i32)))
  (import "helmsman-driver/v1" "point" (func $point (param i32 i32)))
  (import "helmsman-driver/v1" "value" (func $value (param i32 i64)))
  (import "helmsman-driver/v1" "fail" (func $fail (param i32 i32 i32 i32)))
"#;

/// A guest from `IMPORTS`, a memory and `body` (its exports); any of
/// `buffer`, `parse`, `read`, `write` that `body` leaves out is a stub
/// that answers sensibly.
fn guest_src(body: &str) -> String {
    let mut s = format!("(module {IMPORTS} (memory (export \"memory\") 1) {body}");
    if !body.contains("\"buffer\"") {
        s += r#"(func (export "buffer") (param i32) (result i32) (i32.const 1024))"#;
    }
    if !body.contains("\"parse\"") {
        s += r#"(func (export "parse") (param i32 i32) (call $point (i32.const 1) (i32.const 1)))"#;
    }
    if !body.contains("\"read\"") {
        s += r#"(func (export "read") (param i32 i32) (call $value (i32.const 1) (i64.const 7)))"#;
    }
    if !body.contains("\"write\"") {
        s += r#"(func (export "write") (param i32 i32 i32 i64))"#;
    }
    s + ")"
}

#[test]
fn a_stub_guest_loads_and_answers() {
    let mut d = wat_driver(&guest_src(""), &nowhere()).unwrap();
    assert_eq!(d.read(&["x"]).unwrap(), [Value::Int(7)]);
    assert!(d.parse("x").unwrap().writable);
    d.write("x", Value::Int(1)).unwrap();
}

#[test]
fn a_guest_that_asks_for_anything_but_its_socket_is_refused_at_load() {
    let cases = [
        (
            "connect to a host of its choosing",
            r#"(module (import "helmsman-driver/v1" "connect" (func (param i32 i32) (result i32))))"#,
            "\"connect\": not part of helmsman-driver/v1",
        ),
        (
            "WASI sockets",
            r#"(module (import "wasi_snapshot_preview1" "sock_open" (func (param i32 i32 i32) (result i32))))"#,
            "\"sock_open\": not part of",
        ),
        (
            "WASI files",
            r#"(module (import "wasi_snapshot_preview1" "path_open" (func (param i32 i32 i32 i32 i32 i64 i64 i32 i32) (result i32))))"#,
            "\"path_open\": not part of",
        ),
        (
            "a clock",
            r#"(module (import "helmsman/v1" "now_ms" (func (result i64))))"#,
            "\"now_ms\": not part of",
        ),
        (
            "a send that names the peer",
            r#"(module (import "helmsman-driver/v1" "send" (func (param i32 i32 i32 i32) (result i32))))"#,
            "\"send\": must be a function (i32, i32) -> (i32)",
        ),
        (
            "the host's memory",
            r#"(module (import "helmsman-driver/v1" "memory" (memory 1)))"#,
            "\"memory\": not part of",
        ),
        (
            "a table",
            r#"(module (import "env" "table" (table 1 funcref)))"#,
            "\"table\": not part of",
        ),
        (
            "a global",
            r#"(module (import "env" "g" (global i32)))"#,
            "\"g\": not part of",
        ),
    ];
    for (what, src, want) in cases {
        let e = wat_driver(src, &nowhere()).unwrap_err();
        assert!(matches!(&e, Error::Driver(m) if m.contains(want)), "{what}: {e}");
    }
    let e = wasm::check(&wat::parse_str(cases[0].1).unwrap()).unwrap_err();
    assert!(
        e.iter().any(|m| m.contains("no export \"parse\"")),
        "every problem is listed: {e:?}"
    );
    assert!(e.iter().any(|m| m.contains("no exported memory")), "{e:?}");
}

#[test]
fn a_guest_that_breaks_the_shape_of_the_interface_is_refused_at_load() {
    let cases = [
        ("not wasm", "\0asm junk".to_string(), "not a valid WASM module"),
        (
            "a start function",
            guest_src("(func $s) (start $s)"),
            "not a valid WASM module",
        ),
        (
            "parse of the wrong type",
            guest_src(r#"(func (export "parse") (param i32) (result i32) (i32.const 0))"#),
            "export \"parse\" must be a function (i32, i32) -> ()",
        ),
        (
            "more memory than the cap",
            guest_src("").replace(
                "(memory (export \"memory\") 1)",
                "(memory (export \"memory\") 17)",
            ),
            "does not instantiate",
        ),
    ];
    for (what, src, want) in cases {
        let wasm = if what == "not wasm" {
            src.into_bytes()
        } else {
            wat::parse_str(&src).unwrap()
        };
        let e = WasmDriver::load(&wasm, &device(&nowhere(), &[]), Limits::default()).unwrap_err();
        assert!(matches!(&e, Error::Driver(m) if m.contains(want)), "{what}: {e}");
    }
    let mut no_memory = guest_src("");
    no_memory = no_memory.replace("(memory (export \"memory\") 1)", "(memory 1)");
    let e = wat_driver(&no_memory, &nowhere()).unwrap_err();
    assert!(
        matches!(&e, Error::Driver(m) if m.contains("no exported memory")),
        "{e}"
    );
}

/// A listener that records whether anyone connected, without blocking.
fn watch() -> (TcpListener, String) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.set_nonblocking(true).unwrap();
    let addr = l.local_addr().unwrap().to_string();
    (l, addr)
}

fn connected(l: &TcpListener) -> Option<Vec<u8>> {
    match l.accept() {
        Ok((mut c, _)) => {
            c.set_nonblocking(false).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut got = vec![0u8; 5];
            c.read_exact(&mut got).unwrap();
            Some(got)
        }
        Err(e) if e.kind() == ErrorKind::WouldBlock => None,
        Err(e) => panic!("{e}"),
    }
}

#[test]
fn the_socket_reaches_the_device_only_and_never_from_parse() {
    // parse and write both try to send "hello"; connecting is synchronous
    // in send, so a connection either sits in the backlog when the call
    // returns or never happened.
    let src = guest_src(
        r#"(data (i32.const 64) "hello")
        (func (export "parse") (param i32 i32)
          (if (i32.ne (call $send (i32.const 64) (i32.const 5)) (i32.const -1)) (then unreachable))
          (call $point (i32.const 1) (i32.const 1)))
        (func (export "write") (param i32 i32 i32 i64)
          (if (i32.ne (call $send (i32.const 64) (i32.const 5)) (i32.const 0)) (then unreachable)))"#,
    );
    let (device, at) = watch();
    let (other, _) = watch();
    let mut d = wat_driver(&src, &at).unwrap();
    d.parse("x").unwrap();
    assert_eq!(connected(&device), None, "parse got no socket");
    d.write("x", Value::Int(1)).unwrap();
    assert_eq!(
        connected(&device).as_deref(),
        Some(&b"hello"[..]),
        "write reached the device"
    );
    assert_eq!(connected(&other), None);
}

/// A guest whose `read` answers 7 per address, but spins on an address
/// ending in `2` and grows its memory without end on one ending in `3`.
const TRAPS: &str = r#"
  (func (export "read") (param $p i32) (param $n i32)
    (local $end i32) (local $len i32) (local $last i32)
    (local.set $end (i32.add (local.get $p) (local.get $n)))
    (block $done
      (loop $next
        (br_if $done (i32.ge_u (local.get $p) (local.get $end)))
        (local.set $len (i32.load16_u (local.get $p)))
        (local.set $last (i32.load8_u (i32.add (local.get $p) (i32.add (local.get $len) (i32.const 1)))))
        (if (i32.eq (local.get $last) (i32.const 50)) (then (loop $spin (br $spin))))
        (if (i32.eq (local.get $last) (i32.const 51))
          (then (loop $bomb (drop (memory.grow (i32.const 1))) (br $bomb))))
        (call $value (i32.const 1) (i64.const 7))
        (local.set $p (i32.add (local.get $p) (i32.add (local.get $len) (i32.const 2))))
        (br $next))))
"#;

#[test]
fn an_endless_guest_runs_out_of_fuel_and_only_that_call_fails() {
    let mut d = wat_driver(&guest_src(TRAPS), &nowhere()).unwrap();
    let t = Instant::now();
    let e = d.read(&["hr:1", "hr:2"]).unwrap_err();
    assert!(matches!(&e, Error::Driver(m) if m.contains("out of fuel")), "{e}");
    assert!(!e.is_link(), "a point error: the link stays up");
    assert!(t.elapsed() < Duration::from_secs(5));
    assert_eq!(d.read(&["hr:1"]).unwrap(), [Value::Int(7)], "a fresh instance");
}

#[test]
fn a_memory_bomb_is_stopped_at_the_cap_and_only_that_call_fails() {
    for cap in [1 << 20, 4 << 20] {
        let limits = Limits {
            memory: cap,
            ..Limits::default()
        };
        let wasm = wat::parse_str(guest_src(TRAPS)).unwrap();
        let mut d = WasmDriver::load(&wasm, &device(&nowhere(), &[]), limits).unwrap();
        let e = d.read(&["hr:3"]).unwrap_err();
        assert!(
            matches!(&e, Error::Driver(m) if m.contains(&format!("over its memory cap ({cap} bytes)"))),
            "stopped by the cap, not fuel: {e}"
        );
        assert!(!e.is_link());
        assert_eq!(d.read(&["hr:1", "hr:11"]).unwrap(), [Value::Int(7); 2]);
    }
}

#[test]
fn a_guest_that_breaks_the_contract_fails_its_call_as_a_driver_error() {
    let cases = [
        (
            "buffer outside its memory",
            r#"(func (export "buffer") (param i32) (result i32) (i32.const 65535))"#,
        ),
        (
            "buffer at null",
            r#"(func (export "buffer") (param i32) (result i32) (i32.const 0))"#,
        ),
        (
            "send from outside its memory",
            r#"(func (export "read") (param i32 i32) (drop (call $send (i32.const 65530) (i32.const 64))))"#,
        ),
        (
            "recv into outside its memory",
            r#"(func (export "read") (param i32 i32) (drop (call $recv (i32.const -8) (i32.const 4))))"#,
        ),
        (
            "too many values",
            r#"(func (export "read") (param i32 i32)
                 (call $value (i32.const 1) (i64.const 1)) (call $value (i32.const 1) (i64.const 2)))"#,
        ),
        ("no value", r#"(func (export "read") (param i32 i32))"#),
        (
            "a BOOL of 2",
            r#"(func (export "read") (param i32 i32) (call $value (i32.const 0) (i64.const 2)))"#,
        ),
        (
            "a type of 9",
            r#"(func (export "read") (param i32 i32) (call $value (i32.const 9) (i64.const 2)))"#,
        ),
        (
            "an unknown fail kind",
            r#"(func (export "read") (param i32 i32) (call $fail (i32.const 99) (i32.const 0) (i32.const 0) (i32.const 0)))"#,
        ),
        (
            "an exception code past a byte",
            r#"(func (export "read") (param i32 i32) (call $fail (i32.const 4) (i32.const 300) (i32.const 0) (i32.const 0)))"#,
        ),
        ("a trap", r#"(func (export "read") (param i32 i32) unreachable)"#),
    ];
    for (what, body) in cases {
        let mut d = wat_driver(&guest_src(body), &nowhere()).unwrap();
        let e = d.read(&["x"]).unwrap_err();
        assert!(matches!(e, Error::Driver(_)), "{what}: {e}");
    }
    let d = wat_driver(
        &guest_src(r#"(func (export "parse") (param i32 i32))"#),
        &nowhere(),
    )
    .unwrap();
    assert!(
        matches!(d.parse("x"), Err(Error::Driver(_))),
        "parse answered nothing"
    );
    let src = guest_src(
        r#"(func (export "parse") (param i32 i32) (call $point (i32.const 1) (i32.const 1)) (call $point (i32.const 1) (i32.const 1)))"#,
    );
    assert!(matches!(
        wat_driver(&src, &nowhere()).unwrap().parse("x"),
        Err(Error::Driver(_))
    ));
    let src = guest_src(
        r#"(func (export "write") (param i32 i32 i32 i64) (call $value (i32.const 1) (i64.const 1)))"#,
    );
    assert!(
        matches!(
            wat_driver(&src, &nowhere()).unwrap().write("x", Value::Int(1)),
            Err(Error::Driver(_))
        ),
        "value outside read"
    );
    let mut d = wat_driver(&guest_src(""), &nowhere()).unwrap();
    assert!(
        matches!(d.read(&[&"x".repeat(300)]), Err(Error::Address(_))),
        "long address"
    );
}

#[test]
fn a_guest_that_reports_errors_gets_them_back_as_the_contract_says() {
    let body = |kind: i32, code: i32| {
        format!(
            r#"(data (i32.const 64) "why\0a\1b[31m")
            (func (export "read") (param i32 i32) (call $fail (i32.const {kind}) (i32.const {code}) (i32.const 64) (i32.const 9)))
            (func (export "write") (param i32 i32 i32 i64) (call $fail (i32.const {kind}) (i32.const {code}) (i32.const 64) (i32.const 0)))"#
        )
    };
    let read_err = |k, c| {
        let mut d = wat_driver(&guest_src(&body(k, c)), &nowhere()).unwrap();
        d.read(&["x"]).unwrap_err()
    };
    let write_err = |k, c| {
        let mut d = wat_driver(&guest_src(&body(k, c)), &nowhere()).unwrap();
        d.write("x", Value::Real(1.0)).unwrap_err()
    };
    assert!(
        matches!(read_err(1, 0), Error::Address(m) if m == "why  [31m"),
        "control characters blanked"
    );
    assert!(matches!(read_err(3, 0), Error::Protocol(_)));
    assert!(matches!(read_err(4, 2), Error::Exception(2)));
    assert!(matches!(read_err(6, 0), Error::Point(_)));
    assert!(matches!(read_err(7, 0), Error::Io(_)));
    assert!(
        matches!(write_err(5, 0), Error::ReadOnly(a) if a == "x"),
        "no message: the address"
    );
    assert!(
        matches!(
            write_err(2, 1),
            Error::Type {
                want: helmsman_io::Ty::Int,
                got: helmsman_io::Ty::Real
            }
        ),
        "the host knows what it wrote"
    );
    assert!(
        matches!(read_err(2, 1), Error::Driver(_)),
        "a type error on a read means nothing"
    );
}

#[test]
fn a_guest_that_waits_on_a_silent_device_is_cut_off_by_the_calls_deadline() {
    // The device accepts and never answers; the guest ignores every
    // failed recv and tries again, so only the host's deadline (and then
    // fuel) can end the call.
    let device = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = device.local_addr().unwrap().to_string();
    thread::spawn(move || {
        let mut held = Vec::new();
        for c in device.incoming().flatten() {
            held.push(c);
        }
    });
    let src = guest_src(
        r#"(func (export "read") (param i32 i32)
             (drop (call $send (i32.const 64) (i32.const 1)))
             (loop $again (drop (call $recv (i32.const 64) (i32.const 1))) (br $again)))"#,
    );
    let mut d = wat_driver(&src, &addr).unwrap();
    let started = Instant::now();
    let e = d.read(&["x"]).unwrap_err();
    assert!(matches!(&e, Error::Driver(m) if m.contains("fuel")), "{e}");
    // The call's deadline is two time-outs of 300 ms (one address, plus
    // one); after it every recv fails at once, and fuel ends the loop.
    // Without the deadline each recv would wait 300 ms, for ever.
    let took = started.elapsed();
    assert!(
        took >= Duration::from_millis(500),
        "it did wait for the device: {took:?}"
    );
    assert!(took < Duration::from_secs(5), "{took:?}");
}

#[test]
fn modbus_constants_the_guest_mirrors_have_not_moved() {
    // The guest copies these from helmsman_io::modbus; if they change
    // there, the guest's source must follow (and be rebuilt).
    assert_eq!((modbus::MAX_PDU, modbus::MBAP_LEN), (253, 7));
    assert!(MODBUS.starts_with(b"\0asm"));
    assert_eq!(wasm::check(MODBUS), Ok(()));
}
