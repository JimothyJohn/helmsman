//! Replaying modules offline against a [`Recording`]: the evidence an
//! engineer approves an output change on.
//!
//! [`replay`] builds a fresh [`Runtime`] from the modules (each program
//! cold-restarted) over a copy of the signal table, feeds it the recorded
//! values on the recorded timeline and ticks it at its own deadlines, from
//! the recording's `start` to its `end`. The runtime takes `now`, so a
//! replay is a pure function of its inputs: same modules, same recording,
//! same [`Timeline`]. Everything the live runtime enforces holds here
//! because it *is* the live runtime: a module whose input is missing is
//! held with its outputs safe, a fault stops it, a slow module sees a
//! change at its next deadline.
//!
//! Faithfulness: replaying the running modules over a recording made from
//! the runtime's start reproduces the live outputs exactly when the live
//! ticks fell on their deadlines (the replay's are always on time; a live
//! scan that ran late saw a slightly later `now`). A recording that starts
//! later scans every module first at its `start`, with timers and counters
//! cold, so its first moments are a warm-up. The `%I` image (the CIP
//! scanner's assembly) is not recorded and reads as zero.
//!
//! [`propose`] replays the running modules and the same set with one
//! module replaced or added, and [`compare`]s the two per output signal.
//! [`propose_change`] does the same for a whole change set that may bring
//! its own I/O map and calendar: the proposed modules run against the
//! proposed table ([`replay_onto`]), where a signal the recording does not
//! have is missing, so the modules reading it are held, and says so
//! ([`Coverage`]).

use std::fmt;
use std::time::Duration;

use helmsman_iec::{Fault, PortDir};
use helmsman_io::{Ty, Value};

use crate::record::{Kind, Recording, kind_of, same};
use crate::runtime::{ConfigError, Module, Runtime, Status};
use crate::signals::SignalTable;

/// Most ticks one replay may take, so a 1 µs module over a ten-minute
/// window is refused rather than run for hours.
pub const MAX_TICKS: u64 = 5_000_000;

/// One output signal over the replay.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub signal: String,
    /// The module that drives it.
    pub module: String,
    pub ty: Ty,
    /// The value after each tick at which it changed, the first tick
    /// included; a step function. `None` only for a signal that carries a
    /// module's output to other modules, while that module is held or
    /// faulted.
    pub changes: Vec<(Duration, Option<Value>)>,
}

impl Track {
    /// The value from the latest change at or before `at`; `None` before
    /// the first tick.
    #[must_use]
    pub fn value_at(&self, at: Duration) -> Option<Value> {
        let upto = self.changes.partition_point(|(t, _)| *t <= at);
        upto.checked_sub(1).and_then(|k| self.changes[k].1)
    }

    /// Times a `BOOL` output turned on after the first tick: how often it
    /// "fired".
    #[must_use]
    pub fn rises(&self) -> usize {
        self.changes
            .windows(2)
            .filter(|w| w[1].1 == Some(Value::Bool(true)) && w[0].1 != Some(Value::Bool(true)))
            .count()
    }

    /// How long a `BOOL` output was on, up to `end`.
    #[must_use]
    pub fn on_time(&self, end: Duration) -> Duration {
        let mut total = Duration::ZERO;
        for (k, (from, v)) in self.changes.iter().enumerate() {
            if *v == Some(Value::Bool(true)) {
                let to = self.changes.get(k + 1).map_or(end, |c| c.0).min(end);
                total += to.saturating_sub(*from);
            }
        }
        total
    }
}

/// A module's status from `from` until `to`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub from: Duration,
    pub to: Duration,
    pub status: Status,
}

/// One module's statuses over the replay, back to back from its first
/// tick to the end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleTrack {
    pub name: String,
    pub spans: Vec<Span>,
}

