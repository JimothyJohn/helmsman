//! A program's `VAR_INPUT` and `VAR_OUTPUT` bound to the I/O map's points
//! by name, as a box does (docs/devices/e1213-loop/main.st), so a program
//! written on a box runs on a Skipper unchanged. The matching is
//! `helmsman_nodecore::ioplan::bindings` and the image write is `put_at`,
//! both host-tested there; this is the glue to the interpreter's `Value`s.
//!
//! Each scan, before the program runs, an input port is set from the input
//! image; after it, an output port is written into the output image at its
//! point's place. Located variables (`AT %QD18`) work as before. A point
//! written by both a located variable and a name-bound output takes the
//! name-bound value: the binding is applied after the scan, so it wins.
//! A port with no point of its name is left alone.
//!
//! The map's `share` lines bind the same way, by name, but to the share
//! bank (`Live::bank`) that the Modbus TCP server serves instead of the
//! process image: a `VAR_OUTPUT` of a shared name is published to the bank
//! after each scan, a `VAR_INPUT` of one takes the bank's value before it
//! (what another controller wrote, 0 or false until then). Another
//! controller may write a share only if it is a `VAR_INPUT` in a coil or
//! holding table; [`share_masks`] says which, for the server.

use alloc::string::String;
use alloc::vec::Vec;
use helmsman_iec::{PortDir, Program, Value};
use helmsman_nodecore::api::SignalValue;
use helmsman_nodecore::iomap::{Fmt, IoMap, Share};
use helmsman_nodecore::ioplan::{bindings, put_at, put_slot, share_bindings, value_at, value_slot};

/// One bound port: its name, its direction and the point's index in the map.
pub struct Bind {
    name: String,
    input: bool,
    real: bool,
    point: usize,
}

/// The bindings of a program against the map; compute once per load.
#[must_use]
pub fn binds_of(program: &Program, map: &IoMap) -> Vec<Bind> {
    let ports = program.ports();
    let named: Vec<(&str, bool)> = ports
        .iter()
        .map(|p| (p.name.as_str(), p.dir == PortDir::Input))
        .collect();
    bindings(map, &named)
        .into_iter()
        .map(|(port, point)| Bind {
            name: ports[port].name.clone(),
            input: named[port].1,
            real: ports[port].ty.is_real(),
            point,
        })
        .collect()
}

/// Sets every bound `VAR_INPUT` from the input image. A value the
/// program refuses (out of range) leaves the variable as it was.
pub fn apply_inputs(binds: &[Bind], program: &mut Program, map: &IoMap, image: &[u8]) {
    for b in binds.iter().filter(|b| b.input) {
        let Some(point) = map.points.get(b.point) else {
            continue;
        };
        let Some(value) = value_at(image, point).map(|v| to_value(v, b.real)) else {
            continue;
        };
        let _ = program.set_input(&b.name, value);
    }
}

/// An image value as the interpreter's, for a port that is `real` or not.
fn to_value(v: SignalValue, real: bool) -> Value {
    match v {
        SignalValue::Bool(v) => Value::Bool(v),
        #[allow(clippy::cast_precision_loss)]
        SignalValue::Int(v) if real => Value::Real(v as f64),
        SignalValue::Int(v) => Value::Int(i128::from(v)),
        SignalValue::Real(v) => Value::Real(v),
    }
}

/// The interpreter's value as an image value for a place of format `fmt`.
fn from_value(v: Value, fmt: Option<Fmt>) -> SignalValue {
    match v {
        Value::Bool(v) => SignalValue::Bool(v),
        Value::Int(v) => {
            let v = i64::try_from(v).unwrap_or(if v < 0 { i64::MIN } else { i64::MAX });
            if fmt == Some(Fmt::F32) {
                #[allow(clippy::cast_precision_loss)]
                SignalValue::Real(v as f64)
            } else {
                SignalValue::Int(v)
            }
        }
        Value::Real(v) => SignalValue::Real(v),
    }
}

