//! The worked machines and templates (`helmsman_project::starters`) as an
//! engineer meets them: compiled and run in the simulation with their plant
//! model on the simulated scan clock, faults forced through their inputs.

use helmsman_iec::Value;
use helmsman_project::starters::{Kind, STARTERS};

use crate::playground::Session;
use crate::sim_projects::{MemStorage, SimProjects};

/// A started simulation of starter `name`, stopped at time zero.
fn rig(name: &str) -> Session {
    let mut p = SimProjects::load(Box::new(MemStorage::default()));
    p.open_project(name);
    assert_eq!(p.open, name);
    let s = Session::new(&p.build_src());
    assert!(s.diags.is_empty(), "{name}: {:?}", s.diag_lines());
    s
}

/// Runs for `ms` of program time, or until `until` holds; whether it did.
fn run_until(s: &mut Session, ms: u64, mut until: impl FnMut(&Session) -> bool) -> bool {
    s.set_running(true);
    let mut t = 0.0;
    s.advance(t);
    for _ in 0..ms / 50 {
        t += 0.05;
        s.advance(t);
        assert_eq!(s.fault, None, "no runtime fault");
        if until(s) {
            s.set_running(false);
            return true;
        }
    }
    s.set_running(false);
    until(s)
}

fn run(s: &mut Session, ms: u64) {
    run_until(s, ms, |_| false);
}

fn on(s: &Session, name: &str) -> bool {
    s.value(name).unwrap_or_else(|| panic!("no point {name}")) == Value::Bool(true)
}

fn int(s: &Session, name: &str) -> i128 {
    match s.value(name).unwrap_or_else(|| panic!("no point {name}")) {
        Value::Int(n) => n,
        v => panic!("{name} is {v:?}"),
    }
}

fn real(s: &Session, name: &str) -> f64 {
    match s.value(name).unwrap_or_else(|| panic!("no point {name}")) {
        Value::Real(x) => x,
        v => panic!("{name} is {v:?}"),
    }
}

fn set(s: &mut Session, name: &str, v: bool) {
    assert!(s.set_input(name, Value::Bool(v)), "no input {name}");
}

/// A press long enough for any scan to see it.
fn press(s: &mut Session, name: &str) {
    set(s, name, true);
    run(s, 200);
    set(s, name, false);
    run(s, 100);
}

/// A started simulation of starter `name`, Start pressed once.
fn started(name: &str) -> Session {
    let mut s = rig(name);
    press(&mut s, "START_PB");
    s
}

#[test]
fn every_starter_compiles_lowers_with_nesting_and_has_io() {
    for st in STARTERS {
        let s = rig(st.name);
        let b = s.build.as_ref().unwrap();
        let m = b
            .module
            .as_ref()
            .unwrap_or_else(|| panic!("{}: {:?}", st.name, b.module_error));
        let inst = m.instances();
        assert!(
            inst.iter().any(|(path, _)| path.contains('.')),
            "{}: a nested instance, in {inst:?}",
            st.name
        );
        assert!(
            s.inputs().next().is_some() && s.outputs().next().is_some(),
            "{}",
            st.name
        );
    }
    assert!(STARTERS.iter().any(|s| s.kind == Kind::Machine));
}

/// Card starter-autostart-on-box: the plant model never presses Start, so
/// a deployed starter sits still (no output on) until someone presses or
/// forces `start_pb`, and then it moves.
#[test]
fn every_starter_sits_still_until_start_is_pressed() {
    for st in STARTERS {
        let mut s = rig(st.name);
        run(&mut s, 10_000);
        let on_before: Vec<String> = s
            .outputs()
            .filter(|p| s.value(&p.name) == Some(Value::Bool(true)))
            .map(|p| p.name.clone())
            .collect();
        assert_eq!(on_before, Vec::<String>::new(), "{}: still before Start", st.name);
        press(&mut s, "START_PB");
        assert!(
            run_until(&mut s, 60_000, |s| {
                s.outputs().any(|p| s.value(&p.name) == Some(Value::Bool(true)))
            }),
            "{}: moves after Start",
            st.name
        );
    }
}

