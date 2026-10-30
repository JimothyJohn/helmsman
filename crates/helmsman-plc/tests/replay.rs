//! Sim replay's contract: the box records what its modules saw (a bounded
//! RAM window of input signal values and their presence), and a module,
//! proposed or running, is replayed offline against that window on the
//! recorded timeline. The replay is deterministic, honours hold-on-missing
//! and the module's own scan period, and reports each output's command
//! over time, the module's held and faulted spans, and how a proposal
//! differs from what is running.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use helmsman_iec::{FaultKind, ImageSizes, compile};
use helmsman_io::local::Channels;
use helmsman_io::sim::{Fault as SimFault, Sim, State};
use helmsman_io::{Binding, Device, Dir, IoMap, Signal, Ty, Value};
use helmsman_plc::calendar::{Source, load};
use helmsman_plc::link::links;
use helmsman_plc::record::{Channel, Event, Kind, RecordConfig, Recorder, Recording};
use helmsman_plc::replay::{ReplayError, Timeline, compare, propose, replay};
use helmsman_plc::{Module, Runtime, SignalTable, Status, Window};
use proptest::prelude::*;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn module(name: &str, period_ms: u64, src: &str) -> Module {
    let program = compile(src, ImageSizes { i: 0, q: 0, m: 0 }).unwrap_or_else(|d| panic!("{name}: {d:?}"));
    Module {
        name: name.into(),
        program,
        period: ms(period_ms),
        i: Window::new(0, 0),
        q: Window::new(0, 0),
        params: Vec::new(),
    }
}

fn sig(name: &str, ty: Ty, dir: Dir, point: &str) -> Signal {
    Signal {
        name: name.into(),
        ty,
        dir,
        binding: Binding::Device {
            device: "rio".into(),
            point: point.into(),
        },
        unit: None,
        poll: None,
    }
}

fn io_map(endpoint: &str) -> IoMap {
    IoMap {
        devices: vec![Device {
            id: "rio".into(),
            driver: "modbus-tcp".into(),
            endpoint: endpoint.into(),
            params: [("timeout_ms".to_string(), "300".to_string())].into(),
        }],
        signals: vec![
            sig("start", Ty::Bool, Dir::In, "di:1"),
            sig("level", Ty::Int, Dir::In, "hr:5"),
            sig("motor", Ty::Bool, Dir::Out, "coil:0"),
            sig("lamp", Ty::Bool, Dir::Out, "coil:1"),
        ],
    }
}

fn table() -> SignalTable {
    SignalTable::new(&io_map("127.0.0.1:1"), &BTreeMap::new()).unwrap()
}

/// `motor` follows `start` after it has been on for 100 ms; `lamp` is on
/// while `level` is above 10.
const DELAYED: &str = "
PROGRAM line
VAR_INPUT start : BOOL; level : INT; END_VAR
VAR_OUTPUT motor : BOOL; lamp : BOOL; END_VAR
VAR t : TON; END_VAR
  t(IN := start, PT := T#100ms);
  motor := t.Q;
  lamp := level > 10;
END_PROGRAM";

/// The same outputs, no delay.
const DIRECT: &str = "
PROGRAM line
VAR_INPUT start : BOOL; level : INT; END_VAR
VAR_OUTPUT motor : BOOL; lamp : BOOL; END_VAR
  motor := start;
  lamp := level > 10;
END_PROGRAM";

fn channels() -> Vec<Channel> {
    vec![
        Channel {
            name: "start".into(),
            ty: Ty::Bool,
            kind: Kind::Input,
        },
        Channel {
            name: "level".into(),
            ty: Ty::Int,
            kind: Kind::Input,
        },
    ]
}

fn ev(at_ms: u64, channel: u32, value: Option<Value>) -> Event {
    Event {
        at: ms(at_ms),
        channel,
        value,
    }
}

/// `start` and `level` both present from 0 (`false`, `0`), then `events`.
fn recording(end_ms: u64, events: Vec<Event>) -> Recording {
    Recording::new(
        None,
        Duration::ZERO,
        ms(end_ms),
        channels(),
        vec![Some(Value::Bool(false)), Some(Value::Int(0))],
        events,
    )
    .unwrap()
}

fn run(src: &str, period: u64, rec: &Recording) -> Timeline {
    replay(&table(), 0, 0, vec![module("line", period, src)], rec).unwrap()
}

#[allow(
    clippy::unnecessary_wraps,
    reason = "a present bool, as tracks and recordings hold them"
)]
fn on(b: bool) -> Option<Value> {
    Some(Value::Bool(b))
}

// ---- replay semantics ----

