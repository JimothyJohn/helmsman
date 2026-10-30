//! The box's rolling recording of what its modules saw: a bounded RAM
//! window of signal values, not a historian. [`crate::replay`] runs a
//! module against it offline.
//!
//! What is recorded: every signal a module reads that the runtime does not
//! compute itself, as a [`Channel`]:
//! - device and box inputs (the calendar's `in_shift` and friends
//!   included), value or missing ([`Kind::Input`]);
//! - outputs the box drives without a module, the calendar's `drive`
//!   lines ([`Kind::Driven`]), as their command.
//!
//! Module outputs, and the signals that carry one module's output to
//! another, are not recorded: replay recomputes them.
//!
//! When: the scheduler thread calls [`Recorder::observe`] just before each
//! tick, with the tick's `now`, so the recording holds exactly the values
//! the modules scanned against, on the runtime's own clock. A change that
//! came and went between two ticks was never seen by a module and is not
//! recorded. The links are untouched.
//!
//! Bounds: [`RecordConfig::window`] (age) and [`RecordConfig::max_bytes`]
//! (memory for change events, allocated up front and never grown). The
//! oldest changes are folded into a per-channel base value as they are
//! evicted, so a recording is exact from its `start`, however short the
//! window has become.
//!
//! Never blocking the scan: the ring sits behind a mutex the scheduler only
//! `try_lock`s. Changes it cannot hand over wait in a small preallocated
//! backlog, carved out of the same byte budget. If a reader holds the ring
//! long enough for the backlog to fill, the recording restarts from the
//! scheduler's current view at the next handover: history is lost, but the
//! recording never lies.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError};
use std::time::Duration;

use helmsman_io::{Dir, Ty, Value};

use crate::signals::SignalTable;

/// How much the recorder keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordConfig {
    /// Changes older than this (relative to the latest tick) are evicted.
    pub window: Duration,
    /// Bytes for change events, the ring and its backlog together. The
    /// per-channel bookkeeping (a few values per signal) is on top.
    pub max_bytes: usize,
}

impl Default for RecordConfig {
    /// Ten minutes, 4 MiB: about 100 000 changes.
    fn default() -> Self {
        Self {
            window: Duration::from_secs(10 * 60),
            max_bytes: 4 << 20,
        }
    }
}

/// What a channel is in the signal table it was recorded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// An input read from a device or provided by the box.
    Input,
    /// An output the box drives without a module (the calendar).
    Driven,
}

/// One recorded signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    pub name: String,
    pub ty: Ty,
    pub kind: Kind,
}

/// A channel's value changed at `at`; `None` is missing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Event {
    pub at: Duration,
    pub channel: u32,
    pub value: Option<Value>,
}

/// How a signal of `table` is recorded, if it is.
pub(crate) fn kind_of(table: &SignalTable, i: usize) -> Option<Kind> {
    match table.signal(i)? {
        (Dir::In, _) if table.source(i).is_none() => Some(Kind::Input),
        (Dir::Out, _) if table.writer(i).is_some() => Some(Kind::Driven),
        _ => None,
    }
}

/// A window of recorded values: each channel's value at `start` (`base`),
/// then every change up to `end`, in time order.
#[derive(Debug, Clone, PartialEq)]
pub struct Recording {
    /// Wall-clock milliseconds since the Unix epoch at runtime time zero,
    /// if the box knew it; for charts only.
    pub wall_ms: Option<i64>,
    pub start: Duration,
    pub end: Duration,
    pub channels: Vec<Channel>,
    pub base: Vec<Option<Value>>,
    /// Sorted by `at`, each in `start ..= end`. Two changes with the same
    /// `at` apply in order: the later wins.
    pub events: Vec<Event>,
}

fn fits(ty: Ty, v: Option<Value>) -> bool {
    v.is_none_or(|v| v.ty() == ty && !matches!(v, Value::Real(x) if !x.is_finite()))
}

fn name_ok(n: &str) -> bool {
    !n.is_empty() && n.len() <= 128 && n.bytes().all(|b| b.is_ascii_graphic())
}

impl Recording {
    /// Largest text [`Recording::from_text`] reads.
    pub const MAX_TEXT: usize = 64 << 20;

