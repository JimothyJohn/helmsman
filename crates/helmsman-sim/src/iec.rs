//! The plant in a requirement test (Nick, 2026-11-14: tests drive the
//! faults): the program runs against the project's `plant.sim` devices,
//! and the test makes an input misbehave by setting variables named after
//! it, at the scans it chooses:
//!
//! ```text
//! (* With the sensor lost, the heater is off within a scan. *)
//! PROGRAM test_sensor_lost
//! VAR_INPUT heat_on : BOOL; END_VAR
//! VAR_OUTPUT temp_c_absent : BOOL; done : BOOL; END_VAR
//! VAR t : TON; heat_off : ASSERT; END_VAR
//! t(IN := TRUE, PT := T#5s);
//! temp_c_absent := t.Q;
//! IF t.Q THEN heat_off(COND := NOT heat_on); done := TRUE; END_IF;
//! END_PROGRAM
//! ```
//!
//! For an input `X`: `X_absent`, `X_stuck` (at `X_stuck_at` when the test
//! has it), `X_over` and `X_under` are BOOLs; `X_drift` (a second),
//! `X_noise` and `X_slow` are numbers, in effect while not 0 (`X_slow`
//! while not 1); `X_chatter` is a TIME, in effect while above 0. The first
//! that holds, in that order, is the input's mode; none, it is normal. A
//! test starts every input normal: a `mode` line of the file is the
//! popup's scenario, not the tests'.
//!
//! An input the test drives itself stays the test's while it behaves
//! normally; once the test makes it misbehave, the plant takes it, from
//! the test's own value. `X_ok` reads FALSE while `X` is absent, unless the
//! test drives it.

use std::time::Duration;

use helmsman_iec::spec::{self, Machine, Plan, Settings, TestSrc, Verdict};
use helmsman_iec::{Port, PortDir, Ty, Value};

use crate::{Mode, Plant, Spec, Val};

/// The suffixes a test makes an input misbehave with, in the order they
/// win.
pub const FAULTS: [&str; 9] = [
    "ABSENT", "STUCK", "STUCK_AT", "OVER", "UNDER", "CHATTER", "DRIFT", "NOISE", "SLOW",
];

/// A program value as the plant reads it.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn to_val(v: Value) -> Val {
    match v {
        Value::Bool(b) => Val::Bool(b),
        Value::Int(n) => Val::Num(n as f64),
        Value::Real(x) => Val::Num(x),
    }
}

/// What the plant says, as an input of type `ty` holds it: a number
/// rounded and held inside an integer's range, a TIME in seconds, a BOOL
/// on from 0.5.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
pub fn to_value(ty: Ty, v: Val) -> Value {
    match ty {
        Ty::Bool => Value::Bool(v.on()),
        Ty::Real | Ty::LReal => Value::Real(if v.num().is_finite() { v.num() } else { 0.0 }),
        Ty::Time => Value::Int((v.num() * 1e9).clamp(-9.2e18, 9.2e18) as i128),
        _ => {
            let (lo, hi) = ty.range();
            let n = v.num();
            let n = if n.is_finite() { n.round() } else { 0.0 };
            Value::Int((n.clamp(lo as f64, hi as f64) as i128).clamp(lo, hi))
        }
    }
}

/// The plant of a `plant.sim`, as a test's machine.
#[derive(Debug, Clone)]
pub struct TestPlant {
    plant: Plant,
    inputs: Vec<Port>,
    /// The inputs the test has fault variables for, found on its first
    /// scan.
    faulted: Option<Vec<String>>,
}

impl TestPlant {
    /// `spec`'s devices, every input normal.
    #[must_use]
    pub fn new(mut spec: Spec) -> Self {
        spec.modes.clear();
        Self {
            plant: Plant::new(spec),
            inputs: Vec::new(),
            faulted: None,
        }
    }
}

/// The mode the test gives input `name` this scan.
fn mode_in(test: &dyn Fn(&str) -> Option<Value>, name: &str) -> Mode {
    let get = |suffix: &str| test(&format!("{name}_{suffix}"));
    let on = |suffix: &str| get(suffix) == Some(Value::Bool(true));
    let num = |suffix: &str| get(suffix).map(|v| to_val(v).num()).filter(|x| x.is_finite());
    if on("ABSENT") {
        Mode::Absent
    } else if on("STUCK") {
        Mode::Stuck(num("STUCK_AT"))
    } else if on("OVER") {
        Mode::Over
    } else if on("UNDER") {
        Mode::Under
    } else if let Some(ns) = num("CHATTER").filter(|x| *x > 0.0) {
        Mode::Chatter(ns / 1e9)
    } else if let Some(r) = num("DRIFT").filter(|x| *x != 0.0) {
        Mode::Drift(r)
    } else if let Some(a) = num("NOISE").filter(|x| *x > 0.0) {
        Mode::Noise(a)
    } else if let Some(f) = num("SLOW").filter(|x| *x > 0.0 && (*x - 1.0).abs() > f64::EPSILON) {
        Mode::Slow(f)
    } else {
        Mode::Normal
    }
}

impl Machine for TestPlant {
    fn start(&mut self, ports: &[Port]) {
        self.inputs = ports
            .iter()
            .filter(|p| p.dir == PortDir::Input)
            .cloned()
            .collect();
    }

    fn scan(
        &mut self,
        dt: Duration,
        test: &dyn Fn(&str) -> Option<Value>,
        program: &dyn Fn(&str) -> Option<Value>,
    ) -> Vec<(String, Value)> {
        let faulted = self.faulted.get_or_insert_with(|| {
            self.inputs
                .iter()
                .filter(|p| FAULTS.iter().any(|f| test(&format!("{}_{f}", p.name)).is_some()))
                .map(|p| p.name.clone())
                .collect()
        });
        for name in faulted.iter() {
            self.plant.set_mode(name, mode_in(test, name));
        }
        // A hand-set input reads what the test drove it to.
        let read = |n: &str| {
            let n = n.to_ascii_uppercase();
            test(&n).or_else(|| program(&n)).map(to_val)
        };
        let ms = u64::try_from(dt.as_millis()).unwrap_or(u64::MAX);
        let mut out = Vec::new();
        for f in self.plant.step(ms, &read) {
            let name = f.name.to_ascii_uppercase();
            let normal = self.plant.mode(&name) == Mode::Normal;
            if let (Some(v), Some(p)) = (f.value, self.inputs.iter().find(|p| p.name == name))
                && !(normal && test(&name).is_some())
            {
                out.push((name.clone(), to_value(p.ty, v)));
            }
            let ok = format!("{name}_OK");
            if self.inputs.iter().any(|p| p.name == ok && p.ty == Ty::Bool) && test(&ok).is_none() {
                out.push((ok, Value::Bool(f.ok)));
            }
        }
        out
    }
}

/// Runs one test, against the plant of `plant_text` (a project's
/// `plant.sim`; empty: none).
#[must_use]
pub fn run(main: &str, test: &TestSrc, settings: Settings, plant_text: &str) -> Verdict {
    let (spec, _) = crate::parse(plant_text);
    let mut m = TestPlant::new(spec);
    spec::run_with(main, test, settings, Some(&mut m))
}

/// Runs every test of `plan`, each against a fresh plant of `plant_text`.
#[must_use]
pub fn run_all(plan: &Plan, settings: Settings, plant_text: &str) -> Vec<Verdict> {
    let (spec, _) = crate::parse(plant_text);
    spec::run_all_with(plan, settings, &|| {
        Some(Box::new(TestPlant::new(spec.clone())) as Box<dyn Machine>)
    })
}