/// What a replay produced: what a chart draws.
#[derive(Debug, Clone, PartialEq)]
pub struct Timeline {
    pub start: Duration,
    pub end: Duration,
    /// Runtime ticks run.
    pub ticks: u64,
    /// Every signal a module drives, in module then port order.
    pub outputs: Vec<Track>,
    pub modules: Vec<ModuleTrack>,
    /// When each module that faulted did, and why.
    pub faults: Vec<(Duration, String, Fault)>,
}

impl Timeline {
    /// An output signal's track, by name in any case.
    #[must_use]
    pub fn track(&self, signal: &str) -> Option<&Track> {
        self.outputs
            .iter()
            .find(|t| t.signal.eq_ignore_ascii_case(signal))
    }

    #[must_use]
    pub fn module(&self, name: &str) -> Option<&ModuleTrack> {
        self.modules.iter().find(|m| m.name == name)
    }
}

/// A signal recorded as one thing that the map now says is another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    pub signal: String,
    pub recorded: (Ty, Kind),
    /// `None`: the signal is now driven by a module (or carries one's
    /// output), so nothing would be replayed into it.
    pub now: Option<(Ty, Kind)>,
}

/// Why a replay cannot run.
#[derive(Debug, Clone, PartialEq)]
pub enum ReplayError {
    /// The modules do not fit the table, as the live runtime would say.
    Config(Vec<ConfigError>),
    /// The map changed under the recording: signals whose type or role
    /// differ. Replaying would feed the modules values they never saw.
    Changed(Vec<Mismatch>),
    /// A module reads a signal the recording does not have (added to the
    /// map after it was made): it would be held for the whole replay.
    NotRecorded { module: String, signal: String },
    /// More than [`MAX_TICKS`] ticks.
    TooLong { ticks: u64 },
}

fn role(r: (Ty, Kind)) -> String {
    let ty = match r.0 {
        Ty::Bool => "bool",
        Ty::Int => "int",
        Ty::Real => "real",
    };
    let kind = match r.1 {
        Kind::Input => "input",
        Kind::Driven => "box-driven output",
    };
    format!("{ty} {kind}")
}

impl fmt::Display for ReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(errs) => {
                let lines: Vec<String> = errs.iter().map(ToString::to_string).collect();
                write!(f, "modules do not fit together: {}", lines.join("; "))
            }
            Self::Changed(ms) => {
                let lines: Vec<String> = ms
                    .iter()
                    .map(|m| {
                        let now = m.now.map_or_else(|| "driven by a module".into(), role);
                        format!(
                            "{}: recorded as {}, the map now has {now}",
                            m.signal,
                            role(m.recorded)
                        )
                    })
                    .collect();
                write!(
                    f,
                    "the I/O map changed since this recording: {}",
                    lines.join("; ")
                )
            }
            Self::NotRecorded { module, signal } => {
                write!(f, "{module} reads {signal}, which this recording does not have")
            }
            Self::TooLong { ticks } => write!(
                f,
                "replay would take {ticks} scans, over the limit of {MAX_TICKS}"
            ),
        }
    }
}

impl std::error::Error for ReplayError {}

/// What a replay onto another map than the recording's could not feed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Coverage {
    /// Recorded signals that the map types or roles differently: not
    /// replayed, so they are missing (inputs) or safe (box-driven
    /// outputs) throughout.
    pub changed: Vec<Mismatch>,
    /// `(module, signal)`: a module reads a signal the recording does not
    /// have (or has as something else). An input is missing for the whole
    /// replay, so the module is held throughout; a box-driven output reads
    /// as its safe value.
    pub unrecorded: Vec<(String, String)>,
}

impl Coverage {
    /// Whether the replay fed every module everything it reads.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.changed.is_empty() && self.unrecorded.is_empty()
    }
}

/// Each recorded channel's table index and kind (`None`: not in the map
/// any more, ignored), or the channels whose type or role changed.
fn channel_slots(table: &SignalTable, rec: &Recording) -> Result<Vec<Option<(usize, Kind)>>, ReplayError> {
    let (slots, mismatches) = channel_slots_lenient(table, rec);
    if mismatches.is_empty() {
        Ok(slots)
    } else {
        Err(ReplayError::Changed(mismatches))
    }
}

