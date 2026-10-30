//! One thread per sink: take events from a bounded channel, write them to
//! the [`Queue`], deliver the oldest, retry with backoff.
//!
//! - **Never blocks the scan.** The scan thread [`Handle::offer`]s with
//!   `try_send` into a channel of [`CHANNEL`] events; when it is full the
//!   event is counted in [`Stats::dropped_channel`] and lost.
//! - **Queued before sent.** Every event received is appended to the
//!   queue and the batch synced before the next attempt, so what is in
//!   flight is always on disk.
//! - **In order, one at a time.** Only the oldest event is attempted; a
//!   receiver sees events in the order they fired.
//! - **Retry.** A failure waits [`Backoff::delay`]: exponential from
//!   `retry_min_ms`, capped at `retry_max_ms`, with equal jitter (half
//!   fixed, half random from a seeded generator) so a fleet of boxes
//!   recovering together does not stampede the receiver. Success resets
//!   it. A rejection (400, 413, 415, 422) drops that event and moves on.
//! - **Logs** name the sink and its URL's origin only, and log state
//!   changes (down, up, dropping), not every attempt.

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::Log;
use super::emit::Event;
use super::http::{self, Outcome, Request};
use super::json;
use super::queue::{Pushed, Queue};
use super::rng::Rng;
use super::secret::Secret;
use super::url::Url;

/// Events the channel from the scan to a sink holds.
pub const CHANNEL: usize = 1024;

/// Retry timing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    pub min: Duration,
    pub max: Duration,
}

impl Backoff {
    /// The wait after failed attempt number `attempt` (0 for the first):
    /// `cap = min(max, min * 2^attempt)`, then a value in `cap/2..=cap`.
    pub fn delay(&self, attempt: u32, rng: &mut Rng) -> Duration {
        let min = u64::try_from(self.min.as_millis()).unwrap_or(u64::MAX);
        let max = u64::try_from(self.max.as_millis()).unwrap_or(u64::MAX).max(min);
        let cap = min
            .checked_shl(attempt.min(63))
            .filter(|c| *c >> attempt.min(63) == min);
        let cap = cap.map_or(max, |c| c.min(max));
        Duration::from_millis(rng.range(cap / 2, cap))
    }
}

/// One sink, ready to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SinkConfig {
    pub name: String,
    pub url: Url,
    pub headers: Vec<(String, Secret)>,
    pub timeout: Duration,
    pub queue_bytes: usize,
    pub backoff: Backoff,
    pub allow_local: bool,
}

/// A sink's counters since start.
#[derive(Debug, Default)]
pub struct Stats {
    /// 2xx received.
    pub delivered: AtomicU64,
    /// Refused for good by the receiver (and dropped).
    pub rejected: AtomicU64,
    /// Failed attempts.
    pub retries: AtomicU64,
    /// Dropped by the queue: the oldest when full, one too big, or one the
    /// disk would not take.
    pub dropped: AtomicU64,
    /// Lost because the channel from the scan was full.
    pub dropped_channel: AtomicU64,
    /// Events waiting now.
    pub queued: AtomicU64,
}

impl Stats {
    #[must_use]
    pub fn get(c: &AtomicU64) -> u64 {
        c.load(Ordering::Relaxed)
    }
}

fn bump(c: &AtomicU64, n: u64) {
    c.fetch_add(n, Ordering::Relaxed);
}

/// A running sink.
pub struct Handle {
    name: String,
    tx: Option<SyncSender<Event>>,
    stats: Arc<Stats>,
    thread: Option<JoinHandle<()>>,
}