    /// A recording, checked: names unique (in any case) and printable,
    /// one base value per channel, every value of its channel's type (a
    /// real finite), events in time order within `start ..= end`.
    ///
    /// # Errors
    /// The first problem, in words.
    pub fn new(
        wall_ms: Option<i64>,
        start: Duration,
        end: Duration,
        channels: Vec<Channel>,
        base: Vec<Option<Value>>,
        events: Vec<Event>,
    ) -> Result<Self, String> {
        if end < start {
            return Err("the recording ends before it starts".into());
        }
        if u32::try_from(channels.len()).is_err() {
            return Err("too many channels".into());
        }
        for (k, c) in channels.iter().enumerate() {
            if !name_ok(&c.name) {
                return Err(format!("signal {:?}: not a printable name", c.name));
            }
            if channels[..k].iter().any(|p| p.name.eq_ignore_ascii_case(&c.name)) {
                return Err(format!("signal {} recorded twice", c.name));
            }
        }
        if base.len() != channels.len() {
            return Err("one base value per signal".into());
        }
        for (c, v) in channels.iter().zip(&base) {
            if !fits(c.ty, *v) {
                return Err(format!("base value of {} is not a {}", c.name, ty_word(c.ty)));
            }
        }
        let mut last = start;
        for e in &events {
            let Some(c) = usize::try_from(e.channel).ok().and_then(|k| channels.get(k)) else {
                return Err(format!("change of signal #{}, which is not recorded", e.channel));
            };
            if e.at < last {
                return Err(format!(
                    "change of {} at {} ns is out of time order",
                    c.name,
                    e.at.as_nanos()
                ));
            }
            if e.at > end {
                return Err(format!("change of {} after the recording ends", c.name));
            }
            if !fits(c.ty, e.value) {
                return Err(format!("change of {} is not a {}", c.name, ty_word(c.ty)));
            }
            last = e.at;
        }
        Ok(Self {
            wall_ms,
            start,
            end,
            channels,
            base,
            events,
        })
    }

    /// The channel's value at `at`: its base, then every change up to and
    /// including `at`. `None` for a missing value or an unknown channel.
    #[must_use]
    pub fn value_at(&self, channel: u32, at: Duration) -> Option<Value> {
        let k = usize::try_from(channel).ok()?;
        let base = *self.base.get(k)?;
        let upto = self.events.partition_point(|e| e.at <= at);
        self.events[..upto]
            .iter()
            .rev()
            .find(|e| e.channel == channel)
            .map_or(base, |e| e.value)
    }

