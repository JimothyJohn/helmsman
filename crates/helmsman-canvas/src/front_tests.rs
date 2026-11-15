//! The front door (`crate::front`): the tank's proofs are green and have
//! teeth, and the machine does what each fault's words say. No screen, no
//! network, no sleeps: page time is stepped by hand.

use crate::front::{DEPTH, Fault, Front, HIGH_AT, LOW_AT, MAIN, PLANT, TESTS, prove};
use crate::requirements::State;

/// Runs the machine for `secs` of page time from `*t`, a frame every 50 ms.
fn run(f: &mut Front, t: &mut f64, secs: f64) {
    let end = *t + secs;
    while *t < end {
        *t += 0.05;
        f.tick(*t);
    }
}

#[test]
fn every_proof_is_green_and_says_its_requirement() {
    let rows = prove(MAIN);
    assert_eq!(rows.len(), TESTS.len());
    for (r, (file, _)) in rows.iter().zip(TESTS) {
        assert_eq!(r.file, file, "in the order shown");
        assert_eq!(r.state, State::Passed, "{}: {}", r.file, r.why);
        assert!(
            !r.requirement.is_empty() && !r.requirement.starts_with("test_"),
            "{}: a requirement in words, not a file name: {:?}",
            r.file,
            r.requirement
        );
    }
}

/// The proofs are not decoration: a program without the fill timer and
/// the cross-check passes the normal run and fails the fault proofs.
#[test]
fn the_proofs_catch_a_program_without_the_safeguards() {
    let naive = "PROGRAM tank
VAR_INPUT
  level_low  : BOOL;
  level_high : BOOL;
  level      : REAL;
  reset      : BOOL;
END_VAR
VAR_OUTPUT
  fill      : BOOL;
  alarm     : BOOL;
  level_pct : REAL;
END_VAR
IF level_low THEN
  fill := TRUE;
ELSIF level_high THEN
  fill := FALSE;
END_IF;
END_PROGRAM
";
    let rows = prove(naive);
    let state = |file: &str| rows.iter().find(|r| r.file == file).unwrap().state;
    assert_eq!(state("test_fills_and_stops.st"), State::Passed);
    assert_eq!(state("test_high_switch_fails.st"), State::Failed);
    assert_eq!(state("test_both_switches_on.st"), State::Failed);
    assert_eq!(state("test_alarm_needs_reset.st"), State::Failed);
}

#[test]
fn the_plant_names_only_what_the_program_has() {
    let f = Front::new();
    assert_eq!(f.session.diags, Vec::new(), "the program compiles");
    assert!(f.session.plant.is_some(), "{PLANT}");
    assert_eq!(f.session.plant_problems, Vec::new());
    assert_eq!(f.session.plant_unbound(), Vec::new());
}

#[test]
fn with_nothing_wrong_the_tank_cycles_between_its_switches() {
    let (mut f, mut t) = (Front::new(), 0.0);
    let (mut filled, mut stopped_after, mut top, mut bottom) = (false, false, 0.0_f64, DEPTH);
    for _ in 0..800 {
        run(&mut f, &mut t, 0.05);
        filled |= f.filling();
        stopped_after |= filled && !f.filling();
        top = top.max(f.level());
        bottom = bottom.min(f.level());
        assert!(!f.alarm(), "no alarm at {t:.2} s");
    }
    assert!(filled && stopped_after, "it filled and stopped in 40 s");
    assert!(
        top < HIGH_AT + 0.1 && top > HIGH_AT - 0.1,
        "to the high switch: {top}"
    );
    assert!(bottom > LOW_AT - 0.1, "never empty: {bottom}");
}