impl Handle {
    /// Hands an event to the sink without blocking; `false` (and counted)
    /// if the channel is full or the sink has stopped.
    #[must_use]
    pub fn offer(&self, ev: Event) -> bool {
        let Some(tx) = &self.tx else {
            return false;
        };
        match tx.try_send(ev) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                bump(&self.stats.dropped_channel, 1);
                false
            }
        }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    /// Closes the channel and waits for the sink to write what it holds
    /// and exit. An attempt in flight finishes (bounded by its timeout);
    /// undelivered events stay queued on disk.
    pub fn stop(mut self) {
        self.tx = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The queue and boot-counter files of sink `name` under `dir`.
#[must_use]
pub fn paths(dir: &Path, name: &str) -> (PathBuf, PathBuf) {
    (
        dir.join(format!("{name}.queue")),
        dir.join(format!("{name}.boot")),
    )
}

/// Reads, increments and durably rewrites the boot counter. A missing file
/// is boot 0; an unreadable one restarts from the wall clock's seconds,
/// which is past any count a box reaches by restarting.
fn bump_boot(path: &Path) -> io::Result<u64> {
    let prev = match fs::read_to_string(path) {
        Ok(s) => s.trim().parse::<u64>().ok().unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(1 << 32, |d| d.as_secs())
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => 0,
        Err(e) => return Err(e),
    };
    let boot = prev.saturating_add(1);
    let tmp = path.with_extension("boot.tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        writeln!(f, "{boot}")?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    if let Some(d) = path.parent()
        && let Ok(d) = fs::File::open(d)
    {
        let _ = d.sync_all();
    }
    Ok(boot)
}

/// Starts sink `cfg` with its files under `dir`; `seed` seeds its retry
/// jitter.
///
/// # Errors
/// The queue or boot counter cannot be opened or written.
pub fn spawn(cfg: SinkConfig, dir: &Path, seed: u64, log: Log) -> io::Result<Handle> {
    let (qpath, bpath) = paths(dir, &cfg.name);
    let boot = bump_boot(&bpath)?;
    let (queue, rec) = Queue::open(&qpath, cfg.queue_bytes)?;
    let say = |m: &str| log(&format!("sink {} ({}): {m}", cfg.name, cfg.url.origin()));
    say(&format!("boot {boot}, {} event(s) queued", queue.len()));
    if rec.cut_bytes > 0 {
        say(&format!(
            "queue file had {} damaged byte(s) at its end; cut",
            rec.cut_bytes
        ));
    }
    if let Some(p) = &rec.moved_aside {
        say(&format!(
            "queue file was not a queue; moved to {} and started empty",
            p.display()
        ));
    }
    if rec.dropped > 0 {
        say(&format!(
            "dropped {} oldest event(s) to fit queue_bytes",
            rec.dropped
        ));
    }
    let stats = Arc::new(Stats::default());
    stats.queued.store(queue.len() as u64, Ordering::Relaxed);
    stats.dropped.store(rec.dropped, Ordering::Relaxed);
    let (tx, rx) = mpsc::sync_channel(CHANNEL);
    let name = cfg.name.clone();
    let w = Worker {
        cfg,
        queue,
        rx,
        stats: Arc::clone(&stats),
        log,
        rng: Rng::new(seed),
        boot,
        seq: 0,
        attempt: 0,
        next_try: Instant::now(),
        down: false,
    };
    let thread = thread::Builder::new()
        .name(format!("sink-{name}"))
        .spawn(move || w.run())?;
    Ok(Handle {
        name,
        tx: Some(tx),
        stats,
        thread: Some(thread),
    })
}

struct Worker {
    cfg: SinkConfig,
    queue: Queue,
    rx: Receiver<Event>,
    stats: Arc<Stats>,
    log: Log,
    rng: Rng,
    boot: u64,
    seq: u64,
    attempt: u32,
    next_try: Instant,
    down: bool,
}

/// Log the queue dropping events on the first drop and every this many.
const DROP_LOG_EVERY: u64 = 1000;

impl Worker {
    fn say(&self, m: &str) {
        (self.log)(&format!(
            "sink {} ({}): {m}",
            self.cfg.name,
            self.cfg.url.origin()
        ));
    }

    fn run(mut self) {
        loop {
            let got = if self.queue.is_empty() {
                self.rx.recv().map(Some).map_err(|_| ())
            } else {
                match self.next_try.checked_duration_since(Instant::now()) {
                    Some(d) if !d.is_zero() => match self.rx.recv_timeout(d) {
                        Ok(ev) => Ok(Some(ev)),
                        Err(RecvTimeoutError::Timeout) => Ok(None),
                        Err(RecvTimeoutError::Disconnected) => Err(()),
                    },
                    _ => match self.rx.try_recv() {
                        Ok(ev) => Ok(Some(ev)),
                        Err(TryRecvError::Empty) => Ok(None),
                        Err(TryRecvError::Disconnected) => Err(()),
                    },
                }
            };
            let mut open = true;
            match got {
                Ok(Some(ev)) => {
                    self.enqueue(&ev);
                    loop {
                        match self.rx.try_recv() {
                            Ok(ev) => self.enqueue(&ev),
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Disconnected) => {
                                open = false;
                                break;
                            }
                        }
                    }
                    if let Err(e) = self.queue.sync() {
                        self.say(&format!("cannot sync the queue file: {}", e.kind()));
                    }
                }
                Ok(None) => {}
                Err(()) => open = false,
            }
            if !open {
                let _ = self.queue.sync();
                return;
            }
            if !self.queue.is_empty() && Instant::now() >= self.next_try {
                self.deliver();
            }
        }
    }

    fn body(&self, key: &str, ev: &Event) -> String {
        format!(
            "{{\"key\":{},\"boot\":{},\"seq\":{},\"signal\":{},\"value\":{},\"trigger\":{},\"ts_ms\":{}}}",
            json::string(key),
            self.boot,
            self.seq,
            json::string(&ev.signal),
            json::value(ev.value),
            json::string(&ev.trigger.to_string()),
            ev.ts_ms.map_or_else(|| "null".to_string(), |t| t.to_string()),
        )
    }

    fn enqueue(&mut self, ev: &Event) {
        let key = format!("{}-{}-{}", self.cfg.name, self.boot, self.seq);
        let body = self.body(&key, ev);
        self.seq += 1;
        let dropped = match self.queue.push(&key, body.as_bytes()) {
            Ok(Pushed::Queued { dropped }) => dropped,
            Ok(Pushed::TooBig) => {
                self.say(&format!("event {key} is bigger than queue_bytes; dropped"));
                1
            }
            Err(e) => {
                self.say(&format!(
                    "cannot write event {key} to the queue file: {}; dropped",
                    e.kind()
                ));
                1
            }
        };
        if dropped > 0 {
            let before = Stats::get(&self.stats.dropped);
            bump(&self.stats.dropped, dropped);
            if before == 0 || before / DROP_LOG_EVERY != (before + dropped) / DROP_LOG_EVERY {
                self.say(&format!(
                    "queue full: dropping the oldest events ({} dropped so far)",
                    before + dropped
                ));
            }
        }
        self.stats
            .queued
            .store(self.queue.len() as u64, Ordering::Relaxed);
    }

    fn deliver(&mut self) {
        let Some(front) = self.queue.front() else {
            return;
        };
        let (key, body) = (front.key.clone(), front.body.clone());
        let req = Request {
            url: &self.cfg.url,
            headers: &self.cfg.headers,
            key: &key,
            body: &body,
        };
        let outcome = http::post(&req, self.cfg.timeout, self.cfg.allow_local, &http::std_resolve);
        match outcome {
            Outcome::Delivered(_) | Outcome::Rejected(_) => {
                if let Err(e) = self.queue.pop() {
                    self.say(&format!("cannot record delivery in the queue file: {}", e.kind()));
                }
                self.stats
                    .queued
                    .store(self.queue.len() as u64, Ordering::Relaxed);
                if let Outcome::Rejected(s) = outcome {
                    bump(&self.stats.rejected, 1);
                    self.say(&format!("event {key} rejected with status {s}; dropped"));
                } else {
                    bump(&self.stats.delivered, 1);
                }
                self.attempt = 0;
                self.next_try = Instant::now();
                if self.down {
                    self.down = false;
                    self.say(&format!("up; {} event(s) queued", self.queue.len()));
                }
            }
            Outcome::Retry(e) => {
                bump(&self.stats.retries, 1);
                let delay = self.cfg.backoff.delay(self.attempt, &mut self.rng);
                self.attempt = self.attempt.saturating_add(1);
                self.next_try = Instant::now() + delay;
                if !self.down {
                    self.down = true;
                    self.say(&format!(
                        "down: {e}; retrying, {} event(s) queued",
                        self.queue.len()
                    ));
                }
            }
        }
        self.stats
            .queued
            .store(self.queue.len() as u64, Ordering::Relaxed);
    }
}
