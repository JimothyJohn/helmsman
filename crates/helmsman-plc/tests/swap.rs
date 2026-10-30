//! Hot swap: one module's program replaced between its scans, checked like
//! a fresh load, with its retained state carried by name and type and its
//! outputs holding their last values until the new program's first scan.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use helmsman_iec::{FaultKind, ImageSizes, Value as St, compile};
use helmsman_io::local::Channels;
use helmsman_io::sim::{Sim, State};
use helmsman_io::{Binding, Device, Dir, IoMap, Signal, Ty, Value};
use helmsman_plc::link::{links, spawn};
use helmsman_plc::{BindError, ConfigError, Module, Runtime, SignalTable, Status, Window};
use proptest::prelude::*;

const IMAGE: usize = 8;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn module(name: &str, src: &str, period_ms: u64, q: Window) -> Module {
    let sizes = ImageSizes { i: 0, q: q.len, m: 4 };
    let program = compile(src, sizes).unwrap_or_else(|d| panic!("{name}: {d:?}\n{src}"));
    Module {
        name: name.into(),
        program,
        period: ms(period_ms),
        i: Window::new(0, 0),
        q,
        params: Vec::new(),
    }
}

fn get(rt: &Runtime, module: &str, var: &str) -> Option<St> {
    rt.program(module).and_then(|p| p.get(var))
}

