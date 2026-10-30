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
//!   overruns) instead of bursting to catch up;
//! - a module's `VAR_INPUT`s and `VAR_OUTPUT`s are bound to signals by name
//!   (see [`crate::signals`]); one module drives each output signal, and a
//!   module whose inputs are missing is held, not scanned, with its outputs
//!   safe, until they are back.
//!
//! The runtime has no clock of its own: [`Runtime::tick`] takes `now`, so
//! scheduling is deterministic under test.

use std::fmt;
use std::time::Duration;

use helmsman_iec::{Carry, Fault, FaultKind, Image, PortDir, Program};

use crate::signals::{BindError, PortBinding, SignalTable, bind, from_st, to_st};

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

    fn contains(self, byte: usize) -> bool {
        self.start <= byte && self.end().is_some_and(|e| byte < e)
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
    /// Not scanned because a signal it reads has no value; its outputs are
    /// zero and its output signals safe. Runs again when the value returns.
    Held,
    /// Stopped by this fault; its outputs are zero.
    Faulted(Fault),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Scans run, including the one that faulted.
    pub scans: u64,
    /// Deadlines that passed without a scan because the module was late.
    pub overruns: u64,
    /// Deadlines skipped because the module was held.
    pub held: u64,
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
    /// A port that does not bind to the signal table.
    Port {
        module: String,
        err: BindError,
    },
    /// Two modules would drive the same output signal; `a` comes first.
    TwoWriters {
        signal: String,
        a: String,
        b: String,
    },
    /// A swap names no module of the runtime.
    UnknownModule(String),
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
            Self::Port { module, err } => write!(f, "{module}: {err}"),
            Self::TwoWriters { signal, a, b } => write!(f, "{a} and {b} both drive signal {signal}"),
            Self::UnknownModule(n) => write!(f, "no module named {n:?}"),
        }
    }
}

impl std::error::Error for ConfigError {}

#[derive(Debug)]
struct Slot {
    module: Module,
    ports: Vec<PortBinding>,
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
    i_len: usize,
    /// `now` of the last tick.
    last: Option<Duration>,
}

impl Runtime {
    /// Checks the modules against images of `i_len` and `q_len` bytes.
    /// Every module is due at time zero.
    ///
    /// # Errors
    /// Every problem found: per module in order, then each overlapping pair.
    /// A module with ports is refused here ([`ConfigError::Port`]); bind
    /// it with [`Runtime::with_signals`].
    pub fn new(i_len: usize, q_len: usize, modules: Vec<Module>) -> Result<Self, Vec<ConfigError>> {
        Self::with_signals(i_len, q_len, modules, &SignalTable::default())
    }

    /// [`Runtime::new`], with each module's ports bound by name to the
    /// signals of `table`. Pass the same table (or a clone) to
    /// [`Runtime::tick_with`].
    ///
    /// # Errors
    /// As [`Runtime::new`], plus ports that do not bind and output signals
    /// driven by two modules.
    pub fn with_signals(
        i_len: usize,
        q_len: usize,
        modules: Vec<Module>,
        table: &SignalTable,
    ) -> Result<Self, Vec<ConfigError>> {
        let ports = check(i_len, q_len, &modules.iter().collect::<Vec<_>>(), table)?;
        let slots = modules
            .into_iter()
            .zip(ports)
            .map(|(module, ports)| Slot {
                image: Image::new(module.program.sizes()),
                module,
                ports,
                status: Status::Running,
                next: Duration::ZERO,
                stats: Stats::default(),
            })
            .collect();
        Ok(Self {
            slots,
            outputs: vec![0; q_len],
            i_len,
            last: None,
        })
    }

