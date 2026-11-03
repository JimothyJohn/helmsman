//! Scan throughput with tracing off and on.
//!
//! `cargo run --release -p helmsman-iec --example trace_bench`
//!
//! A timing harness, not a gate: it prints numbers for a person (or a PR
//! body) to read. The workload (`trace_bench.st`, shared with the edge
//! node's bench) mixes loops, IF/CASE, arithmetic and function block
//! calls, about 4 000 statements per scan.

use std::time::{Duration, Instant};

use helmsman_iec::{Image, ImageSizes, compile};

const SIZES: ImageSizes = ImageSizes { i: 32, q: 32, m: 64 };

/// The workload, shared with the edge node's bench (crates/helmsman-node)
/// so both measure the same program.
const SRC: &str = include_str!("trace_bench.st");

/// Mean wall time per scan over `scans`, the best of `rounds` runs.
fn per_scan(scans: u32, rounds: u32, traced_every: Option<u32>) -> Duration {
    let mut best = Duration::MAX;
    for _ in 0..rounds {
        let mut p = compile(SRC, SIZES).expect("bench program compiles");
        let mut img = Image::new(SIZES);
        let start = Instant::now();
        for n in 0..scans {
            let now = Duration::from_millis(u64::from(n));
            match traced_every {
                Some(every) if n % every == 0 => {
                    let (r, trace) = p.scan_traced(&mut img, now);
                    r.expect("bench scan");
                    std::hint::black_box(trace);
                }
                _ => p.scan_at(&mut img, now).expect("bench scan"),
            }
        }
        best = best.min(start.elapsed() / scans);
    }
    best
}

fn main() {
    let (scans, rounds) = (2_000, 7);
    let off = per_scan(scans, rounds, None);
    println!("tracing off:            {off:>10.2?} per scan");
    for every in [100, 10, 1] {
        let on = per_scan(scans, rounds, Some(every));
        #[allow(clippy::cast_precision_loss)]
        let pct = (on.as_secs_f64() / off.as_secs_f64() - 1.0) * 100.0;
        println!("traced 1 scan in {every:<3}:   {on:>10.2?} per scan ({pct:+.1}%)");
    }
}
