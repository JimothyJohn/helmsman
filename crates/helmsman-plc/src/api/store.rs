//! Where the API keeps what it is given: the audit log and proposals.
//! In memory for now, behind traits, so the snapshot store's SQLite file
//! ([`crate::store`]) can take them over without the API changing.
//!
//! Both are append-only and bounded. When full they refuse (the caller
//! answers 507) rather than forget: an audit log that drops its oldest
//! entries under a flood is one an attacker can wipe.

use std::sync::{Mutex, MutexGuard, PoisonError};

use serde_json::Value as Json;

/// One mutating call, as it was answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    /// From 1, in order.
    pub seq: u64,
    /// Wall clock, seconds since the Unix epoch (0 if the clock is unset).
    pub at: u64,
    pub method: String,
    /// The request path as sent (percent-encoded, no query).
    pub path: String,
    pub status: u16,
    /// What was done or refused, for a person.
    pub detail: String,
    /// The snapshot the call concerns (a proposal's change set, a
    /// rollback target), as its id.
    pub snapshot: Option<String>,
    /// What the box ran when the call was made, for an apply, rollback or
    /// replay.
    pub base: Option<String>,
    /// The role whose PIN the call carried, if it carried one.
    pub role: Option<String>,
}

/// The store is full; nothing was recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Full;

/// The audit log.
pub trait Audit: Send + Sync {
    /// Appends `e` (its `seq` is assigned) and returns its `seq`.
    ///
    /// # Errors
    /// [`Full`]: nothing was appended.
    fn append(&self, e: AuditEntry) -> Result<u64, Full>;
    /// Up to `limit` entries with `seq > after`, oldest first.
    fn list(&self, after: u64, limit: usize) -> Vec<AuditEntry>;
}

/// A proposal as stored: its id and its JSON rendering, which is what the
/// API serves. The approval path will add its own state alongside.
#[derive(Debug, Clone, PartialEq)]
pub struct Stored {
    pub id: u64,
    pub json: Json,
    /// The change set applying it would commit, when the box runs one.
    pub change: Option<crate::store::ChangeSet>,
}

/// Proposals: drafts awaiting the approval path.
pub trait Proposals: Send + Sync {
    /// Reserves the next id, unless full.
    ///
    /// # Errors
    /// [`Full`].
    fn reserve(&self) -> Result<u64, Full>;
    /// Stores a proposal under an id from [`Proposals::reserve`].
    fn insert(&self, p: Stored);
    fn get(&self, id: u64) -> Option<Stored>;
    /// Replaces the stored proposal with `p.id` (its status after an
    /// apply); no-op for an unknown id.
    fn update(&self, p: Stored);
    /// Every proposal, oldest first.
    fn list(&self) -> Vec<Stored>;
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// [`Audit`] in memory, at most `cap` entries.
#[derive(Debug)]
pub struct MemAudit {
    cap: usize,
    entries: Mutex<Vec<AuditEntry>>,
}

impl MemAudit {
    #[must_use]
    pub const fn new(cap: usize) -> Self {
        Self {
            cap,
            entries: Mutex::new(Vec::new()),
        }
    }
}

impl Audit for MemAudit {
    fn append(&self, mut e: AuditEntry) -> Result<u64, Full> {
        let mut entries = lock(&self.entries);
        if entries.len() >= self.cap {
            return Err(Full);
        }
        e.seq = entries.len() as u64 + 1;
        let seq = e.seq;
        entries.push(e);
        Ok(seq)
    }

    fn list(&self, after: u64, limit: usize) -> Vec<AuditEntry> {
        let entries = lock(&self.entries);
        let from = usize::try_from(after).unwrap_or(usize::MAX).min(entries.len());
        entries[from..].iter().take(limit).cloned().collect()
    }
}

/// [`Proposals`] in memory, at most `cap`.
#[derive(Debug)]
pub struct MemProposals {
    cap: usize,
    inner: Mutex<(u64, Vec<Stored>)>,
}

impl MemProposals {
    #[must_use]
    pub const fn new(cap: usize) -> Self {
        Self {
            cap,
            inner: Mutex::new((0, Vec::new())),
        }
    }
}

impl Proposals for MemProposals {
    fn reserve(&self) -> Result<u64, Full> {
        let mut inner = lock(&self.inner);
        if usize::try_from(inner.0).unwrap_or(usize::MAX) >= self.cap {
            return Err(Full);
        }
        inner.0 += 1;
        Ok(inner.0)
    }

    fn insert(&self, p: Stored) {
        let mut inner = lock(&self.inner);
        let at = inner.1.partition_point(|s| s.id < p.id);
        inner.1.insert(at, p);
    }

    fn get(&self, id: u64) -> Option<Stored> {
        lock(&self.inner).1.iter().find(|s| s.id == id).cloned()
    }

    fn update(&self, p: Stored) {
        if let Some(slot) = lock(&self.inner).1.iter_mut().find(|s| s.id == p.id) {
            *slot = p;
        }
    }

    fn list(&self) -> Vec<Stored> {
        lock(&self.inner).1.clone()
    }
}
