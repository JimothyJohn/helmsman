//! IO-Link sensors and actuators in a node's I/O map: the `iolink`
//! device line, its points, and a port that runs them.
//!
//! ```text
//! device prox iolink port=1 vendor=310 device=666 in_bits=32 out_bits=8
//! in  prox_distance int  prox pd:16:i16
//! in  prox_quality  int  prox pd:8:u8
//! in  prox_near     bool prox pd:0
//! out prox_teach    bool prox pd:0
//! ```
//!
//! `device NAME iolink port=N` puts a Device on the node's Master port N
//! (1 up to the ports the board has). `vendor=` and `device=` (both or
//! neither) are the Device the port is for: another one is refused.
//! `in_bits=` and `out_bits=` are the lengths of its Process Data as its
//! IODD states them; a Device with other lengths is refused, because a
//! point's bit offset counts from the end of the Process Data (IOL Table
//! F.19 rule 3) and would land on other bits. `cycle_ms=` (1 to 132) asks
//! for a slower cycle than the Device's fastest.
//!
//! `in|out NAME bool DEVICE pd:OFFSET` is one bit; `in|out NAME int
//! DEVICE pd:OFFSET:uN|iN` is an unsigned or signed number of N bits
//! (2 to 63, or 64 signed). `helmsman_iolink::iodd` writes these lines
//! from a Device's IODD, so nobody counts bits by hand.
//!
//! [`parse`] reads only the `iolink` devices of a map and their points;
//! the Modbus block is [`crate::iomap`]'s. [`Port`] is one such device on
//! `helmsman_iolink::master::Master`: the firmware's line driver polls
//! it, the scan reads its inputs by point and hands it the outputs.
//!
//! No node has a Master port yet (TODO.md `iolink-transceiver`), so no
//! firmware calls this and [`crate::iomap::parse`] still refuses an
//! `iolink` line: a map a node cannot run does not reach flash. What is
//! here is proven against the simulated Device only. Not here: `real`
//! points, a place for `int` points in the program's image, strings.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use helmsman_iolink::master::{Config, Fault, Identity, Master, State, Step};
use helmsman_iolink::pd;

use crate::iomap::{Bad, MAX_TEXT, bad, is_ident};

/// The driver name of an IO-Link `device` line.
pub const DRIVER: &str = "iolink";
/// Most points one Device has in a map.
pub const MAX_POINTS: usize = 32;
/// The longest Process Data, in bits (IOL Table B.6: 32 octets).
pub const MAX_BITS: u16 = 256;

/// A point's type in the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty {
    Bool,
    Int,
}

/// A point's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value {
    Bool(bool),
    Int(i64),
}

/// One value inside a Device's Process Data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Point {
    pub name: String,
    pub ty: Ty,
    /// From the least significant bit of the last octet.
    pub bit_offset: u16,
    /// 1 for a `bool`, 2 to 64 for an `int`.
    pub bit_length: u8,
    /// An `iN` point: the top bit is the sign.
    pub signed: bool,
}

/// One `iolink` device of a map and its points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortMap {
    pub device: String,
    /// The Master port, from 1.
    pub port: u8,
    pub expect: Option<Identity>,
    /// 0 unless `cycle_ms=` asks for a cycle.
    pub cycle_us: u32,
    /// The IODD's `bitLength` of the input Process Data, when stated.
    pub in_bits: Option<u16>,
    pub out_bits: Option<u16>,
    pub inputs: Vec<Point>,
    pub outputs: Vec<Point>,
}

fn number<T: core::str::FromStr + PartialOrd + Copy + core::fmt::Display>(
    line: usize,
    key: &str,
    value: &str,
    min: T,
    max: T,
) -> Result<T, Bad> {
    value
        .parse::<T>()
        .ok()
        .filter(|v| *v >= min && *v <= max)
        .ok_or_else(|| bad(line, alloc::format!("{key}={value}: {min} to {max}")))
}