#[test]
fn a_ton_over_the_window_fires_on_the_recorded_clock() {
    // start on at 1000 ms, off at 1500 ms; again 2000..2050 (too short).
    let rec = recording(
        3000,
        vec![
            ev(1000, 0, on(true)),
            ev(1500, 0, on(false)),
            ev(2000, 0, on(true)),
            ev(2050, 0, on(false)),
        ],
    );
    let tl = run(DELAYED, 10, &rec);
    let motor = tl.track("motor").unwrap();
    assert_eq!(
        motor.changes,
        [(ms(0), on(false)), (ms(1100), on(true)), (ms(1500), on(false))]
    );
    assert_eq!(motor.rises(), 1, "the 50 ms pulse never fires the timer");
    assert_eq!(motor.on_time(tl.end), ms(400));
    assert_eq!(motor.module, "line");
    assert_eq!(tl.ticks, 301, "every 10 ms deadline from 0 to 3000 inclusive");
}

#[test]
fn the_modules_scan_period_sets_when_it_sees_a_change() {
    // A 250 ms module sees start at its next deadline after 1010 ms.
    let rec = recording(2000, vec![ev(1010, 0, on(true)), ev(1600, 0, on(false))]);
    let tl = run(DIRECT, 250, &rec);
    assert_eq!(
        tl.track("motor").unwrap().changes,
        [(ms(0), on(false)), (ms(1250), on(true)), (ms(1750), on(false))]
    );
}

#[test]
fn a_device_gap_holds_the_module_and_sends_its_outputs_safe() {
    // level present and high, then missing (device down) 1000..1500.
    let rec = recording(
        2000,
        vec![
            ev(100, 1, Some(Value::Int(20))),
            ev(1000, 1, None),
            ev(1000, 0, None),
            ev(1500, 1, Some(Value::Int(20))),
            ev(1500, 0, on(false)),
        ],
    );
    let tl = run(DIRECT, 10, &rec);
    assert_eq!(
        tl.track("lamp").unwrap().changes,
        [
            (ms(0), on(false)),
            (ms(100), on(true)),
            (ms(1000), on(false)),
            (ms(1500), on(true))
        ]
    );
    let spans = &tl.module("line").unwrap().spans;
    let statuses: Vec<_> = spans.iter().map(|s| (s.from, s.to, s.status)).collect();
    assert_eq!(
        statuses,
        [
            (ms(0), ms(1000), Status::Running),
            (ms(1000), ms(1500), Status::Held),
            (ms(1500), ms(2000), Status::Running),
        ]
    );
}

#[test]
fn a_ton_restarts_after_a_hold() {
    // start on from 0, device down 50..60: the timer never reaches 100 ms
    // before the gap and restarts after it, as a scan-skipping hold does live.
    let rec = recording(
        400,
        vec![
            ev(0, 0, on(true)),
            ev(50, 0, None),
            ev(60, 0, on(false)),
            ev(70, 0, on(true)),
        ],
    );
    let tl = run(DELAYED, 10, &rec);
    assert_eq!(
        tl.track("motor").unwrap().changes,
        [(ms(0), on(false)), (ms(170), on(true))]
    );
}

#[test]
fn a_module_that_faults_mid_replay_stops_with_its_outputs_safe() {
    const DIV: &str = "
PROGRAM line
VAR_INPUT start : BOOL; level : INT; END_VAR
VAR_OUTPUT motor : BOOL; lamp : BOOL; END_VAR
VAR q : INT; END_VAR
  q := 100 / level;
  motor := start;
  lamp := q > 1;
END_PROGRAM";
    let base = Recording::new(
        None,
        Duration::ZERO,
        ms(1000),
        channels(),
        vec![Some(Value::Bool(true)), Some(Value::Int(5))],
        vec![ev(400, 1, Some(Value::Int(0))), ev(600, 1, Some(Value::Int(5)))],
    )
    .unwrap();
    let tl = run(DIV, 10, &base);
    assert_eq!(tl.faults.len(), 1);
    let (at, name, fault) = &tl.faults[0];
    assert_eq!(
        (*at, name.as_str(), fault.kind),
        (ms(400), "line", FaultKind::DivisionByZero)
    );
    assert_eq!(
        tl.track("motor").unwrap().changes,
        [(ms(0), on(true)), (ms(400), on(false))]
    );
    let last = tl.module("line").unwrap().spans.last().copied().unwrap();
    assert!(matches!(last.status, Status::Faulted(_)));
    assert_eq!(
        (last.from, last.to),
        (ms(400), ms(1000)),
        "stays stopped though level recovers"
    );
}

#[test]
fn replay_is_deterministic_and_survives_export() {
    let rec = recording(
        5000,
        (0..200u32)
            .map(|k| {
                let at = u64::from(k) * 25;
                if k % 3 == 0 {
                    ev(at, 1, Some(Value::Int(i64::from(k % 17))))
                } else {
                    ev(at, 0, on(k % 2 == 0))
                }
            })
            .collect(),
    );
    let a = run(DELAYED, 10, &rec);
    let b = run(DELAYED, 10, &rec);
    assert_eq!(a, b);
    let back = Recording::from_text(&rec.to_text()).unwrap();
    assert_eq!(back, rec);
    assert_eq!(run(DELAYED, 10, &back), a);
}

