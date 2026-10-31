//! Driver conformance: runs a driver against a device and reports, check by
//! check, whether it reads, writes and fails the way the contract says. The
//! report is plain `PASS`/`FAIL`/`SKIP` lines so an agent writing a driver
//! can iterate on it.
//!
//! Writes are proven through *loops*: an output point wired to an input
//! point (on the bench, a wire from an output terminal to an input terminal;
//! on the simulator, a cross-wire). A driver that only pretends to write, or
//! writes the wrong coil, fails the loop even if its own read-back agrees.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use crate::driver::{Driver, Error};
use crate::model::{Ty, Value};
use crate::sim::{Fault, Faults};

/// What to exercise on a device.
#[derive(Clone, Debug, Default)]
pub struct Bench {
    /// Points that must read with the type their address declares.
    pub points: Vec<String>,
    /// Points whose true value is known (set on the device's own panel, or
    /// fixed by its manual): catches mistakes a round-trip can't, like a
    /// word order that is wrong the same way on write and read.
    pub expect: Vec<(String, Value)>,
    /// `(output, input)`: writing the output must show on the input.
    pub loops: Vec<(String, String)>,
    /// Writable numeric points to write, read back and restore.
    pub registers: Vec<String>,
    /// A point the device does not have: reading it must fail cleanly, as
    /// a point error (a device exception, or the driver refusing the
    /// point), and leave the driver usable.
    pub absent: Option<String>,
    /// How long a loop may take to show a write.
    pub settle: Duration,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Pass,
    Fail(String),
    Skip(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    pub name: String,
    pub outcome: Outcome,
}

/// Values a register round-trip writes: two distinct ones per type, so a
/// stuck or ignored write cannot pass by luck.
fn probes(ty: Ty, addr: &str) -> [Value; 2] {
    match ty {
        Ty::Real => [Value::Real(3.5), Value::Real(-1250.25)],
        Ty::Bool => [Value::Bool(true), Value::Bool(false)],
        Ty::Int if addr.ends_with(":i16") => [Value::Int(-12345), Value::Int(23456)],
        Ty::Int if addr.contains(":i32") => [Value::Int(-123_456_789), Value::Int(987_654_321)],
        Ty::Int if addr.contains(":u32") => [Value::Int(0xDEAD_BEEF), Value::Int(0x0102_0304)],
        Ty::Int => [Value::Int(0xA55A), Value::Int(0x1234)],
    }
}

fn read_one(d: &mut dyn Driver, addr: &str) -> Result<Value, Error> {
    d.read(&[addr]).map(|v| v[0])
}

/// Polls `addr` until it reads `want` or `settle` runs out.
fn await_value(d: &mut dyn Driver, addr: &str, want: Value, settle: Duration) -> Result<(), String> {
    let deadline = Instant::now() + settle;
    loop {
        match read_one(d, addr) {
            Ok(v) if v == want => return Ok(()),
            Ok(v) if Instant::now() >= deadline => return Err(format!("{addr} reads {v:?}, want {want:?}")),
            Err(e) if Instant::now() >= deadline => return Err(format!("{addr}: {e}")),
            _ => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}

fn check(name: impl Into<String>, r: Result<(), String>) -> Check {
    Check {
        name: name.into(),
        outcome: r.map_or_else(Outcome::Fail, |()| Outcome::Pass),
    }
}

/// Runs every check. `sim` enables the fault checks (a real device can't be
/// told to misbehave); without it they are skipped.
#[allow(clippy::too_many_lines)]
pub fn run(d: &mut dyn Driver, bench: &Bench, sim: Option<&dyn Faults>) -> Vec<Check> {
    let mut out = Vec::new();

    for p in &bench.points {
        let r = d
            .parse(p)
            .map_err(|e| e.to_string())
            .and_then(|info| match read_one(d, p) {
                Ok(v) if v.ty() == info.ty => Ok(()),
                Ok(v) => Err(format!("read {v:?}, address declares {:?}", info.ty)),
                Err(e) => Err(e.to_string()),
            });
        out.push(check(format!("read {p}"), r));
    }

    for (p, want) in &bench.expect {
        let r = match read_one(d, p) {
            Ok(v) if v == *want => Ok(()),
            Ok(v) => Err(format!("read {v:?}, the device holds {want:?}")),
            Err(e) => Err(e.to_string()),
        };
        out.push(check(format!("expect {p}"), r));
    }

    for (o, i) in &bench.loops {
        let r = (|| {
            let before = read_one(d, o).map_err(|e| format!("{o}: {e}"))?;
            for v in [true, false, true, false] {
                d.write(o, Value::Bool(v))
                    .map_err(|e| format!("write {o}: {e}"))?;
                await_value(d, i, Value::Bool(v), bench.settle)?;
            }
            d.write(o, before).map_err(|e| format!("restore {o}: {e}"))
        })();
        out.push(check(format!("loop {o} -> {i}"), r));
    }

    for reg in &bench.registers {
        let r = (|| {
            let info = d.parse(reg).map_err(|e| e.to_string())?;
            if !info.writable {
                return Err("not writable".into());
            }
            let before = read_one(d, reg).map_err(|e| e.to_string())?;
            for v in probes(info.ty, reg) {
                d.write(reg, v).map_err(|e| format!("write {v:?}: {e}"))?;
                let got = read_one(d, reg).map_err(|e| e.to_string())?;
                if got != v {
                    return Err(format!("wrote {v:?}, read {got:?}"));
                }
            }
            d.write(reg, before).map_err(|e| format!("restore: {e}"))
        })();
        out.push(check(format!("roundtrip {reg}"), r));
    }

    let first = bench.points.first().cloned();
    let recovers = |d: &mut dyn Driver| -> Result<(), String> {
        match &first {
            Some(p) => read_one(d, p)
                .map(|_| ())
                .map_err(|e| format!("not usable afterwards: {e}")),
            None => Ok(()),
        }
    };

    match &bench.absent {
        Some(a) => {
            let r = match read_one(d, a) {
                Ok(v) => Err(format!("read {v:?} from a point the device lacks")),
                Err(Error::Exception(_) | Error::Point(_)) => recovers(d),
                Err(e) => Err(format!("want a device exception or a point error, got {e}")),
            };
            out.push(check(format!("absent {a}"), r));
        }
        None => out.push(Check {
            name: "absent".into(),
            outcome: Outcome::Skip("no absent point given".into()),
        }),
    }

    // Without a simulator the TCP checks are named, and skipped.
    let faults = sim.map_or_else(crate::sim::tcp_fault_checks, <dyn Faults>::fault_checks);
    for (name, fault) in faults {
        let name = format!("fault {name}");
        let (Some(sim), Some(p)) = (sim, &first) else {
            out.push(Check {
                name,
                outcome: Outcome::Skip("needs the simulator and a point".into()),
            });
            continue;
        };
        sim.clear_faults();
        sim.inject(fault);
        let started = Instant::now();
        let window = sim.fault_window();
        // A cyclic transport answers from its last packet until the
        // connection times out: keep reading, within the window, until the
        // fault shows. A read that succeeds after the window is a failure.
        let first = loop {
            match read_one(d, p) {
                Ok(_) if started.elapsed() < window => std::thread::sleep(Duration::from_millis(5)),
                other => break other,
            }
        };
        let r = match first {
            Ok(v) => Err(format!("read {v:?} through a {fault:?}")),
            Err(_) if started.elapsed() > window + Duration::from_secs(10) => {
                Err("took over 10 s to give up".into())
            }
            Err(_) => {
                sim.clear_faults();
                // A late reply may still be in flight; the driver must not take
                // it for the answer to this read.
                std::thread::sleep(match fault {
                    Fault::Delay(dl) => dl.saturating_sub(started.elapsed()),
                    _ => Duration::ZERO,
                });
                recovers(d)
            }
        };
        sim.clear_faults();
        out.push(check(name, r));
    }
    out
}

/// One line per check, then a summary line.
#[must_use]
pub fn render(checks: &[Check]) -> String {
    let mut s = String::new();
    let (mut pass, mut fail, mut skip) = (0, 0, 0);
    for c in checks {
        let _ = match &c.outcome {
            Outcome::Pass => {
                pass += 1;
                writeln!(s, "PASS  {}", c.name)
            }
            Outcome::Fail(why) => {
                fail += 1;
                writeln!(s, "FAIL  {}: {}", c.name, why.replace('\n', " "))
            }
            Outcome::Skip(why) => {
                skip += 1;
                writeln!(s, "SKIP  {}: {why}", c.name)
            }
        };
    }
    let _ = writeln!(s, "{pass} passed, {fail} failed, {skip} skipped");
    s
}

/// Whether every check passed or was skipped.
#[must_use]
pub fn ok(checks: &[Check]) -> bool {
    checks.iter().all(|c| !matches!(c.outcome, Outcome::Fail(_)))
}
