//! The I/O map as a text file, one declaration per line:
//!
//! ```text
//! # remote I/O block on the bench
//! device rio modbus-tcp 192.168.3.50:502 unit=1 timeout_ms=200 poll_ms=50
//! in  start  bool rio di:1
//! in  level  int  rio hr:5:i16
//! out motor  bool rio coil:0
//! out lamp   bool box do:0  safe=true
//! ```
//!
//! `device ID DRIVER ENDPOINT [KEY=VALUE ...]` declares a device the box
//! masters; the params are the driver's. `in`/`out NAME TYPE DEVICE POINT`
//! declares a signal (`bool`, `int` or `real`) bound to a point in that
//! device's address syntax; the device `box` is the box's own channels
//! (`do:N`, `di:N`). An `out` may end in `safe=VALUE`, what the output is
//! driven to whenever no healthy module drives it (default off / zero).
//! `#` starts a comment. Tokens are separated by spaces or tabs; nothing is
//! quoted, so no token contains either.
//!
//! [`parse`] checks only the syntax; [`crate::IoMap::validate`] checks the
//! map.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::model::{Binding, Device, Dir, IoMap, Signal, Ty, Value};

/// The device name for the box's own channels.
pub const BOX: &str = "box";

/// A parsed map file: the map and the declared safe values.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapFile {
    pub map: IoMap,
    pub safe: BTreeMap<String, Value>,
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn ty(s: &str) -> Option<Ty> {
    match s {
        "bool" => Some(Ty::Bool),
        "int" => Some(Ty::Int),
        "real" => Some(Ty::Real),
        _ => None,
    }
}

fn value(ty: Ty, s: &str) -> Option<Value> {
    match ty {
        Ty::Bool => match s {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            _ => None,
        },
        Ty::Int => s.parse().ok().map(Value::Int),
        Ty::Real => s.parse::<f64>().ok().filter(|x| x.is_finite()).map(Value::Real),
    }
}

/// Only printable ASCII: a token ends up in logs and error messages.
fn printable(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_graphic())
}

/// Parses a map file.
///
/// # Errors
/// Every bad line, as `(line number from 1, why)`.
pub fn parse(src: &str) -> Result<MapFile, Vec<(usize, String)>> {
    let mut out = MapFile::default();
    let mut errs = Vec::new();
    for (n, raw) in src.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or_default();
        let words: Vec<&str> = line.split([' ', '\t']).filter(|w| !w.is_empty()).collect();
        if words.is_empty() {
            continue;
        }
        if let Err(e) = declaration(&words, &mut out) {
            errs.push((n + 1, e));
        }
    }
    if errs.is_empty() { Ok(out) } else { Err(errs) }
}

fn declaration(words: &[&str], out: &mut MapFile) -> Result<(), String> {
    if let Some(w) = words.iter().find(|w| !printable(w)) {
        return Err(format!("{w:?}: only printable ASCII"));
    }
    match words {
        ["device", id, driver, endpoint, params @ ..] => {
            if !is_ident(id) || *id == BOX {
                return Err(format!("device id {id:?}: an identifier other than {BOX}"));
            }
            let mut map = BTreeMap::new();
            for p in params {
                let (k, v) = p
                    .split_once('=')
                    .filter(|(k, _)| !k.is_empty())
                    .ok_or_else(|| format!("{p:?}: expected KEY=VALUE"))?;
                if map.insert(k.to_string(), v.to_string()).is_some() {
                    return Err(format!("{k} given twice"));
                }
            }
            out.map.devices.push(Device {
                id: (*id).into(),
                driver: (*driver).into(),
                endpoint: (*endpoint).into(),
                params: map,
            });
            Ok(())
        }
        [dir @ ("in" | "out"), name, t, device, point, rest @ ..] => {
            let dir = if *dir == "in" { Dir::In } else { Dir::Out };
            let ty = ty(t).ok_or_else(|| format!("type {t:?}: bool, int or real"))?;
            match (dir, rest) {
                (_, []) => {}
                (Dir::Out, [safe]) => {
                    let v = safe
                        .strip_prefix("safe=")
                        .and_then(|v| value(ty, v))
                        .ok_or_else(|| format!("{safe:?}: expected safe=VALUE of type {t}"))?;
                    out.safe.insert((*name).into(), v);
                }
                (Dir::In, [_]) => return Err("only an out signal has a safe value".into()),
                _ => return Err("too many words".into()),
            }
            let binding = if *device == BOX {
                Binding::Local {
                    channel: (*point).into(),
                }
            } else {
                Binding::Device {
                    device: (*device).into(),
                    point: (*point).into(),
                }
            };
            out.map.signals.push(Signal {
                name: (*name).into(),
                ty,
                dir,
                binding,
            });
            Ok(())
        }
        [w, ..] if matches!(*w, "device" | "in" | "out") => Err(format!("{w}: too few words")),
        [w, ..] => Err(format!("{w:?}: expected device, in or out")),
        [] => Ok(()),
    }
}

/// Writes a map file that [`parse`] reads back as `file`.
#[must_use]
pub fn render(file: &MapFile) -> String {
    let mut s = String::new();
    for d in &file.map.devices {
        let _ = write!(s, "device {} {} {}", d.id, d.driver, d.endpoint);
        for (k, v) in &d.params {
            let _ = write!(s, " {k}={v}");
        }
        s.push('\n');
    }
    for sig in &file.map.signals {
        let dir = if sig.dir == Dir::In { "in" } else { "out" };
        let ty = match sig.ty {
            Ty::Bool => "bool",
            Ty::Int => "int",
            Ty::Real => "real",
        };
        let (dev, point) = match &sig.binding {
            Binding::Device { device, point } => (device.as_str(), point.as_str()),
            Binding::Local { channel } => (BOX, channel.as_str()),
        };
        let _ = write!(s, "{dir} {} {ty} {dev} {point}", sig.name);
        match file.safe.get(&sig.name) {
            Some(Value::Bool(b)) => {
                let _ = write!(s, " safe={b}");
            }
            Some(Value::Int(n)) => {
                let _ = write!(s, " safe={n}");
            }
            Some(Value::Real(x)) => {
                let _ = write!(s, " safe={x:?}");
            }
            None => {}
        }
        s.push('\n');
    }
    s
}
