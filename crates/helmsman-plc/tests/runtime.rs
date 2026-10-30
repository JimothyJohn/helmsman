//! The multi-module runtime's contract: several ST modules share one process
//! image, each with its own scan rate, its own `%I`/`%Q` windows and its own
//! `%M`, and a module that faults or overruns harms nobody else.

use std::time::Duration;

use helmsman_iec::{Fault, FaultKind, ImageSizes, Value, compile};
use helmsman_plc::{ConfigError, Module, Runtime, Status, Window};
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
    }
}

/// Counts its scans and writes the count to the first byte of its outputs.
const COUNTER: &str = "
PROGRAM counter
VAR n : USINT; o AT %QB0 : USINT; END_VAR
  n := n + 1;
  o := n;
END_PROGRAM";

/// Counts like [`COUNTER`], then divides by zero on its third scan, after
/// it has written its outputs.
const FAULTS_ON_THIRD: &str = "
PROGRAM faulty
VAR n, d : USINT; o AT %QB0 : USINT; END_VAR
  n := n + 1;
  o := 16#FF;
  d := 10 / (3 - n);
END_PROGRAM";

const ENDLESS: &str = "
PROGRAM spin
VAR o AT %QB0 : USINT; END_VAR
  o := 16#FF;
  WHILE TRUE DO o := o; END_WHILE;
END_PROGRAM";

/// Copies its first input byte to its first output byte.
const COPY: &str = "
PROGRAM copy
VAR a AT %IB0 : USINT; o AT %QB0 : USINT; END_VAR
  o := a;
END_PROGRAM";

/// What a tick that faults nothing returns.
fn none() -> Vec<(String, Fault)> {
    Vec::new()
}

fn count(rt: &Runtime, name: &str) -> u64 {
    match rt.program(name).and_then(|p| p.get("n")) {
        Some(Value::Int(n)) => u64::try_from(n).unwrap(),
        other => panic!("{name}.n = {other:?}"),
    }
}

#[test]
fn modules_write_their_own_output_windows_relative_to_the_window() {
    let mut rt = Runtime::new(
        IMAGE,
        IMAGE,
        vec![
            module("a", COUNTER, 10, Window::new(0, 0), Window::new(0, 1)),
            module("b", COUNTER, 10, Window::new(0, 0), Window::new(4, 2)),
        ],
    )
    .unwrap();
    assert_eq!(rt.tick(ms(0), &[]), none());
    let q = rt.outputs();
    assert_eq!(q.len(), IMAGE);
    assert_eq!(q[0], 1, "a's %QB0 is byte 0");
    assert_eq!(q[4], 1, "b's %QB0 is byte 4, the start of its window");
    assert!(
        q.iter().enumerate().all(|(n, b)| n == 0 || n == 4 || *b == 0),
        "bytes no module owns stay zero: {q:?}"
    );
}

#[test]
fn each_module_scans_at_its_own_rate() {
    let mut rt = Runtime::new(
        IMAGE,
        IMAGE,
        vec![
            module("fast", COUNTER, 10, Window::new(0, 0), Window::new(0, 1)),
            module("slow", COUNTER, 30, Window::new(0, 0), Window::new(1, 1)),
        ],
    )
    .unwrap();
    for t in (0..=90).step_by(10) {
        rt.tick(ms(t), &[]);
    }
    assert_eq!(count(&rt, "fast"), 10, "0, 10, ... 90");
    assert_eq!(count(&rt, "slow"), 4, "0, 30, 60, 90");
    assert_eq!(rt.next_deadline(), Some(ms(100)));
}

#[test]
fn input_windows_may_overlap_and_are_relative() {
    let mut rt = Runtime::new(
        IMAGE,
        IMAGE,
        vec![
            module("a", COPY, 10, Window::new(3, 1), Window::new(0, 1)),
            module("b", COPY, 10, Window::new(3, 2), Window::new(1, 1)),
        ],
    )
    .unwrap();
    let mut inputs = [0u8; IMAGE];
    inputs[3] = 0x5A;
    rt.tick(ms(0), &inputs);
    assert_eq!(&rt.outputs()[..2], &[0x5A, 0x5A]);
}

