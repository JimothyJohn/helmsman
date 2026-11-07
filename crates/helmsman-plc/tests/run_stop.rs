//! Stopping, running and restarting one module of a running box (card
//! monitor-box-run-stop): what an engineer's Stop, Run and Restart on the
//! Monitor do to the runtime. A stopped module is not scanned, its `%Q`
//! window is zero and its output signals sit at their safe values; the
//! others keep their schedule. Run picks it up on its own phase without
//! counting the stop as overruns; Restart is a cold restart. A deploy
//! leaves a stopped module stopped.

use std::collections::BTreeMap;
use std::time::Duration;

use helmsman_iec::{ImageSizes, Value as St, compile};
use helmsman_io::{Binding, Dir, IoMap, Signal, Ty, Value};
use helmsman_plc::{ConfigError, Module, Runtime, SignalTable, Status, Window};
use proptest::prelude::*;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn module(name: &str, src: &str, period_ms: u64, q: Window) -> Module {
    let sizes = ImageSizes { i: 0, q: q.len, m: 4 };
    let program = compile(src, sizes).unwrap_or_else(|d| panic!("{name}: {d:?}"));
    Module {
        name: name.into(),
        program: program.into(),
        period: ms(period_ms),
        i: Window::new(0, 0),
        q,
        params: Vec::new(),
    }
}

/// Counts its scans and writes the count to its first output byte.
const COUNTER: &str = "
PROGRAM counter
VAR n : USINT; o AT %QB0 : USINT; END_VAR
  n := n + 1;
  o := n;
END_PROGRAM";

fn count(rt: &Runtime, name: &str) -> i128 {
    match rt.program(name).and_then(|p| p.get("n")) {
        Some(St::Int(n)) => n,
        other => panic!("{name}.n = {other:?}"),
    }
}

fn two() -> Runtime {
    Runtime::new(
        0,
        2,
        vec![
            module("a", COUNTER, 10, Window::new(0, 1)),
            module("b", COUNTER, 10, Window::new(1, 1)),
        ],
    )
    .unwrap()
}

fn tick(rt: &mut Runtime, at: u64) {
    assert_eq!(rt.tick(ms(at), &[]), Vec::new(), "no faults at {at} ms");
}

#[test]
fn a_stopped_module_is_not_scanned_and_its_outputs_are_zero_while_the_others_run() {
    let mut rt = two();
    tick(&mut rt, 0);
    tick(&mut rt, 10);
    assert_eq!(rt.outputs(), &[2, 2]);

    rt.stop("a", &mut SignalTable::default()).unwrap();
    assert_eq!(rt.status("a"), Some(Status::Stopped));
    assert_eq!(rt.outputs(), &[0, 2], "a's window goes to zero at once");

    for at in [20, 30, 40] {
        tick(&mut rt, at);
    }
    assert_eq!(count(&rt, "a"), 2, "a was not scanned while stopped");
    assert_eq!(count(&rt, "b"), 5, "b kept its schedule");
    assert_eq!(rt.outputs(), &[0, 5]);
    assert_eq!(rt.stats("a").unwrap().scans, 2);
}

#[test]
fn run_resumes_on_the_modules_phase_with_its_state_and_counts_no_overruns() {
    let mut rt = two();
    tick(&mut rt, 0);
    rt.stop("a", &mut SignalTable::default()).unwrap();
    for at in [10, 20, 30, 40, 50] {
        tick(&mut rt, at);
    }
    rt.run("a").unwrap();
    assert_eq!(rt.status("a"), Some(Status::Running));
    tick(&mut rt, 55);
    assert_eq!(count(&rt, "a"), 1, "not due until its next 10 ms deadline");
    tick(&mut rt, 60);
    assert_eq!(count(&rt, "a"), 2, "carries its count from before the stop");
    assert_eq!(rt.stats("a").unwrap().overruns, 0, "a stop is not an overrun");
}

#[test]
fn restart_is_cold_and_scans_at_its_next_deadline() {
    let mut rt = two();
    for at in [0, 10, 20] {
        tick(&mut rt, at);
    }
    assert_eq!(count(&rt, "a"), 3);
    rt.restart("a").unwrap();
    assert_eq!(rt.status("a"), Some(Status::Running));
    assert_eq!(count(&rt, "a"), 0, "initial values");
    tick(&mut rt, 25);
    assert_eq!(count(&rt, "a"), 0, "not before its next deadline");
    tick(&mut rt, 30);
    assert_eq!(count(&rt, "a"), 1, "scans at its next deadline");
    assert_eq!(rt.outputs()[0], 1);
    assert_eq!(rt.stats("a").unwrap().overruns, 0, "a restart is not an overrun");
    assert_eq!(count(&rt, "b"), 4, "b untouched");
}