#[test]
fn batch_mixer_runs_batches_and_trips_on_overtemp_and_a_stuck_valve() {
    let mut s = started("batch-mixer");
    // The whole sequence, step by step, in order.
    for want in [1, 2, 3, 4] {
        assert!(
            run_until(&mut s, 200_000, |s| int(s, "STEP") == want),
            "reached step {want}"
        );
        if want == 2 {
            assert!(real(&s, "LEVEL_M") >= 1.9, "full before heating");
        }
    }
    assert!(
        run_until(&mut s, 120_000, |s| int(s, "BATCHES") >= 1),
        "a batch finished"
    );
    assert!(on(&s, "LAMP_GREEN") && !on(&s, "LAMP_RED"));

    // Over temperature while heating: heater off at once, red, stays down
    // until acknowledged, then a start runs again.
    assert!(run_until(&mut s, 120_000, |s| on(s, "HEATER")));
    set(&mut s, "OVERTEMP_IN", true);
    run(&mut s, 100);
    assert!(!on(&s, "HEATER"));
    assert_eq!(int(&s, "STEP"), 0);
    assert!(!on(&s, "LAMP_GREEN"));
    set(&mut s, "OVERTEMP_IN", false);
    press(&mut s, "START_PB");
    assert_eq!(int(&s, "STEP"), 0, "a start is refused until acknowledged");
    press(&mut s, "ACK_PB");
    press(&mut s, "START_PB");
    assert!(run_until(&mut s, 20_000, |s| int(s, "STEP") > 0), "running again");

    // A stuck inlet valve: the valve's travel timeout trips, and it recovers.
    let mut s = rig("batch-mixer");
    set(&mut s, "SIM_INLET_STUCK", true);
    press(&mut s, "START_PB");
    run(&mut s, 5000);
    assert!(!on(&s, "INLET_VALVE") && !on(&s, "LAMP_GREEN"), "tripped");
    set(&mut s, "SIM_INLET_STUCK", false);
    run(&mut s, 3000);
    press(&mut s, "ACK_PB");
    press(&mut s, "START_PB");
    assert!(run_until(&mut s, 60_000, |s| int(s, "STEP") >= 2), "recovered");
}

#[test]
fn conveyor_sorter_diverts_long_parts_and_a_blocked_eye_stops_the_belt() {
    let mut s = started("conveyor-sorter");
    let mut gate_moved = false;
    assert!(
        run_until(&mut s, 120_000, |s| {
            gate_moved |= on(s, "GATE");
            int(s, "GOOD") >= 4 && int(s, "REJECTED") >= 2
        }),
        "parts sorted both ways"
    );
    assert!(gate_moved && on(&s, "BELT") && !on(&s, "LAMP_RED"));

    set(&mut s, "EYE_IN", true);
    run(&mut s, 3000);
    assert!(!on(&s, "BELT"), "the jam stopped the belt");
    assert!(run_until(&mut s, 2000, |s| on(s, "LAMP_RED")), "red");
    set(&mut s, "EYE_IN", false);
    press(&mut s, "START_PB");
    assert!(!on(&s, "BELT"), "not until acknowledged");
    press(&mut s, "ACK_PB");
    press(&mut s, "START_PB");
    let before = int(&s, "GOOD");
    assert!(on(&s, "BELT") && !on(&s, "LAMP_RED"));
    assert!(
        run_until(&mut s, 30_000, |s| int(s, "GOOD") > before),
        "sorting again"
    );
}

