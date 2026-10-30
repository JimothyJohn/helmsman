//! Sinks: the box core publishing signal events to the outside world.
//! Modules never see the network; a module *emits* by driving a signal
//! (a `VAR_OUTPUT` bound to an `out` signal, or to an `in NAME TYPE module
//! MODULE` signal), and a sinks file names which signals are published,
//! when, and where. The core watches the [`SignalTable`] after every scan
//! and hands each event to that sink's own thread; the scan never waits on
//! the network or the disk.
//!
//! ```text
//! # sinks.conf
//! sink   mes  https://mes.example.com/hooks/line3  timeout_ms=5000 queue_bytes=1048576
//! header mes  Authorization  file:/etc/helmsman-plc/mes.token
//! header mes  X-Line         text:3
//! emit   mes  parts  change
//! emit   mes  jam    rise
//! emit   mes  level  every_ms=60000
//! ```
//!
//! # Event semantics ([`Trigger`])
//!
//! A sink sees a signal the way a module does: its value at the end of a
//! runtime tick (the scan loop), sampled once per tick.
//!
//! - `change`: fires when the value differs from the last value seen. A
//!   real fires on any change at all (no deadband; publish analogs with
//!   `every_ms`).
//! - `rise` / `fall` (`bool` only): fires on false→true / true→false.
//! - `every_ms=N`: fires every N ms (100..=86 400 000) with the current
//!   value; late ticks skip missed periods rather than bursting.
//! - The first value seen after start is a baseline, not an event, so a
//!   restart does not replay "changes". `every_ms` fires on it.
//! - A missing value (device down, source module held or faulted) never
//!   fires and does not reset the baseline: false → missing → true is a
//!   rise, true → missing → true is not.
//! - A change undone within one tick is not seen; a pulse that a reader
//!   must not miss belongs in a counter, published on `change`.
//!
//! Each event is one JSON object, sent as a `POST` with its idempotency key
//! in the `Idempotency-Key` header and in the body:
//!
//! ```text
//! {"key":"mes-3-17","boot":3,"seq":17,"signal":"jam","value":true,"trigger":"rise","ts_ms":1793404800000}
//! ```
//!
//! `boot` is a per-sink counter persisted next to the queue and bumped on
//! every start, `seq` counts events within a boot, so a key is never
//! reused for a different event, even across restarts and an empty queue;
//! a gap in `seq` is a dropped event. `ts_ms` is wall-clock milliseconds
//! when the event fired, or `null` while the clock is untrusted (before
//! 2024-01-01: a Pi with no RTC before NTP).
//!
//! # Delivery
//!
//! At-least-once: an event is written to the sink's on-disk [`Queue`]
//! before any attempt and removed only after a 2xx, so a crash between the
//! two resends it with the same key. The receiver deduplicates by key. See
//! [`worker`] for retries, [`queue`] for the file, [`url`] for which
//! addresses a sink may reach, and [`http`] for the client.

pub mod config;
pub mod emit;
pub mod http;
pub mod json;
pub mod queue;
pub mod rng;
pub mod secret;
pub mod url;
pub mod worker;

pub use config::{EmitDecl, HeaderSource, SinkDecl, SinksFile};
pub use emit::{Emitter, Event, Fired, Trigger};
pub use queue::Queue;
pub use secret::Secret;
pub use url::Url;
pub use worker::{Backoff, Handle, SinkConfig, Stats};

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::SignalTable;

/// Every sink of a sinks file, running, and the emitter that feeds them.
pub struct Sinks {
    emitter: Emitter,
    handles: Vec<Handle>,
}

/// Where the sinks write their log lines.
pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

impl Sinks {
    /// Parses `src`, resolves header values through `env` and `read`,
    /// binds the emits to `table` and starts one thread per sink, each
    /// with its queue under `dir`.
    ///
    /// # Errors
    /// Every problem with the file, as lines to print; a queue directory
    /// that cannot be used.
    pub fn start(
        src: &str,
        dir: &Path,
        table: &SignalTable,
        env: &dyn Fn(&str) -> Option<String>,
        read: &dyn Fn(&Path) -> std::io::Result<String>,
        seed: u64,
        log: &Log,
    ) -> Result<Self, Vec<String>> {
        let file = config::parse(src).map_err(|errs| {
            errs.into_iter()
                .map(|(n, e)| format!("line {n}: {e}"))
                .collect::<Vec<_>>()
        })?;
        let names: Vec<&str> = file.sinks.iter().map(|s| s.name.as_str()).collect();
        let (cfgs, emitter) = match (file.resolve(env, read), Emitter::bind(&file.emits, &names, table)) {
            (Ok(c), Ok(e)) => (c, e),
            (c, e) => {
                let mut errs = c.err().unwrap_or_default();
                errs.extend(e.err().unwrap_or_default());
                return Err(errs);
            }
        };
        std::fs::create_dir_all(dir).map_err(|e| vec![format!("{}: {e}", dir.display())])?;
        let mut handles = Vec::new();
        for (i, cfg) in cfgs.into_iter().enumerate() {
            let name = cfg.name.clone();
            let seed = seed ^ (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            let h = worker::spawn(cfg, dir, seed, Arc::clone(log))
                .map_err(|e| vec![format!("sink {name}: {e}")])?;
            handles.push(h);
        }
        Ok(Self { emitter, handles })
    }

    /// Looks at `table` after a tick and offers each event that fired to
    /// its sink, never blocking: an event a sink's channel has no room for
    /// is counted in its [`Stats::dropped_channel`].
    pub fn observe(&mut self, table: &SignalTable, now: Duration, ts_ms: Option<i64>) {
        for f in self.emitter.observe(table, now) {
            if let Some(h) = self.handles.get(f.sink) {
                let _ = h.offer(f.into_event(ts_ms));
            }
        }
    }

    /// Each sink's name and counters.
    #[must_use]
    pub fn stats(&self) -> Vec<(&str, &Stats)> {
        self.handles.iter().map(|h| (h.name(), h.stats())).collect()
    }
}

/// Wall-clock milliseconds since the epoch, or `None` while the clock is
/// untrusted (before 2024-01-01).
#[must_use]
pub fn wall_ms(now: std::time::SystemTime) -> Option<i64> {
    const TRUSTED_FROM_MS: i64 = 1_704_067_200_000;
    let d = now.duration_since(std::time::UNIX_EPOCH).ok()?;
    i64::try_from(d.as_millis())
        .ok()
        .filter(|&ms| ms >= TRUSTED_FROM_MS)
}