fn device_line(t: &[&str], n: usize, ports: u8) -> Result<PortMap, Bad> {
    let [_, name, _, rest @ ..] = t else {
        return Err(bad(
            n,
            "device: want device NAME iolink port=N [vendor=V device=D]",
        ));
    };
    if !is_ident(name) {
        return Err(bad(
            n,
            alloc::format!("device name {name:?}: letters, digits and '_'"),
        ));
    }
    if ports == 0 {
        return Err(bad(n, "this node has no IO-Link port"));
    }
    let (mut port, mut vendor, mut device) = (None, None, None);
    let mut map = PortMap {
        device: (*name).to_string(),
        port: 0,
        expect: None,
        cycle_us: 0,
        in_bits: None,
        out_bits: None,
        inputs: Vec::new(),
        outputs: Vec::new(),
    };
    for opt in rest {
        let Some((key, value)) = opt.split_once('=') else {
            return Err(bad(n, alloc::format!("{opt:?}: expected KEY=VALUE")));
        };
        match key {
            "port" => port = Some(number(n, key, value, 1, ports)?),
            "vendor" => vendor = Some(number(n, key, value, 1u16, u16::MAX)?),
            "device" => device = Some(number(n, key, value, 1u32, 0xFF_FFFF)?),
            "cycle_ms" => map.cycle_us = number(n, key, value, 1u32, 132)? * 1000,
            "in_bits" => map.in_bits = Some(number(n, key, value, 1, MAX_BITS)?),
            "out_bits" => map.out_bits = Some(number(n, key, value, 1, MAX_BITS)?),
            _ => {
                return Err(bad(
                    n,
                    alloc::format!(
                        "{key}=: an iolink device takes port, vendor, device, in_bits, out_bits and cycle_ms"
                    ),
                ));
            }
        }
    }
    map.port = port.ok_or_else(|| bad(n, alloc::format!("{name}: which port? port=1 to port={ports}")))?;
    map.expect = match (vendor, device) {
        (Some(vendor), Some(device)) => Some(Identity { vendor, device }),
        (None, None) => None,
        _ => return Err(bad(n, alloc::format!("{name}: vendor= and device= go together"))),
    };
    Ok(map)
}

/// `pd:OFFSET` for a `bool`, `pd:OFFSET:uN` or `pd:OFFSET:iN` for an
/// `int`.
fn point(name: &str, ty: Ty, token: &str, n: usize) -> Result<Point, Bad> {
    let want = match ty {
        Ty::Bool => "pd:OFFSET",
        Ty::Int => "pd:OFFSET:uN or pd:OFFSET:iN",
    };
    let wrong = || {
        bad(
            n,
            alloc::format!(
                "{name}: point {token:?}; an IO-Link {} is {want}",
                if ty == Ty::Bool { "bool" } else { "int" }
            ),
        )
    };
    let mut parts = token.split(':');
    if parts.next() != Some("pd") {
        return Err(wrong());
    }
    let offset: u16 = parts.next().and_then(|o| o.parse().ok()).ok_or_else(wrong)?;
    let (bit_length, signed) = match (ty, parts.next()) {
        (Ty::Bool, None) => (1, false),
        (Ty::Int, Some(format)) => {
            let signed = format.starts_with('i');
            let bits: u8 = format
                .strip_prefix(['u', 'i'])
                .and_then(|b| b.parse().ok())
                .ok_or_else(wrong)?;
            if !(2..=if signed { 64 } else { 63 }).contains(&bits) {
                return Err(bad(n, alloc::format!("{name}: {format}; u2 to u63 or i2 to i64")));
            }
            (bits, signed)
        }
        _ => return Err(wrong()),
    };
    if parts.next().is_some() {
        return Err(wrong());
    }
    if offset >= MAX_BITS || offset + u16::from(bit_length) > MAX_BITS {
        return Err(bad(
            n,
            alloc::format!(
                "{name}: bits {offset} to {}; Process Data ends at bit {}",
                u32::from(offset) + u32::from(bit_length) - 1,
                MAX_BITS - 1
            ),
        ));
    }
    Ok(Point {
        name: name.to_string(),
        ty,
        bit_offset: offset,
        bit_length,
        signed,
    })
}

fn overlap(a: &Point, b: &Point) -> bool {
    a.bit_offset < b.bit_offset + u16::from(b.bit_length)
        && b.bit_offset < a.bit_offset + u16::from(a.bit_length)
}