    /// The recording as text, one item per line:
    ///
    /// ```text
    /// helmsman-recording 1
    /// wall_ms 1767225600000       (only if known)
    /// span START_NS END_NS
    /// signal NAME bool|int|real in|driven
    /// base INDEX VALUE            (one per signal; `-` is missing)
    /// at NS INDEX VALUE           (in time order)
    /// ```
    ///
    /// Reals are written in Rust's shortest round-trip form, so a recording
    /// reads back bit for bit.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut s = String::from("helmsman-recording 1\n");
        if let Some(w) = self.wall_ms {
            let _ = writeln!(s, "wall_ms {w}");
        }
        let _ = writeln!(s, "span {} {}", self.start.as_nanos(), self.end.as_nanos());
        for c in &self.channels {
            let kind = match c.kind {
                Kind::Input => "in",
                Kind::Driven => "driven",
            };
            let _ = writeln!(s, "signal {} {} {kind}", c.name, ty_word(c.ty));
        }
        for (k, v) in self.base.iter().enumerate() {
            let _ = writeln!(s, "base {k} {}", value_word(*v));
        }
        for e in &self.events {
            let _ = writeln!(s, "at {} {} {}", e.at.as_nanos(), e.channel, value_word(e.value));
        }
        s
    }

    /// Reads [`Recording::to_text`]'s format. Blank lines and `#` comments
    /// are skipped.
    ///
    /// # Errors
    /// Every problem, with its 1-based line (0 for the file as a whole).
    pub fn from_text(src: &str) -> Result<Self, Vec<(usize, String)>> {
        if src.len() > Self::MAX_TEXT {
            return Err(vec![(0, format!("too large: over {} bytes", Self::MAX_TEXT))]);
        }
        let mut lines = src
            .lines()
            .enumerate()
            .map(|(n, l)| (n + 1, l.trim()))
            .filter(|(_, l)| !l.is_empty() && !l.starts_with('#'));
        match lines.next() {
            Some((_, "helmsman-recording 1")) => {}
            Some((n, _)) => return Err(vec![(n, "expected `helmsman-recording 1`".into())]),
            None => return Err(vec![(0, "empty: expected `helmsman-recording 1`".into())]),
        }
        let mut errs = Vec::new();
        let mut wall_ms = None;
        let mut span = None;
        let mut channels: Vec<Channel> = Vec::new();
        let mut base: Vec<Option<Option<Value>>> = Vec::new();
        let mut events = Vec::new();
        for (n, line) in lines {
            let words: Vec<&str> = line.split_whitespace().collect();
            let r = match words.as_slice() {
                ["wall_ms", w] if wall_ms.is_none() && span.is_none() => w
                    .parse::<i64>()
                    .map(|w| wall_ms = Some(w))
                    .map_err(|_| "wall_ms: not an integer".to_string()),
                ["span", a, b] if span.is_none() => match (nanos(a), nanos(b)) {
                    (Some(a), Some(b)) if a <= b => {
                        span = Some((a, b));
                        Ok(())
                    }
                    (Some(_), Some(_)) => Err("span: the recording ends before it starts".into()),
                    _ => Err("span: expected two nanosecond counts".into()),
                },
                ["signal", name, ty, kind] if span.is_some() && base.is_empty() => {
                    parse_channel(name, ty, kind).and_then(|c| {
                        if channels.iter().any(|p| p.name.eq_ignore_ascii_case(&c.name)) {
                            return Err(format!("signal {} recorded twice", c.name));
                        }
                        channels.push(c);
                        Ok(())
                    })
                }
                ["base", k, v] if events.is_empty() => parse_ref(k, v, &channels).and_then(|(k, v)| {
                    base.resize(channels.len(), None);
                    if base[k].replace(v).is_some() {
                        Err(format!("base value of {} given twice", channels[k].name))
                    } else {
                        Ok(())
                    }
                }),
                ["at", t, k, v] => match (nanos(t), span) {
                    (None, _) => Err("at: expected a nanosecond count".into()),
                    (_, None) => Err("at: before the span line".into()),
                    (Some(at), Some((start, end))) => parse_ref(k, v, &channels).and_then(|(k, value)| {
                        if at < start || at > end {
                            return Err(format!("change of {} outside the span", channels[k].name));
                        }
                        if events.last().is_some_and(|e: &Event| e.at > at) {
                            return Err(format!("change of {} is out of time order", channels[k].name));
                        }
                        events.push(Event {
                            at,
                            channel: u32::try_from(k).unwrap_or(u32::MAX),
                            value,
                        });
                        Ok(())
                    }),
                },
                _ => Err(format!("unexpected line: {}", printable(line))),
            };
            if let Err(e) = r {
                errs.push((n, e));
            }
        }
        if !errs.is_empty() {
            return Err(errs);
        }
        let Some((start, end)) = span else {
            return Err(vec![(0, "no span line".into())]);
        };
        base.resize(channels.len(), None);
        let base: Option<Vec<_>> = base.into_iter().collect();
        let Some(base) = base else {
            return Err(vec![(0, "a signal has no base value".into())]);
        };
        Self::new(wall_ms, start, end, channels, base, events).map_err(|e| vec![(0, e)])
    }
}

/// Up to 32 characters, control characters escaped, for an error message.
fn printable(s: &str) -> String {
    s.chars().take(32).flat_map(char::escape_debug).collect()
}

fn nanos(s: &str) -> Option<Duration> {
    if !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<u64>().ok().map(Duration::from_nanos)
}

const fn ty_word(ty: Ty) -> &'static str {
    match ty {
        Ty::Bool => "bool",
        Ty::Int => "int",
        Ty::Real => "real",
    }
}

fn value_word(v: Option<Value>) -> String {
    match v {
        None => "-".into(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Int(n)) => n.to_string(),
        Some(Value::Real(x)) => format!("{x:?}"),
    }
}

fn parse_channel(name: &str, ty: &str, kind: &str) -> Result<Channel, String> {
    if !name_ok(name) {
        return Err(format!("signal {}: not a printable name", printable(name)));
    }
    let ty = match ty {
        "bool" => Ty::Bool,
        "int" => Ty::Int,
        "real" => Ty::Real,
        _ => return Err(format!("signal {name}: type must be bool, int or real")),
    };
    let kind = match kind {
        "in" => Kind::Input,
        "driven" => Kind::Driven,
        _ => return Err(format!("signal {name}: kind must be in or driven")),
    };
    Ok(Channel {
        name: name.into(),
        ty,
        kind,
    })
}