#[test]
fn a_proposal_is_diffed_against_the_running_module() {
    let mut events = Vec::new();
    for k in 0..10u64 {
        // pulses of 50 ms (too short for the TON) and 200 ms, alternating
        let at = 500 * k + 100;
        let len = if k % 2 == 0 { 50 } else { 200 };
        events.push(ev(at, 0, on(true)));
        events.push(ev(at + len, 0, on(false)));
    }
    let rec = recording(5000, events);
    let running = [module("line", 10, DELAYED)];
    let p = propose(&table(), 0, 0, &running, module("line", 10, DIRECT), &rec).unwrap();
    let motor = p.diff.iter().find(|d| d.signal == "motor").unwrap();
    assert_eq!((motor.current.rises, motor.proposed.rises), (5, 10));
    assert_eq!(motor.extra_rises(), 5, "would have fired 5 more times");
    let lamp = p.diff.iter().find(|d| d.signal == "lamp").unwrap();
    assert_eq!(lamp.extra_rises(), 0);
    assert!(!lamp.differs());
    assert_eq!(compare(&p.current, &p.proposed), p.diff);
}

#[test]
fn a_proposal_can_add_a_module_beside_the_running_ones() {
    const LAMP: &str = "
PROGRAM beacon
VAR_INPUT level : INT; END_VAR
VAR_OUTPUT lamp : BOOL; END_VAR
  lamp := level > 3;
END_PROGRAM";
    const MOTOR: &str = "
PROGRAM line
VAR_INPUT start : BOOL; END_VAR
VAR_OUTPUT motor : BOOL; END_VAR
  motor := start;
END_PROGRAM";
    let rec = recording(1000, vec![ev(200, 1, Some(Value::Int(5)))]);
    let running = [module("line", 10, MOTOR)];
    let p = propose(&table(), 0, 0, &running, module("beacon", 100, LAMP), &rec).unwrap();
    assert!(p.current.track("lamp").is_none(), "nothing drove lamp before");
    let lamp = p.proposed.track("lamp").unwrap();
    assert_eq!(lamp.changes, [(ms(0), on(false)), (ms(200), on(true))]);
    assert_eq!(lamp.module, "beacon");
    let d = p.diff.iter().find(|d| d.signal == "lamp").unwrap();
    assert_eq!((d.current.rises, d.proposed.rises), (0, 1));
}

#[test]
fn a_proposal_that_does_not_fit_is_refused_with_the_runtimes_reasons() {
    const BAD: &str = "
PROGRAM line
VAR_INPUT nonesuch : BOOL; END_VAR
VAR_OUTPUT motor : BOOL; END_VAR
  motor := nonesuch;
END_PROGRAM";
    let rec = recording(100, Vec::new());
    let err = replay(&table(), 0, 0, vec![module("line", 10, BAD)], &rec).unwrap_err();
    assert!(matches!(err, ReplayError::Config(_)), "{err:?}");
    assert!(err.to_string().to_lowercase().contains("nonesuch"), "{err}");
}

#[test]
fn a_recording_from_a_map_whose_types_changed_is_refused() {
    const REAL: &str = "
PROGRAM line
VAR_INPUT start : BOOL; level : REAL; END_VAR
VAR_OUTPUT motor : BOOL; lamp : BOOL; END_VAR
  motor := start;
  lamp := level > 10.0;
END_PROGRAM";
    let mut m = io_map("127.0.0.1:1");
    m.signals[1].ty = Ty::Real; // level was INT when recorded
    m.signals[1].binding = Binding::Device {
        device: "rio".into(),
        point: "hr:5:f32".into(),
    };
    let t = SignalTable::new(&m, &BTreeMap::new()).unwrap();
    let rec = recording(100, Vec::new());
    let err = replay(&t, 0, 0, vec![module("line", 10, REAL)], &rec).unwrap_err();
    let ReplayError::Changed(m) = &err else {
        panic!("{err:?}")
    };
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].signal, "level");
    let msg = err.to_string();
    assert!(
        msg.contains("level") && msg.contains("int") && msg.contains("real"),
        "{msg}"
    );
}

#[test]
fn a_signal_that_was_an_input_and_is_now_an_output_is_refused() {
    let mut m = io_map("127.0.0.1:1");
    m.signals[0].dir = Dir::Out;
    m.signals[0].binding = Binding::Device {
        device: "rio".into(),
        point: "coil:5".into(),
    };
    let t = SignalTable::new(&m, &BTreeMap::new()).unwrap();
    let rec = recording(100, Vec::new());
    let err = replay(&t, 0, 0, Vec::new(), &rec).unwrap_err();
    assert!(
        matches!(&err, ReplayError::Changed(m) if m[0].signal == "start"),
        "{err:?}"
    );
}