/// [`channel_slots`], with the mismatched channels left out and listed.
fn channel_slots_lenient(
    table: &SignalTable,
    rec: &Recording,
) -> (Vec<Option<(usize, Kind)>>, Vec<Mismatch>) {
    let mut mismatches = Vec::new();
    let mut slots = Vec::with_capacity(rec.channels.len());
    for c in &rec.channels {
        let Some(i) = table.index(&c.name) else {
            slots.push(None);
            continue;
        };
        let ty = table.signal(i).map(|(_, ty)| ty);
        let kind = kind_of(table, i);
        if ty == Some(c.ty) && kind == Some(c.kind) {
            slots.push(Some((i, c.kind)));
        } else {
            mismatches.push(Mismatch {
                signal: c.name.clone(),
                recorded: (c.ty, c.kind),
                now: ty.zip(kind),
            });
            slots.push(None);
        }
    }
    (slots, mismatches)
}

/// Cold-restarts each module and returns the output signals each drives,
/// `(module, signal index)`; refuses a module reading an unrecorded
/// signal, and a replay over [`MAX_TICKS`].
fn check_modules(
    table: &SignalTable,
    modules: &mut [Module],
    slots: &[Option<(usize, Kind)>],
    rec: &Recording,
    mut unrecorded: Option<&mut Vec<(String, String)>>,
) -> Result<Vec<(String, usize)>, ReplayError> {
    let recorded = |i: usize| slots.iter().flatten().any(|(j, _)| *j == i);
    let mut drives = Vec::new();
    let mut ticks = 0u64;
    let window = rec.end.saturating_sub(rec.start).as_nanos();
    for m in modules {
        m.program.reset();
        for p in m.program.ports() {
            if m.params.iter().any(|(q, _)| q.eq_ignore_ascii_case(&p.name)) {
                continue;
            }
            let Some(i) = table.index(&p.name) else {
                continue;
            };
            match p.dir {
                PortDir::Input if kind_of(table, i).is_some() && !recorded(i) => {
                    let (module, signal) = (m.name.clone(), table.name(i).unwrap_or_default().to_string());
                    match unrecorded.as_deref_mut() {
                        Some(list) => list.push((module, signal)),
                        None => return Err(ReplayError::NotRecorded { module, signal }),
                    }
                }
                PortDir::Output => drives.push((m.name.clone(), i)),
                PortDir::Input => {}
            }
        }
        let period = m.period.as_nanos().max(1);
        ticks = ticks.saturating_add(u64::try_from(window / period + 1).unwrap_or(u64::MAX));
    }
    if ticks > MAX_TICKS {
        return Err(ReplayError::TooLong { ticks });
    }
    Ok(drives)
}

/// Replays `modules` over `rec` against images of `i_len` and `q_len`
/// bytes and a copy of `table` (the live one, calendar attached; its
/// current values are ignored). See the [module docs](self).
///
/// # Errors
/// [`ReplayError`]: the map changed, a module reads an unrecorded signal,
/// the modules do not fit, or the replay is too long.
pub fn replay(
    table: &SignalTable,
    i_len: usize,
    q_len: usize,
    modules: Vec<Module>,
    rec: &Recording,
) -> Result<Timeline, ReplayError> {
    let slots = channel_slots(table, rec)?;
    run(table, i_len, q_len, modules, rec, &slots, None)
}

