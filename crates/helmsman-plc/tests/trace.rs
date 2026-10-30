//! The runtime's trace tap: a viewer asks, the module's next scan is
//! traced, the sample waits in a one-slot mailbox. Nothing is traced
//! unless asked, and tracing never changes what the modules do.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use std::collections::BTreeMap;

use helmsman_iec::{FaultKind, ImageSizes, Value, compile};
use helmsman_io::{Binding, Device, Dir, IoMap, Signal, Ty, Value as SigValue};
use helmsman_plc::{Module, Runtime, SignalTable, Status, Window};
use proptest::prelude::*;

const IMAGE: usize = 32;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn module(name: &str, src: &str, period_ms: u64, i: Window, q: Window) -> Module {
    let sizes = ImageSizes {
        i: i.len,
        q: q.len,
        m: 16,
    };
    let program = compile(src, sizes).unwrap_or_else(|d| panic!("{name}: {d:?}"));
    Module {
        name: name.into(),
        program,
        period: ms(period_ms),
        i,
        q,
        params: Vec::new(),
    }
}

/// Counts its scans; above an input threshold, counts twice.
const COUNTER: &str = "
PROGRAM counter
VAR n : UDINT; hi AT %IB0 : USINT; o AT %QB0 : USINT; END_VAR
  n := n + 1;
  IF hi > 100 THEN
    n := n + 1;
  END_IF;
  o := UDINT_TO_USINT(n);
END_PROGRAM";

/// Faults on its third scan.
const FAULTS_ON_THIRD: &str = "
PROGRAM faulty
VAR n, d : USINT; o AT %QB0 : USINT; END_VAR
  n := n + 1;
  o := 16#FF;
  d := 10 / (3 - n);
END_PROGRAM";

fn two() -> Runtime {
    Runtime::new(
        IMAGE,
        IMAGE,
        vec![
            module("fast", COUNTER, 1, Window::new(0, 1), Window::new(0, 1)),
            module("slow", COUNTER, 10, Window::new(0, 1), Window::new(1, 1)),
        ],
    )
    .unwrap()
}

fn int(v: Option<Value>) -> i128 {
    match v {
        Some(Value::Int(i)) => i,
        other => panic!("{other:?}"),
    }
}

#[test]
fn nothing_is_traced_unless_asked() {
    let mut rt = two();
    let tap = rt.tap("fast").unwrap();
    for t in 0..20 {
        rt.tick(ms(t), &[0]);
    }
    assert!(tap.take().is_none());
    assert!(!tap.pending());
    assert!(rt.tap("nope").is_none());
}

#[test]
fn a_request_traces_exactly_the_next_scan_of_that_module() {
    let mut rt = two();
    let fast = rt.tap("fast").unwrap();
    let slow = rt.tap("slow").unwrap();
    for t in 0..5 {
        rt.tick(ms(t), &[0]);
    }
    fast.request();
    slow.request();
    assert!(fast.pending());
    rt.tick(ms(5), &[200]);
    // fast scanned at 5 ms; slow is next due at 10 ms.
    let s = fast.take().expect("fast traced");
    assert!(!fast.pending());
    assert_eq!(s.scan, 6);
    assert_eq!(s.scan, rt.stats("fast").unwrap().scans);
    assert_eq!(s.fault, None);
    assert_eq!(s.trace.now(), ms(5));
    assert_eq!(int(s.trace.get("n")), 7, "6 scans, the last counted twice");
    let iff = s
        .trace
        .map()
        .in_pou("counter")
        .find(|&id| s.trace.map().stmt(id).unwrap().line == 5)
        .unwrap();
    assert_eq!(s.trace.arm(iff), Some(0));
    assert!(fast.take().is_none(), "a sample is taken once");
    assert!(slow.take().is_none());
    assert!(slow.pending(), "slow has not scanned since the request");

    rt.tick(ms(6), &[0]);
    assert!(fast.take().is_none(), "one request, one traced scan");
    for t in 7..=10 {
        rt.tick(ms(t), &[0]);
    }
    let s = slow.take().expect("slow traced at its 10 ms scan");
    assert_eq!(s.scan, 2);
    assert_eq!(s.trace.arm(iff), Some(1));
}

#[test]
fn an_untaken_sample_is_replaced_by_the_newest() {
    let mut rt = two();
    let tap = rt.tap("fast").unwrap();
    for t in 0..4 {
        tap.request();
        rt.tick(ms(t), &[0]);
    }
    assert_eq!(tap.take().unwrap().scan, 4);
    assert!(tap.take().is_none());
}

#[test]
fn a_faulting_scan_is_traced_up_to_the_fault() {
    let mut rt = Runtime::new(
        IMAGE,
        IMAGE,
        vec![module(
            "faulty",
            FAULTS_ON_THIRD,
            1,
            Window::new(0, 0),
            Window::new(0, 1),
        )],
    )
    .unwrap();
    let tap = rt.tap("faulty").unwrap();
    rt.tick(ms(0), &[]);
    rt.tick(ms(1), &[]);
    tap.request();
    rt.tick(ms(2), &[]);
    assert!(matches!(rt.status("faulty"), Some(Status::Faulted(_))));
    let s = tap.take().unwrap();
    assert_eq!(s.fault.map(|f| f.kind), Some(FaultKind::DivisionByZero));
    assert_eq!(s.trace.fault(), s.fault);
    let at = s.trace.faulted_at().unwrap();
    assert_eq!(s.trace.map().stmt(at).unwrap().line, 6);
    assert_eq!(int(s.trace.get("n")), 3);
    // The module is stopped: a new request waits.
    tap.request();
    rt.tick(ms(3), &[]);
    assert!(tap.take().is_none());
    assert!(tap.pending());
}