#[test]
fn a_module_reading_a_signal_the_recording_lacks_is_refused() {
    const JAM: &str = "
PROGRAM line
VAR_INPUT jam : BOOL; END_VAR
VAR_OUTPUT motor : BOOL; END_VAR
  motor := NOT jam;
END_PROGRAM";
    // A new sensor added to the map after the recording was made.
    let mut m = io_map("127.0.0.1:1");
    m.signals.push(sig("jam", Ty::Bool, Dir::In, "di:7"));
    let t = SignalTable::new(&m, &BTreeMap::new()).unwrap();
    let rec = recording(100, Vec::new());
    let err = replay(&t, 0, 0, vec![module("line", 10, JAM)], &rec).unwrap_err();
    assert!(
        matches!(&err, ReplayError::NotRecorded { module, signal } if module == "line" && signal == "jam"),
        "{err:?}"
    );
    // The same map is fine for a module that does not read it.
    assert!(replay(&t, 0, 0, vec![module("line", 10, DIRECT)], &rec).is_ok());
}

#[test]
fn a_window_that_starts_late_scans_first_at_its_start() {
    // A rolling window from 10 s to 11 s; the module's deadlines stay on
    // the box's grid (multiples of 300 ms) after the first scan.
    let rec = Recording::new(
        None,
        ms(10_000),
        ms(11_000),
        channels(),
        vec![Some(Value::Bool(true)), Some(Value::Int(0))],
        vec![ev(10_100, 0, on(false))],
    )
    .unwrap();
    let tl = run(DIRECT, 300, &rec);
    assert_eq!(tl.start, ms(10_000));
    assert_eq!(
        tl.track("motor").unwrap().changes,
        [(ms(10_000), on(true)), (ms(10_200), on(false))]
    );
}

#[test]
fn a_replay_that_would_take_too_many_scans_is_refused() {
    let rec = recording(600_000, Vec::new()); // ten minutes
    let mut m = module("line", 10, DIRECT);
    m.period = Duration::from_micros(1);
    let err = replay(&table(), 0, 0, vec![m], &rec).unwrap_err();
    assert!(matches!(err, ReplayError::TooLong { .. }), "{err:?}");
}

// ---- the recorder ----

#[test]
fn the_recorder_keeps_what_modules_saw_and_only_changes() {
    let mut t = table();
    let mut r = Recorder::new(&t, RecordConfig::default(), Some(1_767_225_600_000));
    r.observe(ms(0), &t);
    r.observe(ms(10), &t);
    t.set_input(0, Some(Value::Bool(true)));
    r.observe(ms(20), &t);
    t.set_input(1, Some(Value::Int(7)));
    r.observe(ms(30), &t);
    r.observe(ms(40), &t);
    t.set_input(1, None);
    r.observe(ms(50), &t);
    let rec = r.snapshot();
    assert_eq!(rec.wall_ms, Some(1_767_225_600_000));
    assert_eq!(rec.channels, channels(), "inputs only: outputs are recomputed");
    assert_eq!((rec.start, rec.end), (ms(0), ms(50)));
    assert_eq!(rec.base, [None, None]);
    assert_eq!(
        rec.events,
        [
            ev(20, 0, on(true)),
            ev(30, 1, Some(Value::Int(7))),
            ev(50, 1, None),
        ]
    );
    assert_eq!(rec.value_at(1, ms(35)), Some(Value::Int(7)));
    assert_eq!(rec.value_at(1, ms(50)), None);
}

#[test]
fn time_going_backwards_is_clamped_and_counted() {
    let mut t = table();
    let mut r = Recorder::new(&t, RecordConfig::default(), None);
    r.observe(ms(100), &t);
    t.set_input(0, Some(Value::Bool(true)));
    r.observe(ms(50), &t);
    t.set_input(0, Some(Value::Bool(false)));
    r.observe(ms(100), &t); // a duplicate timestamp is fine
    let rec = r.snapshot();
    assert_eq!(rec.events, [ev(100, 0, on(true)), ev(100, 0, on(false))]);
    assert_eq!(r.stats().clamped, 1);
    assert_eq!(
        rec.value_at(0, ms(100)),
        on(false),
        "the later of two equal stamps wins"
    );
}

#[test]
fn eviction_at_the_byte_cap_keeps_the_recording_exact_from_its_start() {
    let cfg = RecordConfig {
        window: Duration::from_secs(3600),
        max_bytes: 4096,
    };
    let mut t = table();
    let mut r = Recorder::new(&t, cfg, None);
    assert!(r.capacity_bytes() <= 4096, "{}", r.capacity_bytes());
    let mut truth = Vec::new();
    for k in 0..2000u64 {
        let v = Value::Int(i64::try_from(k % 97).unwrap());
        t.set_input(1, Some(v));
        t.set_input(0, Some(Value::Bool(k % 5 == 0)));
        r.observe(ms(k * 10), &t);
        truth.push((ms(k * 10), Some(v), Some(Value::Bool(k % 5 == 0))));
    }
    let rec = r.snapshot();
    assert!(rec.events.len() * size_of::<Event>() <= 4096);
    assert!(
        rec.start > ms(15_000),
        "old events evicted, start moved to {:?}",
        rec.start
    );
    assert_eq!(rec.end, ms(19_990));
    for (at, level, start) in truth.into_iter().filter(|(at, ..)| *at >= rec.start) {
        assert_eq!(rec.value_at(1, at), level, "level at {at:?}");
        assert_eq!(rec.value_at(0, at), start, "start at {at:?}");
    }
    assert!(r.stats().evicted > 0);
}

