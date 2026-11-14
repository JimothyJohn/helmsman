//! Requirement tests against the plant: a test that needs the machine to
//! answer passes only with it, and a test's fault variables reach what the
//! program reads.

use helmsman_iec::spec::{Outcome, Settings, TestSrc, Verdict};
use helmsman_iec::{ImageSizes, Ty, Value};

use crate::Val;
use crate::iec::{run, to_value};

const SIZES: ImageSizes = ImageSizes { i: 8, q: 8, m: 64 };

/// A thermostat that switches off when its sensor is lost.
const OVEN: &str = "\
PROGRAM oven
VAR_INPUT temp_c : REAL; temp_c_ok : BOOL; END_VAR
VAR_OUTPUT heat_on : BOOL; alarm : BOOL; END_VAR
IF temp_c < 180.0 THEN
  heat_on := TRUE;
ELSIF temp_c > 200.0 THEN
  heat_on := FALSE;
END_IF;
alarm := NOT temp_c_ok;
IF alarm THEN heat_on := FALSE; END_IF;
END_PROGRAM
";

/// The same thermostat, blind to a lost sensor.
const BLIND: &str = "\
PROGRAM oven
VAR_INPUT temp_c : REAL; temp_c_ok : BOOL; END_VAR
VAR_OUTPUT heat_on : BOOL; alarm : BOOL; END_VAR
IF temp_c < 180.0 THEN
  heat_on := TRUE;
ELSIF temp_c > 200.0 THEN
  heat_on := FALSE;
END_IF;
END_PROGRAM
";

const PLANT: &str = "heater oven heat=heat_on temp=temp_c ambient=20 max=300 tau=10s\n";

fn test(name: &str, src: &str) -> TestSrc {
    TestSrc {
        file: format!("{name}.st"),
        src: src.into(),
        requirement: name.into(),
        stated: true,
    }
}

fn verdict(main: &str, t: &str, plant: &str) -> Verdict {
    run(main, &test("test_t", t), Settings::new(SIZES), plant)
}