/// Writes every bound `VAR_OUTPUT` into the output image.
pub fn apply_outputs(binds: &[Bind], program: &Program, map: &IoMap, image: &mut [u8]) {
    for b in binds.iter().filter(|b| !b.input) {
        let Some(point) = map.points.get(b.point) else {
            continue;
        };
        let Some(value) = program.get(&b.name).map(|v| from_value(v, point.fmt)) else {
            continue;
        };
        put_at(image, point, value);
    }
}

/// One program port bound to a share of the map.
pub struct ShareBind {
    name: String,
    input: bool,
    real: bool,
    share: usize,
}

/// The shares' bindings against a program, by name; compute once per load.
#[must_use]
pub fn share_binds_of(program: &Program, map: &IoMap) -> Vec<ShareBind> {
    let ports = program.ports();
    let named: Vec<(&str, bool)> = ports
        .iter()
        .map(|p| (p.name.as_str(), p.dir == PortDir::Input))
        .collect();
    share_bindings(&map.shares, &named)
        .into_iter()
        .map(|(port, share)| ShareBind {
            name: ports[port].name.clone(),
            input: named[port].1,
            real: ports[port].ty.is_real(),
            share,
        })
        .collect()
}

/// `(writable, bound)`: bit `k` of the first is set when share `k` is a
/// `VAR_INPUT` in a coil or holding table, so another controller may write
/// it; of the second when the program has a variable of its name.
#[must_use]
pub fn share_masks(binds: &[ShareBind], shares: &[Share]) -> (u32, u32) {
    let (mut writable, mut bound) = (0u32, 0u32);
    for b in binds.iter().filter(|b| b.share < 32) {
        bound |= 1 << b.share;
        if b.input && shares.get(b.share).is_some_and(Share::writable_table) {
            writable |= 1 << b.share;
        }
    }
    (writable, bound)
}

/// Sets every bound shared `VAR_INPUT` from the bank.
pub fn apply_share_inputs(binds: &[ShareBind], program: &mut Program, shares: &[Share], bank: &[u8]) {
    for b in binds.iter().filter(|b| b.input) {
        let Some(s) = shares.get(b.share) else { continue };
        let Some(value) = value_slot(bank, s.fmt, s.at).map(|v| to_value(v, b.real)) else {
            continue;
        };
        let _ = program.set_input(&b.name, value);
    }
}

/// Writes every bound shared `VAR_OUTPUT` into `bank` (a scratch copy;
/// [`publish_share_outputs`] moves them into the live one).
pub fn apply_share_outputs(binds: &[ShareBind], program: &Program, shares: &[Share], bank: &mut [u8]) {
    for b in binds.iter().filter(|b| !b.input) {
        let Some(s) = shares.get(b.share) else { continue };
        let Some(value) = program.get(&b.name).map(|v| from_value(v, s.fmt)) else {
            continue;
        };
        put_slot(bank, s.fmt, s.at, value);
    }
}

/// Copies the output shares' slots from `from` into `to` and nothing else,
/// so what another controller wrote to an input share meanwhile stays.
pub fn publish_share_outputs(binds: &[ShareBind], shares: &[Share], from: &[u8], to: &mut [u8]) {
    for b in binds.iter().filter(|b| !b.input) {
        let Some(s) = shares.get(b.share) else { continue };
        if let Some(v) = value_slot(from, s.fmt, s.at) {
            put_slot(to, s.fmt, s.at, v);
        }
    }
}

/// Zeroes the output shares, as a stopped or faulted program's outputs
/// are: a controller reading them must not see the last value of a program
/// that is not running.
pub fn clear_share_outputs(binds: &[ShareBind], shares: &[Share], bank: &mut [u8]) {
    for b in binds.iter().filter(|b| !b.input) {
        let Some(s) = shares.get(b.share) else { continue };
        let zero = match s.fmt {
            None => SignalValue::Bool(false),
            Some(Fmt::F32) => SignalValue::Real(0.0),
            Some(_) => SignalValue::Int(0),
        };
        put_slot(bank, s.fmt, s.at, zero);
    }
}