#[test]
fn eviction_by_window_drops_what_is_older() {
    let cfg = RecordConfig {
        window: ms(1000),
        max_bytes: 1 << 20,
    };
    let mut t = table();
    let mut r = Recorder::new(&t, cfg, None);
    for k in 0..50u64 {
        t.set_input(1, Some(Value::Int(i64::try_from(k).unwrap())));
        r.observe(ms(k * 100), &t);
    }
    let rec = r.snapshot();
    assert_eq!((rec.start, rec.end), (ms(3900), ms(4900)));
    assert_eq!(rec.base[1], Some(Value::Int(39)));
    assert_eq!(rec.events.first().map(|e| e.at), Some(ms(4000)));
    assert_eq!(rec.value_at(1, ms(3900)), Some(Value::Int(39)));
}

#[test]
fn a_reader_holding_the_recording_never_blocks_the_scan() {
    let mut t = table();
    let mut r = Recorder::new(&t, RecordConfig::default(), None);
    let tap = r.tap();
    r.observe(ms(0), &t);
    {
        let _held = tap.hold_for_test();
        for k in 1..=20u64 {
            t.set_input(1, Some(Value::Int(i64::try_from(k).unwrap())));
            r.observe(ms(k), &t); // must not wait on the reader
        }
    }
    r.observe(ms(21), &t); // the backlog lands once the reader lets go
    let rec = tap.snapshot();
    assert_eq!(rec.events.len(), 20);
    assert_eq!(rec.value_at(1, ms(21)), Some(Value::Int(20)));
    assert!(r.stats().deferred >= 20);
}

#[test]
fn readers_snapshotting_in_parallel_always_see_a_consistent_recording() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    let cfg = RecordConfig {
        window: Duration::from_secs(3600),
        max_bytes: 64 << 10,
    };
    let mut t = table();
    let mut r = Recorder::new(&t, cfg, None);
    let stop = Arc::new(AtomicBool::new(false));
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let tap = r.tap();
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                let mut n = 0u32;
                while !stop.load(Ordering::Relaxed) {
                    let rec = tap.snapshot();
                    // Every snapshot is a valid recording: sorted, in span.
                    Recording::new(
                        rec.wall_ms,
                        rec.start,
                        rec.end,
                        rec.channels,
                        rec.base,
                        rec.events,
                    )
                    .unwrap();
                    n += 1;
                }
                n
            })
        })
        .collect();
    // level = k at tick k, every tick: a snapshot is exact iff level at
    // any recorded time t equals t in ticks.
    for k in 0..20_000u64 {
        t.set_input(1, Some(Value::Int(i64::try_from(k).unwrap())));
        r.observe(ms(k), &t);
    }
    r.observe(ms(20_000), &t);
    stop.store(true, Ordering::Relaxed);
    let reads: u32 = readers.into_iter().map(|h| h.join().unwrap()).sum();
    assert!(reads > 0);
    let rec = r.snapshot();
    let restarts = r.stats().restarts;
    let from = rec.start.as_millis();
    for at in (from..=19_999).step_by(7) {
        let at = u64::try_from(at).unwrap();
        assert_eq!(
            rec.value_at(1, ms(at)),
            Some(Value::Int(i64::try_from(at).unwrap())),
            "at {at} ms (start {from}, {restarts} restarts)"
        );
    }
}

#[test]
fn a_backlog_past_its_bound_restarts_the_recording_consistently() {
    let cfg = RecordConfig {
        window: Duration::from_secs(3600),
        max_bytes: 4096,
    };
    let mut t = table();
    let mut r = Recorder::new(&t, cfg, None);
    let tap = r.tap();
    {
        let _held = tap.hold_for_test();
        for k in 0..1000u64 {
            t.set_input(1, Some(Value::Int(i64::try_from(k).unwrap())));
            r.observe(ms(k), &t);
        }
    }
    r.observe(ms(1000), &t);
    let rec = tap.snapshot();
    assert!(r.stats().restarts >= 1);
    assert_eq!(rec.value_at(1, rec.end), Some(Value::Int(999)));
    assert!(rec.start > ms(0), "history before the restart is gone");
}