/// [`replay`] against `table` built from another I/O map or calendar than
/// the one `rec` was recorded under (a proposal's): recorded signals the
/// table types or roles differently are not replayed, and a module that
/// reads a signal the recording lacks is not refused but runs with it
/// missing (held throughout) or, for a box-driven output, safe. What was
/// left out is the [`Coverage`].
///
/// # Errors
/// As [`replay`], but never [`ReplayError::Changed`] or
/// [`ReplayError::NotRecorded`].
pub fn replay_onto(
    table: &SignalTable,
    i_len: usize,
    q_len: usize,
    modules: Vec<Module>,
    rec: &Recording,
) -> Result<(Timeline, Coverage), ReplayError> {
    let (slots, changed) = channel_slots_lenient(table, rec);
    let mut unrecorded = Vec::new();
    let tl = run(table, i_len, q_len, modules, rec, &slots, Some(&mut unrecorded))?;
    unrecorded.sort();
    unrecorded.dedup();
    Ok((tl, Coverage { changed, unrecorded }))
}

fn run(
    table: &SignalTable,
    i_len: usize,
    q_len: usize,
    mut modules: Vec<Module>,
    rec: &Recording,
    slots: &[Option<(usize, Kind)>],
    unrecorded: Option<&mut Vec<(String, String)>>,
) -> Result<Timeline, ReplayError> {
    let drives = check_modules(table, &mut modules, slots, rec, unrecorded)?;
    let names: Vec<String> = modules.iter().map(|m| m.name.clone()).collect();
    let mut t = table.clone();
    t.reset();
    let mut rt = Runtime::with_signals(i_len, q_len, modules, &t).map_err(ReplayError::Config)?;

    let apply = |t: &mut SignalTable, channel: usize, v: Option<Value>| {
        let Some(Some((i, kind))) = slots.get(channel).copied() else {
            return;
        };
        match (kind, v) {
            (Kind::Input, v) => t.set_input(i, v),
            (Kind::Driven, Some(v)) => t.drive(i, v),
            (Kind::Driven, None) => t.make_safe(i),
        }
    };
    for (k, v) in rec.base.iter().enumerate() {
        apply(&mut t, k, *v);
    }
    let mut tl = Timeline {
        start: rec.start,
        end: rec.end,
        ticks: 0,
        outputs: drives
            .iter()
            .map(|(module, i)| Track {
                signal: t.name(*i).unwrap_or_default().into(),
                module: module.clone(),
                ty: t.signal(*i).map_or(Ty::Bool, |(_, ty)| ty),
                changes: Vec::new(),
            })
            .collect(),
        modules: names
            .iter()
            .map(|name| ModuleTrack {
                name: name.clone(),
                spans: Vec::new(),
            })
            .collect(),
        faults: Vec::new(),
    };
    let mut next = 0;
    while let Some(due) = rt.next_deadline() {
        let now = due.max(rec.start);
        if now > rec.end {
            break;
        }
        while let Some(e) = rec.events.get(next).filter(|e| e.at <= now) {
            apply(&mut t, usize::try_from(e.channel).unwrap_or(usize::MAX), e.value);
            next += 1;
        }
        for (name, fault) in rt.tick_with(now, &[], &mut t) {
            tl.faults.push((now, name, fault));
        }
        tl.ticks += 1;
        for (track, (_, i)) in tl.outputs.iter_mut().zip(&drives) {
            let v = t.value(*i);
            if track.changes.last().is_none_or(|(_, last)| !same(*last, v)) {
                track.changes.push((now, v));
            }
        }
        for m in &mut tl.modules {
            let Some(status) = rt.status(&m.name) else {
                continue;
            };
            match m.spans.last_mut() {
                Some(s) if s.status == status => {}
                last => {
                    if let Some(s) = last {
                        s.to = now;
                    }
                    m.spans.push(Span {
                        from: now,
                        to: rec.end,
                        status,
                    });
                }
            }
        }
    }
    Ok(tl)
}

/// One output signal, summarised for a review.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Summary {
    /// Times it turned on ([`Track::rises`]).
    pub rises: usize,
    /// Times its value changed after the first tick.
    pub changes: usize,
    /// How long it was on ([`Track::on_time`]).
    pub on_time: Duration,
}

impl Summary {
    fn of(track: Option<&Track>, end: Duration) -> Self {
        track.map_or_else(Self::default, |t| Self {
            rises: t.rises(),
            changes: t.changes.len().saturating_sub(1),
            on_time: t.on_time(end),
        })
    }
}