#[test]
fn short_or_long_input_images_neither_panic_nor_leak() {
    let mut rt = Runtime::new(
        IMAGE,
        IMAGE,
        vec![module("a", COPY, 10, Window::new(31, 1), Window::new(0, 1))],
    )
    .unwrap();
    rt.tick(ms(0), &[7; 4]);
    assert_eq!(rt.outputs()[0], 0, "missing input bytes read as zero");
    rt.tick(ms(10), &[9; 100]);
    assert_eq!(rt.outputs()[0], 9);
}

#[test]
fn a_fault_zeroes_only_that_modules_outputs_and_stops_only_it() {
    let mut rt = Runtime::new(
        IMAGE,
        IMAGE,
        vec![
            module("bad", FAULTS_ON_THIRD, 10, Window::new(0, 0), Window::new(0, 2)),
            module("good", COUNTER, 10, Window::new(0, 0), Window::new(2, 1)),
        ],
    )
    .unwrap();
    assert_eq!(rt.tick(ms(0), &[]), none());
    assert_eq!(rt.tick(ms(10), &[]), none());
    assert_eq!(rt.outputs()[0], 0xFF);

    let faults = rt.tick(ms(20), &[]);
    assert_eq!(faults.len(), 1, "{faults:?}");
    assert_eq!(faults[0].0, "bad");
    assert_eq!(faults[0].1.kind, FaultKind::DivisionByZero);
    assert_eq!(
        &rt.outputs()[..2],
        &[0, 0],
        "bad's window zeroed, partial scan not published"
    );
    assert_eq!(rt.outputs()[2], 3, "good scanned in the same tick");
    assert!(
        matches!(rt.status("bad"), Some(Status::Faulted(f)) if f.line == 6),
        "the division, line 6"
    );
    assert_eq!(rt.status("good"), Some(Status::Running));

    for t in (30..=100).step_by(10) {
        assert_eq!(rt.tick(ms(t), &[]), none(), "a stopped module faults once");
    }
    assert_eq!(&rt.outputs()[..2], &[0, 0], "and stays zeroed");
    assert_eq!(count(&rt, "bad"), 3, "never scanned again");
    assert_eq!(count(&rt, "good"), 11);
}

#[test]
fn a_runaway_module_hits_its_watchdog_and_the_rest_carry_on() {
    let mut spin = module("spin", ENDLESS, 10, Window::new(0, 0), Window::new(0, 1));
    spin.program.set_budget(10_000);
    let mut rt = Runtime::new(
        IMAGE,
        IMAGE,
        vec![
            spin,
            module("good", COUNTER, 10, Window::new(0, 0), Window::new(1, 1)),
        ],
    )
    .unwrap();
    let faults = rt.tick(ms(0), &[]);
    assert_eq!(faults.len(), 1);
    assert_eq!(faults[0].1.kind, FaultKind::Watchdog);
    assert_eq!(&rt.outputs()[..2], &[0, 1]);
    rt.tick(ms(10), &[]);
    assert_eq!(rt.outputs()[1], 2);
}

#[test]
fn when_every_module_has_stopped_there_is_no_deadline() {
    let mut rt = Runtime::new(
        IMAGE,
        IMAGE,
        vec![module(
            "bad",
            FAULTS_ON_THIRD,
            10,
            Window::new(0, 0),
            Window::new(0, 1),
        )],
    )
    .unwrap();
    for t in [0, 10, 20] {
        rt.tick(ms(t), &[]);
    }
    assert_eq!(rt.next_deadline(), None);
    assert_eq!(Runtime::new(IMAGE, IMAGE, vec![]).unwrap().next_deadline(), None);
}

#[test]
fn a_late_tick_skips_missed_periods_instead_of_bursting() {
    let mut rt = Runtime::new(
        IMAGE,
        IMAGE,
        vec![module("a", COUNTER, 10, Window::new(0, 0), Window::new(0, 1))],
    )
    .unwrap();
    rt.tick(ms(0), &[]);
    rt.tick(ms(35), &[]);
    assert_eq!(count(&rt, "a"), 2, "one late scan, not three");
    let s = rt.stats("a").unwrap();
    assert_eq!((s.scans, s.overruns), (2, 2), "deadlines 20 and 30 were skipped");
    assert_eq!(rt.next_deadline(), Some(ms(40)), "back on the original phase");
}

