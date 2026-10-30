//! Handing one module's scan traces from the scheduler to a viewer.
//!
//! A viewer (the canvas, through the box API) wants what a module is doing
//! at its own rate, 30 Hz say, while the module may scan every
//! millisecond. So nothing is traced unless asked: the viewer calls
//! [`TraceTap::request`], the module's next scan is traced and its
//! [`Sample`] left in a one-slot mailbox, which [`TraceTap::take`] empties.
//! A viewer that requests once per frame gets at most one traced scan per
//! frame; every other scan runs untraced.
//!
//! The tap is a cheap clone (`Arc`), `Send + Sync`: the scheduler thread
//! owns the [`crate::Runtime`], the viewer's thread holds a tap. The
//! scheduler never blocks on it: an untraced scan reads one atomic flag,
//! and if the viewer is holding the mailbox at the moment a sample is
//! ready, that sample is dropped and the next scan is traced instead.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, TryLockError};

use helmsman_iec::Fault;
use helmsman_iec::trace::Trace;

/// One traced scan of a module.
#[derive(Debug, Clone)]
pub struct Sample {
    /// Which scan of the module this was ([`crate::Stats::scans`] after it).
    pub scan: u64,
    /// What the module's scan ended with, as the runtime saw it: the
    /// interpreter's fault, or one the runtime raised after a clean scan
    /// (an output its signal cannot carry, [`helmsman_iec::FaultKind::OutputRange`]).
    pub fault: Option<Fault>,
    pub trace: Trace,
}

#[derive(Debug, Default)]
struct Inner {
    want: AtomicBool,
    mailbox: Mutex<Option<Sample>>,
}

/// A module's trace handoff; see the [module docs](self).
#[derive(Debug, Clone, Default)]
pub struct TraceTap(Arc<Inner>);

impl TraceTap {
    /// Trace the module's next scan. Idempotent until that scan runs; a
    /// held or stopped module keeps the request until it scans again.
    pub fn request(&self) {
        self.0.want.store(true, Ordering::Release);
    }

    /// Whether a request is waiting for a scan.
    #[must_use]
    pub fn pending(&self) -> bool {
        self.0.want.load(Ordering::Acquire)
    }

    /// The newest sample not yet taken, if any.
    #[must_use]
    pub fn take(&self) -> Option<Sample> {
        self.0
            .mailbox
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// Scheduler side: claims a pending request. One relaxed load when
    /// there is none.
    pub(crate) fn claim(&self) -> bool {
        self.0.want.load(Ordering::Relaxed) && self.0.want.swap(false, Ordering::AcqRel)
    }

    /// Scheduler side: posts a sample, replacing one not yet taken. If the
    /// viewer holds the mailbox right now, the sample is dropped and the
    /// request stands, so the next scan is traced.
    pub(crate) fn post(&self, sample: Sample) {
        match self.0.mailbox.try_lock() {
            Ok(mut slot) => *slot = Some(sample),
            Err(TryLockError::Poisoned(p)) => *p.into_inner() = Some(sample),
            Err(TryLockError::WouldBlock) => self.request(),
        }
    }
}