#[test]
fn restart_of_a_stopped_module_runs_it_from_initial_values() {
    let mut rt = two();
    tick(&mut rt, 0);
    tick(&mut rt, 10);
    rt.stop("a", &mut SignalTable::default()).unwrap();
    rt.restart("a").unwrap();
    tick(&mut rt, 20);
    assert_eq!(count(&rt, "a"), 1);
    assert_eq!(rt.status("a"), Some(Status::Running));
}

#[test]
fn stop_run_and_restart_are_idempotent_and_unknown_names_change_nothing() {
    let mut rt = two();
    tick(&mut rt, 0);
    let mut t = SignalTable::default();
    rt.stop("a", &mut t).unwrap();
    rt.stop("a", &mut t).unwrap();
    assert_eq!(rt.status("a"), Some(Status::Stopped));
    rt.run("a").unwrap();
    rt.run("a").unwrap();
    assert_eq!(rt.status("a"), Some(Status::Running));
    for name in ["", "A", "c", "a ", "a/../b"] {
        assert_eq!(
            rt.stop(name, &mut t),
            Err(ConfigError::UnknownModule(name.into())),
            "{name:?}"
        );
        assert_eq!(rt.run(name), Err(ConfigError::UnknownModule(name.into())));
        assert_eq!(rt.restart(name), Err(ConfigError::UnknownModule(name.into())));
    }
    assert_eq!(rt.status("b"), Some(Status::Running));
}

#[test]
fn every_module_stopped_means_no_deadline_until_one_runs() {
    let mut rt = two();
    tick(&mut rt, 0);
    let mut t = SignalTable::default();
    rt.stop("a", &mut t).unwrap();
    rt.stop("b", &mut t).unwrap();
    assert_eq!(rt.next_deadline(), None);
    rt.run("b").unwrap();
    assert_eq!(rt.next_deadline(), Some(ms(10)));
}

#[test]
fn run_clears_a_fault_and_resumes() {
    const FAULTS_ON_SECOND: &str = "
PROGRAM faulty
VAR n, d : USINT; o AT %QB0 : USINT; END_VAR
  n := n + 1;
  o := n;
  IF n = 2 THEN d := 10 / (n - 2); END_IF;
END_PROGRAM";
    let mut rt = Runtime::new(0, 1, vec![module("f", FAULTS_ON_SECOND, 10, Window::new(0, 1))]).unwrap();
    tick(&mut rt, 0);
    assert_eq!(rt.tick(ms(10), &[]).len(), 1, "faults on its second scan");
    assert!(matches!(rt.status("f"), Some(Status::Faulted(_))));
    rt.run("f").unwrap();
    assert_eq!(rt.status("f"), Some(Status::Running));
    tick(&mut rt, 20);
    assert_eq!(count(&rt, "f"), 3, "resumed with its state");
    assert_eq!(rt.outputs(), &[3]);
}

#[test]
fn a_deploy_or_a_swap_keeps_a_stopped_module_stopped() {
    let mut rt = two();
    tick(&mut rt, 0);
    let mut t = SignalTable::default();
    rt.stop("a", &mut t).unwrap();
    rt.reconfigure(
        vec![
            module("a", COUNTER, 10, Window::new(0, 1)),
            module("b", COUNTER, 10, Window::new(1, 1)),
        ],
        &mut t,
    )
    .unwrap();
    assert_eq!(rt.status("a"), Some(Status::Stopped), "an engineer stopped it");
    tick(&mut rt, 10);
    assert_eq!(count(&rt, "a"), 1);
    assert_eq!(rt.outputs()[0], 0, "still zero after the deploy");

    let mut s = Module::clone(rt.module("a").unwrap());
    s.period = ms(20);
    rt.swap(s, &mut t).unwrap();
    assert_eq!(
        rt.status("a"),
        Some(Status::Stopped),
        "a swap keeps it stopped too"
    );
}

fn out(name: &str, channel: &str) -> Signal {
    Signal {
        name: name.into(),
        ty: Ty::Bool,
        dir: Dir::Out,
        binding: Binding::Local {
            channel: channel.into(),
        },
        unit: None,
        poll: None,
    }
}

