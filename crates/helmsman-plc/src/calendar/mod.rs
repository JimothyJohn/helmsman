//! The shift calendar: shifts, breaks and planned downtime as box data.
//!
//! A [`Calendar`] is weekly patterns plus dated exceptions, in local time
//! with an explicit UTC offset (and explicit offset changes, for daylight
//! saving). [`Calendar::at`] is a pure function of a wall-clock instant
//! (seconds since the Unix epoch), total over every `i64`: the same instant
//! always gives the same [`State`], so tests are deterministic and the box
//! needs nothing but a clock.
//!
//! What the box does with it ([`source`]): the state becomes input signals
//! (`in_shift`, `on_break`, ...) that modules bind to by name like any
//! device signal, and `drive` lines turn a boolean output signal into one
//! the calendar commands, under the same rules as a module's outputs.
//!
//! Rules, all checked by [`Calendar::validate`]:
//! - a shift is `start..end` local time on the day it starts; `end <= start`
//!   crosses midnight; at most 24 h; the end minute is not in the shift;
//! - shifts never overlap (refused at load), weekly or dated;
//! - a dated shift `N` on a date replaces shift `N`'s weekly occurrence
//!   starting that date (overtime, a longer Friday); `holiday DATE` cancels
//!   every weekly shift starting that date, `holiday DATE N` only shift `N`;
//!   dated shifts still run on a holiday;
//! - a break belongs to a shift and applies to each occurrence of it that
//!   wholly contains it; a break no occurrence pattern of its shift
//!   contains is refused;
//! - planned downtime is independent of shifts and may overlap anything;
//! - local time is the offset in force at the instant; across an offset
//!   change the local clock jumps, and shifts follow the local clock (a
//!   night shift over a spring-forward change is an hour shorter).
//!
//! No time zone database: `zone` takes fixed offsets only (`+01:00`), and
//! daylight saving is written as dated offset changes.

pub mod civil;
pub mod source;
pub mod text;

use std::collections::BTreeSet;
use std::fmt;

pub use civil::{Date, utc};
pub use source::{AttachError, CLOCK_FLOOR, SIGNALS, Source};
pub use text::{load, parse, render};

const DAY: i128 = 86_400;
const WEEK_MIN: i64 = 7 * 1440;

/// Most offset a zone may have, either side of UTC, in minutes.
pub const MAX_OFFSET_MIN: i16 = 18 * 60;
/// Largest shift id; `0` means "no shift" to a module.
pub const MAX_SHIFT: u8 = 99;

/// A local time window on the day it starts: `start` minutes after
/// midnight, `len` minutes long (1..=1440). It may run past midnight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: u16,
    pub len: u16,
}

impl Span {
    /// `None` unless `start < 1440` and `1 <= len < 1440`, or it is the
    /// whole day from midnight (`0`, `1440`): a 24 h window starting at any
    /// other time could not be written as `HH:MM-HH:MM`.
    #[must_use]
    pub const fn new(start: u16, len: u16) -> Option<Self> {
        if start < 1440 && len >= 1 && (len < 1440 || (len == 1440 && start == 0)) {
            Some(Self { start, len })
        } else {
            None
        }
    }
}

/// When a pattern recurs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum On {
    /// The weekdays it starts on: bit 0 Monday .. bit 6 Sunday.
    Weekly(u8),
    /// One date.
    Date(Date),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shift {
    /// 1..=[`MAX_SHIFT`].
    pub id: u8,
    pub on: On,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Break {
    pub shift: u8,
    /// Local clock time; it applies to the first such window at or after
    /// the start of each occurrence of the shift, if the occurrence holds it.
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Down {
    pub on: On,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Holiday {
    pub date: Date,
    /// `None`: every weekly shift starting that date.
    pub shift: Option<u8>,
}

/// A UTC offset, in force from `from` (seconds since the epoch, UTC) until
/// the next zone's `from`; the one without `from` is in force before all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Zone {
    pub offset_min: i16,
    pub from: Option<i64>,
}

/// What a calendar-driven output follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cond {
    InShift,
    OnBreak,
    PlannedDown,
    /// In a shift, not on a break, not in planned downtime.
    Working,
    /// In shift `N`.
    Shift(u8),
}

/// A boolean output signal commanded by the calendar: on while `cond`
/// holds (or while it does not, with `negate`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drive {
    pub signal: String,
    pub cond: Cond,
    pub negate: bool,
}

