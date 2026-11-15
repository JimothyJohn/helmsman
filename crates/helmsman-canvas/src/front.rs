//! The front door (TODO.md `front-door`, interview 2026-11-14): what a
//! controls engineer who lands on the public page cold sees before any
//! editor. Three things on one screen: what was asked for in plain words,
//! a machine running the program written for it, and the proofs, each a
//! requirement test that throws a fault at the machine (a switch that
//! fails, a module that is lost, an impossible reading) and checks the
//! program holds safe. The visitor then throws the same faults by hand
//! and watches. The editor, the diagram and every tab are one button away
//! ("Open the editor"), on the same project.
//!
//! The machine is a tank that tops itself up (`assets/front/`): `main.st`,
//! its `plant.sim` and five `test_*.st`. It is also a built-in project
//! ([`PROJECT`]), so the editor opens on exactly what the front door ran.
//!
//! [`Front`] is the state, with no screen: its own [`Session`] running
//! the program against the plant, the proofs' results, and the fault the
//! visitor chose. [`ui`] draws it.

use helmsman_iec::Value;
use helmsman_sim::Mode;

use crate::playground::Session;
use crate::requirements::{Results, Row, State};

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

/// What was asked for, as the visitor reads it: the instruction an
/// engineer would give, one line each.
pub const ASKED: [&str; 4] = [
    "Keep the tank topped up: open the inlet when it runs low, close it when it is full.",
    "Never overflow it, even if the high switch fails.",
    "If the level switches are lost or disagree, close the inlet and raise the alarm.",
    "An alarm stays until someone presses reset.",
];

/// The tank's depth, m (`max=` of the plant).
pub const DEPTH: f64 = 2.0;
/// Where its switches make, m (`low_at=`, `high_at=`).
pub const LOW_AT: f64 = 0.2;
pub const HIGH_AT: f64 = 1.6;
/// Seconds between one proof appearing and the next.
pub const REVEAL: f64 = 0.7;
/// Seconds the reset button stays pressed after a click.
const PRESS: f64 = 0.3;

/// A fault the visitor throws at the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Fault {
    #[default]
    None,
    /// The high switch never makes.
    HighStuck,
    /// The module the level switches are wired to is gone.
    SwitchesLost,
    /// Both switches read on at once.
    BothOn,
}

impl Fault {
    pub const ALL: [Self; 4] = [Self::None, Self::HighStuck, Self::SwitchesLost, Self::BothOn];

    /// The button's words.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "Everything works",
            Self::HighStuck => "Fail the high switch",
            Self::SwitchesLost => "Lose the level switches",
            Self::BothOn => "Cross the switch wiring",
        }
    }

    /// What it does to the machine, for the button's tip.
    #[must_use]
    pub const fn tip(self) -> &'static str {
        match self {
            Self::None => "Every switch reads what the tank does.",
            Self::HighStuck => {
                "The high switch stays off however full the tank gets. Watch the inlet close anyway, before the brim."
            }
            Self::SwitchesLost => {
                "The module both switches are wired to drops off the network. The program is held and its outputs go to their safe state."
            }
            Self::BothOn => "Both switches read on at once, which no real level can do.",
        }
    }

    /// What the visitor should see happen, once it has.
    #[must_use]
    pub const fn outcome(self) -> &'static str {
        match self {
            Self::None => "The tank fills when it runs low and stops when it is full.",
            Self::HighStuck => {
                "The fill ran longer than a fill takes, so the inlet closed below the brim and the alarm came on."
            }
            Self::SwitchesLost => {
                "With its inputs gone the program is held: the inlet is closed until the switches are back."
            }
            Self::BothOn => {
                "The readings cannot both be true, so the inlet stays closed and the alarm is on."
            }
        }
    }
}

/// The front door's state.
pub struct Front {
    /// The program running against the plant.
    pub session: Session,
    /// Every proof's result, in [`TESTS`]' order.
    pub proofs: Vec<Row>,
    /// When the page first drew it (page time, s): the proofs appear one
    /// by one from then.
    since: Option<f64>,
    pub fault: Fault,
    /// The highest the level has been since the fault was chosen, m.
    pub peak: f64,
    /// Reset is pressed until (page time, s).
    pressed_until: Option<f64>,
}