#[test]
fn pump_station_shares_duty_assists_and_survives_a_trip_and_a_low_level() {
    let mut s = started("pump-station");
    let (mut a, mut b, mut both) = (false, false, false);
    run_until(&mut s, 900_000, |s| {
        a |= on(s, "PUMP_A") && !on(s, "PUMP_B");
        b |= on(s, "PUMP_B") && !on(s, "PUMP_A");
        both |= on(s, "PUMP_A") && on(s, "PUMP_B");
        a && b && both
    });
    assert!(
        a && b && both,
        "duty alone each way, and the assist: {a} {b} {both}"
    );
    assert!(!on(&s, "LAMP_RED"));

    // A's overload with A running: B takes over, the alarm waits for ack.
    assert!(run_until(&mut s, 300_000, |s| on(s, "PUMP_A")));
    set(&mut s, "OVERLOAD_A_IN", true);
    run(&mut s, 300);
    assert!(!on(&s, "PUMP_A") && on(&s, "PUMP_B"), "the standby took over");
    set(&mut s, "OVERLOAD_A_IN", false);
    run(&mut s, 2000);
    assert!(!on(&s, "PUMP_A"), "stays out until reset");
    press(&mut s, "ACK_PB");
    run(&mut s, 1500);
    assert!(!on(&s, "LAMP_RED"), "cleared");

    // The low float stops the pumps at once; they return when it clears.
    assert!(run_until(&mut s, 300_000, |s| on(s, "PUMP_A") || on(s, "PUMP_B")));
    set(&mut s, "LOW_FLOAT_IN", true);
    run(&mut s, 200);
    assert!(!on(&s, "PUMP_A") && !on(&s, "PUMP_B"), "no dry running");
    assert!(run_until(&mut s, 2000, |s| on(s, "LAMP_RED")));
    set(&mut s, "LOW_FLOAT_IN", false);
    press(&mut s, "ACK_PB");
    assert!(
        run_until(&mut s, 300_000, |s| on(s, "PUMP_A") || on(s, "PUMP_B")),
        "back"
    );
}

#[test]
fn pick_and_place_cycles_and_a_stuck_cylinder_or_low_air_stops_it() {
    let mut s = started("pick-and-place");
    let mut seen = [false; 9];
    assert!(
        run_until(&mut s, 120_000, |s| {
            seen[usize::try_from(int(s, "STEP")).unwrap()] = true;
            int(s, "PARTS") >= 3
        }),
        "parts moved"
    );
    assert_eq!(seen, [true; 9], "every step");

    set(&mut s, "LOW_AIR_IN", true);
    run(&mut s, 800);
    assert!(!on(&s, "LAMP_GREEN"), "low air stops the machine");
    assert!(run_until(&mut s, 2000, |s| on(s, "LAMP_RED")));
    assert_eq!(int(&s, "STEP"), 0);
    set(&mut s, "LOW_AIR_IN", false);
    run(&mut s, 2000);
    press(&mut s, "ACK_PB");
    press(&mut s, "START_PB");
    let before = int(&s, "PARTS");
    assert!(
        run_until(&mut s, 60_000, |s| int(s, "PARTS") > before),
        "recovered"
    );

    let mut s = rig("pick-and-place");
    set(&mut s, "SIM_LIFT_STUCK", true);
    press(&mut s, "START_PB");
    assert!(
        run_until(&mut s, 60_000, |s| int(s, "STEP") == 1),
        "the cycle started"
    );
    run(&mut s, 3000);
    assert!(
        !on(&s, "LAMP_GREEN") && !on(&s, "LIFT_COIL"),
        "tripped, lift home"
    );
    set(&mut s, "SIM_LIFT_STUCK", false);
    run(&mut s, 1000);
    press(&mut s, "ACK_PB");
    press(&mut s, "START_PB");
    assert!(run_until(&mut s, 30_000, |s| int(s, "PARTS") >= 1), "recovered");
}

#[test]
fn bottle_filler_fills_and_caps_and_trips_on_no_bottles_and_a_low_tank() {
    let mut s = started("bottle-filler");
    let (mut filled, mut capped, mut indexed) = (false, false, false);
    assert!(
        run_until(&mut s, 120_000, |s| {
            filled |= on(s, "FILL_VALVE");
            capped |= on(s, "CAP_PRESS");
            indexed |= on(s, "BELT");
            int(s, "BOTTLES") >= 3
        }),
        "bottles out"
    );
    assert!(filled && capped && indexed);
    assert!(!on(&s, "LAMP_RED"));

    set(&mut s, "TANK_LOW_IN", true);
    run(&mut s, 1500);
    assert!(
        !on(&s, "FILL_VALVE") && !on(&s, "BELT") && !on(&s, "LAMP_GREEN"),
        "stopped"
    );
    set(&mut s, "TANK_LOW_IN", false);
    press(&mut s, "ACK_PB");
    press(&mut s, "START_PB");
    let before = int(&s, "BOTTLES");
    assert!(
        run_until(&mut s, 60_000, |s| int(s, "BOTTLES") > before),
        "recovered"
    );

    // The infeed runs out: the index times out instead of running forever.
    assert!(run_until(&mut s, 60_000, |s| int(s, "STEP") == 2));
    set(&mut s, "SIM_NO_BOTTLES", true);
    assert!(run_until(&mut s, 30_000, |s| !on(s, "LAMP_GREEN")), "tripped");
    assert!(!on(&s, "BELT"));
    set(&mut s, "SIM_NO_BOTTLES", false);
    press(&mut s, "ACK_PB");
    press(&mut s, "START_PB");
    let before = int(&s, "BOTTLES");
    assert!(
        run_until(&mut s, 60_000, |s| int(s, "BOTTLES") > before),
        "recovered"
    );
}