/// A channel index and a value of its type.
fn parse_ref(k: &str, v: &str, channels: &[Channel]) -> Result<(usize, Option<Value>), String> {
    let Some((k, c)) = k
        .parse::<usize>()
        .ok()
        .and_then(|k| channels.get(k).map(|c| (k, c)))
    else {
        return Err(format!("no signal #{}", printable(k)));
    };
    if v == "-" {
        return Ok((k, None));
    }
    let value = match c.ty {
        Ty::Bool => match v {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            _ => None,
        },
        Ty::Int => v.parse::<i64>().ok().map(Value::Int),
        Ty::Real => v.parse::<f64>().ok().filter(|x| x.is_finite()).map(Value::Real),
    };
    value
        .map(|v| (k, Some(v)))
        .ok_or_else(|| format!("{}: not a {} value: {}", c.name, ty_word(c.ty), printable(v)))
}

/// Counters a status page can show.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecordStats {
    /// Observations whose `now` was before the previous one's; stamped
    /// with the previous `now` instead.
    pub clamped: u64,
    /// Changes that waited in the backlog because a reader held the ring.
    pub deferred: u64,
    /// Changes folded into the base by the byte cap or the window.
    pub evicted: u64,
    /// Times the backlog filled and the recording restarted.
    pub restarts: u64,
}

#[derive(Debug)]
struct Ring {
    cap: usize,
    window: Duration,
    start: Duration,
    end: Duration,
    base: Vec<Option<Value>>,
    events: VecDeque<Event>,
    evicted: u64,
}

impl Ring {
    fn fold(&mut self, e: Event) {
        if let Some(slot) = usize::try_from(e.channel).ok().and_then(|k| self.base.get_mut(k)) {
            *slot = e.value;
        }
        self.evicted += 1;
    }

    fn push(&mut self, e: Event) {
        if self.events.len() == self.cap
            && let Some(old) = self.events.pop_front()
        {
            self.start = self.start.max(old.at);
            self.fold(old);
        }
        self.events.push_back(e);
    }

    /// Advances `end` to `now` and folds in what has aged out.
    fn age(&mut self, now: Duration) {
        self.end = self.end.max(now);
        let Some(cutoff) = self.end.checked_sub(self.window) else {
            return;
        };
        while let Some(e) = self.events.front().copied().filter(|e| e.at <= cutoff) {
            self.events.pop_front();
            self.fold(e);
        }
        self.start = self.start.max(cutoff);
    }

    fn restart(&mut self, now: Duration, view: &[Option<Value>]) {
        self.events.clear();
        self.base.copy_from_slice(view);
        self.start = now;
        self.end = now;
    }
}

#[derive(Debug)]
struct Shared {
    channels: Vec<Channel>,
    wall_ms: Option<i64>,
    ring: Mutex<Ring>,
}

/// A reader's handle on the recording: a cheap clone, `Send + Sync`.
#[derive(Debug, Clone)]
pub struct RecordTap(Arc<Shared>);

impl RecordTap {
    fn ring(&self) -> MutexGuard<'_, Ring> {
        self.0.ring.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The recording as of the last handover from the scheduler. Holds the
    /// ring for one copy; the scheduler keeps recording meanwhile.
    #[must_use]
    pub fn snapshot(&self) -> Recording {
        let ring = self.ring();
        Recording {
            wall_ms: self.0.wall_ms,
            start: ring.start,
            end: ring.end,
            channels: self.0.channels.clone(),
            base: ring.base.clone(),
            events: ring.events.iter().copied().collect(),
        }
    }

    /// Holds the ring as a slow reader would, for tests of the handover.
    #[doc(hidden)]
    #[must_use]
    pub fn hold_for_test(&self) -> impl Drop + '_ {
        self.ring()
    }
}

/// The scheduler's side; see the [module docs](self).
#[derive(Debug)]
pub struct Recorder {
    /// Table index of each channel.
    signals: Vec<usize>,
    kinds: Vec<Kind>,
    /// What each channel was at the last observation.
    view: Vec<Option<Value>>,
    backlog: Vec<Event>,
    backlog_cap: usize,
    /// The backlog overflowed: restart at the next handover.
    lost: bool,
    observed: bool,
    last: Duration,
    stats: RecordStats,
    cfg: RecordConfig,
    shared: Arc<Shared>,
}