/// Reads the `iolink` devices of a map and their points, for a node
/// with `ports` Master ports. Lines of other devices are left to
/// [`crate::iomap`].
///
/// # Errors
/// The first IO-Link line a node cannot take, and why.
pub fn parse(text: &str, ports: u8) -> Result<Vec<PortMap>, Bad> {
    if text.len() > MAX_TEXT {
        return Err(bad(
            0,
            alloc::format!("the map is {} bytes; a node keeps {MAX_TEXT}", text.len()),
        ));
    }
    let mut maps: Vec<PortMap> = Vec::new();
    for (k, raw) in text.lines().enumerate() {
        let n = k + 1;
        let line = raw.split('#').next().unwrap_or("").trim();
        let t: Vec<&str> = line.split_whitespace().collect();
        match t.as_slice() {
            ["device", _, driver, ..] if *driver == DRIVER => {
                let map = device_line(&t, n, ports)?;
                if maps.iter().any(|m| m.device.eq_ignore_ascii_case(&map.device)) {
                    return Err(bad(
                        n,
                        alloc::format!("{}: a second device of that name", map.device),
                    ));
                }
                if let Some(other) = maps.iter().find(|m| m.port == map.port) {
                    return Err(bad(
                        n,
                        alloc::format!("{}: port {} already has {}", map.device, map.port, other.device),
                    ));
                }
                maps.push(map);
            }
            [dir @ ("in" | "out"), name, ty, device, token, ..] => {
                // A point of the Modbus block, or of a device not seen:
                // not ours to judge.
                if !maps.iter().any(|m| m.device == *device) {
                    continue;
                }
                let ty = match *ty {
                    "bool" => Ty::Bool,
                    "int" => Ty::Int,
                    other => {
                        return Err(bad(
                            n,
                            alloc::format!("{name}: type {other:?}; an IO-Link point is bool or int"),
                        ));
                    }
                };
                if !is_ident(name) {
                    return Err(bad(
                        n,
                        alloc::format!("{name:?}: a name is letters, digits and '_'"),
                    ));
                }
                if maps
                    .iter()
                    .flat_map(|m| m.inputs.iter().chain(&m.outputs))
                    .any(|p| p.name.eq_ignore_ascii_case(name))
                {
                    return Err(bad(n, alloc::format!("{name}: named twice")));
                }
                let point = point(name, ty, token, n)?;
                let Some(map) = maps.iter_mut().find(|m| m.device == *device) else {
                    continue;
                };
                let (list, bits) = if *dir == "in" {
                    (&mut map.inputs, map.in_bits)
                } else {
                    (&mut map.outputs, map.out_bits)
                };
                if let Some(bits) = bits
                    && point.bit_offset + u16::from(point.bit_length) > bits
                {
                    return Err(bad(
                        n,
                        alloc::format!(
                            "{name}: ends at bit {} of {bits} bits of Process Data",
                            point.bit_offset + u16::from(point.bit_length)
                        ),
                    ));
                }
                // Two outputs on one bit would overwrite each other; two
                // inputs may read the same bits.
                if *dir == "out"
                    && let Some(other) = list.iter().find(|other| overlap(other, &point))
                {
                    return Err(bad(n, alloc::format!("{name}: shares bits with {}", other.name)));
                }
                if list.len() == MAX_POINTS {
                    return Err(bad(
                        n,
                        alloc::format!("{name}: a device has at most {MAX_POINTS} points a direction"),
                    ));
                }
                list.push(point);
            }
            _ => {}
        }
    }
    Ok(maps)
}

/// Why a port's points give no values although a Device answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    /// The Master refused the Device.
    Refused(Fault),
    /// The Device's input Process Data is not the length the map was
    /// written for (bits as Direct Parameter page 1 states them).
    InBits { map: u16, device: u16 },
    /// The same for the output Process Data.
    OutBits { map: u16, device: u16 },
    /// A point reaches past the Device's Process Data.
    Point { input: bool, index: usize },
}

/// The length Direct Parameter page 1 states for an IODD `bitLength`
/// (IODD §7.5.5 p.51).
const fn page_bits(bits: u16) -> u16 {
    if bits <= 16 { bits } else { bits.div_ceil(8) * 8 }
}

/// One `iolink` device running on a Master port.
#[derive(Debug)]
pub struct Port {
    map: PortMap,
    master: Master,
}

impl Port {
    #[must_use]
    pub fn new(map: PortMap) -> Self {
        let master = Master::new(Config {
            cycle_us: map.cycle_us,
            expect: map.expect,
        });
        Self { map, master }
    }

    #[must_use]
    pub const fn map(&self) -> &PortMap {
        &self.map
    }

    #[must_use]
    pub const fn master(&self) -> &Master {
        &self.master
    }