#[test]
fn the_calendar_is_recorded_and_its_driven_outputs_replay() {
    const GATED: &str = "
PROGRAM line
VAR_INPUT start : BOOL; in_shift : BOOL; END_VAR
VAR_OUTPUT motor : BOOL; END_VAR
  motor := in_shift;
END_PROGRAM";
    let mut t = table();
    let cal = load("zone UTC\nshift 1 mon-sun 06:00-14:00\ndrive lamp in_shift\n").unwrap();
    let src = Source::attach(cal, &mut t).unwrap();
    let mut r = Recorder::new(&t, RecordConfig::default(), None);
    let day = 1_767_225_600; // 2026-01-01 00:00 UTC
    // One observation per simulated minute across 05:58..06:02.
    for k in 0..5u64 {
        let wall = day + 6 * 3600 - 120 + i64::try_from(k).unwrap() * 60;
        src.update(Some(wall), &mut t);
        r.observe(Duration::from_secs(k * 60), &t);
    }
    let rec = r.snapshot();
    let names: Vec<&str> = rec.channels.iter().map(|c| c.name.as_str()).collect();
    assert!(
        names.contains(&"in_shift") && names.contains(&"shift_minute"),
        "{names:?}"
    );
    let lamp = rec.channels.iter().position(|c| c.name == "lamp").unwrap();
    assert_eq!(rec.channels[lamp].kind, Kind::Driven);
    assert_eq!(
        rec.value_at(u32::try_from(lamp).unwrap(), Duration::from_secs(150)),
        on(true)
    );

    // A module gated on in_shift replays against the recorded calendar.
    let tl = replay(&t, 0, 0, vec![module("line", 1000, GATED)], &rec).unwrap();
    // start was never read from a device here, so the module is held
    // throughout: replay says so rather than inventing a value.
    assert_eq!(tl.module("line").unwrap().spans[0].status, Status::Held);
    assert_eq!(tl.track("motor").unwrap().rises(), 0);

    // With start present from the first observation, motor follows the
    // recorded shift start.
    let mut r = Recorder::new(&t, RecordConfig::default(), None);
    t.set_input(0, Some(Value::Bool(false)));
    for k in 0..5u64 {
        let wall = day + 6 * 3600 - 120 + i64::try_from(k).unwrap() * 60;
        src.update(Some(wall), &mut t);
        r.observe(Duration::from_secs(k * 60), &t);
    }
    let tl = replay(&t, 0, 0, vec![module("line", 1000, GATED)], &r.snapshot()).unwrap();
    assert_eq!(
        tl.track("motor").unwrap().changes,
        [(ms(0), on(false)), (Duration::from_secs(120), on(true))]
    );
}

// ---- export / import ----

#[test]
fn the_text_format_reads_back_and_is_versioned() {
    let rec = Recording::new(
        Some(-5),
        ms(1),
        ms(9),
        vec![
            Channel {
                name: "a".into(),
                ty: Ty::Real,
                kind: Kind::Input,
            },
            Channel {
                name: "lamp".into(),
                ty: Ty::Bool,
                kind: Kind::Driven,
            },
        ],
        vec![Some(Value::Real(-0.0)), Some(Value::Bool(true))],
        vec![
            ev(2, 0, Some(Value::Real(1e300))),
            ev(2, 0, Some(Value::Real(0.1))),
            ev(3, 0, None),
            ev(9, 1, Some(Value::Bool(false))),
        ],
    )
    .unwrap();
    let text = rec.to_text();
    assert!(text.starts_with("helmsman-recording 1\n"), "{text}");
    let back = Recording::from_text(&text).unwrap();
    assert_eq!(back, rec);
    let Some(Value::Real(z)) = back.base[0] else {
        panic!()
    };
    assert!(z.is_sign_negative(), "-0.0 survives bit for bit");
}

#[test]
fn malformed_recordings_are_refused_with_the_line() {
    let good = recording(100, vec![ev(10, 0, on(true))]).to_text();
    let bad: &[(&str, &str)] = &[
        ("", "empty"),
        ("helmsman-recording 2\n", "version"),
        ("helmsman-recording\n", "version"),
        ("hello\n", "version"),
    ];
    for (src, why) in bad {
        let e = Recording::from_text(src).unwrap_err();
        assert!(!e.is_empty(), "{why}");
    }
    let swaps: &[(&str, &str)] = &[
        ("at 10000000 0 true", "at 10000000 0 maybe"),
        ("at 10000000 0 true", "at 10000000 7 true"),
        ("at 10000000 0 true", "at 200000000 0 true"),
        ("at 10000000 0 true", "at -1 0 true"),
        ("at 10000000 0 true", "at 10000000 0 true extra"),
        ("at 10000000 0 true", "at 99999999999999999999999 0 true"),
        ("base 1 0", "base 1 1.5"),
        ("base 1 0", "base 0 0"),
        ("base 1 0", "base 1 99999999999999999999"),
        ("signal level int in", "signal level float in"),
        ("signal level int in", "signal level int sideways"),
        ("signal level int in", "signal start int in"),
        ("signal level int in", "signal le\u{7}vel int in"),
        ("span 0 100000000", "span 100000000 0"),
        ("span 0 100000000", "span 0"),
    ];
    for (from, to) in swaps {
        assert!(good.contains(from), "fixture lacks {from:?}:\n{good}");
        let src = good.replacen(from, to, 1);
        let e = Recording::from_text(&src).unwrap_err();
        assert!(e.iter().any(|(line, _)| *line > 1), "{to}: {e:?}");
    }
}

