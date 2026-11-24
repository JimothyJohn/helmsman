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

use alloc::string::String;
use alloc::vec::Vec;
use helmsman_iec::{PortDir, Program, Value};
use helmsman_nodecore::api::SignalValue;
use helmsman_nodecore::iomap::{Fmt, IoMap};
use helmsman_nodecore::ioplan::{bindings, put_at, value_at};

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
        let value = match value_at(image, point) {
            Some(SignalValue::Bool(v)) => Value::Bool(v),
            #[allow(clippy::cast_precision_loss)]
            Some(SignalValue::Int(v)) if b.real => Value::Real(v as f64),
            Some(SignalValue::Int(v)) => Value::Int(i128::from(v)),
            Some(SignalValue::Real(v)) => Value::Real(v),
            None => continue,
        };
        let _ = program.set_input(&b.name, value);
    }
}

/// Writes every bound `VAR_OUTPUT` into the output image.
pub fn apply_outputs(binds: &[Bind], program: &Program, map: &IoMap, image: &mut [u8]) {
    for b in binds.iter().filter(|b| !b.input) {
        let Some(point) = map.points.get(b.point) else {
            continue;
        };
        let value = match program.get(&b.name) {
            Some(Value::Bool(v)) => SignalValue::Bool(v),
            Some(Value::Int(v)) => {
                let v = i64::try_from(v).unwrap_or(if v < 0 { i64::MIN } else { i64::MAX });
                if point.fmt == Some(Fmt::F32) {
                    #[allow(clippy::cast_precision_loss)]
                    SignalValue::Real(v as f64)
                } else {
                    SignalValue::Int(v)
                }
            }
            Some(Value::Real(v)) => SignalValue::Real(v),
            None => continue,
        };
        put_at(image, point, value);
    }
}