impl Default for Front {
    fn default() -> Self {
        Self::new()
    }
}

/// Runs `main` against the front door's plant and tests: one row a test,
/// in [`TESTS`]' order.
#[must_use]
pub fn prove(main: &str) -> Vec<Row> {
    let files = std::iter::once((helmsman_project::MAIN, main)).chain(TESTS);
    let plan = helmsman_iec::spec::plan(files);
    let settings = helmsman_iec::spec::Settings::new(crate::playground::SIZES);
    let rows = Results::from_verdicts(&helmsman_sim::iec::run_all(&plan, settings, PLANT)).rows;
    TESTS
        .iter()
        .filter_map(|(file, _)| rows.iter().find(|r| r.file == *file).cloned())
        .collect()
}

impl Front {
    /// The machine running, every proof run.
    #[must_use]
    pub fn new() -> Self {
        let mut session = Session::new(MAIN);
        session.set_plant(PLANT);
        session.set_running(true);
        Self {
            session,
            proofs: prove(MAIN),
            since: None,
            fault: Fault::None,
            peak: 0.0,
            pressed_until: None,
        }
    }

    /// One frame at page time `t` (s): the machine moves on, a pressed
    /// reset is let go.
    pub fn tick(&mut self, t: f64) {
        self.since.get_or_insert(t);
        if self.pressed_until.is_some_and(|until| t >= until) {
            self.pressed_until = None;
            self.session.set_input("reset", Value::Bool(false));
        }
        self.session.advance(t);
        self.peak = self.peak.max(self.level());
    }

    /// How many proofs are on the screen at page time `t`.
    #[must_use]
    pub fn shown(&self, t: f64) -> usize {
        let Some(since) = self.since else { return 0 };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let n = ((t - since).max(0.0) / REVEAL) as usize;
        n.min(self.proofs.len())
    }

    /// Whether every proof is green.
    #[must_use]
    pub fn proved(&self) -> bool {
        !self.proofs.is_empty() && self.proofs.iter().all(|r| r.state == State::Passed)
    }

    /// Machine time the proofs covered, s.
    #[must_use]
    pub fn proved_s(&self) -> u64 {
        self.proofs.iter().map(|r| r.elapsed_ms).sum::<u64>() / 1000
    }

    /// Throws `fault` at the machine (or takes it away).
    pub fn set_fault(&mut self, fault: Fault) {
        self.fault = fault;
        self.peak = self.level();
        let Some(plant) = &mut self.session.plant else {
            return;
        };
        let (low, high) = match fault {
            Fault::None => (Mode::Normal, Mode::Normal),
            Fault::HighStuck => (Mode::Normal, Mode::Stuck(Some(0.0))),
            Fault::SwitchesLost => (Mode::Absent, Mode::Absent),
            Fault::BothOn => (Mode::Stuck(Some(1.0)), Mode::Stuck(Some(1.0))),
        };
        plant.set_mode("level_low", low);
        plant.set_mode("level_high", high);
    }

    /// The operator presses reset at page time `t`.
    pub fn press_reset(&mut self, t: f64) {
        self.session.set_input("reset", Value::Bool(true));
        self.pressed_until = Some(t + PRESS);
    }

    fn flag(&self, name: &str) -> bool {
        matches!(self.session.value(name), Some(Value::Bool(true)))
    }

    /// The tank's level, m.
    #[must_use]
    pub fn level(&self) -> f64 {
        match self.session.value("level") {
            Some(Value::Real(v)) => v,
            _ => 0.0,
        }
    }

    /// The inlet is open.
    #[must_use]
    pub fn filling(&self) -> bool {
        self.flag("fill")
    }

    #[must_use]
    pub fn alarm(&self) -> bool {
        self.flag("alarm")
    }

    /// What the low switch reads.
    #[must_use]
    pub fn low(&self) -> bool {
        self.flag("level_low")
    }

    /// What the high switch reads.
    #[must_use]
    pub fn high(&self) -> bool {
        self.flag("level_high")
    }

    /// The program is held, as a box holds one whose inputs are gone.
    #[must_use]
    pub fn held(&self) -> bool {
        self.session.held
    }
}