#[test]
fn a_tick_before_any_deadline_does_nothing() {
    let mut rt = Runtime::new(
        IMAGE,
        IMAGE,
        vec![module("a", COUNTER, 10, Window::new(0, 0), Window::new(0, 1))],
    )
    .unwrap();
    rt.tick(ms(0), &[]);
    rt.tick(ms(5), &[]);
    rt.tick(ms(3), &[]);
    assert_eq!(count(&rt, "a"), 1);
}

#[test]
fn a_clock_at_the_end_of_time_does_not_panic() {
    let mut rt = Runtime::new(
        IMAGE,
        IMAGE,
        vec![module("a", COUNTER, 10, Window::new(0, 0), Window::new(0, 1))],
    )
    .unwrap();
    rt.tick(ms(0), &[]);
    rt.tick(Duration::MAX, &[]);
    rt.tick(Duration::MAX, &[]);
    assert!(count(&rt, "a") >= 2);
}

#[test]
fn two_modules_fighting_for_an_output_byte_are_refused() {
    let err = Runtime::new(
        IMAGE,
        IMAGE,
        vec![
            module("a", COUNTER, 10, Window::new(0, 0), Window::new(0, 4)),
            module("b", COUNTER, 10, Window::new(0, 0), Window::new(4, 1)),
            module("c", COUNTER, 10, Window::new(0, 0), Window::new(3, 2)),
        ],
    )
    .unwrap_err();
    assert_eq!(
        err,
        vec![
            ConfigError::OutputOverlap {
                a: "a".into(),
                b: "c".into()
            },
            ConfigError::OutputOverlap {
                a: "b".into(),
                b: "c".into()
            },
        ],
        "a and b only touch; c overlaps both"
    );
}

#[test]
fn every_configuration_problem_is_reported() {
    let mut zero = module("z", COUNTER, 10, Window::new(0, 0), Window::new(9, 1));
    zero.period = Duration::ZERO;
    let mut mismatch = module("m", COUNTER, 10, Window::new(0, 0), Window::new(10, 1));
    mismatch.q = Window::new(10, 2);
    let err = Runtime::new(
        IMAGE,
        IMAGE,
        vec![
            module("a", COUNTER, 10, Window::new(0, 0), Window::new(0, 1)),
            module("a", COUNTER, 10, Window::new(0, 0), Window::new(1, 1)),
            module("wide", COPY, 10, Window::new(30, 4), Window::new(2, 1)),
            module("far", COUNTER, 10, Window::new(0, 0), Window::new(usize::MAX, 1)),
            zero,
            mismatch,
        ],
    )
    .unwrap_err();
    assert_eq!(
        err,
        vec![
            ConfigError::DuplicateName("a".into()),
            ConfigError::OutsideImage {
                module: "wide".into(),
                area: 'I'
            },
            ConfigError::OutsideImage {
                module: "far".into(),
                area: 'Q'
            },
            ConfigError::BadPeriod("z".into()),
            ConfigError::SizeMismatch {
                module: "m".into(),
                area: 'Q'
            },
        ]
    );
    let shown: Vec<String> = err.iter().map(ToString::to_string).collect();
    assert_eq!(
        shown,
        [
            "module \"a\" defined twice",
            "wide: %I window reaches past the image",
            "far: %Q window reaches past the image",
            "z: scan period must be 1 ns ..= 24 h",
            "m: compiled for a different %Q size than its window",
        ]
    );
}

#[test]
fn a_module_cannot_address_past_its_window() {
    let sizes = ImageSizes { i: 0, q: 1, m: 0 };
    let escape = "PROGRAM esc VAR o AT %QX1.0 : BOOL; END_VAR o := TRUE; END_PROGRAM";
    assert!(
        compile(escape, sizes).is_err(),
        "the checker rejects %QX1.0 in a 1-byte window"
    );
    let read = "PROGRAM esc VAR o AT %QB0 : BYTE; END_VAR o := %IB0; END_PROGRAM";
    assert!(compile(read, sizes).is_err(), "and any %I with no input window");
}

