//! Signals in the scan. A module's `VAR_INPUT`s and `VAR_OUTPUT`s are bound
//! by name (case-insensitive, as in ST) to the signals of an [`IoMap`]; the
//! [`SignalTable`] holds their values between the device links that poll
//! and write devices and the runtime that scans modules.
//!
//! - An input signal's value is what its device last read, or nothing when
//!   the device is down or has not answered yet.
//! - An output signal's *command* is what a module last computed, or its
//!   safe value: before any module has driven it, while its module is held
//!   or faulted, and when no module drives it at all.
//! - A module never computes on missing data: if any signal it reads has no
//!   value, it is held (not scanned) and its outputs go to their safe values
//!   until every input is back.

use std::collections::BTreeMap;
use std::fmt;

use tcpethernet_iec::{Port, PortDir, Ty as StTy, Value as StValue};
use tcpethernet_io::{Binding, Dir, IoMap, MapError, Ty, Value};

#[derive(Debug, Clone)]
struct Sig {
    name: String,
    ty: Ty,
    dir: Dir,
    /// Inputs: the last good read.
    value: Option<Value>,
    /// Outputs: what the links write.
    command: Value,
    safe: Value,
}

/// Why an I/O map and its safe values cannot become a table.
#[derive(Debug, Clone, PartialEq)]
pub enum TableError {
    Map(MapError),
    /// Two signal names that differ only in case: ST could not tell them apart.
    SameName {
        a: String,
        b: String,
    },
    /// A safe value for something that is not an output signal.
    SafeNotOutput(String),
    /// A safe value of the wrong type.
    SafeType(String),
}

impl fmt::Display for TableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Map(e) => e.fmt(f),
            Self::SameName { a, b } => write!(f, "signals {a} and {b} differ only in case"),
            Self::SafeNotOutput(n) => write!(f, "safe value for {n}, which is not an output signal"),
            Self::SafeType(n) => write!(f, "safe value for {n} has the wrong type"),
        }
    }
}

impl std::error::Error for TableError {}

/// Every signal's current value. Shared, behind a mutex, between the
/// runtime and the device links.
#[derive(Debug, Clone, Default)]
pub struct SignalTable {
    sigs: Vec<Sig>,
}

const fn zero(ty: Ty) -> Value {
    match ty {
        Ty::Bool => Value::Bool(false),
        Ty::Int => Value::Int(0),
        Ty::Real => Value::Real(0.0),
    }
}

impl SignalTable {
    /// One entry per signal of `map`, in map order. Outputs start at their
    /// safe value: `safe[name]` if given, else off / zero.
    ///
    /// # Errors
    /// Every problem: the map's own ([`IoMap::validate`]), names equal but
    /// for case, and safe values that name no output or have the wrong type.
    pub fn new(map: &IoMap, safe: &BTreeMap<String, Value>) -> Result<Self, Vec<TableError>> {
        let mut errs: Vec<TableError> = map
            .validate()
            .err()
            .unwrap_or_default()
            .into_iter()
            .map(TableError::Map)
            .collect();
        let mut seen: BTreeMap<String, &str> = BTreeMap::new();
        for s in &map.signals {
            if let Some(prev) = seen.insert(s.name.to_ascii_uppercase(), &s.name)
                && prev != s.name
            {
                errs.push(TableError::SameName {
                    a: prev.into(),
                    b: s.name.clone(),
                });
            }
        }
        for (name, v) in safe {
            match map.signals.iter().find(|s| &s.name == name) {
                Some(s) if s.dir == Dir::Out => {
                    if v.ty() != s.ty || matches!(v, Value::Real(x) if !x.is_finite()) {
                        errs.push(TableError::SafeType(name.clone()));
                    }
                }
                _ => errs.push(TableError::SafeNotOutput(name.clone())),
            }
        }
        if !errs.is_empty() {
            return Err(errs);
        }
        let sigs = map
            .signals
            .iter()
            .map(|s| {
                let safe = safe.get(&s.name).copied().unwrap_or(zero(s.ty));
                Sig {
                    name: s.name.clone(),
                    ty: s.ty,
                    dir: s.dir,
                    value: None,
                    command: safe,
                    safe,
                }
            })
            .collect();
        Ok(Self { sigs })
    }

    /// Index of a signal, by name in any case.
    #[must_use]
    pub fn index(&self, name: &str) -> Option<usize> {
        self.sigs.iter().position(|s| s.name.eq_ignore_ascii_case(name))
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.sigs.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sigs.is_empty()
    }

    /// What a module reading this signal sees: an input's last good read
    /// (`None` while its device is down), an output's command.
    #[must_use]
    pub fn value(&self, i: usize) -> Option<Value> {
        let s = self.sigs.get(i)?;
        match s.dir {
            Dir::In => s.value,
            Dir::Out => Some(s.command),
        }
    }