/// The heater goes off by itself within a minute: only a heater that
/// answers gets there.
const CYCLES: &str = "\
(* The heater turns off by itself once the oven is hot. *)
PROGRAM test_t
VAR_INPUT heat_on : BOOL; END_VAR
VAR_OUTPUT done : BOOL; END_VAR
VAR t : TON; went_off : ASSERT; was_on : BOOL; END_VAR
was_on := was_on OR heat_on;
t(IN := TRUE, PT := T#60s);
IF was_on AND NOT heat_on THEN
  went_off(COND := TRUE);
  done := TRUE;
ELSIF t.Q THEN
  went_off(COND := FALSE);
  done := TRUE;
END_IF;
END_PROGRAM
";

/// With the sensor lost after 5 s, the heater is off and the alarm on.
const LOST: &str = "\
(* With the sensor lost, the heater is off and the alarm is on. *)
PROGRAM test_t
VAR_INPUT heat_on : BOOL; alarm : BOOL; END_VAR
VAR_OUTPUT temp_c_absent : BOOL; done : BOOL; END_VAR
VAR t : TON; u : TON; off : ASSERT; alarmed : ASSERT; END_VAR
t(IN := TRUE, PT := T#5s);
temp_c_absent := t.Q;
u(IN := t.Q, PT := T#100ms);
IF u.Q THEN
  off(COND := NOT heat_on);
  alarmed(COND := alarm);
  done := TRUE;
END_IF;
END_PROGRAM
";

#[test]
fn a_test_that_needs_the_heater_passes_only_with_the_plant() {
    assert!(
        verdict(OVEN, CYCLES, PLANT).passed(),
        "{:?}",
        verdict(OVEN, CYCLES, PLANT).outcome
    );
    let alone = verdict(OVEN, CYCLES, "");
    assert!(
        matches!(alone.outcome, Outcome::Failed { .. }),
        "{:?}",
        alone.outcome
    );
}

#[test]
fn a_test_makes_the_sensor_absent_and_the_program_must_cope() {
    let v = verdict(OVEN, LOST, PLANT);
    assert!(v.passed(), "{:?}", v.outcome);
    let v = verdict(BLIND, LOST, PLANT);
    assert!(matches!(v.outcome, Outcome::Failed { .. }), "{:?}", v.outcome);
    // No device needed: absent on a hand-set input.
    let v = verdict(OVEN, LOST, "");
    assert!(v.passed(), "{:?}", v.outcome);
}

#[test]
fn a_program_without_x_ok_is_held_with_its_outputs_off_as_on_a_box() {
    // No temp_c_ok: absent holds the program, so the heater reads off
    // and the alarm, never set by a held program, reads off too.
    let held = "\
PROGRAM oven
VAR_INPUT temp_c : REAL; END_VAR
VAR_OUTPUT heat_on : BOOL; alarm : BOOL; END_VAR
heat_on := temp_c < 180.0;
alarm := TRUE;
END_PROGRAM
";
    let t = "\
(* With the sensor lost, every output is off. *)
PROGRAM test_t
VAR_INPUT heat_on : BOOL; alarm : BOOL; END_VAR
VAR_OUTPUT temp_c_absent : BOOL; done : BOOL; END_VAR
VAR n : INT; on_before : ASSERT; off : ASSERT; END_VAR
n := n + 1;
IF n = 3 THEN on_before(COND := heat_on AND alarm); END_IF;
temp_c_absent := n >= 4;
IF n = 6 THEN off(COND := NOT heat_on AND NOT alarm); done := TRUE; END_IF;
END_PROGRAM
";
    let v = verdict(held, t, PLANT);
    assert!(v.passed(), "{:?}", v.outcome);
    // With temp_c_ok it runs on: the alarm it sets stays on.
    let watched = held.replace("temp_c : REAL;", "temp_c : REAL; temp_c_ok : BOOL;");
    let v = verdict(&watched, t, PLANT);
    assert!(matches!(v.outcome, Outcome::Failed { .. }), "{:?}", v.outcome);
}

#[test]
fn a_stuck_sensor_holds_the_value_the_test_gives() {
    let t = "\
(* A sensor stuck at 250 turns the heater off. *)
PROGRAM test_t
VAR_INPUT heat_on : BOOL; END_VAR
VAR_OUTPUT temp_c_stuck : BOOL; temp_c_stuck_at : REAL := 250.0; done : BOOL; END_VAR
VAR off : ASSERT; n : INT; END_VAR
temp_c_stuck := TRUE;
n := n + 1;
IF n > 3 THEN off(COND := NOT heat_on); done := TRUE; END_IF;
END_PROGRAM
";
    let v = verdict(OVEN, t, PLANT);
    assert!(v.passed(), "{:?}", v.outcome);
}

#[test]
fn a_fault_takes_an_input_the_test_drives_from_the_tests_own_value() {
    // The test drives temp_c at 100, then makes it read over its range.
    let t = "\
(* An over-range reading turns the heater off. *)
PROGRAM test_t
VAR_INPUT heat_on : BOOL; END_VAR
VAR_OUTPUT temp_c : REAL := 100.0; temp_c_ok : BOOL := TRUE; temp_c_over : BOOL; done : BOOL; END_VAR
VAR on_first : ASSERT; off_after : ASSERT; n : INT; END_VAR
n := n + 1;
IF n = 3 THEN on_first(COND := heat_on); temp_c_over := TRUE; END_IF;
IF n = 6 THEN off_after(COND := NOT heat_on); done := TRUE; END_IF;
END_PROGRAM
";
    let v = verdict(OVEN, t, PLANT);
    assert!(v.passed(), "{:?}", v.outcome);
}

#[test]
fn the_files_mode_lines_do_not_reach_the_tests() {
    let v = verdict(OVEN, CYCLES, &format!("{PLANT}mode temp_c absent\n"));
    assert!(v.passed(), "{:?}", v.outcome);
}

#[test]
fn a_test_without_a_plant_runs_as_it_always_did() {
    let t = "\
(* Cold, the heater is on. *)
PROGRAM test_t
VAR_INPUT heat_on : BOOL; END_VAR
VAR_OUTPUT temp_c : REAL := 20.0; temp_c_ok : BOOL := TRUE; done : BOOL; END_VAR
VAR on : ASSERT; n : INT; END_VAR
n := n + 1;
IF n = 2 THEN on(COND := heat_on); done := TRUE; END_IF;
END_PROGRAM
";
    let with = verdict(OVEN, t, PLANT);
    let without = verdict(OVEN, t, "");
    assert!(
        with.passed() && without.passed(),
        "{:?} {:?}",
        with.outcome,
        without.outcome
    );
    assert_eq!(with.scans, without.scans);
}

#[test]
fn values_take_the_inputs_type() {
    assert_eq!(to_value(Ty::Bool, Val::Num(0.7)), Value::Bool(true));
    assert_eq!(to_value(Ty::Real, Val::Bool(true)), Value::Real(1.0));
    assert_eq!(to_value(Ty::Int, Val::Num(40_000.4)), Value::Int(32_767));
    assert_eq!(to_value(Ty::USInt, Val::Num(-3.0)), Value::Int(0));
    assert_eq!(to_value(Ty::DInt, Val::Num(2.5)), Value::Int(3));
    assert_eq!(to_value(Ty::Int, Val::Num(f64::NAN)), Value::Int(0));
    assert_eq!(to_value(Ty::Time, Val::Num(1.5)), Value::Int(1_500_000_000));
}
