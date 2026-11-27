//! The tank example (`assets/front/`): a tank that tops itself up,
//! `main.st`, its `plant.sim` and five `test_*.st`, a built-in project of
//! the simulation ([`PROJECT`]). It was the front door's machine (TODO.md
//! `front-door`, 2026-11-14); the front door is gone (Nick, 2026-11-27:
//! the public page goes straight to the playground), the example stays.

/// The built-in project's name.
pub const PROJECT: &str = "tank";
/// The program.
pub const MAIN: &str = include_str!("../assets/front/main.st");
/// The machine it runs against.
pub const PLANT: &str = include_str!("../assets/front/plant.sim");
/// The requirement tests, in the order the proofs are shown.
pub const TESTS: [(&str, &str); 5] = [
    (
        "test_fills_and_stops.st",
        include_str!("../assets/front/test_fills_and_stops.st"),
    ),
    (
        "test_high_switch_fails.st",
        include_str!("../assets/front/test_high_switch_fails.st"),
    ),
    (
        "test_switches_lost.st",
        include_str!("../assets/front/test_switches_lost.st"),
    ),
    (
        "test_both_switches_on.st",
        include_str!("../assets/front/test_both_switches_on.st"),
    ),
    (
        "test_alarm_needs_reset.st",
        include_str!("../assets/front/test_alarm_needs_reset.st"),
    ),
];

/// The I/O, named before anything was asked: what each point is.
#[cfg(test)]
pub const IO: [(&str, &str); 7] = [
    ("level_low", "input, on when the tank is nearly empty"),
    ("level_high", "input, on when the tank is full"),
    ("level", "input, the level in metres (the tank is 2 m deep)"),
    ("reset", "input, the operator's alarm reset"),
    ("fill", "output, opens the inlet valve"),
    ("alarm", "output"),
    (
        "level_pct",
        "output, the level in percent for the operator's screen",
    ),
];

/// What was asked for, as the visitor reads it: the instruction the
/// Agent was given, one line each ([`instruction`] is the whole of it).
#[cfg(test)]
pub const ASKED: [&str; 4] = [
    "Keep the tank topped up: open the inlet when it runs low, close it when it is full.",
    "Never overflow it, even if the high switch fails: a fill takes under 6 s, so one that runs 7 s closes the inlet and raises the alarm.",
    "If the low and high switches are both on, close the inlet and raise the alarm.",
    "An alarm stays, with the inlet closed, until reset is pressed and the switches no longer disagree.",
];

/// The instruction the tank's program was written from: the
/// I/O by name, then [`ASKED`].
#[cfg(test)]
#[must_use]
pub fn instruction() -> String {
    use std::fmt::Write as _;
    let mut s = String::from("A tank that keeps itself topped up. Its I/O, with these names:\n");
    for (name, what) in IO {
        let _ = writeln!(s, "- {name}: {what}");
    }
    s.push_str("What it must do:\n");
    for line in ASKED {
        let _ = writeln!(s, "- {line}");
    }
    s
}

/// Runs `main` against the tank's plant and tests: one row a test, in
/// [`TESTS`]' order.
#[cfg(test)]
#[must_use]
pub fn prove(main: &str) -> Vec<crate::requirements::Row> {
    use crate::requirements::Results;
    let files = std::iter::once((helmsman_project::MAIN, main)).chain(TESTS);
    let plan = helmsman_iec::spec::plan(files);
    let settings = helmsman_iec::spec::Settings::new(crate::playground::SIZES);
    let rows = Results::from_verdicts(&helmsman_sim::iec::run_all(&plan, settings, PLANT)).rows;
    TESTS
        .iter()
        .filter_map(|(file, _)| rows.iter().find(|r| r.file == *file).cloned())
        .collect()
}
