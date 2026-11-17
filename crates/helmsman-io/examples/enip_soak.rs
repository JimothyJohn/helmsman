//! `enip-io` held against a real adapter for a while: how often the link
//! is lost, the longest stretch without a good read, and whether the
//! connection comes back by itself.
//!
//! ```text
//! cargo run --release -p helmsman-io --example enip_soak -- 192.168.3.60 \
//!     --param profile=moxa-e1213 --param rpi_ms=100 --seconds 600 --toggle do:3
//! ```
//!
//! Reads `--read` (default `in:0.0`) every `--period-ms` (default 10) and,
//! with `--toggle OUT`, flips that output twice a second. One line a
//! minute and one at the end; exit 1 if the link was lost at all, so a
//! script can hold a device to "no loss in ten minutes". The output is
//! left off.

use std::collections::BTreeMap;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use helmsman_io::enip::EnipDriver;
use helmsman_io::{Driver, Value};

const USAGE: &str = "usage: enip_soak HOST[:PORT] [--param KEY=VALUE]... [--seconds N] [--period-ms N]
                 [--read ADDR] [--toggle OUT]";

struct Tally {
    reads: u64,
    errors: u64,
    /// The longest stretch without a successful read.
    longest_gap: Duration,
    last_ok: Instant,
    /// Each distinct error once, in the order seen.
    seen: Vec<String>,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(host) = args.first().filter(|h| !h.starts_with('-')) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let mut params = BTreeMap::new();
    let (mut seconds, mut period_ms) = (60u64, 10u64);
    let mut read = "in:0.0".to_string();
    let mut toggle = None;
    for pair in args[1..].chunks(2) {
        let [flag, v] = pair else {
            eprintln!("{} needs a value\n{USAGE}", pair[0]);
            return ExitCode::from(2);
        };
        let number = |into: &mut u64| v.parse().map(|n| *into = n).is_ok();
        let ok = match flag.as_str() {
            "--param" => v
                .split_once('=')
                .map(|(k, x)| params.insert(k.to_string(), x.to_string()))
                .is_some(),
            "--seconds" => number(&mut seconds),
            "--period-ms" => number(&mut period_ms) && period_ms > 0,
            "--read" => {
                read.clone_from(v);
                true
            }
            "--toggle" => {
                toggle = Some(v.clone());
                true
            }
            _ => false,
        };
        if !ok {
            eprintln!("{flag} {v}: not understood\n{USAGE}");
            return ExitCode::from(2);
        }
    }
    let mut d = match EnipDriver::from_params(host, &params) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{host}: {e}");
            return ExitCode::from(2);
        }
    };

    let started = Instant::now();
    let length = Duration::from_secs(seconds);
    let period = Duration::from_millis(period_ms);
    let mut t = Tally {
        reads: 0,
        errors: 0,
        longest_gap: Duration::ZERO,
        last_ok: started,
        seen: Vec::new(),
    };
    let (mut next, mut minute, mut flips, mut on) = (started, 1u64, 0u64, false);
    while started.elapsed() < length {
        if let Some(out) = &toggle
            && started.elapsed() >= Duration::from_millis(500) * u32::try_from(flips + 1).unwrap_or(u32::MAX)
        {
            on = !on;
            flips += 1;
            if let Err(e) = d.write(out, Value::Bool(on)) {
                note(&mut t, format!("write {out}: {e}"));
            }
        }
        match d.read(&[read.as_str()]) {
            Ok(_) => {
                t.reads += 1;
                t.longest_gap = t.longest_gap.max(t.last_ok.elapsed());
                t.last_ok = Instant::now();
            }
            Err(e) => note(&mut t, format!("read {read}: {e}")),
        }
        if started.elapsed() >= Duration::from_secs(60) * u32::try_from(minute).unwrap_or(u32::MAX) {
            println!("{minute:>4} min  {}", line(&t, &d));
            minute += 1;
        }
        next += period;
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
    }
    t.longest_gap = t.longest_gap.max(t.last_ok.elapsed());
    if let Some(out) = &toggle
        && let Err(e) = d.write(out, Value::Bool(false))
    {
        note(&mut t, format!("write {out}: {e}"));
    }
    println!("done      {}", line(&t, &d));
    for e in &t.seen {
        println!("  {e}");
    }
    if t.errors == 0 && d.opens == 1 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn note(t: &mut Tally, e: String) {
    t.errors += 1;
    if !t.seen.contains(&e) && t.seen.len() < 20 {
        t.seen.push(e);
    }
}

fn line(t: &Tally, d: &EnipDriver) -> String {
    format!(
        "{} reads, {} errors, {} connections opened, longest between good reads {} ms",
        t.reads,
        t.errors,
        d.opens,
        t.longest_gap.as_millis()
    )
}