#[test]
fn air_compressors_stage_rotate_and_skip_a_hot_machine() {
    let mut s = started("air-compressors");
    let mut units = [false; 3];
    let mut most = 0;
    let (mut leads, mut low, mut high) = ([false; 3], f64::MAX, f64::MIN);
    run_until(&mut s, 600_000, |s| {
        let running = [on(s, "COMP_1"), on(s, "COMP_2"), on(s, "COMP_3")];
        for k in 0..3 {
            units[k] |= running[k];
        }
        most = most.max(running.iter().filter(|r| **r).count());
        leads[usize::try_from(int(s, "LEAD")).unwrap()] = true;
        low = low.min(real(s, "HEADER_BAR"));
        high = high.max(real(s, "HEADER_BAR"));
        assert!(
            !on(s, "LAMP_RED"),
            "no alarm in normal running: t={} bar={} stages={} running={running:?}",
            s.now_ms,
            real(s, "HEADER_BAR"),
            int(s, "STAGES")
        );
        false
    });
    assert_eq!(units, [true; 3], "every compressor gets a turn");
    assert_eq!(leads, [true; 3], "the lead rotates");
    assert!(most >= 2, "staged up: {most}");
    assert!(low > 4.0 && high < 9.0, "held in band: {low}..{high}");

    // The running lead goes hot: it stops, and the stage moves to another.
    assert!(run_until(&mut s, 120_000, |s| on(s, "COMP_1")));
    set(&mut s, "HOT_1_IN", true);
    run(&mut s, 200);
    assert!(!on(&s, "COMP_1"), "stopped at once");
    assert!(
        run_until(&mut s, 60_000, |s| on(s, "COMP_2") || on(s, "COMP_3")),
        "another takes the stage"
    );
    assert!(on(&s, "LAMP_RED") || run_until(&mut s, 2000, |s| on(s, "LAMP_RED")));
    set(&mut s, "HOT_1_IN", false);
    press(&mut s, "ACK_PB");
    assert!(
        run_until(&mut s, 300_000, |s| on(s, "COMP_1")),
        "back in the rotation"
    );

    // Over pressure drops all of them, past the minimum run, until ack.
    set(&mut s, "OVERPRESSURE_IN", true);
    run(&mut s, 200);
    assert!(!on(&s, "COMP_1") && !on(&s, "COMP_2") && !on(&s, "COMP_3"));
    assert!(!on(&s, "LAMP_GREEN"));
    set(&mut s, "OVERPRESSURE_IN", false);
    press(&mut s, "ACK_PB");
    press(&mut s, "START_PB");
    assert!(run_until(&mut s, 2000, |s| on(s, "LAMP_GREEN")), "running again");
}

