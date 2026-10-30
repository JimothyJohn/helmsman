//! The calendar as a signal source. [`Source::attach`] adds the calendar's
//! signals to a [`SignalTable`] as box-provided inputs, so a module binds to
//! `in_shift` the way it binds to a device signal: by naming a `VAR_INPUT`.
//! It also claims every output a `drive` line names, so a module that tries
//! to drive one too is refused when the runtime is built
//! ([`crate::ConfigError::TwoWriters`], with `calendar` as the first writer).
//!
//! [`Source::update`] takes the wall clock. A clock the box cannot trust
//! (unknown, or before [`CLOCK_FLOOR`], as on a board with no RTC that has
//! not reached NTP yet) makes every calendar signal missing, so modules
//! reading them are held, and sends every driven output to its safe value.

use std::fmt;

use helmsman_io::{Dir, Ty, Value};

use super::{Calendar, Drive, State};
use crate::signals::{SignalTable, TableError};

/// The calendar's signals, in the order [`Source`] adds them.
pub const SIGNALS: [(&str, Ty); 7] = [
    ("in_shift", Ty::Bool),
    ("on_break", Ty::Bool),
    ("planned_down", Ty::Bool),
    ("working", Ty::Bool),
    ("shift_id", Ty::Int),
    ("shift_minute", Ty::Int),
    ("shift_left", Ty::Int),
];

/// 2024-01-01T00:00Z. A wall clock before this is not set.
pub const CLOCK_FLOOR: i64 = 1_704_067_200;

/// Who drives a calendar-driven output, for [`crate::ConfigError::TwoWriters`].
pub const WRITER: &str = "calendar";

/// Why a calendar does not fit a signal table.
#[derive(Debug, Clone, PartialEq)]
pub enum AttachError {
    /// A calendar signal's name is taken by a signal of the I/O map.
    Name(TableError),
    /// `drive` names no signal.
    UnknownSignal(String),
    /// `drive` names an input signal.
    NotOutput(String),
    /// `drive` names a signal that is not `bool`.
    NotBool(String),
}

impl fmt::Display for AttachError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Name(e) => write!(f, "calendar signal: {e}"),
            Self::UnknownSignal(s) => write!(f, "drive {s}: no signal of that name"),
            Self::NotOutput(s) => write!(f, "drive {s}: an input signal cannot be driven"),
            Self::NotBool(s) => write!(f, "drive {s}: the calendar drives bool outputs only"),
        }
    }
}

impl std::error::Error for AttachError {}

/// A calendar bound to a signal table.
#[derive(Debug, Clone)]
pub struct Source {
    cal: Calendar,
    inputs: Vec<usize>,
    drives: Vec<(usize, Drive)>,
}

impl Source {
    /// Adds [`SIGNALS`] to `table` and claims each driven output. `cal`
    /// should already be valid ([`Calendar::validate`]). On error the table
    /// may hold some of the calendar's signals; build it again.
    ///
    /// # Errors
    /// Every problem: names taken, and drives of unknown, input or non-bool
    /// signals.
    pub fn attach(cal: Calendar, table: &mut SignalTable) -> Result<Self, Vec<AttachError>> {
        let mut errs = Vec::new();
        let mut inputs = Vec::new();
        for (name, ty) in SIGNALS {
            match table.add_box_input(name, ty) {
                Ok(i) => inputs.push(i),
                Err(e) => errs.push(AttachError::Name(e)),
            }
        }
        let mut drives = Vec::new();
        for d in &cal.drives {
            let Some(i) = table.index(&d.signal) else {
                errs.push(AttachError::UnknownSignal(d.signal.clone()));
                continue;
            };
            match table.signal(i) {
                Some((Dir::Out, Ty::Bool)) => {
                    table.claim(i, WRITER);
                    drives.push((i, d.clone()));
                }
                Some((Dir::Out, _)) => errs.push(AttachError::NotBool(d.signal.clone())),
                _ => errs.push(AttachError::NotOutput(d.signal.clone())),
            }
        }
        if errs.is_empty() {
            Ok(Self { cal, inputs, drives })
        } else {
            Err(errs)
        }
    }

    #[must_use]
    pub const fn calendar(&self) -> &Calendar {
        &self.cal
    }

    /// The state at wall-clock `wall` (seconds since the epoch, UTC), or
    /// `None` if that clock is not to be trusted.
    #[must_use]
    pub fn state(&self, wall: Option<i64>) -> Option<State> {
        wall.filter(|t| *t >= CLOCK_FLOOR).map(|t| self.cal.at(t))
    }

    /// Sets the calendar's signals and driven outputs for wall-clock time
    /// `wall`; see the module docs for an untrusted clock.
    pub fn update(&self, wall: Option<i64>, table: &mut SignalTable) {
        let Some(s) = self.state(wall) else {
            for &i in &self.inputs {
                table.set_input(i, None);
            }
            for (i, _) in &self.drives {
                table.make_safe(*i);
            }
            return;
        };
        let values = [
            Value::Bool(s.in_shift),
            Value::Bool(s.on_break),
            Value::Bool(s.planned_down),
            Value::Bool(s.working),
            Value::Int(s.shift_id),
            Value::Int(s.shift_minute),
            Value::Int(s.shift_left),
        ];
        for (&i, v) in self.inputs.iter().zip(values) {
            table.set_input(i, Some(v));
        }
        for (i, d) in &self.drives {
            table.drive(*i, Value::Bool(d.eval(&s)));
        }
    }
}