#[test]
fn events_out_of_order_are_refused_on_import() {
    let mut text = recording(100, vec![ev(10, 0, on(true)), ev(20, 0, on(false))]).to_text();
    text = text.replace("at 10000000 0 true", "at 30000000 0 true");
    let e = Recording::from_text(&text).unwrap_err();
    assert!(e.iter().any(|(_, m)| m.contains("order")), "{e:?}");
    // and when built in code
    let e = Recording::new(
        None,
        Duration::ZERO,
        ms(100),
        channels(),
        vec![None, None],
        vec![ev(20, 0, on(true)), ev(10, 0, on(false))],
    )
    .unwrap_err();
    assert!(e.contains("order"), "{e}");
}

#[test]
fn a_real_that_is_not_finite_is_refused() {
    let chans = vec![Channel {
        name: "x".into(),
        ty: Ty::Real,
        kind: Kind::Input,
    }];
    for bad in [f64::NAN, f64::INFINITY] {
        let e = Recording::new(
            None,
            Duration::ZERO,
            ms(1),
            chans.clone(),
            vec![Some(Value::Real(bad))],
            Vec::new(),
        );
        assert!(e.is_err());
    }
    let text = "helmsman-recording 1\nspan 0 1\nsignal x real in\nbase 0 NaN\n";
    assert!(Recording::from_text(text).is_err());
}

#[test]
fn an_oversized_file_is_refused_before_it_is_read() {
    let mut text = String::from("helmsman-recording 1\nspan 0 1\nsignal x bool in\nbase 0 -\n");
    text.push_str(&"#".repeat(Recording::MAX_TEXT + 1));
    let e = Recording::from_text(&text).unwrap_err();
    assert!(e[0].1.contains("large"), "{e:?}");
}

fn arb_value(ty: Ty) -> BoxedStrategy<Option<Value>> {
    let v: BoxedStrategy<Value> = match ty {
        Ty::Bool => any::<bool>().prop_map(Value::Bool).boxed(),
        Ty::Int => any::<i64>().prop_map(Value::Int).boxed(),
        Ty::Real => any::<f64>()
            .prop_filter("finite", |x| x.is_finite())
            .prop_map(Value::Real)
            .boxed(),
    };
    prop::option::of(v).boxed()
}

