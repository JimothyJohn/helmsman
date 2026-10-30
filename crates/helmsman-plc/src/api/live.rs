//! What the scheduler hands the API: a published [`View`] of the box,
//! replaced at most every [`Publisher::every`].
//!
//! The scan thread owns the [`Runtime`] and locks the [`SignalTable`] for
//! its tick; the API must never make it wait. So the API never touches
//! either. The scan thread builds a [`View`] (a copy of every signal's
//! value, each module's status and stats, the output image) and offers it
//! with a `try_lock`: if an API thread happens to be holding the slot at
//! that instant (only ever long enough to clone an `Arc`), the view is
//! dropped and the next one is published instead. API threads read the
//! newest view by cloning its `Arc`, then serialize and write at the
//! client's pace with no lock held.

use std::sync::{Arc, Mutex, PoisonError, TryLockError};
use std::time::Duration;

use helmsman_io::Value;

use crate::signals::SignalTable;
use crate::{Runtime, Stats, Status};

/// One module's state at publication.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModuleLive {
    pub status: Status,
    pub stats: Stats,
}

/// The box as the scheduler last published it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct View {
    /// Counts publications; 0 is the empty view before the first.
    pub seq: u64,
    /// The scheduler's clock at publication (time since it started).
    pub at: Duration,
    /// Per signal, in table order: what a module reading it would see
    /// ([`SignalTable::value`]); for outputs, the command.
    pub signals: Vec<Option<Value>>,
    /// Per module, in scan order ([`Runtime::names`]).
    pub modules: Vec<ModuleLive>,
    /// The output image (`%Q`, assembly 100).
    pub outputs: Vec<u8>,
}

/// The slot views are published into.
#[derive(Debug, Default)]
pub struct Live {
    slot: Mutex<Arc<View>>,
}

impl Live {
    /// The newest view (the empty one before the first publication).
    #[must_use]
    pub fn view(&self) -> Arc<View> {
        Arc::clone(&self.slot.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Scheduler side: replaces the view unless a reader holds the slot
    /// right now. Never blocks. Returns whether it was published.
    pub fn offer(&self, view: Arc<View>) -> bool {
        match self.slot.try_lock() {
            Ok(mut slot) => {
                *slot = view;
                true
            }
            Err(TryLockError::Poisoned(p)) => {
                *p.into_inner() = view;
                true
            }
            Err(TryLockError::WouldBlock) => false,
        }
    }
}

/// Scheduler side: builds and offers views at most every `every`.
#[derive(Debug)]
pub struct Publisher {
    live: Arc<Live>,
    pub every: Duration,
    last: Option<Duration>,
    seq: u64,
}

impl Publisher {
    #[must_use]
    pub const fn new(live: Arc<Live>, every: Duration) -> Self {
        Self {
            live,
            every,
            last: None,
            seq: 0,
        }
    }

    /// Whether a view is due at `now`.
    #[must_use]
    pub fn due(&self, now: Duration) -> bool {
        self.last.is_none_or(|l| now.saturating_sub(l) >= self.every)
    }

    /// Publishes a view of `rt` and `table` if one is due at `now`.
    /// Called on the scan thread with the table already locked; costs one
    /// copy of the signal values and the output image.
    pub fn publish(&mut self, now: Duration, rt: &Runtime, table: &SignalTable) {
        if !self.due(now) {
            return;
        }
        self.seq += 1;
        let view = View {
            seq: self.seq,
            at: now,
            signals: (0..table.len()).map(|i| table.value(i)).collect(),
            modules: rt
                .names()
                .map(|n| ModuleLive {
                    status: rt.status(n).unwrap_or(Status::Held),
                    stats: rt.stats(n).unwrap_or_default(),
                })
                .collect(),
            outputs: rt.outputs().to_vec(),
        };
        if self.live.offer(Arc::new(view)) {
            self.last = Some(now);
        }
    }
}