    /// Replaces the module named `module.name` between its scans: its
    /// program, and its windows and period if those changed. The other
    /// modules, the output image and the signal commands are untouched.
    ///
    /// - Checked like a fresh load, against the other modules as they are:
    ///   windows, sizes, period, ports against `table`, one writer per
    ///   output byte and per output signal. A refused swap changes nothing.
    /// - Retained state carries by name and type
    ///   ([`helmsman_iec::Program::restore`]): a timer keeps its elapsed
    ///   time, a counter its count; a variable that is new, or whose type
    ///   changed, starts at its initial value. `%M` keeps its bytes over the
    ///   length both sizes share.
    /// - No glitch: the `%Q` bytes and output signals the module keeps hold
    ///   their last values until the new program's first scan (its `%Q`
    ///   image starts as what the output image holds). Bytes it gives up go
    ///   to zero and signals it stops driving go safe now, as no module
    ///   drives them any more.
    /// - A faulted or held module restarts ([`Status::Running`]): the fault
    ///   is cleared and its state carries like any other. A faulted module
    ///   is next due at its first deadline after the last tick, so the time
    ///   it spent stopped is not counted as overruns. Otherwise the next
    ///   deadline stays as scheduled and a new period applies after it.
    ///
    /// # Errors
    /// [`ConfigError::UnknownModule`], or every problem the new module
    /// would have in a fresh load beside the others.
    pub fn swap(&mut self, module: Module, table: &mut SignalTable) -> Result<Carry, Vec<ConfigError>> {
        let Some(k) = self.slots.iter().position(|s| s.module.name == module.name) else {
            return Err(vec![ConfigError::UnknownModule(module.name)]);
        };
        let all: Vec<&Module> = self
            .slots
            .iter()
            .enumerate()
            .map(|(n, s)| if n == k { &module } else { &s.module })
            .collect();
        let ports = check(self.i_len, self.outputs.len(), &all, table)?.swap_remove(k);
        let mut module = module;
        let s = &mut self.slots[k];
        let carry = module.program.restore(&s.module.program.state());

        let (old_q, new_q) = (s.module.q, module.q);
        for (n, b) in self.outputs[old_q.start..][..old_q.len].iter_mut().enumerate() {
            if !new_q.contains(old_q.start + n) {
                *b = 0;
            }
        }
        for b in s.ports.iter().filter(|b| b.dir == PortDir::Output) {
            if !ports
                .iter()
                .any(|p| p.dir == PortDir::Output && p.signal == b.signal)
            {
                table.make_safe(b.signal);
            }
        }
        let mut image = Image::new(module.program.sizes());
        image.q.copy_from_slice(&self.outputs[new_q.start..][..new_q.len]);
        let m = image.m.len().min(s.image.m.len());
        image.m[..m].copy_from_slice(&s.image.m[..m]);

        if let (Status::Faulted(_), Some(last)) = (s.status, self.last)
            && s.next <= last
        {
            let period = module.period.as_nanos();
            let rem = u64::try_from(last.saturating_sub(s.next).as_nanos() % period)
                .expect("remainder below a 24 h period");
            s.next = last
                .saturating_add(module.period)
                .saturating_sub(Duration::from_nanos(rem));
        }
        s.status = Status::Running;
        s.module = module;
        s.ports = ports;
        s.image = image;
        Ok(carry)
    }

    /// Runs every running module whose deadline is at or before `now`, in
    /// the order they were given, each against its window of `inputs`
    /// (bytes past the end of `inputs` read as zero). Returns the modules
    /// that faulted in this tick. For a runtime without signals.
    pub fn tick(&mut self, now: Duration, inputs: &[u8]) -> Vec<(String, Fault)> {
        self.tick_with(now, inputs, &mut SignalTable::default())
    }

