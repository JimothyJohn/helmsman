//! Scan throughput with tracing off and on.
//!
//! `cargo run --release -p helmsman-iec --example trace_bench`
//!
//! A timing harness, not a gate: it prints numbers for a person (or a PR
//! body) to read. The workload mixes loops, IF/CASE, arithmetic and
//! function block calls, about 4 000 statements per scan.

use std::time::{Duration, Instant};

use helmsman_iec::{Image, ImageSizes, compile};

const SIZES: ImageSizes = ImageSizes { i: 32, q: 32, m: 64 };

const SRC: &str = "
FUNCTION_BLOCK acc
VAR_INPUT x : DINT; END_VAR
VAR_OUTPUT sum : DINT; END_VAR
  IF x > 0 THEN sum := sum + x; ELSE sum := sum - 1; END_IF;
END_FUNCTION_BLOCK
PROGRAM bench
VAR
  i, j, k, total : DINT;
  a : ARRAY[0..63] OF DINT;
  sums : ARRAY[0..3] OF acc;
  t : TON;
  on : BOOL;
END_VAR
  t(IN := NOT t.Q, PT := T#5ms);
  IF t.Q THEN on := NOT on; END_IF;
  total := 0;
  FOR i := 0 TO 63 DO
    a[i] := a[i] + i * 3 - total MOD 7;
    CASE i MOD 4 OF
      0: total := total + a[i];
      1, 2: total := total - 1;
    ELSE
      total := total + i / 2;
    END_CASE;
    FOR j := 0 TO 3 DO
      k := (k + j) MOD 1000;
      sums[j](x := k - 500);
    END_FOR;
  END_FOR;
  j := 0;
  WHILE j < 100 DO
    j := j + 1;
    IF j = 90 THEN EXIT; END_IF;
  END_WHILE;
  %QD0 := DINT_TO_DWORD(total);
END_PROGRAM";

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