fn sig(name: &str, ty: Ty, dir: Dir, binding: Binding) -> Signal {
    Signal {
        name: name.into(),
        ty,
        dir,
        binding,
    }
}

/// `go` (a box input) and `speed` (a REAL on a Modbus register; nothing
/// connects to it here, the table only needs the map).
fn table() -> SignalTable {
    let map = IoMap {
        devices: vec![Device {
            id: "rio".into(),
            driver: "modbus-tcp".into(),
            endpoint: "127.0.0.1:1".into(),
            params: BTreeMap::new(),
        }],
        signals: vec![
            sig(
                "go",
                Ty::Bool,
                Dir::In,
                Binding::Local {
                    channel: "di:0".into(),
                },
            ),
            sig(
                "speed",
                Ty::Real,
                Dir::Out,
                Binding::Device {
                    device: "rio".into(),
                    point: "hr:2:f32".into(),
                },
            ),
        ],
    };
    SignalTable::new(&map, &BTreeMap::new()).unwrap()
}

const DRIVE: &str = "
PROGRAM drive
VAR_INPUT go : BOOL; END_VAR
VAR_OUTPUT speed : REAL; END_VAR
VAR zero : REAL; END_VAR
  speed := 1.5;
  IF go THEN speed := 1.0 / zero; END_IF;
END_PROGRAM";

fn drive() -> Module {
    module("drive", DRIVE, 10, Window::new(0, 0), Window::new(0, 0))
}

/// A fault the runtime raises after a clean scan (an infinite REAL cannot
/// leave the box) is the sample's fault; the interpreter's trace is clean.
#[test]
fn an_output_the_signal_cannot_carry_is_the_samples_fault() {
    let mut t = table();
    let mut rt = Runtime::with_signals(0, 0, vec![drive()], &t).unwrap();
    let go = t.index("go").unwrap();
    let tap = rt.tap("drive").unwrap();
    t.set_input(go, Some(SigValue::Bool(true)));
    tap.request();
    rt.tick_with(ms(0), &[], &mut t);
    let s = tap.take().unwrap();
    assert_eq!(s.fault.map(|f| f.kind), Some(FaultKind::OutputRange));
    assert_eq!(s.trace.fault(), None);
    assert_eq!(s.trace.get("speed"), Some(Value::Real(f64::INFINITY)));
}

/// A held module is not scanned, so a request waits for its inputs.
#[test]
fn a_held_module_keeps_the_request_until_it_scans() {
    let mut t = table();
    let mut rt = Runtime::with_signals(0, 0, vec![drive()], &t).unwrap();
    let go = t.index("go").unwrap();
    let tap = rt.tap("drive").unwrap();
    tap.request();
    rt.tick_with(ms(0), &[], &mut t);
    assert_eq!(rt.status("drive"), Some(Status::Held));
    assert!(tap.take().is_none());
    assert!(tap.pending());
    t.set_input(go, Some(SigValue::Bool(false)));
    rt.tick_with(ms(10), &[], &mut t);
    assert_eq!(rt.status("drive"), Some(Status::Running));
    let s = tap.take().unwrap();
    assert_eq!(s.scan, 1);
    assert_eq!(s.fault, None);
    assert_eq!(s.trace.get("speed"), Some(Value::Real(1.5)));
}

/// The scheduler on its own thread, a viewer on another asking at its own
/// rate: every sample is a real scan, in order, each consistent with
/// itself, and the scheduler never waits for the viewer.
#[test]
fn a_viewer_thread_samples_a_running_scheduler() {
    let mut rt = two();
    let tap = rt.tap("fast").unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let scheduler = {
        let stop = Arc::clone(&stop);
        thread::spawn(move || {
            let mut t = 0;
            while !stop.load(Ordering::Relaxed) {
                rt.tick(ms(t), &[0]);
                t += 1;
            }
            rt
        })
    };
    let mut seen = Vec::new();
    while seen.len() < 50 {
        tap.request();
        if let Some(s) = tap.take() {
            // n counts scans one by one: the trace is that scan's state.
            assert_eq!(int(s.trace.get("n")), i128::from(s.scan));
            assert_eq!(s.trace.now(), ms(s.scan - 1));
            seen.push(s.scan);
        } else {
            thread::yield_now();
        }
    }
    stop.store(true, Ordering::Relaxed);
    let rt = scheduler.join().unwrap();
    assert!(seen.windows(2).all(|w| w[0] < w[1]), "{seen:?}");
    assert!(rt.stats("fast").unwrap().scans >= *seen.last().unwrap());
}

proptest! {
    /// Requesting traces at random never changes outputs, statuses or
    /// variables.
    #[test]
    fn tracing_never_changes_what_modules_do(
        inputs in proptest::collection::vec(any::<u8>(), 1..40),
        asks in proptest::collection::vec(any::<bool>(), 40),
    ) {
        let (mut plain, mut traced) = (two(), two());
        let taps = [traced.tap("fast").unwrap(), traced.tap("slow").unwrap()];
        for (t, &inp) in inputs.iter().enumerate() {
            if asks[t] {
                taps[t % 2].request();
            }
            let now = ms(u64::try_from(t).unwrap());
            prop_assert_eq!(plain.tick(now, &[inp]), traced.tick(now, &[inp]));
            prop_assert_eq!(plain.outputs(), traced.outputs());
            for name in ["fast", "slow"] {
                prop_assert_eq!(plain.status(name), traced.status(name));
                prop_assert_eq!(plain.stats(name), traced.stats(name));
                prop_assert_eq!(
                    plain.program(name).unwrap().get("n"),
                    traced.program(name).unwrap().get("n")
                );
            }
            for tap in &taps {
                let _ = tap.take();
            }
        }
    }
}