impl Drive {
    #[must_use]
    pub fn eval(&self, s: &State) -> bool {
        let v = match self.cond {
            Cond::InShift => s.in_shift,
            Cond::OnBreak => s.on_break,
            Cond::PlannedDown => s.planned_down,
            Cond::Working => s.working,
            Cond::Shift(n) => s.shift_id == i64::from(n),
        };
        v != self.negate
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Calendar {
    pub zones: Vec<Zone>,
    pub shifts: Vec<Shift>,
    pub breaks: Vec<Break>,
    pub downs: Vec<Down>,
    pub holidays: Vec<Holiday>,
    pub drives: Vec<Drive>,
}

/// The calendar at one instant: one field per signal it becomes.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub in_shift: bool,
    /// In a shift and in one of its breaks.
    pub on_break: bool,
    pub planned_down: bool,
    /// `in_shift && !on_break && !planned_down`.
    pub working: bool,
    /// The current shift's id, `0` outside shifts.
    pub shift_id: i64,
    /// Whole local minutes since the shift started, `0` outside shifts.
    pub shift_minute: i64,
    /// Local minutes until it ends, rounded up (`1..=1440`); `0` outside.
    pub shift_left: i64,
}

/// Why a calendar cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CalError {
    /// Shifts, breaks, downtime or holidays without a `zone`.
    NoZone,
    /// More than one `zone` without `from`.
    TwoBaseZones,
    /// Offset changes without a base `zone`, or not in increasing order.
    ZoneOrder,
    /// Two shifts overlap; `at` names where.
    Overlap { a: u8, b: u8, at: String },
    /// A break, holiday or drive names a shift that is never declared.
    NoSuchShift(u8),
    /// A break that no occurrence of its shift contains.
    BreakOutside(u8),
    /// Two `drive` lines for one signal.
    DriveTwice(String),
}

impl fmt::Display for CalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoZone => write!(f, "no zone: say which UTC offset the times are in (zone +01:00)"),
            Self::TwoBaseZones => write!(f, "more than one zone without `from`"),
            Self::ZoneOrder => write!(f, "offset changes need a base zone and increasing `from` times"),
            Self::Overlap { a, b, at } => write!(f, "shifts {a} and {b} overlap ({at})"),
            Self::NoSuchShift(n) => write!(f, "shift {n} is never declared"),
            Self::BreakOutside(n) => write!(f, "a break of shift {n} lies outside every occurrence of it"),
            Self::DriveTwice(s) => write!(f, "{s} is driven twice"),
        }
    }
}

impl std::error::Error for CalError {}

/// A concrete occurrence in local seconds since the epoch.
#[derive(Clone, Copy, Debug)]
struct Occ {
    id: u8,
    start: i128,
    end: i128,
}

fn on_day(mask: u8, day: i64) -> bool {
    mask & (1 << civil::weekday(day)) != 0
}

fn window(day: i64, span: Span) -> (i128, i128) {
    let start = i128::from(day) * DAY + i128::from(span.start) * 60;
    (start, start + i128::from(span.len) * 60)
}

fn days_of(on: On) -> impl Iterator<Item = u8> {
    (0..7u8).filter(move |d| matches!(on, On::Weekly(m) if m & (1 << d) != 0))
}

