//! The harness is what an agent's driver is judged by, so it must pass a
//! correct driver and catch the classic driver bugs, each by a named check.

use std::collections::BTreeMap;
use std::time::Duration;

use helmsman_io::harness::{self, Bench, Check, Outcome};
use helmsman_io::modbus::{ModbusDriver, Point};
use helmsman_io::sim::{Sim, State};
use helmsman_io::{Driver, Error, PointInfo, Value};

fn sim() -> Sim {
    let mut s = State::new(64);
    s.wires = (0..8).map(|n| (n, n)).collect();
    s.input[0] = 4242;
    s.holding[20] = 0x3FC0; // 1.5f32, high word first
    Sim::start(s).unwrap()
}

fn bench() -> Bench {
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

fn driver(s: &Sim) -> ModbusDriver {
    let params = BTreeMap::from([("timeout_ms".to_string(), "200".to_string())]);
    ModbusDriver::from_params(&s.endpoint(), &params).unwrap()
}

fn failed(checks: &[Check]) -> Vec<String> {
    checks
        .iter()
        .filter(|c| matches!(c.outcome, Outcome::Fail(_)))
        .map(|c| c.name.clone())
        .collect()
}

#[test]
fn a_correct_driver_passes_everything_on_the_simulator() {
    let s = sim();
    let checks = harness::run(&mut driver(&s), &bench(), Some(&s));
    assert!(harness::ok(&checks), "{}", harness::render(&checks));
    assert!(
        checks.iter().all(|c| c.outcome == Outcome::Pass),
        "{}",
        harness::render(&checks)
    );
}

#[test]
fn without_the_simulator_fault_checks_skip_rather_than_pass() {
    let s = sim();
    let checks = harness::run(&mut driver(&s), &bench(), None);
    let skipped: Vec<&str> = checks
        .iter()
        .filter(|c| matches!(c.outcome, Outcome::Skip(_)))
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(skipped.len(), 5, "{}", harness::render(&checks));
    assert!(harness::ok(&checks));
}

/// Wraps the real driver and breaks it in one way.
struct Buggy {
    inner: ModbusDriver,
    bug: Bug,
    cache: BTreeMap<String, Value>,
}

#[derive(Clone, Copy)]
enum Bug {
    /// Writes coil N+1: a 1-based manual read as 0-based.
    OffByOneWrite,
    /// Remembers writes and returns them; never touches the device.
    Liar,
    /// Word order backwards on both read and write: round-trips agree.
    SymmetricWordSwap,
    /// Turns device exceptions into zeros.
    SwallowsExceptions,
}

fn shift(addr: &str, by: u16) -> String {
    let p = Point::parse(addr).unwrap();
    let (kind, rest) = addr.split_once(':').unwrap();
    let tail = rest
        .split_once(':')
        .map_or(String::new(), |(_, f)| format!(":{f}"));
    format!("{kind}:{}{tail}", p.addr + by)
}

fn swap_format(addr: &str) -> String {
    if let Some(base) = addr.strip_suffix("sw") {
        base.to_string()
    } else if addr.ends_with(":u32") || addr.ends_with(":i32") || addr.ends_with(":f32") {
        format!("{addr}sw")
    } else {
        addr.to_string()
    }
}

impl Driver for Buggy {
    fn parse(&self, addr: &str) -> Result<PointInfo, Error> {
        self.inner.parse(addr)
    }

    fn read(&mut self, addrs: &[&str]) -> Result<Vec<Value>, Error> {
        match self.bug {
            Bug::Liar => addrs
                .iter()
                .map(|a| match self.cache.get(*a) {
                    Some(v) => Ok(*v),
                    None => self.inner.read(&[a]).map(|v| v[0]),
                })
                .collect(),
            Bug::SymmetricWordSwap => {
                let swapped: Vec<String> = addrs.iter().map(|a| swap_format(a)).collect();
                let refs: Vec<&str> = swapped.iter().map(String::as_str).collect();
                self.inner.read(&refs)
            }
            Bug::SwallowsExceptions => addrs
                .iter()
                .map(|a| match self.inner.read(&[a]) {
                    Err(Error::Exception(_)) => Ok(Value::Int(0)),
                    r => r.map(|v| v[0]),
                })
                .collect(),
            Bug::OffByOneWrite => self.inner.read(addrs),
        }
    }

    fn write(&mut self, addr: &str, value: Value) -> Result<(), Error> {
        match self.bug {
            Bug::OffByOneWrite if addr.starts_with("coil:") => self.inner.write(&shift(addr, 1), value),
            Bug::Liar => {
                self.cache.insert(addr.to_string(), value);
                Ok(())
            }
            Bug::SymmetricWordSwap => self.inner.write(&swap_format(addr), value),
            _ => self.inner.write(addr, value),
        }
    }
}

fn run_buggy(bug: Bug) -> Vec<String> {
    let s = sim();
    let mut d = Buggy {
        inner: driver(&s),
        bug,
        cache: BTreeMap::new(),
    };
    failed(&harness::run(&mut d, &bench(), Some(&s)))
}

#[test]
fn off_by_one_writes_fail_the_loops() {
    assert_eq!(
        run_buggy(Bug::OffByOneWrite),
        ["loop coil:0 -> di:0", "loop coil:5 -> di:5"]
    );
}

#[test]
fn a_driver_that_only_pretends_to_write_fails_the_loops() {
    let f = run_buggy(Bug::Liar);
    assert!(f.contains(&"loop coil:0 -> di:0".to_string()), "{f:?}");
    assert!(f.contains(&"loop coil:5 -> di:5".to_string()), "{f:?}");
}

#[test]
fn symmetric_word_order_bugs_pass_round_trips_but_fail_known_values() {
    let f = run_buggy(Bug::SymmetricWordSwap);
    assert_eq!(f, ["expect hr:20:f32"], "only a known value can catch this");
}

#[test]
fn swallowed_exceptions_fail_the_absent_and_device_failure_checks() {
    // A zero in place of an exception hides a missing point and a failing
    // device alike.
    assert_eq!(
        run_buggy(Bug::SwallowsExceptions),
        ["absent hr:9999", "fault device failure"]
    );
}

#[test]
fn render_is_one_line_per_check_plus_a_summary() {
    let checks = vec![
        Check {
            name: "a".into(),
            outcome: Outcome::Pass,
        },
        Check {
            name: "b".into(),
            outcome: Outcome::Fail("two\nlines".into()),
        },
        Check {
            name: "c".into(),
            outcome: Outcome::Skip("why".into()),
        },
    ];
    assert_eq!(
        harness::render(&checks),
        "PASS  a\nFAIL  b: two lines\nSKIP  c: why\n1 passed, 1 failed, 1 skipped\n"
    );
    assert!(!harness::ok(&checks));
}
