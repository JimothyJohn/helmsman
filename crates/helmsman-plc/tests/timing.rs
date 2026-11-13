//! Scan load: each module's execution time and start lateness over its
//! newest `TIMING_WINDOW` scans (min, mean, max, p99), and when it last
//! overran. Execution time is read through a scripted [`Clock`], so every
//! number here is exact.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use helmsman_iec::{ImageSizes, compile};
use helmsman_io::{Binding, Dir, IoMap, Signal, Ty, Value};
use helmsman_plc::{Clock, Module, Runtime, SignalTable, Spread, Status, TIMING_WINDOW, Timing, Window};
use proptest::prelude::*;

const COUNTER: &str = "
PROGRAM counter
VAR n : USINT; o AT %QB0 : USINT; END_VAR
  n := n + 1;
  o := n;
END_PROGRAM";

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn us(n: u64) -> Duration {
    Duration::from_micros(n)
}

fn module(name: &str, period_ms: u64, q: usize) -> Module {
    let sizes = ImageSizes { i: 0, q: 1, m: 16 };
    Module {
        name: name.into(),
        program: compile(COUNTER, sizes).unwrap().into(),
        period: ms(period_ms),
        i: Window::new(0, 0),
        q: Window::new(q, 1),
        params: Vec::new(),
    }
}

/// A clock whose scans take the given times in turn (then the last one
/// again): each scan reads it twice, so it advances by the scan's time
/// on the second read of each pair. Counts its reads.
fn scripted(times: Vec<Duration>) -> (Clock, Arc<AtomicU64>) {
    let reads = Arc::new(AtomicU64::new(0));
    let state = Mutex::new((Duration::ZERO, 0usize));
    let r = Arc::clone(&reads);
    let clock = Clock::new(move || {
        let n = r.fetch_add(1, Ordering::SeqCst);
        let mut s = state.lock().unwrap();
        if n % 2 == 1 {
            let k = s.1.min(times.len() - 1);
            s.0 += times[k];
            s.1 += 1;
        }
        s.0
    });
    (clock, reads)
}

#[test]
fn no_scan_yet_is_an_empty_window() {
    let rt = Runtime::new(4, 4, vec![module("a", 10, 0)]).unwrap();
    assert_eq!(rt.timing("a"), Some(Timing::default()));
    assert_eq!(rt.timing("nobody"), None);
}

#[test]
fn execution_time_is_what_the_clock_saw_around_each_scan() {
    let mut rt = Runtime::new(4, 4, vec![module("a", 10, 0)]).unwrap();
    let (clock, _) = scripted(vec![us(40), us(10), us(70), us(20)]);
    rt.set_clock(clock);
    for t in 0..4 {
        rt.tick(ms(t * 10), &[]);
    }
    let t = rt.timing("a").unwrap();
    assert_eq!(t.scans, 4);
    assert_eq!(
        t.exec,
        Spread {
            min: us(10),
            mean: us(35),
            max: us(70),
            p99: us(70),
        }
    );
    assert_eq!(t.late, Spread::default(), "every tick came on its deadline");
    assert_eq!(t.last_overrun, None);
}

#[test]
fn lateness_is_the_tick_minus_the_deadline() {
    let mut rt = Runtime::new(4, 4, vec![module("a", 10, 0)]).unwrap();
    rt.set_clock(scripted(vec![us(5)]).0);
    // Due at 0, 10, 20: started 1, 13 and 20 ms in.
    rt.tick(ms(1), &[]);
    rt.tick(ms(13), &[]);
    rt.tick(ms(20), &[]);
    let t = rt.timing("a").unwrap();
    assert_eq!(t.scans, 3);
    assert_eq!(t.late.min, ms(0));
    assert_eq!(t.late.max, ms(3));
    assert_eq!(t.late.mean, ms(4) / 3);
    assert_eq!(t.late.p99, ms(3));
}

#[test]
fn an_overrun_is_stamped_with_its_tick() {
    let mut rt = Runtime::new(4, 4, vec![module("a", 10, 0)]).unwrap();
    rt.set_clock(scripted(vec![us(5)]).0);
    rt.tick(ms(0), &[]);
    rt.tick(ms(35), &[]);
    let t = rt.timing("a").unwrap();
    assert_eq!(rt.stats("a").unwrap().overruns, 2);
    assert_eq!(t.last_overrun, Some(ms(35)));
    assert_eq!(t.late.max, ms(25), "due at 10, started at 35");
    rt.tick(ms(40), &[]);
    assert_eq!(
        rt.timing("a").unwrap().last_overrun,
        Some(ms(35)),
        "an on-time scan keeps the last overrun's time"
    );
}

#[test]
fn the_window_keeps_the_newest_scans_only() {
    let mut rt = Runtime::new(4, 4, vec![module("a", 1, 0)]).unwrap();
    // One slow scan, then a full window of fast ones pushes it out.
    let mut times = vec![ms(9)];
    times.extend(std::iter::repeat_n(us(2), TIMING_WINDOW));
    rt.set_clock(scripted(times).0);
    rt.tick(ms(0), &[]);
    assert_eq!(rt.timing("a").unwrap().exec.max, ms(9));
    for t in 1..=TIMING_WINDOW as u64 {
        rt.tick(ms(t), &[]);
    }
    let t = rt.timing("a").unwrap();
    assert_eq!(t.scans, TIMING_WINDOW);
    assert_eq!(t.exec.max, us(2), "the slow scan left the window");
    assert_eq!(rt.stats("a").unwrap().scans, TIMING_WINDOW as u64 + 1);
}