#[test]
fn a_stop_sends_the_modules_output_signals_to_their_safe_values_and_run_drives_them_again() {
    let m = IoMap {
        devices: Vec::new(),
        signals: vec![out("brake", "do:0"), out("lamp", "do:1")],
    };
    let safe: BTreeMap<String, Value> = [("brake".to_string(), Value::Bool(true))].into();
    let mut table = SignalTable::new(&m, &safe).unwrap();
    let src = "
PROGRAM drive
VAR_OUTPUT brake : BOOL; lamp : BOOL; END_VAR
  brake := FALSE;
  lamp := TRUE;
END_PROGRAM";
    let mut rt =
        Runtime::with_signals(0, 0, vec![module("drive", src, 10, Window::new(0, 0))], &table).unwrap();
    let (brake, lamp) = (table.index("brake").unwrap(), table.index("lamp").unwrap());
    assert_eq!(rt.tick_with(ms(0), &[], &mut table), Vec::new());
    assert_eq!(table.command(brake), Some(Value::Bool(false)));
    assert_eq!(table.command(lamp), Some(Value::Bool(true)));

    rt.stop("drive", &mut table).unwrap();
    assert_eq!(table.command(brake), Some(Value::Bool(true)), "declared safe");
    assert_eq!(table.command(lamp), Some(Value::Bool(false)), "default safe");
    assert_eq!(rt.tick_with(ms(10), &[], &mut table), Vec::new());
    assert_eq!(
        table.command(brake),
        Some(Value::Bool(true)),
        "stays safe while stopped"
    );

    rt.run("drive").unwrap();
    assert_eq!(rt.tick_with(ms(20), &[], &mut table), Vec::new());
    assert_eq!(table.command(brake), Some(Value::Bool(false)), "driven again");
}

/// Writes its tag to its output byte every scan and counts its scans.
fn tagged(tag: u8) -> String {
    format!(
        "PROGRAM t{tag}
VAR n : USINT; o AT %QB0 : USINT; END_VAR
  n := n + 1;
  o := {tag};
END_PROGRAM"
    )
}

#[derive(Debug, Clone, Copy)]
enum Op {
    Tick,
    Stop(usize),
    Run(usize),
    Restart(usize),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => Just(Op::Tick),
        1 => (0usize..4).prop_map(Op::Stop),
        1 => (0usize..4).prop_map(Op::Run),
        1 => (0usize..4).prop_map(Op::Restart),
    ]
}

proptest! {
    /// Any mix of stops, runs, restarts and ticks across four modules: a
    /// stopped module's byte is zero and it is never scanned; every other
    /// module's byte is its tag once it has scanned; no stop ever counts
    /// as an overrun.
    #[test]
    fn stops_runs_and_restarts_in_any_order_hold_the_invariants(ops in proptest::collection::vec(op(), 1..60)) {
        let modules: Vec<Module> = (0..4u8)
            .map(|n| module(&format!("m{n}"), &tagged(n + 1), 10, Window::new(usize::from(n), 1)))
            .collect();
        let mut rt = Runtime::new(0, 4, modules).unwrap();
        let mut t = SignalTable::default();
        let mut now = 0u64;
        prop_assert!(rt.tick(ms(now), &[]).is_empty());
        for op in ops {
            let before: Vec<u64> = (0..4).map(|n| rt.stats(&format!("m{n}")).unwrap().scans).collect();
            match op {
                Op::Tick => {
                    now += 10;
                    prop_assert!(rt.tick(ms(now), &[]).is_empty());
                }
                Op::Stop(n) => rt.stop(&format!("m{n}"), &mut t).unwrap(),
                Op::Run(n) => rt.run(&format!("m{n}")).unwrap(),
                Op::Restart(n) => rt.restart(&format!("m{n}")).unwrap(),
            }
            for n in 0..4u8 {
                let name = format!("m{n}");
                let stats = rt.stats(&name).unwrap();
                let byte = rt.outputs()[usize::from(n)];
                match rt.status(&name).unwrap() {
                    Status::Stopped => {
                        prop_assert_eq!(byte, 0, "{} stopped", name);
                        prop_assert_eq!(stats.scans, before[usize::from(n)], "{} scanned while stopped", name);
                    }
                    Status::Running => prop_assert!(byte == 0 || byte == n + 1, "{} wrote {}", name, byte),
                    other => prop_assert!(false, "{} is {:?}", name, other),
                }
                prop_assert_eq!(stats.overruns, 0, "{}", name);
            }
        }
    }
}
