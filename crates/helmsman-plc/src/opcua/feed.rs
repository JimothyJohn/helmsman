//! What the scan thread hands the OPC UA server: a published
//! [`Snapshot`], replaced at most every [`FeedPublisher::every`].
//!
//! Same rule as the box API's view ([`crate::api::live`]): the server never
//! touches the runtime or the signal table. The scan thread copies the
//! values it needs and offers them with a `try_lock`; if a server thread
//! holds the slot at that instant (only long enough to clone an `Arc`), the
//! snapshot is dropped and the next one goes out instead. The shape of the
//! box (signal names, types, units, which module drives what) is rebuilt
//! only when it changes: at start, and after an apply or a reload.

use std::sync::{Arc, Mutex, PoisonError, TryLockError};
use std::time::Duration;

use helmsman_iec::PortDir;
use helmsman_io::{Dir, Ty, Value};

use crate::signals::SignalTable;
use crate::{Runtime, Status};

/// One signal, as the address space describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigShape {
    pub name: String,
    pub ty: Ty,
    pub dir: Dir,
    pub unit: Option<String>,
    /// The module whose output this is (`in NAME TYPE module M`).
    pub source: Option<String>,
    /// Read from or written to a device point (or one of the box's own
    /// channels): missing means the link is down.
    pub device: bool,
}

/// One module: its `VAR_OUTPUT`s, each with the signal it drives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModShape {
    pub name: String,
    pub outputs: Vec<(String, usize)>,
}

/// The box's shape: what the address space is built from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Shape {
    /// In signal table order.
    pub signals: Vec<SigShape>,
    /// In scan order.
    pub modules: Vec<ModShape>,
}

impl Shape {
    /// The shape of `rt` running against `table`.
    #[must_use]
    pub fn of(rt: &Runtime, table: &SignalTable) -> Self {
        let signals = (0..table.len())
            .filter_map(|i| {
                let (dir, ty) = table.signal(i)?;
                Some(SigShape {
                    name: table.name(i)?.to_string(),
                    ty,
                    dir,
                    unit: table.unit(i).map(str::to_string),
                    source: table.source(i).map(str::to_string),
                    device: table
                        .point(i)
                        .is_some_and(|(d, _)| d != helmsman_io::text::SCREEN),
                })
            })
            .collect();
        let modules = rt
            .names()
            .map(|n| ModShape {
                name: n.to_string(),
                outputs: rt
                    .program(n)
                    .map(|p| p.ports())
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|p| p.dir == PortDir::Output)
                    .filter_map(|p| {
                        table
                            .index(&p.name)
                            .map(|i| (table.name(i).unwrap_or(&p.name).to_string(), i))
                    })
                    .collect(),
            })
            .collect();
        Self { signals, modules }
    }
}

/// A module's state at publication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModState {
    Running,
    Held,
    Faulted(String),
}

impl ModState {
    #[must_use]
    pub fn text(&self) -> String {
        match self {
            Self::Running => "running".into(),
            Self::Held => "held".into(),
            Self::Faulted(why) => format!("faulted: {why}"),
        }
    }
}

/// The box as the scan thread last published it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    /// Counts publications; 0 is the empty snapshot before the first.
    pub seq: u64,
    pub shape: Arc<Shape>,
    /// Per signal: what a module reading it would see; for outputs, the
    /// command.
    pub values: Vec<Option<Value>>,
    /// Per signal: whether its link's last attempt at its point failed.
    pub errors: Vec<bool>,
    /// Per signal: when its value or quality last changed, Unix ms; `None`
    /// while the wall clock is untrusted (as for the calendar and sinks).
    pub changed: Vec<Option<i64>>,
    /// Per module, in scan order.
    pub modules: Vec<ModState>,
}

/// The slot snapshots are published into.
#[derive(Debug, Default)]
pub struct Feed {
    slot: Mutex<Arc<Snapshot>>,
}

impl Feed {
    /// The newest snapshot (the empty one before the first publication).
    #[must_use]
    pub fn snapshot(&self) -> Arc<Snapshot> {
        Arc::clone(&self.slot.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Scan side: replaces the snapshot unless a reader holds the slot
    /// right now. Never blocks. Returns whether it was published.
    pub fn offer(&self, s: Arc<Snapshot>) -> bool {
        match self.slot.try_lock() {
            Ok(mut slot) => {
                *slot = s;
                true
            }
            Err(TryLockError::Poisoned(p)) => {
                *p.into_inner() = s;
                true
            }
            Err(TryLockError::WouldBlock) => false,
        }
    }
}

/// Scan side: builds and offers snapshots at most every `every`.
#[derive(Debug)]
pub struct FeedPublisher {
    feed: Arc<Feed>,
    pub every: Duration,
    last: Option<Duration>,
    seq: u64,
    shape: Option<(u64, Arc<Shape>)>,
    prev: Vec<(Option<Value>, bool)>,
    changed: Vec<Option<i64>>,
}

impl FeedPublisher {
    #[must_use]
    pub const fn new(feed: Arc<Feed>, every: Duration) -> Self {
        Self {
            feed,
            every,
            last: None,
            seq: 0,
            shape: None,
            prev: Vec::new(),
            changed: Vec::new(),
        }
    }

    /// The box changed shape (a module applied, the map reloaded): the
    /// next publication describes it again.
    pub fn invalidate(&mut self) {
        self.shape = None;
    }

    /// Publishes a snapshot of `rt` and `table` if one is due at `now`;
    /// `wall_ms` is the trusted wall clock, if any. Called on the scan
    /// thread with the table locked: one copy of the values, plus the
    /// shape when it changed.
    pub fn publish(&mut self, now: Duration, wall_ms: Option<i64>, rt: &Runtime, table: &SignalTable) {
        if self.last.is_some_and(|l| now.saturating_sub(l) < self.every) {
            return;
        }
        let stale = self.shape.as_ref().is_none_or(|(epoch, s)| {
            *epoch != table.epoch()
                || s.signals.len() != table.len()
                || !s.modules.iter().map(|m| m.name.as_str()).eq(rt.names())
        });
        if stale {
            self.shape = Some((table.epoch(), Arc::new(Shape::of(rt, table))));
            self.prev.clear();
            self.changed.clear();
        }
        let Some((_, shape)) = &self.shape else {
            return;
        };
        let n = table.len();
        self.prev.resize(n, (None, false));
        self.changed.resize(n, wall_ms);
        let mut values = Vec::with_capacity(n);
        let mut errors = Vec::with_capacity(n);
        for i in 0..n {
            let v = table.value(i);
            let e = table.point_error(i).is_some();
            let same = match (self.prev[i].0, v) {
                (Some(Value::Real(a)), Some(Value::Real(b))) => a.to_bits() == b.to_bits(),
                (a, b) => a == b,
            };
            if !same || self.prev[i].1 != e {
                self.changed[i] = wall_ms;
                self.prev[i] = (v, e);
            }
            values.push(v);
            errors.push(e);
        }
        self.seq += 1;
        let snap = Snapshot {
            seq: self.seq,
            shape: Arc::clone(shape),
            values,
            errors,
            changed: self.changed.clone(),
            modules: rt
                .names()
                .map(|m| match rt.status(m) {
                    Some(Status::Running) => ModState::Running,
                    Some(Status::Faulted(f)) => ModState::Faulted(f.to_string()),
                    Some(Status::Held) | None => ModState::Held,
                })
                .collect(),
        };
        if self.feed.offer(Arc::new(snap)) {
            self.last = Some(now);
        }
    }
}
