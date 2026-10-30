//! Several Structured Text modules in one process image. Each module has
//! its own scan period, an `%I` window and a `%Q` window into the box's
//! images, and its own `%M`. It is compiled against its windows' sizes, so
//! `%QX0.0` is bit 0 of *its* output window and the checker refuses any
//! address past it.
//!
//! Isolation rules, all enforced here rather than by the modules:
//! - output windows never share a byte ([`ConfigError::OutputOverlap`]);
//!   input windows may overlap, reading is free;
//! - a module's outputs reach the image only when its scan completes;
//! - a module that faults has its output window zeroed and is stopped, and
//!   the others keep their schedule;
//! - a module that falls behind skips the periods it missed (counted as
//!   overruns) instead of bursting to catch up.
//!
//! The runtime has no clock of its own: [`Runtime::tick`] takes `now`, so
//! scheduling is deterministic under test.

use std::fmt;
use std::time::Duration;

use tcpethernet_iec::{Fault, Image, Program};

/// Longest scan period a module may ask for.
pub const MAX_PERIOD: Duration = Duration::from_secs(24 * 60 * 60);

/// A byte range of a process image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub start: usize,
    pub len: usize,
}

impl Window {
    #[must_use]
    pub const fn new(start: usize, len: usize) -> Self {
        Self { start, len }
    }

    /// One past the last byte, or `None` if that overflows.
    #[must_use]
    pub const fn end(self) -> Option<usize> {
        self.start.checked_add(self.len)
    }

    fn fits(self, image: usize) -> bool {
        self.end().is_some_and(|e| e <= image)
    }

    fn overlaps(self, other: Self) -> bool {
        match (self.end(), other.end()) {
            (Some(a), Some(b)) => self.start < b && other.start < a && self.len > 0 && other.len > 0,
            _ => false,
        }
    }
}

/// A compiled program and where it sits in the box's images.
#[derive(Debug, Clone)]
pub struct Module {
    /// Unique within the runtime.
    pub name: String,
    /// Compiled against `ImageSizes { i: i.len, q: q.len, m }`.
    pub program: Program,
    pub period: Duration,
    pub i: Window,
    pub q: Window,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Running,
    /// Stopped by this fault; its outputs are zero.
    Faulted(Fault),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Scans run, including the one that faulted.
    pub scans: u64,
    /// Deadlines that passed without a scan because the module was late.
    pub overruns: u64,
}

/// Why a set of modules cannot run together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    DuplicateName(String),
    /// A window reaches past the end of the box's `area` image (`'I'` or `'Q'`).
    OutsideImage {
        module: String,
        area: char,
    },
    /// The program was compiled for a different size than its window.
    SizeMismatch {
        module: String,
        area: char,
    },
    /// Zero, or longer than [`MAX_PERIOD`].
    BadPeriod(String),
    /// Two modules would drive the same output byte; `a` comes first.
    OutputOverlap {
        a: String,
        b: String,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateName(n) => write!(f, "module {n:?} defined twice"),
            Self::OutsideImage { module, area } => {
                write!(f, "{module}: %{area} window reaches past the image")
            }
            Self::SizeMismatch { module, area } => {
                write!(
                    f,
                    "{module}: compiled for a different %{area} size than its window"
                )
            }
            Self::BadPeriod(n) => write!(f, "{n}: scan period must be 1 ns ..= 24 h"),
            Self::OutputOverlap { a, b } => write!(f, "{a} and {b} drive the same %Q bytes"),
        }
    }
}

impl std::error::Error for ConfigError {}

#[derive(Debug)]
struct Slot {
    module: Module,
    image: Image,
    status: Status,
    next: Duration,
    stats: Stats,
}

/// The modules and the box's images they share.
#[derive(Debug)]
pub struct Runtime {
    slots: Vec<Slot>,
    outputs: Vec<u8>,
}

