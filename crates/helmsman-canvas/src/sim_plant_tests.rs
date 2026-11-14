//! The simulated plant (`plant.sim`) in the page's simulation: a program
//! runs against devices that answer its outputs, and each input's mode
//! reaches what the program reads.

use helmsman_iec::Value;
use helmsman_sim::Mode;

use crate::playground::Session;

const OVEN: &str = "\
PROGRAM oven
VAR_INPUT temp_c : REAL; temp_c_ok : BOOL; start_pb : BOOL; END_VAR
VAR_OUTPUT heat_on : BOOL; alarm : BOOL; END_VAR
IF temp_c < 180.0 THEN
  heat_on := TRUE;
ELSIF temp_c > 200.0 THEN
  heat_on := FALSE;
END_IF;
IF NOT temp_c_ok THEN
  heat_on := FALSE;
  alarm := TRUE;
END_IF;
END_PROGRAM
";

const PLANT: &str = "heater oven heat=heat_on temp=temp_c ambient=20 max=300 tau=10s\n";

fn session(plant: &str) -> Session {
    let mut s = Session::new(OVEN);
    assert!(s.build.is_some(), "{:?}", s.diags);
    s.set_plant(plant);
    assert_eq!(s.plant_problems, []);
    s
}

fn steps(s: &mut Session, n: usize) {
    for _ in 0..n {
        s.step();
    }
}

fn real(s: &Session, name: &str) -> f64 {
    match s.value(name) {
        Some(Value::Real(x)) => x,
        v => panic!("{name} = {v:?}"),
    }
}

#[test]
fn a_thermostat_holds_a_simulated_oven_in_its_band() {
    let mut s = session(PLANT);
    steps(&mut s, 1);
    assert!(
        (real(&s, "temp_c") - 20.0).abs() < 1e-6,
        "ambient at the first scan"
    );
    assert_eq!(s.value("temp_c_ok"), Some(Value::Bool(true)));
    steps(&mut s, 3_000);
    assert!(real(&s, "temp_c") > 150.0, "it heats: {}", real(&s, "temp_c"));
    let (mut lo, mut hi, mut flips) = (f64::MAX, f64::MIN, 0);
    let mut was = s.value("heat_on");
    for _ in 0..6_000 {
        s.step();
        let t = real(&s, "temp_c");
        lo = lo.min(t);
        hi = hi.max(t);
        if s.value("heat_on") != was {
            flips += 1;
            was = s.value("heat_on");
        }
    }
    assert!(lo > 175.0 && hi < 205.0, "held in its band: {lo}..{hi}");
    assert!(flips >= 4, "the heater cycles: {flips}");
}

#[test]
fn an_absent_sensor_holds_its_value_and_clears_its_ok() {
    let mut s = session(PLANT);
    steps(&mut s, 2_000);
    let held = real(&s, "temp_c");
    s.plant.as_mut().expect("plant").set_mode("temp_c", Mode::Absent);
    steps(&mut s, 500);
    assert!((real(&s, "temp_c") - held).abs() < 1.0, "held near {held}");
    assert_eq!(s.value("temp_c_ok"), Some(Value::Bool(false)));
    assert_eq!(s.value("alarm"), Some(Value::Bool(true)));
    assert_eq!(s.value("heat_on"), Some(Value::Bool(false)));
}

#[test]
fn a_stuck_sensor_keeps_the_heater_on_past_its_band() {
    let mut s = session(&format!("{PLANT}mode temp_c stuck 100\n"));
    steps(&mut s, 4_000);
    assert!((real(&s, "temp_c") - 100.0).abs() < 1e-6);
    assert_eq!(s.value("heat_on"), Some(Value::Bool(true)));
}

#[test]
fn a_mode_on_a_hand_set_input_overrides_the_panel() {
    let mut s = session("mode start_pb stuck true\n");
    assert!(s.set_input("start_pb", Value::Bool(false)));
    steps(&mut s, 2);
    assert_eq!(s.value("start_pb"), Some(Value::Bool(true)));
    s.set_plant("");
    assert!(s.plant.is_none());
    assert!(s.set_input("start_pb", Value::Bool(false)));
    steps(&mut s, 1);
    assert_eq!(s.value("start_pb"), Some(Value::Bool(false)), "the panel's again");
}

#[test]
fn a_cold_restart_starts_the_plant_over_and_an_edit_carries_it() {
    let mut s = session(PLANT);
    steps(&mut s, 2_000);
    let warm = real(&s, "temp_c");
    assert!(warm > 100.0);
    s.set_plant(&PLANT.replace("max=300", "max=320"));
    steps(&mut s, 1);
    assert!((real(&s, "temp_c") - warm).abs() < 2.0, "carried");
    s.restart();
    steps(&mut s, 1);
    assert!((real(&s, "temp_c") - 20.0).abs() < 1e-6, "back at ambient");
}

#[test]
fn what_the_file_names_and_the_program_lacks_is_listed() {
    let mut s = Session::new(OVEN);
    s.set_plant("heater oven heat=fan_on temp=temp_c\ntank t fill=heat_on level=depth\nwidget w\n");
    assert_eq!(s.plant_problems.len(), 1);
    assert_eq!(s.plant_problems[0].line, 3);
    let unbound = s.plant_unbound();
    assert_eq!(unbound.len(), 2, "{unbound:?}");
    assert_eq!(unbound[0].0, 1);
    assert!(unbound[0].1.contains("no output named fan_on"));
    assert_eq!(unbound[1].0, 2);
    assert!(unbound[1].1.contains("no input named depth"));
}

#[test]
fn a_number_past_an_integer_input_is_held_at_its_limit() {
    let src =
        "PROGRAM p\nVAR_INPUT raw : INT; END_VAR\nVAR_OUTPUT q : BOOL; END_VAR\nq := raw > 0;\nEND_PROGRAM\n";
    let mut s = Session::new(src);
    s.set_plant("source s value=raw shape=step from=40000 to=40000\n");
    steps(&mut s, 2);
    assert_eq!(s.value("raw"), Some(Value::Int(32_767)));
}