    /// An output's command; `None` for inputs and unknown indices.
    #[must_use]
    pub fn command(&self, i: usize) -> Option<Value> {
        self.sigs.get(i).filter(|s| s.dir == Dir::Out).map(|s| s.command)
    }

    /// Records a read of an input signal; `None` marks it missing. A value
    /// of the wrong type, or a non-finite real, counts as missing too.
    pub fn set_input(&mut self, i: usize, v: Option<Value>) {
        if let Some(s) = self.sigs.get_mut(i).filter(|s| s.dir == Dir::In) {
            s.value = v.filter(|v| v.ty() == s.ty && !matches!(v, Value::Real(x) if !x.is_finite()));
        }
    }

    fn command_mut(&mut self, i: usize) -> Option<&mut Sig> {
        self.sigs.get_mut(i).filter(|s| s.dir == Dir::Out)
    }

    pub(crate) fn drive(&mut self, i: usize, v: Value) {
        if let Some(s) = self.command_mut(i) {
            s.command = v;
        }
    }

    pub(crate) fn make_safe(&mut self, i: usize) {
        if let Some(s) = self.command_mut(i) {
            s.command = s.safe;
        }
    }
}

/// A module port bound to a signal.
#[derive(Debug, Clone)]
pub(crate) struct PortBinding {
    pub port: String,
    pub dir: PortDir,
    pub ty: StTy,
    pub signal: usize,
}

/// Why a module's ports do not fit the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindError {
    UnknownSignal(String),
    /// A `VAR_OUTPUT` bound to an input signal.
    DrivesInput(String),
    /// The port's ST type cannot carry the signal's type.
    Type {
        port: String,
        signal: Ty,
        st: StTy,
    },
}

impl fmt::Display for BindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownSignal(p) => write!(f, "port {p}: no signal of that name"),
            Self::DrivesInput(p) => write!(f, "port {p}: a VAR_OUTPUT cannot drive an input signal"),
            Self::Type { port, signal, st } => {
                write!(f, "port {port}: {st} cannot carry a {signal:?} signal")
            }
        }
    }
}

/// Whether a port of ST type `st` facing `dir` can carry signal type `ty`.
/// Integers cross as `i64`, so outputs wider than that are refused; inputs
/// are range-checked on every scan instead.
fn carries(st: StTy, dir: PortDir, ty: Ty) -> bool {
    match ty {
        Ty::Bool => st == StTy::Bool,
        Ty::Real => st.is_real(),
        Ty::Int => {
            st.is_integral()
                && st != StTy::Bool
                && (dir == PortDir::Input || !matches!(st, StTy::ULInt | StTy::LWord))
        }
    }
}

pub(crate) fn bind(ports: &[Port], table: &SignalTable) -> (Vec<PortBinding>, Vec<BindError>) {
    let mut out = Vec::new();
    let mut errs = Vec::new();
    for p in ports {
        let Some(i) = table.index(&p.name) else {
            errs.push(BindError::UnknownSignal(p.name.clone()));
            continue;
        };
        let s = &table.sigs[i];
        if p.dir == PortDir::Output && s.dir == Dir::In {
            errs.push(BindError::DrivesInput(p.name.clone()));
            continue;
        }
        if !carries(p.ty, p.dir, s.ty) {
            errs.push(BindError::Type {
                port: p.name.clone(),
                signal: s.ty,
                st: p.ty,
            });
            continue;
        }
        out.push(PortBinding {
            port: p.name.clone(),
            dir: p.dir,
            ty: p.ty,
            signal: i,
        });
    }
    (out, errs)
}

pub(crate) fn to_st(v: Value) -> StValue {
    match v {
        Value::Bool(b) => StValue::Bool(b),
        Value::Int(n) => StValue::Int(i128::from(n)),
        Value::Real(x) => StValue::Real(x),
    }
}

/// An ST output value as a signal value; `None` if it cannot be carried
/// (an integer past `i64`, a non-finite real).
pub(crate) fn from_st(v: StValue, ty: Ty) -> Option<Value> {
    match (v, ty) {
        (StValue::Bool(b), Ty::Bool) => Some(Value::Bool(b)),
        (StValue::Int(n), Ty::Int) => i64::try_from(n).ok().map(Value::Int),
        (StValue::Real(x), Ty::Real) if x.is_finite() => Some(Value::Real(x)),
        _ => None,
    }
}

/// Signals bound to devices, grouped by device id (`None` for the box's
/// own channels), with their addresses.
pub(crate) fn by_device(map: &IoMap) -> BTreeMap<Option<String>, Vec<(usize, String, Dir)>> {
    let mut groups: BTreeMap<Option<String>, Vec<(usize, String, Dir)>> = BTreeMap::new();
    for (i, s) in map.signals.iter().enumerate() {
        let (dev, addr) = match &s.binding {
            Binding::Device { device, point } => (Some(device.clone()), point.clone()),
            Binding::Local { channel } => (None, channel.clone()),
        };
        groups.entry(dev).or_default().push((i, addr, s.dir));
    }
    groups
}