/// How one output signal differs between two replays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalDiff {
    pub signal: String,
    pub current: Summary,
    pub proposed: Summary,
    /// The two tracks change at the same moments to the same values (or
    /// neither replay drives the signal).
    pub identical: bool,
}

impl SignalDiff {
    /// "Would have fired N more times" (negative: fewer).
    #[must_use]
    pub fn extra_rises(&self) -> i64 {
        let c = i64::try_from(self.current.rises).unwrap_or(i64::MAX);
        let p = i64::try_from(self.proposed.rises).unwrap_or(i64::MAX);
        p - c
    }

    #[must_use]
    pub const fn differs(&self) -> bool {
        !self.identical
    }
}

/// Per output signal driven in either replay (`current`'s first, then
/// those only `proposed` drives): how they differ.
#[must_use]
pub fn compare(current: &Timeline, proposed: &Timeline) -> Vec<SignalDiff> {
    let mut names: Vec<&str> = current.outputs.iter().map(|t| t.signal.as_str()).collect();
    for t in &proposed.outputs {
        if !names.iter().any(|n| n.eq_ignore_ascii_case(&t.signal)) {
            names.push(&t.signal);
        }
    }
    names
        .into_iter()
        .map(|n| {
            let (a, b) = (current.track(n), proposed.track(n));
            SignalDiff {
                signal: n.into(),
                current: Summary::of(a, current.end),
                proposed: Summary::of(b, proposed.end),
                identical: match (a, b) {
                    (Some(a), Some(b)) => {
                        a.changes.len() == b.changes.len()
                            && a.changes
                                .iter()
                                .zip(&b.changes)
                                .all(|(x, y)| x.0 == y.0 && same(x.1, y.1))
                    }
                    (None, None) => true,
                    _ => false,
                },
            }
        })
        .collect()
}

/// A proposal replayed beside what runs now.
#[derive(Debug, Clone, PartialEq)]
pub struct Proposal {
    pub current: Timeline,
    pub proposed: Timeline,
    pub diff: Vec<SignalDiff>,
}

/// Replays `running` and then `running` with `proposal` in place of the
/// module of the same name (or added, if none), over the same recording.
///
/// # Errors
/// Either replay's [`ReplayError`], the current one's first.
pub fn propose(
    table: &SignalTable,
    i_len: usize,
    q_len: usize,
    running: &[Module],
    proposal: Module,
    rec: &Recording,
) -> Result<Proposal, ReplayError> {
    let current = replay(table, i_len, q_len, running.to_vec(), rec)?;
    let mut set = running.to_vec();
    match set.iter_mut().find(|m| m.name == proposal.name) {
        Some(slot) => *slot = proposal,
        None => set.push(proposal),
    }
    let proposed = replay(table, i_len, q_len, set, rec)?;
    let diff = compare(&current, &proposed);
    Ok(Proposal {
        current,
        proposed,
        diff,
    })
}

/// Replays `running` against `table` (what runs: its map and calendar)
/// and `target` against `target_table` (a change set's own map and
/// calendar, which may be `table` itself) over the same recording, and
/// compares them. The current side is strict ([`replay`]); the proposed
/// side is [`replay_onto`], and its [`Coverage`] says what it could not
/// feed.
///
/// # Errors
/// Either replay's [`ReplayError`], the current one's first.
pub fn propose_change(
    table: &SignalTable,
    running: Vec<Module>,
    target_table: &SignalTable,
    target: Vec<Module>,
    (i_len, q_len): (usize, usize),
    rec: &Recording,
) -> Result<(Proposal, Coverage), ReplayError> {
    let current = replay(table, i_len, q_len, running, rec)?;
    let (proposed, coverage) = replay_onto(target_table, i_len, q_len, target, rec)?;
    let diff = compare(&current, &proposed);
    Ok((
        Proposal {
            current,
            proposed,
            diff,
        },
        coverage,
    ))
}