impl Runtime {
    /// Checks the modules against images of `i_len` and `q_len` bytes.
    /// Every module is due at time zero.
    ///
    /// # Errors
    /// Every problem found: per module in order, then each overlapping pair.
    pub fn new(i_len: usize, q_len: usize, modules: Vec<Module>) -> Result<Self, Vec<ConfigError>> {
        let mut errs = Vec::new();
        for (n, m) in modules.iter().enumerate() {
            let name = || m.name.clone();
            if modules[..n].iter().any(|p| p.name == m.name) {
                errs.push(ConfigError::DuplicateName(name()));
            }
            let sizes = m.program.sizes();
            for (area, w, image, compiled) in [('I', m.i, i_len, sizes.i), ('Q', m.q, q_len, sizes.q)] {
                if !w.fits(image) {
                    errs.push(ConfigError::OutsideImage { module: name(), area });
                } else if w.len != compiled {
                    errs.push(ConfigError::SizeMismatch { module: name(), area });
                }
            }
            if m.period.is_zero() || m.period > MAX_PERIOD {
                errs.push(ConfigError::BadPeriod(name()));
            }
        }
        for (n, a) in modules.iter().enumerate() {
            for b in &modules[n + 1..] {
                if a.q.overlaps(b.q) {
                    errs.push(ConfigError::OutputOverlap {
                        a: a.name.clone(),
                        b: b.name.clone(),
                    });
                }
            }
        }
        if !errs.is_empty() {
            return Err(errs);
        }
        let slots = modules
            .into_iter()
            .map(|module| Slot {
                image: Image::new(module.program.sizes()),
                module,
                status: Status::Running,
                next: Duration::ZERO,
                stats: Stats::default(),
            })
            .collect();
        Ok(Self {
            slots,
            outputs: vec![0; q_len],
        })
    }

    /// Runs every running module whose deadline is at or before `now`, in
    /// the order they were given, each against its window of `inputs`
    /// (bytes past the end of `inputs` read as zero). Returns the modules
    /// that faulted in this tick.
    pub fn tick(&mut self, now: Duration, inputs: &[u8]) -> Vec<(String, Fault)> {
        let mut faults = Vec::new();
        for s in &mut self.slots {
            if s.status != Status::Running || s.next > now {
                continue;
            }
            let w = s.module.i;
            for (k, b) in s.image.i.iter_mut().enumerate() {
                *b = inputs.get(w.start + k).copied().unwrap_or(0);
            }
            s.stats.scans = s.stats.scans.saturating_add(1);
            let q = &mut self.outputs[s.module.q.start..][..s.module.q.len];
            match s.module.program.scan_at(&mut s.image, now) {
                Ok(()) => q.copy_from_slice(&s.image.q),
                Err(fault) => {
                    q.fill(0);
                    s.status = Status::Faulted(fault);
                    faults.push((s.module.name.clone(), fault));
                }
            }
            s.next = s.next.saturating_add(s.module.period);
            if s.next <= now {
                // Late: jump to the first deadline after `now` on the
                // original phase, counting the ones skipped.
                let behind = now.saturating_sub(s.next).as_nanos();
                let period = s.module.period.as_nanos();
                let rem = u64::try_from(behind % period).expect("remainder below a 24 h period");
                let skipped = u64::try_from(behind / period + 1).unwrap_or(u64::MAX);
                s.stats.overruns = s.stats.overruns.saturating_add(skipped);
                s.next = now
                    .saturating_add(s.module.period)
                    .saturating_sub(Duration::from_nanos(rem));
            }
        }
        faults
    }

    /// The output image as the modules last left it.
    #[must_use]
    pub fn outputs(&self) -> &[u8] {
        &self.outputs
    }

    /// When the next running module is due; `None` once all have stopped.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Duration> {
        self.slots
            .iter()
            .filter(|s| s.status == Status::Running)
            .map(|s| s.next)
            .min()
    }

    fn slot(&self, name: &str) -> Option<&Slot> {
        self.slots.iter().find(|s| s.module.name == name)
    }

    #[must_use]
    pub fn status(&self, name: &str) -> Option<Status> {
        self.slot(name).map(|s| s.status)
    }

    #[must_use]
    pub fn stats(&self, name: &str) -> Option<Stats> {
        self.slot(name).map(|s| s.stats)
    }

    /// A module's program, to read its variables.
    #[must_use]
    pub fn program(&self, name: &str) -> Option<&Program> {
        self.slot(name).map(|s| &s.module.program)
    }

    /// Module names, in scan order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.slots.iter().map(|s| s.module.name.as_str())
    }
}