impl Recorder {
    /// A recorder of `table`'s recordable signals (attach the calendar
    /// first). `wall_ms` is the wall clock at runtime time zero, if known.
    #[must_use]
    pub fn new(table: &SignalTable, cfg: RecordConfig, wall_ms: Option<i64>) -> Self {
        let (signals, channels): (Vec<usize>, Vec<Channel>) = (0..table.len())
            .filter_map(|i| {
                let kind = kind_of(table, i)?;
                let (_, ty) = table.signal(i)?;
                let name = table.name(i)?.to_string();
                Some((i, Channel { name, ty, kind }))
            })
            .unzip();
        let total = (cfg.max_bytes / size_of::<Event>()).max(2);
        let backlog_cap = (total / 8).clamp(1, 1024);
        let cap = total - backlog_cap;
        let kinds = channels.iter().map(|c| c.kind).collect();
        let n = channels.len();
        Self {
            signals,
            kinds,
            view: vec![None; n],
            backlog: Vec::with_capacity(backlog_cap),
            backlog_cap,
            lost: false,
            observed: false,
            last: Duration::ZERO,
            stats: RecordStats::default(),
            cfg,
            shared: Arc::new(Shared {
                channels,
                wall_ms,
                ring: Mutex::new(Ring {
                    cap,
                    window: cfg.window,
                    start: Duration::ZERO,
                    end: Duration::ZERO,
                    base: vec![None; n],
                    events: VecDeque::with_capacity(cap),
                    evicted: 0,
                }),
            }),
        }
    }

    /// A new recorder, with this one's bounds and wall clock, of `table`:
    /// what a reload of the I/O map or calendar starts recording with.
    /// The recording starts again (its channels are the new table's), at
    /// the next observation; readers need the new [`Recorder::tap`].
    #[must_use]
    pub fn rebuilt(&self, table: &SignalTable) -> Self {
        Self::new(table, self.cfg, self.shared.wall_ms)
    }

    /// Bytes reserved for change events: never more than
    /// [`RecordConfig::max_bytes`] (unless that is under two events).
    #[must_use]
    pub fn capacity_bytes(&self) -> usize {
        let cap = self
            .shared
            .ring
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .cap;
        (cap + self.backlog_cap) * size_of::<Event>()
    }

    /// A reader's handle.
    #[must_use]
    pub fn tap(&self) -> RecordTap {
        RecordTap(Arc::clone(&self.shared))
    }

    /// [`RecordTap::snapshot`].
    #[must_use]
    pub fn snapshot(&self) -> Recording {
        self.tap().snapshot()
    }

    #[must_use]
    pub const fn stats(&self) -> RecordStats {
        self.stats
    }

    /// Records what `table` holds at `now`, the time of the tick about to
    /// run. Allocates nothing and never waits: one comparison per channel,
    /// and a `try_lock` to hand changes to the ring.
    pub fn observe(&mut self, now: Duration, table: &SignalTable) {
        let now = if now < self.last {
            self.stats.clamped += 1;
            self.last
        } else {
            now
        };
        self.last = now;
        let first = !self.observed;
        let mut added = 0u64;
        self.observed = true;
        for (k, &i) in self.signals.iter().enumerate() {
            let v = match self.kinds[k] {
                Kind::Input => table.value(i),
                Kind::Driven => table.command(i),
            };
            if same(self.view[k], v) {
                continue;
            }
            self.view[k] = v;
            if self.lost {
                continue;
            }
            if self.backlog.len() == self.backlog_cap {
                self.lost = true;
                self.backlog.clear();
                continue;
            }
            self.backlog.push(Event {
                at: now,
                channel: u32::try_from(k).expect("channel count checked by the table"),
                value: v,
            });
            added += 1;
        }
        let mut ring = match self.shared.ring.try_lock() {
            Ok(r) => r,
            Err(TryLockError::Poisoned(p)) => p.into_inner(),
            Err(TryLockError::WouldBlock) => {
                self.stats.deferred += added;
                return;
            }
        };
        if first {
            ring.start = now;
        }
        if self.lost {
            self.lost = false;
            self.stats.restarts += 1;
            ring.restart(now, &self.view);
        } else {
            for e in self.backlog.drain(..) {
                ring.push(e);
            }
        }
        ring.age(now);
        self.stats.evicted = ring.evicted;
    }
}

/// Equal, with reals compared bit for bit (so `-0.0` and `0.0` differ, and
/// a change is never lost to float equality).
pub(crate) fn same(a: Option<Value>, b: Option<Value>) -> bool {
    match (a, b) {
        (Some(Value::Real(x)), Some(Value::Real(y))) => x.to_bits() == y.to_bits(),
        _ => a == b,
    }
}