#[test]
fn p99_is_the_nearest_rank() {
    let mut rt = Runtime::new(4, 4, vec![module("a", 1, 0)]).unwrap();
    // 1000 scans of 1..=1000 µs: the 990th smallest is the p99.
    let times: Vec<Duration> = (1..=1000).map(us).collect();
    rt.set_clock(scripted(times).0);
    for t in 0..1000 {
        rt.tick(ms(t), &[]);
    }
    let e = rt.timing("a").unwrap().exec;
    assert_eq!((e.min, e.max, e.p99), (us(1), us(1000), us(990)));
    assert_eq!(e.mean, Duration::from_nanos(500_500));
}

#[test]
fn a_held_module_reads_no_clock_and_records_nothing() {
    let src = "
PROGRAM h
VAR_INPUT start : BOOL; END_VAR
VAR_OUTPUT motor : BOOL; END_VAR
  motor := start;
END_PROGRAM";
    let map = IoMap {
        devices: vec![],
        signals: vec![
            local("start", Ty::Bool, Dir::In, "di:0"),
            local("motor", Ty::Bool, Dir::Out, "do:0"),
        ],
    };
    let mut t = SignalTable::new(&map, &BTreeMap::new()).unwrap();
    let sizes = ImageSizes { i: 0, q: 0, m: 4 };
    let m = Module {
        name: "h".into(),
        program: compile(src, sizes).unwrap().into(),
        period: ms(10),
        i: Window::new(0, 0),
        q: Window::new(0, 0),
        params: Vec::new(),
    };
    let mut rt = Runtime::with_signals(0, 0, vec![m], &t).unwrap();
    let (clock, reads) = scripted(vec![us(4)]);
    rt.set_clock(clock);
    rt.tick_with(ms(0), &[], &mut t);
    rt.tick_with(ms(10), &[], &mut t);
    assert_eq!(rt.status("h"), Some(Status::Held));
    assert_eq!(reads.load(Ordering::SeqCst), 0, "no scan, no clock read");
    assert_eq!(rt.timing("h").unwrap().scans, 0);
    t.set_input(0, Some(Value::Bool(true)));
    rt.tick_with(ms(20), &[], &mut t);
    assert_eq!(reads.load(Ordering::SeqCst), 2, "one scan, two reads");
    let tm = rt.timing("h").unwrap();
    assert_eq!((tm.scans, tm.exec.max, tm.late.max), (1, us(4), ms(0)));
}

fn local(name: &str, ty: Ty, dir: Dir, channel: &str) -> Signal {
    Signal {
        name: name.into(),
        ty,
        dir,
        binding: Binding::Local {
            channel: channel.into(),
        },
        unit: None,
        poll: None,
    }
}

#[test]
fn timing_is_per_module_and_kept_across_a_restart_and_a_deploy() {
    let mut rt = Runtime::new(4, 4, vec![module("a", 10, 0), module("b", 10, 1)]).unwrap();
    // In scan order: a's first scan takes 3 µs, b's 7 µs.
    rt.set_clock(scripted(vec![us(3), us(7)]).0);
    rt.tick(ms(0), &[]);
    rt.restart("a").unwrap();
    let mut t = SignalTable::default();
    rt.reconfigure(
        vec![module("b", 10, 1), module("a", 10, 0), module("c", 10, 2)],
        &mut t,
    )
    .unwrap();
    let a = rt.timing("a").unwrap();
    let b = rt.timing("b").unwrap();
    assert_eq!((a.scans, a.exec.max), (1, us(3)));
    assert_eq!((b.scans, b.exec.max), (1, us(7)));
    assert_eq!(
        rt.timing("c"),
        Some(Timing::default()),
        "a new module starts empty"
    );
}

proptest! {
    /// Whatever the scan times: min <= p99 <= max, min <= mean <= max,
    /// and the window holds min(scans, TIMING_WINDOW).
    #[test]
    fn spreads_are_ordered(times in prop::collection::vec(0u64..5_000_000, 1..1200)) {
        let mut rt = Runtime::new(4, 4, vec![module("a", 1, 0)]).unwrap();
        let n = times.len();
        rt.set_clock(scripted(times.into_iter().map(Duration::from_nanos).collect()).0);
        for t in 0..n as u64 {
            rt.tick(ms(t), &[]);
        }
        let t = rt.timing("a").unwrap();
        prop_assert_eq!(t.scans, n.min(TIMING_WINDOW));
        for s in [t.exec, t.late] {
            prop_assert!(s.min <= s.mean && s.mean <= s.max, "{:?}", s);
            prop_assert!(s.min <= s.p99 && s.p99 <= s.max, "{:?}", s);
        }
    }
}