impl Calendar {
    /// The UTC offset in force at `t`, in minutes: the zone with the latest
    /// `from` at or before `t`, else the base zone, else UTC.
    #[must_use]
    pub fn offset_at(&self, t: i64) -> i16 {
        let changed = self
            .zones
            .iter()
            .filter_map(|z| z.from.filter(|f| *f <= t).map(|f| (f, z.offset_min)))
            .max_by_key(|(f, _)| *f);
        changed
            .map(|(_, o)| o)
            .or_else(|| self.zones.iter().find(|z| z.from.is_none()).map(|z| z.offset_min))
            .unwrap_or(0)
    }

    fn holiday(&self, day: i64, id: u8) -> bool {
        self.holidays
            .iter()
            .any(|h| h.date.day() == day && h.shift.is_none_or(|s| s == id))
    }

    /// The shifts that start on local day `day`, after exceptions.
    fn occurrences(&self, day: i64) -> impl Iterator<Item = Occ> + '_ {
        self.shifts.iter().filter_map(move |s| {
            let runs = match s.on {
                On::Date(d) => d.day() == day,
                On::Weekly(mask) => {
                    on_day(mask, day)
                        && !self.holiday(day, s.id)
                        && !self
                            .shifts
                            .iter()
                            .any(|o| o.id == s.id && matches!(o.on, On::Date(d) if d.day() == day))
                }
            };
            runs.then(|| occ(s.id, day, s.span))
        })
    }

    /// The calendar at `t` (seconds since the Unix epoch, UTC). Total: any
    /// instant, any calendar, validated or not.
    #[must_use]
    pub fn at(&self, t: i64) -> State {
        let local = i128::from(t) + i128::from(self.offset_at(t)) * 60;
        // Within ±2^63 s plus 18 h, so the day number fits an i64.
        let today = i64::try_from(local.div_euclid(DAY)).unwrap_or(0);
        let days = [today.saturating_sub(1), today];
        let occ = days
            .iter()
            .flat_map(|d| self.occurrences(*d))
            .find(|o| o.start <= local && local < o.end);
        let planned_down = days.iter().any(|&day| {
            self.downs.iter().any(|d| {
                let runs = match d.on {
                    On::Weekly(m) => on_day(m, day),
                    On::Date(x) => x.day() == day,
                };
                let (s, e) = window(day, d.span);
                runs && s <= local && local < e
            })
        });
        let Some(o) = occ else {
            return State {
                planned_down,
                ..State::default()
            };
        };
        let on_break = self
            .breaks
            .iter()
            .filter(|b| b.shift == o.id)
            .filter_map(|b| break_in(o, b.span))
            .any(|(s, e)| s <= local && local < e);
        // Both are within one day of minutes, so they fit.
        let minute = i64::try_from((local - o.start) / 60).unwrap_or(0);
        let left = i64::try_from((o.end - local + 59) / 60).unwrap_or(0);
        State {
            in_shift: true,
            on_break,
            planned_down,
            working: !on_break && !planned_down,
            shift_id: i64::from(o.id),
            shift_minute: minute,
            shift_left: left,
        }
    }

    /// Checks everything; returns every problem.
    ///
    /// # Errors
    /// The problems found.
    pub fn validate(&self) -> Result<(), Vec<CalError>> {
        let mut errs = Vec::new();
        self.check_zones(&mut errs);
        let ids: BTreeSet<u8> = self.shifts.iter().map(|s| s.id).collect();
        self.check_weekly_overlap(&mut errs);
        self.check_dated_overlap(&mut errs);
        for b in &self.breaks {
            if !ids.contains(&b.shift) {
                errs.push(CalError::NoSuchShift(b.shift));
            } else if !self
                .shifts
                .iter()
                .filter(|s| s.id == b.shift)
                .any(|s| break_in(occ(s.id, 0, s.span), b.span).is_some())
            {
                errs.push(CalError::BreakOutside(b.shift));
            }
        }
        for h in &self.holidays {
            if let Some(n) = h.shift.filter(|n| !ids.contains(n)) {
                errs.push(CalError::NoSuchShift(n));
            }
        }
        let mut driven = BTreeSet::new();
        for d in &self.drives {
            if !driven.insert(d.signal.to_ascii_uppercase()) {
                errs.push(CalError::DriveTwice(d.signal.clone()));
            }
            if let Cond::Shift(n) = d.cond
                && !ids.contains(&n)
            {
                errs.push(CalError::NoSuchShift(n));
            }
        }
        if errs.is_empty() { Ok(()) } else { Err(errs) }
    }

    fn check_zones(&self, errs: &mut Vec<CalError>) {
        let uses_time = !(self.shifts.is_empty()
            && self.breaks.is_empty()
            && self.downs.is_empty()
            && self.holidays.is_empty());
        let bases = self.zones.iter().filter(|z| z.from.is_none()).count();
        if bases == 0 && uses_time {
            errs.push(CalError::NoZone);
        }
        if bases > 1 {
            errs.push(CalError::TwoBaseZones);
        }
        let froms: Vec<i64> = self.zones.iter().filter_map(|z| z.from).collect();
        if (!froms.is_empty() && bases == 0) || froms.windows(2).any(|w| w[0] >= w[1]) {
            errs.push(CalError::ZoneOrder);
        }
    }

    /// Weekly patterns against each other, on a week that wraps.
    fn check_weekly_overlap(&self, errs: &mut Vec<CalError>) {
        let mut iv: Vec<(i64, i64, u8, u8)> = Vec::new();
        for s in &self.shifts {
            for d in days_of(s.on) {
                let start = i64::from(d) * 1440 + i64::from(s.span.start);
                let end = start + i64::from(s.span.len);
                iv.push((start, end, s.id, d));
                iv.push((start + WEEK_MIN, end + WEEK_MIN, s.id, d));
            }
        }
        iv.sort_unstable();
        let mut seen = BTreeSet::new();
        for w in iv.windows(2) {
            let ((_, a_end, a, day), (b_start, _, b, _)) = (w[0], w[1]);
            if a_end > b_start && seen.insert((a.min(b), a.max(b))) {
                errs.push(CalError::Overlap {
                    a,
                    b,
                    at: format!("weekly, from {}", text::DAY_NAMES[usize::from(day)]),
                });
            }
        }
    }

    /// Dated shifts against everything near their date.
    fn check_dated_overlap(&self, errs: &mut Vec<CalError>) {
        let dates: BTreeSet<i64> = self
            .shifts
            .iter()
            .filter_map(|s| match s.on {
                On::Date(d) => Some(d.day()),
                On::Weekly(_) => None,
            })
            .collect();
        let mut seen = BTreeSet::new();
        for day in dates {
            let mut occ: Vec<Occ> = (day - 1..=day + 1).flat_map(|d| self.occurrences(d)).collect();
            occ.sort_unstable_by_key(|o| (o.start, o.end, o.id));
            for w in occ.windows(2) {
                if w[0].end > w[1].start && seen.insert((w[0].start, w[1].start)) {
                    let (y, m, d) = civil::civil_from_days(day);
                    errs.push(CalError::Overlap {
                        a: w[0].id,
                        b: w[1].id,
                        at: format!("around {y:04}-{m:02}-{d:02}"),
                    });
                }
            }
        }
    }
}

fn occ(id: u8, day: i64, span: Span) -> Occ {
    let (start, end) = window(day, span);
    Occ { id, start, end }
}

/// The break window of `span` in occurrence `o`: the first one at or after
/// its start, if the occurrence holds all of it.
fn break_in(o: Occ, span: Span) -> Option<(i128, i128)> {
    let day = o.start.div_euclid(DAY);
    let mut s = day * DAY + i128::from(span.start) * 60;
    if s < o.start {
        s += DAY;
    }
    let e = s + i128::from(span.len) * 60;
    (e <= o.end).then_some((s, e))
}
