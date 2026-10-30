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
//!   safe, until they are back;
//! - a module may read another module's output through a signal bound to
//!   it; the module it names must be loaded and drive it
//!   ([`ConfigError::NoSource`]), and modules may not read each other's
//!   outputs in a cycle ([`ConfigError::Cycle`]), which would hold them
//!   all forever. A reader sees the value from its source's latest
//!   completed scan: within one tick if the source comes first in scan
//!   order, the tick before otherwise.
//!
//! The runtime has no clock of its own: [`Runtime::tick`] takes `now`, so
//! scheduling is deterministic under test.

use std::fmt;
use std::time::Duration;

use helmsman_iec::{Fault, FaultKind, Image, PortDir, Program};

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
    /// `VAR_INPUT`s held at a fixed value instead of bound to a signal
    /// (a module manifest's `param`s), set before every scan.
    pub params: Vec<(String, helmsman_io::Value)>,
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
    /// A param that names no `VAR_INPUT`, is given twice, or whose value
    /// the input cannot hold.
    Param {
        module: String,
        param: String,
    },
    /// `module` reads `signal`, another module's output, but no loaded
    /// module of that name drives it.
    NoSource {
        module: String,
        signal: String,
    },
    /// Modules that read each other's outputs in a loop, each reading the
    /// one before it (the first reads the last).
    Cycle(Vec<String>),
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
            Self::Param { module, param } => write!(
                f,
                "{module}: param {param} names no VAR_INPUT that can hold its value, or is given twice"
            ),
            Self::NoSource { module, signal } => write!(
                f,
                "{module}: reads {signal}, a module output that no loaded module drives"
            ),
            Self::Cycle(names) => write!(
                f,
                "modules read each other's outputs in a loop: {}",
                names.join(" -> ")
            ),
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
            let mut probe = m.program.clone();
            for (k, (p, v)) in m.params.iter().enumerate() {
                let twice = m.params[..k].iter().any(|(q, _)| q.eq_ignore_ascii_case(p));
                if twice || probe.set_input(p, to_st(*v)).is_err() {
                    errs.push(ConfigError::Param {
                        module: name(),
                        param: p.clone(),
                    });
                }
            }
            let signal_ports: Vec<_> = m
                .program
                .ports()
                .into_iter()
                .filter(|p| !m.params.iter().any(|(q, _)| q.eq_ignore_ascii_case(&p.name)))
                .collect();
            let (bound, bad) = bind(&m.name, &signal_ports, table);
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
        errs.extend(module_links(&modules, &ports, table, &writers));
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
        })
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

    /// Module names, in scan order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.slots.iter().map(|s| s.module.name.as_str())
    }
}

/// Checks the signals modules read from each other: each has a loaded
/// source that drives it, and no loop. `writers[signal]` is the module
/// driving it.
fn module_links(
    modules: &[Module],
    ports: &[Vec<PortBinding>],
    table: &SignalTable,
    writers: &[Option<usize>],
) -> Vec<ConfigError> {
    let mut errs = Vec::new();
    // feeds[a] = modules reading an output of module a.
    let mut feeds: Vec<Vec<usize>> = vec![Vec::new(); modules.len()];
    for (n, bound) in ports.iter().enumerate() {
        for b in bound.iter().filter(|b| b.dir == PortDir::Input) {
            let Some(source) = table.source(b.signal) else {
                continue;
            };
            match writers[b.signal].filter(|&w| modules[w].name == source) {
                Some(w) => {
                    if !feeds[w].contains(&n) {
                        feeds[w].push(n);
                    }
                }
                None => errs.push(ConfigError::NoSource {
                    module: modules[n].name.clone(),
                    signal: b.port.clone(),
                }),
            }
        }
    }
    if let Some(cycle) = find_cycle(&feeds) {
        errs.push(ConfigError::Cycle(
            cycle.into_iter().map(|n| modules[n].name.clone()).collect(),
        ));
    }
    errs
}

/// The first loop in the graph `edges`, from the lowest-numbered node a
/// depth-first search reaches it from; deterministic for a given graph.
fn find_cycle(edges: &[Vec<usize>]) -> Option<Vec<usize>> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        New,
        Open,
        Done,
    }
    let mut mark = vec![Mark::New; edges.len()];
    for root in 0..edges.len() {
        if mark[root] != Mark::New {
            continue;
        }
        // (node, next edge to follow); the stack is the current path.
        let mut stack = vec![(root, 0usize)];
        mark[root] = Mark::Open;
        while let Some(&mut (node, ref mut k)) = stack.last_mut() {
            let Some(&next) = edges[node].get(*k) else {
                mark[node] = Mark::Done;
                stack.pop();
                continue;
            };
            *k += 1;
            match mark[next] {
                Mark::New => {
                    mark[next] = Mark::Open;
                    stack.push((next, 0));
                }
                Mark::Open => {
                    let from = stack.iter().position(|&(n, _)| n == next)?;
                    return Some(stack[from..].iter().map(|&(n, _)| n).collect());
                }
                Mark::Done => {}
            }
        }
    }
    None
}

/// Sets every `VAR_INPUT` of the module from its signal. False if any
/// signal has no value or the value does not fit the port; inputs already
/// set stay set, which is harmless as the module is then not scanned.
fn set_inputs(s: &mut Slot, table: &SignalTable) -> bool {
    for (p, v) in &s.module.params {
        // Checked when the runtime was built.
        let _ = s.module.program.set_input(p, to_st(*v));
    }
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
