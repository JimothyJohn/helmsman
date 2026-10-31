//! Devices, signals and what binds them. Logic names signals; only this map
//! knows addresses, so moving a light from a box output to a remote I/O block
//! is a one-line change here and none in the program.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::time::Duration;

use crate::driver::{PointInfo, output_keys, parse_point_on};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Ty {
    Bool,
    Int,
    Real,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Real(f64),
}

impl Value {
    #[must_use]
    pub fn ty(self) -> Ty {
        match self {
            Self::Bool(_) => Ty::Bool,
            Self::Int(_) => Ty::Int,
            Self::Real(_) => Ty::Real,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dir {
    /// The box reads it.
    In,
    /// The box drives it.
    Out,
}

/// An endpoint the box is master of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// Unique within the map; what bindings refer to.
    pub id: String,
    /// Driver name, e.g. `modbus-tcp`.
    pub driver: String,
    /// `host:port` for network drivers.
    pub endpoint: String,
    /// Driver-specific settings (Modbus: `unit`, `timeout_ms`).
    pub params: BTreeMap<String, String>,
}

/// Where a signal's value comes from (or, for an output, goes).
///
/// Extension point: a new kind of *device* (a CIP controller read by
/// symbolic tag or by class/instance/attribute, say) is a new driver
/// behind [`Binding::Device`], with its own point syntax, not a new
/// variant: the link machinery, polling and validation then apply as they
/// are. A new variant is for a source that is not polled through a
/// driver, as [`Binding::Module`] is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Binding {
    /// A point on a device, in the device's driver's address syntax.
    Device { device: String, point: String },
    /// One of the box's own channels: `do:N` (digital out, e.g. a GPIO
    /// through a 24 V output stage) or `di:N` (digital in).
    Local { channel: String },
    /// Another module's output: the `VAR_OUTPUT` of the same name in the
    /// module called `module`. An input signal; any other module may read
    /// it, and nothing is polled.
    Module { module: String },
    /// A tile on the box's floor screen (the andon view the box serves):
    /// an output no device takes. A module drives it like any output (one
    /// writer, safe value, approval tier); the screen layout says how the
    /// tile shows it. Several signals may feed one tile (its state, a
    /// count).
    Screen { tile: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signal {
    /// What logic calls it: an identifier, unique within the map.
    pub name: String,
    pub ty: Ty,
    pub dir: Dir,
    pub binding: Binding,
    /// Engineering unit, for people and the agent (`degC`, `mm/s`, `%`).
    /// The runtime does not convert.
    pub unit: Option<String>,
    /// How often an input is read; `None` is its device's poll period.
    pub poll: Option<Duration>,
}

/// Longest [`Signal::unit`], in bytes.
pub const MAX_UNIT: usize = 16;
/// Slowest per-signal poll period.
pub const MAX_POLL: Duration = Duration::from_secs(60);

/// A unit: 1..=[`MAX_UNIT`] printable ASCII characters, no `#`.
#[must_use]
pub fn unit_ok(u: &str) -> bool {
    (1..=MAX_UNIT).contains(&u.len()) && u.bytes().all(|b| b.is_ascii_graphic() && b != b'#')
}

/// A screen tile id: 1..=32 of `A-Z a-z 0-9 _`, not starting with a digit.
#[must_use]
pub fn tile_ok(t: &str) -> bool {
    t.len() <= 32 && is_ident(t)
}

/// A module name as the runtime takes it: 1..=64 of `A-Z a-z 0-9 _ -`.
#[must_use]
pub fn module_name_ok(m: &str) -> bool {
    (1..=64).contains(&m.len())
        && m.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IoMap {
    pub devices: Vec<Device>,
    pub signals: Vec<Signal>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MapError {
    DuplicateDevice(String),
    BadName(String),
    DuplicateSignal(String),
    UnknownDevice {
        signal: String,
        device: String,
    },
    BadPoint {
        signal: String,
        why: String,
    },
    TypeMismatch {
        signal: String,
        signal_ty: Ty,
        point_ty: Ty,
    },
    /// An output bound to something the box cannot write.
    NotWritable(String),
    /// Two outputs driving the same point or channel.
    TwoWriters {
        a: String,
        b: String,
    },
    /// A unit that is empty, too long, or not printable ASCII.
    BadUnit(String),
    /// A poll period on an output or a module's output, or outside
    /// 1 ms ..= [`MAX_POLL`].
    BadPoll(String),
}

impl fmt::Display for MapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateDevice(d) => write!(f, "device {d:?} defined twice"),
            Self::BadName(n) => write!(f, "{n:?} is not an identifier"),
            Self::DuplicateSignal(n) => write!(f, "signal {n:?} defined twice"),
            Self::UnknownDevice { signal, device } => write!(f, "{signal}: no device {device:?}"),
            Self::BadPoint { signal, why } => write!(f, "{signal}: {why}"),
            Self::TypeMismatch {
                signal,
                signal_ty,
                point_ty,
            } => {
                write!(f, "{signal}: signal is {signal_ty:?}, point is {point_ty:?}")
            }
            Self::NotWritable(s) => write!(f, "{s}: output bound to a read-only point"),
            Self::TwoWriters { a, b } => write!(f, "{a} and {b} drive the same point"),
            Self::BadUnit(s) => write!(f, "{s}: unit must be 1..={MAX_UNIT} printable ASCII characters"),
            Self::BadPoll(s) => write!(
                f,
                "{s}: a poll period is for device inputs only, 1 ms ..= {} s",
                MAX_POLL.as_secs()
            ),
        }
    }
}

impl std::error::Error for MapError {}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Parses a box channel: `do:N` is a writable output, `di:N` an input,
/// `i2c:BUS:ADDR:REG[:FMT]` an integer input (see [`crate::board`]).
pub(crate) fn local_point(channel: &str) -> Result<PointInfo, String> {
    crate::board::Channel::parse(channel)
        .map(crate::board::Channel::info)
        .map_err(|_| format!("bad channel {channel:?} (want do:N, di:N or i2c:BUS:ADDR:REG[:FMT])"))
}

/// What is wrong with a signal no driver serves (another module's output,
/// a screen tile), if anything.
fn undriven(s: &Signal) -> Option<String> {
    match &s.binding {
        Binding::Module { module } if !module_name_ok(module) => {
            Some(format!("{module:?} is not a module name"))
        }
        Binding::Module { .. } if s.dir == Dir::Out => {
            Some("a module's output is an in signal: modules drive it, the map does not".into())
        }
        Binding::Screen { tile } if !tile_ok(tile) => {
            Some(format!("{tile:?} is not a tile id (1..=32 of A-Z a-z 0-9 _)"))
        }
        Binding::Screen { .. } if s.dir == Dir::In => {
            Some("a screen tile is something the box shows: an out signal".into())
        }
        _ => None,
    }
}

impl IoMap {
    /// Checks everything that can be checked without touching a device.
    /// Returns every problem, not just the first.
    ///
    /// # Errors
    /// The list of problems.
    pub fn validate(&self) -> Result<(), Vec<MapError>> {
        self.validate_with(&mut parse_point_on)
    }

    /// [`IoMap::validate`], parsing each device point with `parse` rather
    /// than the built-in drivers alone: a box passes one that also knows
    /// the WASM drivers it has approved (`wasm:NAME`, [`crate::driver::WASM`]),
    /// asking the guest, and falls back to [`parse_point_on`] for the rest.
    ///
    /// # Errors
    /// The list of problems.
    pub fn validate_with(
        &self,
        parse: &mut dyn FnMut(&Device, &str) -> Result<PointInfo, crate::Error>,
    ) -> Result<(), Vec<MapError>> {
        let mut errs = Vec::new();
        let mut devices = BTreeMap::new();
        for d in &self.devices {
            if devices.insert(d.id.as_str(), d).is_some() {
                errs.push(MapError::DuplicateDevice(d.id.clone()));
            }
        }
        let mut names = BTreeSet::new();
        // Written target (device, point) or ("", channel) -> signal name.
        let mut writers: BTreeMap<(String, String), &str> = BTreeMap::new();
        for s in &self.signals {
            if !is_ident(&s.name) {
                errs.push(MapError::BadName(s.name.clone()));
            }
            if !names.insert(s.name.as_str()) {
                errs.push(MapError::DuplicateSignal(s.name.clone()));
            }
            if s.unit.as_deref().is_some_and(|u| !unit_ok(u)) {
                errs.push(MapError::BadUnit(s.name.clone()));
            }
            let polled =
                s.dir == Dir::In && !matches!(s.binding, Binding::Module { .. } | Binding::Screen { .. });
            if let Some(p) = s.poll
                && (!polled || p < Duration::from_millis(1) || p > MAX_POLL)
            {
                errs.push(MapError::BadPoll(s.name.clone()));
            }
            let (info, target) = match &s.binding {
                Binding::Device { device, point } => {
                    let Some(d) = devices.get(device.as_str()) else {
                        errs.push(MapError::UnknownDevice {
                            signal: s.name.clone(),
                            device: device.clone(),
                        });
                        continue;
                    };
                    match parse(d, point) {
                        Ok(i) => (i, (device.clone(), point.clone(), d.driver.clone())),
                        Err(e) => {
                            errs.push(MapError::BadPoint {
                                signal: s.name.clone(),
                                why: e.to_string(),
                            });
                            continue;
                        }
                    }
                }
                Binding::Local { channel } => match local_point(channel) {
                    Ok(i) => (i, (String::new(), channel.clone(), String::new())),
                    Err(why) => {
                        errs.push(MapError::BadPoint {
                            signal: s.name.clone(),
                            why,
                        });
                        continue;
                    }
                },
                Binding::Module { .. } | Binding::Screen { .. } => {
                    if let Some(why) = undriven(s) {
                        errs.push(MapError::BadPoint {
                            signal: s.name.clone(),
                            why,
                        });
                    }
                    continue;
                }
            };
            if info.ty != s.ty {
                errs.push(MapError::TypeMismatch {
                    signal: s.name.clone(),
                    signal_ty: s.ty,
                    point_ty: info.ty,
                });
            }
            if s.dir == Dir::Out {
                if !info.writable {
                    errs.push(MapError::NotWritable(s.name.clone()));
                }
                let (device, point, driver) = target;
                let mut clash = None;
                for key in output_keys(&driver, &point) {
                    match writers.entry((device.clone(), key)) {
                        std::collections::btree_map::Entry::Occupied(o) => clash = clash.or(Some(*o.get())),
                        std::collections::btree_map::Entry::Vacant(v) => {
                            v.insert(&s.name);
                        }
                    }
                }
                if let Some(other) = clash {
                    errs.push(MapError::TwoWriters {
                        a: other.into(),
                        b: s.name.clone(),
                    });
                }
            }
        }
        if errs.is_empty() { Ok(()) } else { Err(errs) }
    }
}