#[test]
fn a_failed_high_switch_stops_the_fill_below_the_brim_with_the_alarm() {
    let (mut f, mut t) = (Front::new(), 0.0);
    f.set_fault(Fault::HighStuck);
    run(&mut f, &mut t, 30.0);
    assert!(f.alarm() && !f.filling());
    assert!(!f.high(), "the switch never made");
    assert!(
        f.peak > HIGH_AT && f.peak < DEPTH,
        "past the switch, short of the brim: {}",
        f.peak
    );
    // The switch mended, the alarm holds until reset; then it fills again
    // once the tank has run low.
    f.set_fault(Fault::None);
    run(&mut f, &mut t, 2.0);
    assert!(f.alarm() && !f.filling(), "latched");
    f.press_reset(t);
    run(&mut f, &mut t, 1.0);
    assert!(!f.alarm(), "reset clears it");
    let mut filled = false;
    for _ in 0..600 {
        run(&mut f, &mut t, 0.05);
        filled |= f.filling();
    }
    assert!(filled && !f.alarm(), "back to work");
}

#[test]
fn lost_switches_hold_the_program_with_the_inlet_closed() {
    let (mut f, mut t) = (Front::new(), 0.0);
    // Until it is filling.
    for _ in 0..200 {
        run(&mut f, &mut t, 0.05);
        if f.filling() {
            break;
        }
    }
    assert!(f.filling());
    f.set_fault(Fault::SwitchesLost);
    run(&mut f, &mut t, 0.5);
    assert!(f.held() && !f.filling());
    f.set_fault(Fault::None);
    run(&mut f, &mut t, 0.5);
    assert!(!f.held(), "back when the switches are");
}

#[test]
fn crossed_switches_close_the_inlet_and_alarm() {
    let (mut f, mut t) = (Front::new(), 0.0);
    f.set_fault(Fault::BothOn);
    run(&mut f, &mut t, 1.0);
    assert!(f.alarm() && !f.filling() && f.low() && f.high());
    // Reset does nothing while the readings still disagree.
    f.press_reset(t);
    run(&mut f, &mut t, 1.0);
    assert!(f.alarm());
}

/// The first run plays back: the request line by line, the program being
/// written, then the proofs one by one.
#[test]
fn the_first_run_plays_the_request_the_writing_and_then_the_proofs() {
    use crate::front::{ASK_EVERY, ASKED, REVEAL, WRITING};
    let mut f = Front::new();
    assert_eq!(
        (f.asked_shown(5.0), f.shown(5.0)),
        (0, 0),
        "nothing before the first frame"
    );
    f.tick(10.0);
    assert_eq!(f.asked_shown(10.0), 1);
    assert_eq!(f.asked_shown(10.0 + ASK_EVERY * 2.5), 3);
    assert!(f.writing(10.0 + ASK_EVERY).is_none() && !f.written(10.0 + ASK_EVERY));
    #[allow(clippy::cast_precision_loss)]
    let asked = 10.0 + ASKED.len() as f64 * ASK_EVERY;
    let half = f.writing(asked + WRITING / 2.0).expect("writing");
    assert!((half - 0.5).abs() < 1e-9);
    assert_eq!(f.shown(asked + WRITING / 2.0), 0, "no proof before the program");
    let written = asked + WRITING;
    assert!(f.written(written) && f.writing(written).is_none());
    assert_eq!(f.shown(written + REVEAL * 2.5), 2);
    assert_eq!(f.shown(100.0), TESTS.len());
    assert!(
        crate::front::summary(&f).starts_with("All 5 proved: "),
        "{}",
        crate::front::summary(&f)
    );
}

/// The page says when and how the program was written, from the record
/// the recording test left, and the request it shows is the one given.
#[test]
fn the_record_says_when_the_agent_wrote_it_and_the_request_is_the_one_shown() {
    use crate::front::{ASKED, IO, instruction, recorded};
    let on = recorded("on");
    assert!(
        on.len() == 10 && on.as_bytes()[4] == b'-' && on.as_bytes()[7] == b'-',
        "a date: {on:?}"
    );
    assert!(recorded("requests").parse::<u32>().is_ok_and(|n| n >= 1));
    assert!(recorded("seconds").parse::<u32>().is_ok());
    let asked = instruction();
    for line in ASKED {
        assert!(asked.contains(line), "{line}");
    }
    for (name, _) in IO {
        assert!(asked.contains(name), "{name}");
        assert!(
            MAIN.to_ascii_lowercase().contains(name),
            "the program uses the name it was given: {name}"
        );
    }
}