    /// [`Runtime::tick`] with signals: each due module's `VAR_INPUT`s are
    /// set from `table` before its scan and its `VAR_OUTPUT`s become the
    /// commands of their signals after it. A module with an input that has
    /// no value (or one that does not fit its port) is held instead.
    pub fn tick_with(
        &mut self,
        now: Duration,
        inputs: &[u8],
        table: &mut SignalTable,
    ) -> Vec<(String, Fault)> {
        let mut faults = Vec::new();
        self.last = Some(now);
        for s in &mut self.slots {
            if matches!(s.status, Status::Faulted(_)) || s.next > now {
                continue;
            }
            let q = &mut self.outputs[s.module.q.start..][..s.module.q.len];
            if set_inputs(s, table) {
                s.status = Status::Running;
                let w = s.module.i;
                for (k, b) in s.image.i.iter_mut().enumerate() {
                    *b = inputs.get(w.start + k).copied().unwrap_or(0);
                }
                s.stats.scans = s.stats.scans.saturating_add(1);
                let result = s
                    .module
                    .program
                    .scan_at(&mut s.image, now)
                    .and_then(|()| outputs(s));
                match result {
                    Ok(commands) => {
                        q.copy_from_slice(&s.image.q);
                        for (signal, v) in commands {
                            table.drive(signal, v);
                        }
                    }
                    Err(fault) => {
                        q.fill(0);
                        make_safe(s, table);
                        s.status = Status::Faulted(fault);
                        faults.push((s.module.name.clone(), fault));
                    }
                }
            } else {
                q.fill(0);
                make_safe(s, table);
                s.status = Status::Held;
                s.stats.held = s.stats.held.saturating_add(1);
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

    /// When the next running or held module is due; `None` once all have
    /// stopped.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Duration> {
        self.slots
            .iter()
            .filter(|s| !matches!(s.status, Status::Faulted(_)))
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

    /// A module as it runs now: program, period and windows.
    #[must_use]
    pub fn module(&self, name: &str) -> Option<&Module> {
        self.slot(name).map(|s| &s.module)
    }

    /// Module names, in scan order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.slots.iter().map(|s| s.module.name.as_str())
    }
}

/// Checks `modules` as one load against images of `i_len` and `q_len`
/// bytes; each module's port bindings, in order, or every problem.
fn check(
    i_len: usize,
    q_len: usize,
    modules: &[&Module],
    table: &SignalTable,
) -> Result<Vec<Vec<PortBinding>>, Vec<ConfigError>> {
    let mut errs = Vec::new();
    let mut ports = Vec::with_capacity(modules.len());
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
        let (bound, bad) = bind(&m.program.ports(), table);
        errs.extend(
            bad.into_iter()
                .map(|err| ConfigError::Port { module: name(), err }),
        );
        ports.push(bound);
    }
    let mut writers: Vec<Option<usize>> = vec![None; table.len()];
    for (n, bound) in ports.iter().enumerate() {
        for b in bound.iter().filter(|b| b.dir == PortDir::Output) {
            match writers[b.signal] {
                Some(prev) => errs.push(ConfigError::TwoWriters {
                    signal: b.port.clone(),
                    a: modules[prev].name.clone(),
                    b: modules[n].name.clone(),
                }),
                None => writers[b.signal] = Some(n),
            }
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
    if errs.is_empty() { Ok(ports) } else { Err(errs) }
}

/// Sets every `VAR_INPUT` of the module from its signal. False if any
/// signal has no value or the value does not fit the port; inputs already
/// set stay set, which is harmless as the module is then not scanned.
fn set_inputs(s: &mut Slot, table: &SignalTable) -> bool {
    s.ports.iter().filter(|b| b.dir == PortDir::Input).all(|b| {
        table
            .value(b.signal)
            .is_some_and(|v| s.module.program.set_input(&b.port, to_st(v)).is_ok())
    })
}

/// The module's `VAR_OUTPUT`s as signal commands, or an
/// [`FaultKind::OutputRange`] fault if one cannot be carried.
fn outputs(s: &Slot) -> Result<Vec<(usize, helmsman_io::Value)>, Fault> {
    let unfit = Fault {
        kind: FaultKind::OutputRange,
        line: 0,
    };
    s.ports
        .iter()
        .filter(|b| b.dir == PortDir::Output)
        .map(|b| {
            let v = s.module.program.get(&b.port).ok_or(unfit)?;
            from_st(v, signal_ty(b.ty)).map(|v| (b.signal, v)).ok_or(unfit)
        })
        .collect()
}

fn signal_ty(st: helmsman_iec::Ty) -> helmsman_io::Ty {
    if st == helmsman_iec::Ty::Bool {
        helmsman_io::Ty::Bool
    } else if st.is_real() {
        helmsman_io::Ty::Real
    } else {
        helmsman_io::Ty::Int
    }
}

fn make_safe(s: &Slot, table: &mut SignalTable) {
    for b in s.ports.iter().filter(|b| b.dir == PortDir::Output) {
        table.make_safe(b.signal);
    }
}
