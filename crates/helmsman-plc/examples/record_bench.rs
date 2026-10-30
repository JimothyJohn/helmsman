//! What the recorder costs the scan: `Recorder::observe` per tick, beside
//! a small module's tick, with and without a reader snapshotting the ring
//! in a loop on another thread.
//!
//! `cargo run --release -p helmsman-plc --example record_bench`

use std::collections::BTreeMap;
use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use helmsman_iec::{ImageSizes, compile};
use helmsman_io::{Binding, Dir, IoMap, Signal, Ty, Value};
use helmsman_plc::record::{RecordConfig, Recorder};
use helmsman_plc::{Module, Runtime, SignalTable, Window};

const TICKS: u64 = 200_000;

/// `out` (signal 0), then `n` box inputs, alternately bool and int.
fn table(n: usize) -> SignalTable {
    let out = Signal {
        name: "out".into(),
        ty: Ty::Bool,
        dir: Dir::Out,
        binding: Binding::Local {
            channel: "do:0".into(),
        },
        unit: None,
        poll: None,
    };
    let map = IoMap {
        devices: Vec::new(),
        signals: vec![out],
    };
    let mut t = SignalTable::new(&map, &BTreeMap::new()).unwrap();
    for k in 0..n {
        let ty = if k % 2 == 0 { Ty::Bool } else { Ty::Int };
        t.add_box_input(&format!("in{k}"), ty).unwrap();
    }
    t
}

/// `changing` inputs change on every tick.
fn set(t: &mut SignalTable, n: usize, changing: usize, tick: u64) {
    for k in 0..changing.min(n) {
        let v = if k % 2 == 0 {
            Value::Bool(tick % 2 == 0)
        } else {
            Value::Int(i64::try_from(tick).unwrap_or(0))
        };
        t.set_input(k + 1, Some(v));
    }
}

/// Per-tick nanoseconds: mean and worst.
fn stats(samples: &[u64]) -> (u64, u64) {
    let n = u64::try_from(samples.len()).unwrap_or(1).max(1);
    (
        samples.iter().sum::<u64>() / n,
        samples.iter().copied().max().unwrap_or(0),
    )
}

fn bench_observe(n: usize, changing: usize, reader: bool) -> (u64, u64) {
    let mut t = table(n);
    set(&mut t, n, n, 0);
    let mut rec = Recorder::new(&t, RecordConfig::default(), None);
    let stop = Arc::new(AtomicBool::new(false));
    let handle = reader.then(|| {
        let tap = rec.tap();
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut reads = 0u64;
            while !stop.load(Ordering::Relaxed) {
                black_box(tap.snapshot());
                reads += 1;
            }
            reads
        })
    });
    let mut samples = Vec::with_capacity(usize::try_from(TICKS).unwrap_or(0));
    for tick in 0..TICKS {
        set(&mut t, n, changing, tick);
        let at = Duration::from_millis(tick);
        let s = Instant::now();
        rec.observe(at, &t);
        samples.push(u64::try_from(s.elapsed().as_nanos()).unwrap_or(u64::MAX));
    }
    stop.store(true, Ordering::Relaxed);
    if let Some(h) = handle {
        let reads = h.join().unwrap();
        let st = rec.stats();
        eprintln!(
            "    reader took {reads} snapshots; {} changes deferred, {} restarts",
            st.deferred, st.restarts
        );
    }
    stats(&samples)
}

fn bench_tick(n: usize) -> (u64, u64) {
    let mut t = table(n);
    set(&mut t, n, n, 0);
    let src = "PROGRAM p VAR_INPUT in0 : BOOL; in1 : INT; END_VAR VAR_OUTPUT out : BOOL; END_VAR \
               out := in0 AND in1 > 3; END_PROGRAM";
    let program = compile(src, ImageSizes { i: 0, q: 0, m: 0 }).unwrap();
    let m = Module {
        name: "p".into(),
        program,
        period: Duration::from_millis(1),
        i: Window::new(0, 0),
        q: Window::new(0, 0),
        params: Vec::new(),
    };
    let mut rt = Runtime::with_signals(0, 0, vec![m], &t).unwrap();
    let mut samples = Vec::with_capacity(usize::try_from(TICKS).unwrap_or(0));
    for tick in 0..TICKS {
        set(&mut t, n, 2, tick);
        let s = Instant::now();
        black_box(rt.tick_with(Duration::from_millis(tick), &[], &mut t));
        samples.push(u64::try_from(s.elapsed().as_nanos()).unwrap_or(u64::MAX));
    }
    stats(&samples)
}

/// How long one snapshot of a full ring (the default 4 MiB) holds it.
fn bench_snapshot() -> (usize, Duration) {
    let n = 64;
    let mut t = table(n);
    let mut rec = Recorder::new(&t, RecordConfig::default(), None);
    let mut tick = 0;
    while rec.stats().evicted == 0 {
        set(&mut t, n, n, tick);
        rec.observe(Duration::from_millis(tick), &t);
        tick += 1;
    }
    let tap = rec.tap();
    let s = Instant::now();
    let snap = black_box(tap.snapshot());
    (snap.events.len(), s.elapsed())
}

fn main() {
    let (events, took) = bench_snapshot();
    println!("snapshot of a full default ring ({events} changes): {took:?}");
    for n in [16, 64, 256] {
        let (tm, tw) = bench_tick(n);
        println!("{n:>4} signals: one-line module tick   mean {tm:>6} ns  worst {tw:>8} ns");
        for (changing, reader) in [(0, false), (n / 10, false), (n, false), (n / 10, true)] {
            let (m, w) = bench_observe(n, changing, reader);
            let r = if reader { ", reader looping" } else { "" };
            println!("      observe, {changing:>3} changing{r:<16} mean {m:>6} ns  worst {w:>8} ns");
        }
    }
}