fn arb_recording() -> impl Strategy<Value = Recording> {
    let ty = prop_oneof![Just(Ty::Bool), Just(Ty::Int), Just(Ty::Real)];
    let kind = prop_oneof![Just(Kind::Input), Just(Kind::Driven)];
    prop::collection::vec((ty, kind), 1..6).prop_flat_map(|sigs| {
        let n = sigs.len();
        let chans: Vec<Channel> = sigs
            .iter()
            .enumerate()
            .map(|(k, (ty, kind))| Channel {
                name: format!("s{k}"),
                ty: *ty,
                kind: *kind,
            })
            .collect();
        let base: Vec<_> = chans.iter().map(|c| arb_value(c.ty)).collect();
        let events = prop::collection::vec(
            (0..1_000_000u64, 0..n).prop_flat_map({
                let chans = chans.clone();
                move |(at, k)| arb_value(chans[k].ty).prop_map(move |v| (at, k, v))
            }),
            0..40,
        );
        (Just(chans), base, events, any::<Option<i64>>()).prop_map(|(chans, base, mut events, wall)| {
            events.sort_by_key(|e| e.0);
            let events = events
                .into_iter()
                .map(|(at, k, value)| Event {
                    at: Duration::from_nanos(at),
                    channel: u32::try_from(k).unwrap(),
                    value,
                })
                .collect();
            Recording::new(
                wall,
                Duration::ZERO,
                Duration::from_nanos(1_000_000),
                chans,
                base,
                events,
            )
            .unwrap()
        })
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn any_recording_reads_back_exactly(rec in arb_recording()) {
        let back = Recording::from_text(&rec.to_text()).unwrap();
        prop_assert_eq!(&back, &rec);
        prop_assert_eq!(back.to_text(), rec.to_text());
    }

    #[test]
    fn arbitrary_text_never_panics(src in "\\PC{0,400}") {
        let _ = Recording::from_text(&src);
    }

    #[test]
    fn mangled_recordings_never_panic(rec in arb_recording(), cut in 0usize..2000, junk in "\\PC{0,8}") {
        let text = rec.to_text();
        let cut = (0..=cut.min(text.len())).rev().find(|&c| text.is_char_boundary(c)).unwrap_or(0);
        let mangled = format!("{}{junk}{}", &text[..cut], &text[cut..]);
        let _ = Recording::from_text(&mangled);
    }
}

// ---- live run → replay ----

/// What a module drives, after each tick of a live run.
type Outputs = Vec<(Duration, Value, Value)>;

/// The modules a live run and its replay both build: a fast one with a
/// TON and a slow one with a counter, so period and timing both matter.
fn live_modules() -> Vec<Module> {
    const SLOW: &str = "
PROGRAM slow
VAR_INPUT level : INT; END_VAR
VAR_OUTPUT lamp : BOOL; END_VAR
VAR n : INT; END_VAR
  IF level > 10 THEN n := n + 1; ELSE n := 0; END_IF;
  lamp := n >= 2;
END_PROGRAM";
    const FAST: &str = "
PROGRAM fast
VAR_INPUT start : BOOL; END_VAR
VAR_OUTPUT motor : BOOL; END_VAR
VAR t : TON; END_VAR
  t(IN := start, PT := T#40ms);
  motor := t.Q;
END_PROGRAM";
    vec![module("fast", 10, FAST), module("slow", 30, SLOW)]
}

#[derive(Debug, Clone)]
enum Step {
    Start(bool),
    Level(u16),
    Drop,
    Idle,
}

fn arb_step() -> impl Strategy<Value = Step> {
    prop_oneof![
        3 => any::<bool>().prop_map(Step::Start),
        3 => (0u16..30).prop_map(Step::Level),
        1 => Just(Step::Drop),
        4 => Just(Step::Idle),
    ]
}

/// Runs the modules live against the Modbus simulator over TCP, stepping
/// links and scans by hand on a simulated clock, recording as the box
/// would. Returns the recording and the outputs after every tick.
fn live_run(sim: &Sim, steps: &[Step]) -> (Recording, Outputs) {
    let m = io_map(&sim.endpoint());
    let t = SignalTable::new(&m, &BTreeMap::new()).unwrap();
    let mut rt = Runtime::with_signals(0, 0, live_modules(), &t).unwrap();
    let mut links = links(&m, helmsman_io::open, Box::new(Channels::new())).unwrap();
    let mut rec = Recorder::new(&t, RecordConfig::default(), None);
    let table = Mutex::new(t);
    let mut live = Vec::new();
    for step in steps {
        match step {
            Step::Start(b) => sim.state().discrete[1] = *b,
            Step::Level(v) => sim.state().holding[5] = *v,
            Step::Drop => sim.inject(SimFault::Drop),
            Step::Idle => {}
        }
        let now = rt.next_deadline().unwrap();
        for l in &mut links {
            let _ = l.cycle(&table);
        }
        let mut t = table.lock().unwrap();
        rec.observe(now, &t);
        let faults = rt.tick_with(now, &[], &mut t);
        assert!(faults.is_empty(), "{faults:?}");
        let cmd = |n: &str| t.command(t.index(n).unwrap()).unwrap();
        live.push((now, cmd("motor"), cmd("lamp")));
    }
    (rec.snapshot(), live)
}

fn steps_of(tl: &Timeline, live: &Outputs) -> Outputs {
    let motor = tl.track("motor").unwrap();
    let lamp = tl.track("lamp").unwrap();
    live.iter()
        .map(|(at, ..)| (*at, motor.value_at(*at).unwrap(), lamp.value_at(*at).unwrap()))
        .collect()
}

#[test]
fn replaying_a_live_run_reproduces_its_outputs() {
    let sim = Sim::start(State::new(16)).unwrap();
    let mut steps = Vec::new();
    for k in 0..60u16 {
        steps.push(match k % 13 {
            0 => Step::Start(true),
            7 => Step::Start(false),
            3 => Step::Level(k % 25),
            11 if k > 30 => Step::Drop,
            _ => Step::Idle,
        });
    }
    let (rec, live) = live_run(&sim, &steps);
    assert!(
        live.iter().any(|(_, m, _)| *m == Value::Bool(true)),
        "the TON fired live"
    );
    assert!(
        live.iter().any(|(_, _, l)| *l == Value::Bool(true)),
        "the lamp lit live"
    );
    let tl = replay(&table(), 0, 0, live_modules(), &rec).unwrap();
    assert_eq!(steps_of(&tl, &live), live);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn replaying_the_running_modules_reproduces_any_live_run(steps in prop::collection::vec(arb_step(), 1..80)) {
        let sim = Sim::start(State::new(16)).unwrap();
        let (rec, live) = live_run(&sim, &steps);
        let tl = replay(&table(), 0, 0, live_modules(), &rec).unwrap();
        prop_assert_eq!(steps_of(&tl, &live), live);
        // and through the file format
        let back = Recording::from_text(&rec.to_text()).unwrap();
        prop_assert_eq!(replay(&table(), 0, 0, live_modules(), &back).unwrap(), tl);
    }
}