/// A program that writes `tag` to its first output byte each scan and
/// divides by zero on scan `fault_at` (never, if 0).
fn tagged(tag: u8, fault_at: u8) -> String {
    let fault = if fault_at == 0 {
        String::new()
    } else {
        format!("IF n = {fault_at} THEN d := 1 / z; END_IF;")
    };
    format!(
        "PROGRAM t VAR n, d, z : USINT; o AT %QB0 : USINT; END_VAR
           n := n + 1; o := {tag}; {fault}
         END_PROGRAM"
    )
}

proptest! {
    /// Refusal happens exactly when two output windows share a byte.
    #[test]
    fn output_overlap_is_refused_iff_windows_share_a_byte(
        windows in prop::collection::vec((0usize..12, 0usize..5), 1..5)
    ) {
        let modules: Vec<Module> = windows
            .iter()
            .enumerate()
            .map(|(n, &(start, len))| {
                let sizes = ImageSizes { i: 0, q: len, m: 0 };
                let src = "PROGRAM p END_PROGRAM";
                Module {
                    name: format!("m{n}"),
                    program: compile(src, sizes).unwrap(),
                    period: ms(10),
                    i: Window::new(0, 0),
                    q: Window::new(start, len),
                }
            })
            .collect();
        // Brute force: who owns each byte, every pair that shares one.
        let mut owners: Vec<Vec<usize>> = vec![Vec::new(); 16];
        let mut expect = std::collections::BTreeSet::new();
        for (n, &(start, len)) in windows.iter().enumerate() {
            for byte in &mut owners[start..start + len] {
                expect.extend(byte.iter().map(|&p| (p, n)));
                byte.push(n);
            }
        }
        let expect: Vec<(usize, usize)> = expect.into_iter().collect();
        let got = match Runtime::new(0, 16, modules) {
            Ok(_) => Vec::new(),
            Err(errs) => errs
                .into_iter()
                .map(|e| match e {
                    ConfigError::OutputOverlap { a, b } => {
                        (a[1..].parse::<usize>().unwrap(), b[1..].parse::<usize>().unwrap())
                    }
                    other => panic!("{other}"),
                })
                .collect(),
        };
        prop_assert_eq!(got, expect);
    }

    /// Every deadline up to `now` is either scanned or counted as skipped,
    /// whatever the tick times, and the next deadline is in the future.
    #[test]
    fn every_deadline_is_scanned_or_counted(
        period in 1u64..50,
        steps in prop::collection::vec(0u64..120, 1..30),
    ) {
        let mut rt = Runtime::new(
            0,
            1,
            vec![module("a", COUNTER, period, Window::new(0, 0), Window::new(0, 1))],
        ).unwrap();
        let mut now = 0;
        for step in steps {
            now += step;
            rt.tick(ms(now), &[]);
            let s = rt.stats("a").unwrap();
            prop_assert_eq!(s.scans + s.overruns, now / period + 1);
            prop_assert!(rt.next_deadline().unwrap() > ms(now));
        }
    }

    /// With modules faulting at arbitrary scans, after every tick each
    /// stopped module's window is zero and each running one shows its tag.
    #[test]
    fn faults_never_leak_across_windows(
        mods in prop::collection::vec((1u64..40, 0u8..6), 1..6),
        ticks in 1u64..30,
    ) {
        let modules: Vec<Module> = mods
            .iter()
            .enumerate()
            .map(|(n, &(period, fault_at))| {
                let tag = u8::try_from(n + 1).unwrap();
                module(&format!("m{n}"), &tagged(tag, fault_at), period, Window::new(0, 0), Window::new(n, 1))
            })
            .collect();
        let mut rt = Runtime::new(0, mods.len(), modules).unwrap();
        for t in 0..ticks {
            rt.tick(ms(t * 7), &[]);
            for n in 0..mods.len() {
                let name = format!("m{n}");
                let want = match rt.status(&name).unwrap() {
                    Status::Running => u8::try_from(n + 1).unwrap(),
                    Status::Faulted(_) | Status::Held => 0,
                };
                prop_assert_eq!(rt.outputs()[n], want, "{} at {} ms", name, t * 7);
            }
        }
    }
}