    /// What the line does next (`helmsman_iolink::master::Master::poll`).
    pub fn poll(&mut self, now: u64) -> Step<'_> {
        self.master.poll(now)
    }

    /// The octets that arrived for the last exchange.
    pub fn reply(&mut self, now: u64, octets: &[u8]) {
        self.master.reply(now, octets);
    }

    /// Why the points give no values, when it is not simply that no
    /// Device answers yet.
    #[must_use]
    pub fn problem(&self) -> Option<Problem> {
        if let Some(fault) = self.master.fault() {
            return Some(Problem::Refused(fault));
        }
        let page = self.master.page()?;
        let device_in = page.process_data_in().map_or(0, |pd| pd.bits);
        let device_out = page.process_data_out().map_or(0, |pd| pd.bits);
        if let Some(map) = self.map.in_bits.filter(|bits| page_bits(*bits) != device_in) {
            return Some(Problem::InBits {
                map,
                device: device_in,
            });
        }
        if let Some(map) = self.map.out_bits.filter(|bits| page_bits(*bits) != device_out) {
            return Some(Problem::OutBits {
                map,
                device: device_out,
            });
        }
        // What travels is whole octets (IOL Table A.10).
        let past = |points: &[Point], bits: u16| {
            points
                .iter()
                .position(|p| p.bit_offset + u16::from(p.bit_length) > bits.div_ceil(8) * 8)
        };
        if let Some(index) = past(&self.map.inputs, device_in) {
            return Some(Problem::Point { input: true, index });
        }
        past(&self.map.outputs, device_out).map(|index| Problem::Point { input: false, index })
    }

    /// The value of input point `index` (map order) from the last good
    /// cycle. `None` while the port is not exchanging valid Process
    /// Data, or has a [`Problem`].
    #[must_use]
    pub fn input(&self, index: usize) -> Option<Value> {
        let point = self.map.inputs.get(index)?;
        if self.problem().is_some() {
            return None;
        }
        let data = self.master.inputs()?;
        let (offset, length) = (usize::from(point.bit_offset), usize::from(point.bit_length));
        Some(match point.ty {
            Ty::Bool => Value::Bool(pd::get(data, offset, length)? != 0),
            Ty::Int if point.signed => Value::Int(pd::get_signed(data, offset, length)?),
            Ty::Int => Value::Int(i64::try_from(pd::get(data, offset, length)?).ok()?),
        })
    }

    /// The output points' values in map order, sent from the next cycle
    /// on; `None` declares the outputs invalid (a stopped or faulted
    /// program). False, with the outputs declared invalid, when the
    /// values are not one per output point of its type, a number does
    /// not fit its bits, or the port has no accepted Device or a
    /// [`Problem`].
    pub fn set_outputs(&mut self, values: Option<&[Value]>) -> bool {
        let packed = values.and_then(|values| self.pack(values));
        let done = match &packed {
            Some((octets, len)) => self.master.set_outputs(Some(&octets[..*len])).is_ok(),
            None => false,
        };
        if !done {
            let _ = self.master.set_outputs(None);
        }
        done || values.is_none()
    }

    fn pack(&self, values: &[Value]) -> Option<([u8; 32], usize)> {
        if self.problem().is_some() || values.len() != self.map.outputs.len() {
            return None;
        }
        let len = usize::from(self.master.page()?.process_data_out().ok()?.octets());
        let mut octets = [0; 32];
        for (point, value) in self.map.outputs.iter().zip(values) {
            let bits = u32::from(point.bit_length);
            let raw = match (point.ty, *value) {
                (Ty::Bool, Value::Bool(on)) => u64::from(on),
                (Ty::Int, Value::Int(v)) if point.signed => {
                    let half = 1i128 << (bits - 1);
                    if !(-half..half).contains(&i128::from(v)) {
                        return None;
                    }
                    // Two's complement in `bits` bits.
                    #[allow(clippy::cast_sign_loss)]
                    let raw = v as u64;
                    if bits == 64 { raw } else { raw & ((1 << bits) - 1) }
                }
                (Ty::Int, Value::Int(v)) => u64::try_from(v).ok().filter(|v| v >> bits == 0)?,
                _ => return None,
            };
            if !pd::put(
                &mut octets[..len],
                point.bit_offset.into(),
                point.bit_length.into(),
                raw,
            ) {
                return None;
            }
        }
        Some((octets, len))
    }

    /// The port is exchanging Process Data with an accepted Device.
    #[must_use]
    pub fn running(&self) -> bool {
        self.master.state() == State::Operate && self.problem().is_none()
    }
}