#[test]
fn air_handler_proves_air_holds_temperature_and_trips_on_freeze() {
    let mut s = started("air-handler");
    assert!(run_until(&mut s, 60_000, |s| int(s, "STATE") == 3), "proved");
    assert!(on(&s, "DAMPER") && on(&s, "FAN"));
    run(&mut s, 240_000);
    let t = real(&s, "SUPPLY_C");
    assert!((t - 18.0).abs() < 1.0, "held at setpoint: {t}");
    let v = real(&s, "VALVE_PCT");
    assert!(v > 20.0 && v < 60.0, "valve modulating: {v}");
    assert!(!on(&s, "LAMP_RED"));

    set(&mut s, "FREEZE_IN", true);
    run(&mut s, 200);
    assert!(!on(&s, "FAN") && !on(&s, "DAMPER"), "shut down");
    assert!(
        (real(&s, "VALVE_PCT") - 100.0).abs() < 1e-6,
        "coil valve wide open"
    );
    set(&mut s, "FREEZE_IN", false);
    press(&mut s, "START_PB");
    assert!(!on(&s, "DAMPER"), "not until acknowledged");
    press(&mut s, "ACK_PB");
    press(&mut s, "START_PB");
    assert!(
        run_until(&mut s, 60_000, |s| int(s, "STATE") == 3),
        "running again"
    );

    // A broken belt: the fan runs, the air does not prove, the unit trips.
    let mut s = rig("air-handler");
    set(&mut s, "SIM_BELT_BROKEN", true);
    press(&mut s, "START_PB");
    assert!(run_until(&mut s, 60_000, |s| on(s, "FAN")));
    assert!(run_until(&mut s, 30_000, |s| !on(s, "FAN")), "tripped");
    assert!(!on(&s, "LAMP_GREEN"));
    set(&mut s, "SIM_BELT_BROKEN", false);
    run(&mut s, 5000);
    press(&mut s, "ACK_PB");
    press(&mut s, "START_PB");
    assert!(run_until(&mut s, 60_000, |s| int(s, "STATE") == 3), "recovered");
}

#[test]
fn the_templates_move_and_their_blocks_trip() {
    let mut s = started("discrete-template");
    assert!(run_until(&mut s, 60_000, |s| int(s, "CYCLES") >= 3), "cycles");
    set(&mut s, "MOTOR_OVERLOAD_IN", true);
    assert!(run_until(&mut s, 20_000, |s| on(s, "LAMP_RED")), "overload seen");
    assert!(!on(&s, "MOTOR"));
    set(&mut s, "MOTOR_OVERLOAD_IN", false);
    press(&mut s, "ACK_PB");
    press(&mut s, "START_PB");
    let before = int(&s, "CYCLES");
    assert!(
        run_until(&mut s, 30_000, |s| int(s, "CYCLES") > before),
        "recovered"
    );

    let mut s = started("process-template");
    run(&mut s, 180_000);
    let p = real(&s, "PRESSURE");
    assert!((p - 4.0).abs() < 0.5, "held at setpoint: {p}");
    assert!(on(&s, "PUMP_A") || on(&s, "PUMP_B"));
    let was_a = on(&s, "PUMP_A");
    set(
        &mut s,
        if was_a { "OVERLOAD_A_IN" } else { "OVERLOAD_B_IN" },
        true,
    );
    run(&mut s, 300);
    assert_eq!(on(&s, "PUMP_A"), !was_a, "the other pump took over");
    assert_eq!(on(&s, "PUMP_B"), was_a);
}

/// The ST reads as written for a plant engineer, and says what it is not.
#[test]
fn every_starter_is_honest_about_safety_and_marks_its_plant_model() {
    for st in STARTERS {
        let main = st.files.iter().find(|(n, _)| *n == "main.st").unwrap().1;
        let words = main.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            words.contains("nothing here is a safety function"),
            "{}: says it is no safety function",
            st.name
        );
        let plant = st.files.iter().find(|(n, _)| *n == "plant_model.st").unwrap().1;
        assert!(plant.starts_with("(* PLANT MODEL:"), "{}", st.name);
        // A plant statement may run over lines; it ends on its PLANT mark.
        for stmt in main.split(';') {
            let rest = main[main.find(stmt).unwrap() + stmt.len()..]
                .lines()
                .next()
                .unwrap_or("");
            if stmt.contains("plant.") || stmt.contains("plant(") {
                assert!(
                    rest.contains("PLANT"),
                    "{}: an unmarked plant line: {stmt}",
                    st.name
                );
            }
        }
        for word in ["Rust", "crate", "Raspberry", "Claude", "Anthropic"] {
            for (f, text) in st.files {
                assert!(!text.contains(word), "{}: {f} says {word}", st.name);
            }
        }
    }
}