/// A timer started at time zero, a scan counter, and the count on `%QB0`.
const TIMED: &str = "
PROGRAM timed
VAR t : TON; n : UINT; level : INT := 7; tag : BOOL := TRUE; o AT %QB0 : BYTE; END_VAR
  t(IN := TRUE, PT := T#500ms);
  n := n + 1;
  o := UINT_TO_BYTE(n);
END_PROGRAM";

/// [`TIMED`] edited: `level` is now REAL, `tag` is renamed `label`, and the
/// output byte shows the count plus 100.
const TIMED_V2: &str = "
PROGRAM timed
VAR t : TON; n : UINT; level : REAL := 2.5; label : BOOL; o AT %QB0 : BYTE; END_VAR
  t(IN := TRUE, PT := T#500ms);
  n := n + 1;
  o := UINT_TO_BYTE(n + 100);
END_PROGRAM";

/// Writes 16#AA to every byte of a 2-byte window, and counts.
const MARK: &str = "
PROGRAM mark
VAR n : UINT; a AT %QB0 : BYTE; b AT %QB1 : BYTE; END_VAR
  n := n + 1; a := 16#AA; b := 16#AA;
END_PROGRAM";

const DIV_ON_THIRD: &str = "
PROGRAM faulty
VAR n, d : UINT; o AT %QB0 : BYTE; END_VAR
  n := n + 1;
  o := 16#FF;
  d := 10 / (3 - n);
END_PROGRAM";

const COUNT_ONLY: &str = "
PROGRAM faulty
VAR n : UINT; o AT %QB0 : BYTE; END_VAR
  n := n + 1;
  o := UINT_TO_BYTE(n);
END_PROGRAM";

#[test]
fn a_swap_mid_run_keeps_the_timer_and_the_counter() {
    let mut rt = Runtime::new(IMAGE, IMAGE, vec![module("timed", TIMED, 10, Window::new(0, 1))]).unwrap();
    for t in (0..300).step_by(10) {
        rt.tick(ms(t), &[]);
    }
    assert_eq!(get(&rt, "timed", "n"), Some(St::Int(30)));
    assert_eq!(rt.outputs()[0], 30);

    let carry = rt
        .swap(
            module("timed", TIMED_V2, 10, Window::new(0, 1)),
            &mut SignalTable::default(),
        )
        .unwrap();
    assert_eq!(carry.kept, ["T", "N"]);
    assert_eq!(carry.reset, ["LEVEL", "LABEL"]);
    assert_eq!(carry.dropped, ["TAG"]);
    assert_eq!(
        get(&rt, "timed", "level"),
        Some(St::Real(2.5)),
        "INT -> REAL: initial value"
    );
    assert_eq!(get(&rt, "timed", "label"), Some(St::Bool(false)));
    assert_eq!(
        rt.outputs()[0],
        30,
        "the output holds until the new program scans"
    );
    assert_eq!(rt.status("timed"), Some(Status::Running));

    rt.tick(ms(300), &[]);
    assert_eq!(rt.outputs()[0], 131, "count 31, plus the new program's 100");
    for t in (310..500).step_by(10) {
        rt.tick(ms(t), &[]);
    }
    assert_eq!(get(&rt, "timed", "t.Q"), Some(St::Bool(false)));
    rt.tick(ms(500), &[]);
    assert_eq!(
        get(&rt, "timed", "t.Q"),
        Some(St::Bool(true)),
        "fires 500 ms after the old program started it, not after the swap"
    );
    assert_eq!(rt.stats("timed").unwrap().scans, 51);
    assert_eq!(rt.stats("timed").unwrap().overruns, 0);
}

/// Two runtimes built alike: `rt` gets a refused swap, `twin` none. They
/// must stay indistinguishable.
fn assert_refused_changes_nothing(
    mut rt: Runtime,
    mut twin: Runtime,
    swap: Module,
    table: &mut SignalTable,
    want: &[ConfigError],
) {
    let name = swap.name.clone();
    for t in (0..50).step_by(10) {
        rt.tick_with(ms(t), &[], table);
        twin.tick_with(ms(t), &[], &mut table.clone());
    }
    let before = rt.module(&name).map(|m| (m.q, m.period, m.program.ports()));
    let commands: Vec<_> = (0..table.len()).map(|i| table.command(i)).collect();
    assert_eq!(rt.swap(swap, table).unwrap_err(), want);
    assert_eq!(
        rt.module(&name).map(|m| (m.q, m.period, m.program.ports())),
        before
    );
    assert_eq!(
        (0..table.len()).map(|i| table.command(i)).collect::<Vec<_>>(),
        commands
    );
    assert_eq!(rt.outputs(), twin.outputs());
    for t in (50..100).step_by(10) {
        rt.tick_with(ms(t), &[], table);
        twin.tick_with(ms(t), &[], &mut table.clone());
        assert_eq!(rt.outputs(), twin.outputs());
        for n in twin.names() {
            assert_eq!(get(&rt, n, "n"), get(&twin, n, "n"), "{n}");
            assert_eq!(rt.status(n), twin.status(n));
            assert_eq!(rt.stats(n), twin.stats(n));
        }
    }
}

fn pair() -> Vec<Module> {
    vec![
        module("a", MARK, 10, Window::new(0, 2)),
        module("b", MARK, 10, Window::new(4, 2)),
    ]
}

#[test]
fn a_swap_onto_another_modules_output_bytes_is_refused_and_changes_nothing() {
    for (start, len) in [(1, 2), (0, 2), (1, 1), (0, 8)] {
        let src = if len == 1 {
            "PROGRAM mark VAR n : UINT; END_VAR n := n + 1; END_PROGRAM"
        } else {
            MARK
        };
        let swap = if len == 8 {
            let program = compile(src, ImageSizes { i: 0, q: 8, m: 4 }).unwrap();
            Module {
                name: "b".into(),
                program,
                period: ms(10),
                i: Window::new(0, 0),
                q: Window::new(start, len),
                params: Vec::new(),
            }
        } else {
            module("b", src, 10, Window::new(start, len))
        };
        assert_refused_changes_nothing(
            Runtime::new(IMAGE, IMAGE, pair()).unwrap(),
            Runtime::new(IMAGE, IMAGE, pair()).unwrap(),
            swap,
            &mut SignalTable::default(),
            &[ConfigError::OutputOverlap {
                a: "a".into(),
                b: "b".into(),
            }],
        );
    }
}

#[test]
fn every_fresh_load_check_applies_to_a_swap() {
    let cases = [
        (
            module("b", MARK, 10, Window::new(4, 2)).program,
            Window::new(7, 2),
            10,
            ConfigError::OutsideImage {
                module: "b".into(),
                area: 'Q',
            },
        ),
        (
            module("b", MARK, 10, Window::new(4, 2)).program,
            Window::new(4, 3),
            10,
            ConfigError::SizeMismatch {
                module: "b".into(),
                area: 'Q',
            },
        ),
        (
            module("b", MARK, 10, Window::new(4, 2)).program,
            Window::new(4, 2),
            0,
            ConfigError::BadPeriod("b".into()),
        ),
    ];
    for (program, q, period, err) in cases {
        let swap = Module {
            name: "b".into(),
            program,
            period: ms(period),
            i: Window::new(0, 0),
            q,
            params: Vec::new(),
        };
        assert_refused_changes_nothing(
            Runtime::new(IMAGE, IMAGE, pair()).unwrap(),
            Runtime::new(IMAGE, IMAGE, pair()).unwrap(),
            swap,
            &mut SignalTable::default(),
            &[err],
        );
    }
    let mut rt = Runtime::new(IMAGE, IMAGE, pair()).unwrap();
    assert_eq!(
        rt.swap(
            module("c", MARK, 10, Window::new(6, 2)),
            &mut SignalTable::default()
        )
        .unwrap_err(),
        [ConfigError::UnknownModule("c".into())]
    );
}

#[test]
fn bytes_a_module_gives_up_go_to_zero_and_the_ones_it_keeps_hold() {
    let mut rt = Runtime::new(IMAGE, IMAGE, pair()).unwrap();
    rt.tick(ms(0), &[]);
    assert_eq!(rt.outputs(), [0xAA, 0xAA, 0, 0, 0xAA, 0xAA, 0, 0]);
    // b moves one byte right: byte 4 is nobody's now, byte 5 is kept, 6 is new.
    let quiet = "PROGRAM mark VAR n : UINT; END_VAR n := n + 1; END_PROGRAM";
    rt.swap(
        module("b", quiet, 10, Window::new(5, 2)),
        &mut SignalTable::default(),
    )
    .unwrap();
    assert_eq!(rt.outputs(), [0xAA, 0xAA, 0, 0, 0, 0xAA, 0, 0]);
    assert_eq!(get(&rt, "b", "n"), Some(St::Int(1)), "the counter carried");
    rt.tick(ms(10), &[]);
    assert_eq!(
        rt.outputs(),
        [0xAA, 0xAA, 0, 0, 0, 0xAA, 0, 0],
        "the new program writes nothing, so it keeps what its window held"
    );
    assert_eq!(get(&rt, "b", "n"), Some(St::Int(2)));
}

#[test]
fn m_keeps_its_bytes() {
    let src = "PROGRAM m VAR x AT %MB1 : USINT; END_VAR x := x + 1; END_PROGRAM";
    let mut rt = Runtime::new(IMAGE, IMAGE, vec![module("m", src, 10, Window::new(0, 0))]).unwrap();
    for t in (0..30).step_by(10) {
        rt.tick(ms(t), &[]);
    }
    let renamed = "PROGRAM m VAR y AT %MB1 : USINT; o AT %QB0 : BYTE; END_VAR y := y + 1; o := USINT_TO_BYTE(y); END_PROGRAM";
    rt.swap(
        module("m", renamed, 10, Window::new(0, 1)),
        &mut SignalTable::default(),
    )
    .unwrap();
    rt.tick(ms(30), &[]);
    assert_eq!(rt.outputs()[0], 4);
}

#[test]
fn a_faulted_module_swapped_restarts_with_its_state() {
    let mut rt = Runtime::new(
        IMAGE,
        IMAGE,
        vec![module("faulty", DIV_ON_THIRD, 10, Window::new(0, 1))],
    )
    .unwrap();
    for t in (0..30).step_by(10) {
        rt.tick(ms(t), &[]);
    }
    assert!(matches!(
        rt.status("faulty"),
        Some(Status::Faulted(f)) if f.kind == FaultKind::DivisionByZero
    ));
    assert_eq!(rt.outputs()[0], 0);
    // Stopped for a while; the scheduler kept ticking.
    rt.tick(ms(1000), &[]);
    assert_eq!(rt.next_deadline(), None);

    let carry = rt
        .swap(
            module("faulty", COUNT_ONLY, 10, Window::new(0, 1)),
            &mut SignalTable::default(),
        )
        .unwrap();
    assert_eq!(carry.kept, ["N"]);
    assert_eq!(carry.dropped, ["D"]);
    assert_eq!(rt.status("faulty"), Some(Status::Running));
    assert_eq!(
        rt.outputs()[0],
        0,
        "zeroed outputs stay zero until the first scan"
    );
    assert_eq!(
        rt.next_deadline(),
        Some(ms(1010)),
        "next deadline after the last tick, on phase"
    );
    rt.tick(ms(1010), &[]);
    assert_eq!(rt.outputs()[0], 4, "n was 3 when it faulted");
    assert_eq!(
        rt.stats("faulty").unwrap().overruns,
        0,
        "time stopped is not overrun"
    );
    assert_eq!(rt.stats("faulty").unwrap().scans, 4);
}

// ---- signals ----

fn local(name: &str, ty: Ty, dir: Dir, channel: &str) -> Signal {
    Signal {
        name: name.into(),
        ty,
        dir,
        binding: Binding::Local {
            channel: channel.into(),
        },
    }
}

fn table() -> SignalTable {
    let map = IoMap {
        devices: vec![],
        signals: vec![
            local("start", Ty::Bool, Dir::In, "di:0"),
            local("motor", Ty::Bool, Dir::Out, "do:0"),
            local("lamp", Ty::Bool, Dir::Out, "do:1"),
            local("horn", Ty::Bool, Dir::Out, "do:2"),
        ],
    };
    let safe = BTreeMap::from([("horn".to_string(), Value::Bool(true))]);
    SignalTable::new(&map, &safe).unwrap()
}

fn cmd(t: &SignalTable, name: &str) -> Value {
    t.command(t.index(name).unwrap()).unwrap()
}

const LINE: &str = "
PROGRAM line
VAR_INPUT start : BOOL; END_VAR
VAR_OUTPUT motor : BOOL; lamp : BOOL; END_VAR
VAR n : UINT; END_VAR
  n := n + 1;
  motor := start;
  lamp := TRUE;
END_PROGRAM";

const HORN: &str = "
PROGRAM horn
VAR_OUTPUT horn : BOOL; END_VAR
VAR n : UINT; END_VAR
  n := n + 1;
  horn := FALSE;
END_PROGRAM";

fn plant(t: &SignalTable) -> Runtime {
    Runtime::with_signals(
        0,
        0,
        vec![
            module("line", LINE, 10, Window::new(0, 0)),
            module("horn", HORN, 10, Window::new(0, 0)),
        ],
        t,
    )
    .unwrap()
}

#[test]
fn a_swap_that_would_drive_another_modules_signal_is_refused() {
    let mut t = table();
    t.set_input(0, Some(Value::Bool(true)));
    let steal =
        "PROGRAM horn VAR_OUTPUT horn : BOOL; motor : BOOL; END_VAR VAR n : UINT; END_VAR END_PROGRAM";
    assert_refused_changes_nothing(
        plant(&t),
        plant(&t),
        module("horn", steal, 10, Window::new(0, 0)),
        &mut t,
        &[ConfigError::TwoWriters {
            signal: "MOTOR".into(),
            a: "line".into(),
            b: "horn".into(),
        }],
    );
}

#[test]
fn a_swap_whose_ports_do_not_bind_is_refused() {
    let mut t = table();
    t.set_input(0, Some(Value::Bool(true)));
    let cases = [
        (
            "PROGRAM horn VAR_OUTPUT horn : BOOL; siren : BOOL; END_VAR VAR n : UINT; END_VAR END_PROGRAM",
            BindError::UnknownSignal("SIREN".into()),
        ),
        (
            "PROGRAM horn VAR_OUTPUT horn : BOOL; start : BOOL; END_VAR VAR n : UINT; END_VAR END_PROGRAM",
            BindError::DrivesInput("START".into()),
        ),
        (
            "PROGRAM horn VAR_OUTPUT horn : INT; END_VAR VAR n : UINT; END_VAR END_PROGRAM",
            BindError::Type {
                port: "HORN".into(),
                signal: Ty::Bool,
                st: helmsman_iec::Ty::Int,
            },
        ),
    ];
    for (src, err) in cases {
        assert_refused_changes_nothing(
            plant(&t),
            plant(&t),
            module("horn", src, 10, Window::new(0, 0)),
            &mut t,
            &[ConfigError::Port {
                module: "horn".into(),
                err,
            }],
        );
    }
}

#[test]
fn a_swap_cannot_take_an_output_the_calendar_drives() {
    let mut t = table();
    helmsman_plc::calendar::Source::attach(
        helmsman_plc::calendar::load("drive lamp in_shift").unwrap(),
        &mut t,
    )
    .unwrap();
    let only_horn =
        || Runtime::with_signals(0, 0, vec![module("horn", HORN, 10, Window::new(0, 0))], &t).unwrap();
    let (rt, twin) = (only_horn(), only_horn());
    let takes_lamp =
        "PROGRAM horn VAR_OUTPUT horn : BOOL; lamp : BOOL; END_VAR VAR n : UINT; END_VAR END_PROGRAM";
    assert_refused_changes_nothing(
        rt,
        twin,
        module("horn", takes_lamp, 10, Window::new(0, 0)),
        &mut t,
        &[ConfigError::TwoWriters {
            signal: "LAMP".into(),
            a: "calendar".into(),
            b: "horn".into(),
        }],
    );
}

#[test]
fn a_swap_changes_params_and_refuses_bad_ones() {
    const TUNED: &str = "
PROGRAM horn
VAR_INPUT limit : INT; END_VAR
VAR_OUTPUT horn : BOOL; END_VAR
VAR n : UINT; END_VAR
  n := n + 1;
  horn := n > limit;
END_PROGRAM";
    let with = |params: &[(&str, Value)]| {
        let mut m = module("horn", TUNED, 10, Window::new(0, 0));
        m.params = params.iter().map(|(k, v)| ((*k).to_string(), *v)).collect();
        m
    };
    let mut t = table();
    t.set_input(0, Some(Value::Bool(true)));
    for (params, bad) in [
        (vec![("limit", Value::Int(100_000))], "limit"),
        (vec![("limit", Value::Int(1)), ("LIMIT", Value::Int(2))], "LIMIT"),
        (vec![("limit", Value::Bool(true))], "limit"),
    ] {
        assert_refused_changes_nothing(
            plant(&t),
            plant(&t),
            with(&params),
            &mut t,
            &[ConfigError::Param {
                module: "horn".into(),
                param: bad.into(),
            }],
        );
    }
    let mut rt = plant(&t);
    rt.tick_with(ms(0), &[], &mut t);
    rt.swap(with(&[("limit", Value::Int(3))]), &mut t).unwrap();
    for (k, want) in [(1, false), (2, false), (3, true)] {
        rt.tick_with(ms(k * 10), &[], &mut t);
        assert_eq!(cmd(&t, "horn"), Value::Bool(want), "scan {}", k + 1);
    }
    rt.swap(with(&[("limit", Value::Int(10))]), &mut t).unwrap();
    rt.tick_with(ms(40), &[], &mut t);
    assert_eq!(
        cmd(&t, "horn"),
        Value::Bool(false),
        "the new param applies on the next scan"
    );
}

#[test]
fn signals_kept_hold_their_command_and_signals_dropped_go_safe() {
    let mut t = table();
    t.set_input(0, Some(Value::Bool(true)));
    let mut rt = plant(&t);
    rt.tick_with(ms(0), &[], &mut t);
    assert_eq!(cmd(&t, "motor"), Value::Bool(true));
    assert_eq!(cmd(&t, "lamp"), Value::Bool(true));
    assert_eq!(cmd(&t, "horn"), Value::Bool(false));

    // line stops driving lamp and inverts motor.
    let v2 = "
PROGRAM line
VAR_INPUT start : BOOL; END_VAR
VAR_OUTPUT motor : BOOL; END_VAR
VAR n : UINT; END_VAR
  n := n + 1;
  motor := NOT start;
END_PROGRAM";
    let carry = rt
        .swap(module("line", v2, 10, Window::new(0, 0)), &mut t)
        .unwrap();
    assert_eq!(carry.kept, ["START", "MOTOR", "N"]);
    assert_eq!(carry.dropped, ["LAMP"]);
    assert_eq!(cmd(&t, "motor"), Value::Bool(true), "held until the first scan");
    assert_eq!(cmd(&t, "lamp"), Value::Bool(false), "nobody drives it: safe");
    assert_eq!(cmd(&t, "horn"), Value::Bool(false), "other modules untouched");
    rt.tick_with(ms(10), &[], &mut t);
    assert_eq!(cmd(&t, "motor"), Value::Bool(false));
    assert_eq!(get(&rt, "line", "n"), Some(St::Int(2)));
    assert_eq!(get(&rt, "horn", "n"), Some(St::Int(2)));

    // Now horn may take lamp, as nobody drives it.
    let lamp =
        "PROGRAM horn VAR_OUTPUT horn : BOOL; lamp : BOOL; END_VAR horn := FALSE; lamp := TRUE; END_PROGRAM";
    rt.swap(module("horn", lamp, 10, Window::new(0, 0)), &mut t)
        .unwrap();
    rt.tick_with(ms(20), &[], &mut t);
    assert_eq!(cmd(&t, "lamp"), Value::Bool(true));
}

#[test]
fn a_held_module_swapped_is_running_and_holds_again_if_its_input_is_still_missing() {
    let mut t = table();
    let mut rt = plant(&t);
    rt.tick_with(ms(0), &[], &mut t);
    assert_eq!(rt.status("line"), Some(Status::Held));
    rt.swap(module("line", LINE, 10, Window::new(0, 0)), &mut t)
        .unwrap();
    assert_eq!(rt.status("line"), Some(Status::Running));
    rt.tick_with(ms(10), &[], &mut t);
    assert_eq!(rt.status("line"), Some(Status::Held));
    assert_eq!(cmd(&t, "motor"), Value::Bool(false));
    t.set_input(0, Some(Value::Bool(true)));
    rt.tick_with(ms(20), &[], &mut t);
    assert_eq!(rt.status("line"), Some(Status::Running));
    assert_eq!(cmd(&t, "motor"), Value::Bool(true));
}

// ---- property: random swap sequences ----

/// A generated replacement: which module, its `%Q` window, and which
/// outputs it drives (motor, lamp, horn).
#[derive(Debug, Clone)]
struct Swap {
    target: usize,
    start: usize,
    len: usize,
    drives: [bool; 3],
    reads_start: bool,
    period: u64,
}

fn swap_strategy() -> impl Strategy<Value = Swap> {
    (
        0usize..4,
        0usize..10,
        0usize..4,
        any::<[bool; 3]>(),
        any::<bool>(),
        0u64..30,
    )
        .prop_map(|(target, start, len, drives, reads_start, period)| Swap {
            target,
            start,
            len,
            drives,
            reads_start,
            period,
        })
}

const NAMES: [&str; 3] = ["a", "b", "c"];
const OUTS: [&str; 3] = ["motor", "lamp", "horn"];

fn generated(s: &Swap) -> Module {
    let mut src = String::from("PROGRAM g\n");
    if s.reads_start {
        src.push_str("VAR_INPUT start : BOOL; END_VAR\n");
    }
    let outs: Vec<&str> = OUTS
        .iter()
        .zip(s.drives)
        .filter(|(_, d)| *d)
        .map(|(o, _)| *o)
        .collect();
    if !outs.is_empty() {
        src += &["VAR_OUTPUT ", &outs.join(", "), " : BOOL; END_VAR\n"].concat();
    }
    src.push_str("VAR n : UINT; END_VAR\n  n := n + 1;\n");
    for o in &outs {
        src += &["  ", o, " := TRUE;\n"].concat();
    }
    if s.len > 0 {
        src.push_str("  %QB0 := 16#5A;\n");
    }
    src.push_str("END_PROGRAM");
    let name = NAMES.get(s.target).copied().unwrap_or("nobody");
    let program = compile(&src, ImageSizes { i: 0, q: s.len, m: 2 }).unwrap();
    Module {
        name: name.into(),
        program,
        period: ms(s.period),
        i: Window::new(0, 0),
        q: Window::new(s.start, s.len),
        params: Vec::new(),
    }
}

/// Whether the runtime as it stands has two modules on one output byte or
/// one output signal, or a byte nobody owns that is not zero.
fn check_invariants(rt: &Runtime, t: &SignalTable) -> Result<(), TestCaseError> {
    let mods: Vec<&Module> = rt.names().map(|n| rt.module(n).unwrap()).collect();
    let mut owner = [None; IMAGE];
    for (k, m) in mods.iter().enumerate() {
        for (b, o) in owner.iter_mut().enumerate().skip(m.q.start).take(m.q.len) {
            prop_assert!(o.is_none(), "byte {} driven twice", b);
            *o = Some(k);
        }
    }
    for (b, o) in owner.iter().enumerate() {
        if o.is_none() {
            prop_assert_eq!(rt.outputs()[b], 0, "unowned byte {} is not zero", b);
        }
    }
    for out in OUTS {
        let writers = mods
            .iter()
            .filter(|m| m.program.ports().iter().any(|p| p.name.eq_ignore_ascii_case(out)))
            .count();
        prop_assert!(writers <= 1, "{} has {} writers", out, writers);
        if writers == 0 {
            let safe = if out == "horn" {
                Value::Bool(true)
            } else {
                Value::Bool(false)
            };
            prop_assert_eq!(cmd(t, out), safe, "{} has no writer but is not safe", out);
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Random swaps between ticks: never a panic, never two modules on one
    /// byte or one signal, and a refused swap leaves the runtime as it was.
    #[test]
    fn random_swaps_never_let_two_modules_drive_one_output(
        swaps in prop::collection::vec((swap_strategy(), any::<bool>()), 1..24),
    ) {
        let mut t = table();
        let initial = [Swap { target: 0, start: 0, len: 2, drives: [true, false, false], reads_start: true, period: 10 },
            Swap { target: 1, start: 2, len: 2, drives: [false, true, false], reads_start: false, period: 20 },
            Swap { target: 2, start: 4, len: 0, drives: [false, false, false], reads_start: false, period: 10 }];
        let mut rt = Runtime::with_signals(0, IMAGE, initial.iter().map(generated).collect(), &t).unwrap();
        let mut now = 0;
        for (s, start_on) in &swaps {
            t.set_input(0, start_on.then_some(Value::Bool(true)));
            rt.tick_with(ms(now), &[], &mut t);
            now += 7;
            check_invariants(&rt, &t)?;
            let name = NAMES.get(s.target).copied().unwrap_or("nobody");
            let before = rt.module(name).map(|m| (m.q, m.period, m.program.ports()));
            let outputs = rt.outputs().to_vec();
            let commands: Vec<_> = (0..t.len()).map(|i| t.command(i)).collect();
            match rt.swap(generated(s), &mut t) {
                Ok(_) => {
                    let m = rt.module(name).unwrap();
                    prop_assert_eq!(m.q, Window::new(s.start, s.len));
                    prop_assert_eq!(rt.status(name), Some(Status::Running));
                    for (b, v) in outputs.iter().enumerate() {
                        if m.q.start <= b && b < m.q.start + m.q.len {
                            prop_assert_eq!(rt.outputs()[b], *v, "kept byte {} glitched", b);
                        }
                    }
                }
                Err(errs) => {
                    prop_assert!(!errs.is_empty());
                    prop_assert_eq!(rt.module(name).map(|m| (m.q, m.period, m.program.ports())), before);
                    prop_assert_eq!(rt.outputs(), &outputs[..]);
                    prop_assert_eq!((0..t.len()).map(|i| t.command(i)).collect::<Vec<_>>(), commands);
                }
            }
            check_invariants(&rt, &t)?;
        }
    }
}

// ---- threaded links ----

fn rio_map(sim: &Sim) -> IoMap {
    IoMap {
        devices: vec![Device {
            id: "rio".into(),
            driver: "modbus-tcp".into(),
            endpoint: sim.endpoint(),
            params: [("timeout_ms".to_string(), "300".to_string())].into(),
        }],
        signals: vec![
            Signal {
                name: "start".into(),
                ty: Ty::Bool,
                dir: Dir::In,
                binding: Binding::Device {
                    device: "rio".into(),
                    point: "di:1".into(),
                },
            },
            Signal {
                name: "motor".into(),
                ty: Ty::Bool,
                dir: Dir::Out,
                binding: Binding::Device {
                    device: "rio".into(),
                    point: "coil:0".into(),
                },
            },
            local("lamp", Ty::Bool, Dir::Out, "do:0"),
        ],
    }
}

/// Same outputs as [`LINE`], a different body: the swap must not move them.
const LINE_V2: &str = "
PROGRAM line
VAR_INPUT start : BOOL; END_VAR
VAR_OUTPUT motor : BOOL; lamp : BOOL; END_VAR
VAR n : UINT; spare : INT; END_VAR
  n := n + 1;
  IF start THEN motor := TRUE; ELSE motor := FALSE; END_IF;
  lamp := n > 0;
END_PROGRAM";

#[test]
fn swapping_under_live_links_drops_no_writes_and_glitches_nothing() {
    let sim = Sim::start(State::new(16)).unwrap();
    let m = rio_map(&sim);
    let table = Arc::new(Mutex::new(SignalTable::new(&m, &BTreeMap::new()).unwrap()));
    let panel = Channels::new();
    let mut rt = Runtime::with_signals(
        0,
        0,
        vec![module("line", LINE, 10, Window::new(0, 0))],
        &table.lock().unwrap(),
    )
    .unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let handles: Vec<_> = links(&m, helmsman_io::open, Box::new(panel.clone()))
        .unwrap()
        .into_iter()
        .map(|l| spawn(l, Arc::clone(&table), Arc::clone(&stop), |_, _| {}))
        .collect();
    sim.state().discrete[1] = true;
    let start = Instant::now();
    let deadline = start + Duration::from_secs(5);
    while !(sim.state().coils[0] && panel.output(0)) {
        assert!(Instant::now() < deadline, "the line never came up");
        rt.tick_with(start.elapsed(), &[], &mut table.lock().unwrap());
        std::thread::sleep(ms(2));
    }
    // Swap back and forth between scans while the links write and read.
    let mut glitches = Vec::new();
    for k in 0..40 {
        {
            let mut t = table.lock().unwrap();
            let src = if k % 2 == 0 { LINE_V2 } else { LINE };
            rt.swap(module("line", src, 10, Window::new(0, 0)), &mut t)
                .unwrap();
            if cmd(&t, "motor") != Value::Bool(true) || cmd(&t, "lamp") != Value::Bool(true) {
                glitches.push(format!("swap {k}: commands moved"));
            }
            rt.tick_with(start.elapsed(), &[], &mut t);
        }
        std::thread::sleep(ms(2));
        if !sim.state().coils[0] || !panel.output(0) {
            glitches.push(format!("swap {k}: an output dropped"));
        }
    }
    assert_eq!(glitches, Vec::<String>::new());
    let scans = rt.stats("line").unwrap().scans;
    assert_eq!(
        get(&rt, "line", "n"),
        Some(St::Int(scans.into())),
        "the scan count carried through 40 swaps"
    );

    // A swap to a program that inverts the motor reaches the device.
    let inverted = "
PROGRAM line
VAR_INPUT start : BOOL; END_VAR
VAR_OUTPUT motor : BOOL; lamp : BOOL; END_VAR
  motor := NOT start; lamp := TRUE;
END_PROGRAM";
    rt.swap(
        module("line", inverted, 10, Window::new(0, 0)),
        &mut table.lock().unwrap(),
    )
    .unwrap();
    while sim.state().coils[0] {
        assert!(Instant::now() < deadline, "the new program's write never arrived");
        rt.tick_with(start.elapsed(), &[], &mut table.lock().unwrap());
        std::thread::sleep(ms(2));
    }
    assert!(panel.output(0));
    stop.store(true, Ordering::Relaxed);
    for h in handles {
        h.join().unwrap();
    }
}
