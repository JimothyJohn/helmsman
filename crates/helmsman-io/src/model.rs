//! Devices, signals and what binds them. Logic names signals; only this map
//! knows addresses, so moving a light from a box output to a remote I/O block
//! is a one-line change here and none in the program.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::driver::{PointInfo, parse_point};

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Binding {
    /// A point on a device, in the device's driver's address syntax.
    Device { device: String, point: String },
    /// One of the box's own channels: `do:N` (digital out, e.g. a GPIO
    /// through a 24 V output stage) or `di:N` (digital in).
    Local { channel: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signal {
    /// What logic calls it: an identifier, unique within the map.
    pub name: String,
    pub ty: Ty,
    pub dir: Dir,
    pub binding: Binding,
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
        }
    }
}

impl std::error::Error for MapError {}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Parses a box channel: `do:N` is a writable output, `di:N` an input.
pub(crate) fn local_point(channel: &str) -> Result<PointInfo, String> {
    let (kind, n) = channel
        .split_once(':')
        .ok_or_else(|| format!("bad channel {channel:?}"))?;
    n.parse::<u16>()
        .map_err(|_| format!("bad channel number in {channel:?}"))?;
    match kind {
        "do" => Ok(PointInfo {
            ty: Ty::Bool,
            writable: true,
        }),
        "di" => Ok(PointInfo {
            ty: Ty::Bool,
            writable: false,
        }),
        _ => Err(format!("bad channel kind in {channel:?} (want do:N or di:N)")),
    }
}

impl IoMap {
    /// Checks everything that can be checked without touching a device.
    /// Returns every problem, not just the first.
    ///
    /// # Errors
    /// The list of problems.
    pub fn validate(&self) -> Result<(), Vec<MapError>> {
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
            let (info, target) = match &s.binding {
                Binding::Device { device, point } => {
                    let Some(d) = devices.get(device.as_str()) else {
                        errs.push(MapError::UnknownDevice {
                            signal: s.name.clone(),
                            device: device.clone(),
                        });
                        continue;
                    };
                    match parse_point(&d.driver, point) {
                        Ok(i) => (i, (device.clone(), point.clone())),
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
                    Ok(i) => (i, (String::new(), channel.clone())),
                    Err(why) => {
                        errs.push(MapError::BadPoint {
                            signal: s.name.clone(),
                            why,
                        });
                        continue;
                    }
                },
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
                if let Some(other) = writers.insert(target, &s.name) {
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
