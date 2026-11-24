//! What a node's I/O tasks do with a parsed map ([`crate::iomap`]), kept
//! here so the host tests run it: for each device, the requests a round
//! makes ([`ModbusPlan`]) or the assembly bytes it moves ([`EnipPlan`]),
//! and how each value goes between the device's bytes and the process
//! image. The firmware owns the sockets and the clock and calls these.
//!
//! The image holds every value little-endian in the slot the map gives
//! it ([`crate::iomap::At`]): a bool is a bit, `u8 i8 u16 i16` take 2
//! bytes (8-bit values widened, `i8` sign-extended), `u32 i32 f32` take 4.
//! A Modbus 32-bit value is two registers, high word first unless the
//! format ends in `sw`; an EtherNet/IP value is little-endian as CIP is.
//!
//! What a round writes: the coils of a device from coil 0 up to the
//! highest one the map names (an unnamed one below it is driven false, as
//! a one-block node always did), and holding registers only where a point
//! names them, adjacent ones in one request.

use alloc::vec::Vec;

use crate::api::SignalValue;
use crate::iomap::{Addr, At, Dir, EnipLink, Fmt, IoMap, Link, Point, Table};

/// Registers apart two reads may be and still share one request.
pub const READ_GAP: u16 = 8;
/// Most registers one read request asks for (Modbus Application Protocol
/// §6.3, 6.4: 125; docs/SOURCES.md).
pub const MAX_READ_REGS: u16 = 125;
/// Most registers one write request carries (§6.12: 123).
pub const MAX_WRITE_REGS: u16 = 123;

/// One point of a request: where in the reply or request it is, and where
/// in the image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Item {
    /// Bit or register index from the request's start.
    pub offset: u16,
    pub at: At,
    pub fmt: Option<Fmt>,
    pub swapped: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadKind {
    /// FC 0x02.
    Discrete,
    /// FC 0x04.
    Input,
    /// FC 0x03.
    Holding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Read {
    pub kind: ReadKind,
    pub start: u16,
    pub qty: u16,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteKind {
    /// FC 0x0F.
    Coils,
    /// FC 0x10.
    Holding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Write {
    pub kind: WriteKind,
    pub start: u16,
    pub qty: u16,
    pub items: Vec<Item>,
}

/// A Modbus TCP device's round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModbusPlan {
    pub unit: u8,
    pub reads: Vec<Read>,
    pub writes: Vec<Write>,
}

/// One point of an assembly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnipItem {
    /// Byte of the application data (after any run/idle header).
    pub byte: u16,
    pub bit: Option<u8>,
    pub at: At,
    pub fmt: Option<Fmt>,
}

/// An EtherNet/IP device's connection and the points it moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnipPlan {
    pub link: EnipLink,
    pub inputs: Vec<EnipItem>,
    pub outputs: Vec<EnipItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    Modbus(ModbusPlan),
    Enip(EnipPlan),
}

fn item(p: &Point, offset: u16) -> Item {
    let swapped = matches!(p.addr, Addr::Modbus { swapped: true, .. });
    Item {
        offset,
        at: p.at,
        fmt: p.fmt,
        swapped,
    }
}

fn words(p: &Point) -> u16 {
    p.fmt.map_or(1, |f| u16::from(f.width() / 2))
}

/// Spans of `(address, words, item)` sorted by address: points join a span
/// while they start within `gap` of its end and it stays within `max`.
fn spans(mut pts: Vec<(u16, u16, &Point)>, gap: u16, max: u16) -> Vec<(u16, u16, Vec<Item>)> {
    pts.sort_by_key(|(a, _, _)| *a);
    let mut out: Vec<(u16, u16, Vec<Item>)> = Vec::new();
    for (addr, w, p) in pts {
        let end = addr + w;
        if let Some((start, to, items)) = out.last_mut()
            && addr <= to.saturating_add(gap)
            && end.max(*to) - *start <= max
        {
            items.push(item(p, addr - *start));
            *to = end.max(*to);
            continue;
        }
        out.push((addr, end, alloc::vec![item(p, 0)]));
    }
    out
}

impl IoMap {
    /// The plan of device `k`.
    ///
    /// # Panics
    /// If `k` is not a device of the map.
    #[must_use]
    pub fn plan(&self, k: usize) -> Plan {
        let mine = || self.points.iter().filter(move |p| p.device == k);
        match &self.devices[k].link {
            Link::Modbus { unit } => {
                let table = |p: &Point| match p.addr {
                    Addr::Modbus { table, .. } => table,
                    Addr::Enip { .. } => Table::Coil,
                };
                let addr = |p: &Point| match p.addr {
                    Addr::Modbus { addr, .. } => addr,
                    Addr::Enip { .. } => 0,
                };
                let mut reads = Vec::new();
                let mut writes = Vec::new();
                for (kind, t) in [
                    (ReadKind::Discrete, Table::Discrete),
                    (ReadKind::Input, Table::Input),
                    (ReadKind::Holding, Table::Holding),
                ] {
                    let pts: Vec<(u16, u16, &Point)> = mine()
                        .filter(|p| p.dir == Dir::In && table(p) == t)
                        .map(|p| (addr(p), words(p), p))
                        .collect();
                    if kind == ReadKind::Discrete {
                        // From bit 0, one request: at most 32 bits.
                        if let Some(top) = pts.iter().map(|(a, _, _)| *a).max() {
                            let items = pts.iter().map(|(a, _, p)| item(p, *a)).collect();
                            reads.push(Read {
                                kind,
                                start: 0,
                                qty: top + 1,
                                items,
                            });
                        }
                    } else {
                        for (start, end, items) in spans(pts, READ_GAP, MAX_READ_REGS) {
                            reads.push(Read {
                                kind,
                                start,
                                qty: end - start,
                                items,
                            });
                        }
                    }
                }
                let coils: Vec<&Point> = mine()
                    .filter(|p| p.dir == Dir::Out && table(p) == Table::Coil)
                    .collect();
                if let Some(top) = coils.iter().map(|p| addr(p)).max() {
                    writes.push(Write {
                        kind: WriteKind::Coils,
                        start: 0,
                        qty: top + 1,
                        items: coils.iter().map(|p| item(p, addr(p))).collect(),
                    });
                }
                let regs: Vec<(u16, u16, &Point)> = mine()
                    .filter(|p| p.dir == Dir::Out && table(p) == Table::Holding)
                    .map(|p| (addr(p), words(p), p))
                    .collect();
                for (start, end, items) in spans(regs, 0, MAX_WRITE_REGS) {
                    writes.push(Write {
                        kind: WriteKind::Holding,
                        start,
                        qty: end - start,
                        items,
                    });
                }
                Plan::Modbus(ModbusPlan {
                    unit: *unit,
                    reads,
                    writes,
                })
            }
            Link::Enip(link) => {
                let items = |dir: Dir| -> Vec<EnipItem> {
                    mine()
                        .filter(|p| p.dir == dir)
                        .filter_map(|p| match p.addr {
                            Addr::Enip { byte, bit } => Some(EnipItem {
                                byte,
                                bit,
                                at: p.at,
                                fmt: p.fmt,
                            }),
                            Addr::Modbus { .. } => None,
                        })
                        .collect()
                };
                Plan::Enip(EnipPlan {
                    link: link.clone(),
                    inputs: items(Dir::In),
                    outputs: items(Dir::Out),
                })
            }
        }
    }
}

/// The bytes of an image slot, if the image reaches that far.
fn slot(image: &[u8], at: At) -> Option<&[u8]> {
    let b = usize::from(at.byte);
    image.get(b..b + usize::from(at.bytes))
}

fn slot_mut(image: &mut [u8], at: At) -> Option<&mut [u8]> {
    let b = usize::from(at.byte);
    image.get_mut(b..b + usize::from(at.bytes))
}

/// Sets or clears a bool's bit.
pub fn set_bit(image: &mut [u8], at: At, on: bool) {
    if let (Some(bit), Some(b)) = (at.bit, image.get_mut(usize::from(at.byte))) {
        if on {
            *b |= 1 << bit;
        } else {
            *b &= !(1 << bit);
        }
    }
}

/// A bool's bit.
#[must_use]
pub fn bit_of(image: &[u8], at: At) -> bool {
    at.bit
        .zip(image.get(usize::from(at.byte)))
        .is_some_and(|(bit, b)| b & (1 << bit) != 0)
}

fn le16(raw: &[u8]) -> Option<u16> {
    <[u8; 2]>::try_from(raw).ok().map(u16::from_le_bytes)
}

fn le32(raw: &[u8]) -> Option<u32> {
    <[u8; 4]>::try_from(raw).ok().map(u32::from_le_bytes)
}

impl Read {
    /// Stores the bits a discrete-input read answered.
    pub fn store_bits(&self, bits: &[bool], image: &mut [u8]) {
        for item in &self.items {
            if let Some(on) = bits.get(usize::from(item.offset)) {
                set_bit(image, item.at, *on);
            }
        }
    }

    /// Stores the registers a register read answered.
    pub fn store_regs(&self, regs: &[u16], image: &mut [u8]) {
        for item in &self.items {
            let Some(fmt) = item.fmt else { continue };
            let first = usize::from(item.offset);
            let Some(words) = regs.get(first..first + usize::from(fmt.width() / 2)) else {
                continue;
            };
            let Some(dst) = slot_mut(image, item.at) else {
                continue;
            };
            match *words {
                [one] => dst.copy_from_slice(&one.to_le_bytes()),
                [first_word, second_word] => {
                    let (high, low) = if item.swapped {
                        (second_word, first_word)
                    } else {
                        (first_word, second_word)
                    };
                    dst.copy_from_slice(&((u32::from(high) << 16) | u32::from(low)).to_le_bytes());
                }
                _ => {}
            }
        }
    }
}

impl Write {
    /// The coils to write, from `start`: those the map names follow the
    /// image, the rest are false.
    #[must_use]
    pub fn coils(&self, image: &[u8]) -> Vec<bool> {
        let mut coils = alloc::vec![false; usize::from(self.qty)];
        for item in &self.items {
            if let Some(coil) = coils.get_mut(usize::from(item.offset)) {
                *coil = bit_of(image, item.at);
            }
        }
        coils
    }

    /// The registers to write, from `start`.
    #[must_use]
    pub fn registers(&self, image: &[u8]) -> Vec<u16> {
        let mut regs = alloc::vec![0u16; usize::from(self.qty)];
        for item in &self.items {
            let Some(fmt) = item.fmt else { continue };
            let Some(src) = slot(image, item.at) else { continue };
            let first = usize::from(item.offset);
            if fmt.width() == 4 {
                let Some(value) = le32(src) else { continue };
                let (high, low) = ((value >> 16) as u16, (value & 0xFFFF) as u16);
                let (one, two) = if item.swapped { (low, high) } else { (high, low) };
                if let Some(pair) = regs.get_mut(first..first + 2) {
                    pair[0] = one;
                    pair[1] = two;
                }
            } else if let (Some(word), Some(reg)) = (le16(src), regs.get_mut(first)) {
                *reg = word;
            }
        }
        regs
    }
}

impl EnipPlan {
    /// Stores the T->O assembly's points into the image. A point past the
    /// data (a short packet) keeps what it had.
    pub fn store_inputs(&self, data: &[u8], image: &mut [u8]) {
        for item in &self.inputs {
            let first = usize::from(item.byte);
            match (item.bit, item.fmt) {
                (Some(bit), _) => {
                    if let Some(byte) = data.get(first) {
                        set_bit(image, item.at, byte & (1 << bit) != 0);
                    }
                }
                (None, Some(fmt)) => {
                    let Some(src) = data.get(first..first + usize::from(fmt.width())) else {
                        continue;
                    };
                    let Some(dst) = slot_mut(image, item.at) else {
                        continue;
                    };
                    match fmt {
                        Fmt::U8 => dst.copy_from_slice(&[src[0], 0]),
                        Fmt::I8 => dst.copy_from_slice(&[src[0], if src[0] & 0x80 == 0 { 0 } else { 0xFF }]),
                        _ => dst.copy_from_slice(src),
                    }
                }
                (None, None) => {}
            }
        }
    }

    /// The O->T assembly's application bytes from the image: every point
    /// the map names, zero elsewhere. `out` is the assembly's length.
    pub fn fill_outputs(&self, image: &[u8], out: &mut [u8]) {
        out.fill(0);
        for item in &self.outputs {
            let first = usize::from(item.byte);
            match (item.bit, item.fmt) {
                (Some(bit), _) => {
                    if bit_of(image, item.at)
                        && let Some(byte) = out.get_mut(first)
                    {
                        *byte |= 1 << bit;
                    }
                }
                (None, Some(fmt)) => {
                    let Some(src) = slot(image, item.at) else { continue };
                    let Some(dst) = out.get_mut(first..first + usize::from(fmt.width())) else {
                        continue;
                    };
                    match fmt {
                        Fmt::U8 => dst[0] = u8::try_from(le16(src).unwrap_or(0)).unwrap_or(u8::MAX),
                        Fmt::I8 => {
                            let value = le16(src).unwrap_or(0).cast_signed().clamp(-128, 127);
                            dst[0] = i8::try_from(value).unwrap_or(0).cast_unsigned();
                        }
                        _ => dst.copy_from_slice(src),
                    }
                }
                (None, None) => {}
            }
        }
    }
}

/// A point's value in an image, as the signals list shows it.
#[must_use]
pub fn value_at(image: &[u8], point: &Point) -> Option<SignalValue> {
    let Some(fmt) = point.fmt else {
        return Some(SignalValue::Bool(bit_of(image, point.at)));
    };
    let raw = slot(image, point.at)?;
    Some(match fmt {
        Fmt::U8 | Fmt::U16 => SignalValue::Int(i64::from(le16(raw)?)),
        Fmt::I8 | Fmt::I16 => SignalValue::Int(i64::from(le16(raw)?.cast_signed())),
        Fmt::U32 => SignalValue::Int(i64::from(le32(raw)?)),
        Fmt::I32 => SignalValue::Int(i64::from(le32(raw)?.cast_signed())),
        Fmt::F32 => SignalValue::Real(f64::from(f32::from_bits(le32(raw)?))),
    })
}

/// Writes a value into a point's place in an image: the inverse of
/// [`value_at`]. A bool sets or clears its bit and touches no other bit;
/// an integer is clamped to the point's format and stored the way the I/O
/// tasks read the output image (U8 and I8 as a 16-bit slot, little-endian);
/// a real is stored as `f32` bits. A value of the wrong kind for the point,
/// a real into an integer point and so on, changes nothing, and neither
/// does a point past the image's end.
pub fn put_at(image: &mut [u8], point: &Point, value: SignalValue) {
    let Some(fmt) = point.fmt else {
        if let SignalValue::Bool(on) = value {
            set_bit(image, point.at, on);
        }
        return;
    };
    let Some(raw) = slot_mut(image, point.at) else {
        return;
    };
    match (fmt, value) {
        (Fmt::U8, SignalValue::Int(v)) => raw.copy_from_slice(&u16::from(clamp_u8(v)).to_le_bytes()),
        (Fmt::I8, SignalValue::Int(v)) => {
            let v = i16::try_from(v.clamp(-128, 127)).unwrap_or(0);
            raw.copy_from_slice(&v.to_le_bytes());
        }
        (Fmt::U16, SignalValue::Int(v)) => {
            let v = u16::try_from(v.clamp(0, i64::from(u16::MAX))).unwrap_or(0);
            raw.copy_from_slice(&v.to_le_bytes());
        }
        (Fmt::I16, SignalValue::Int(v)) => {
            let v = i16::try_from(v.clamp(i64::from(i16::MIN), i64::from(i16::MAX))).unwrap_or(0);
            raw.copy_from_slice(&v.to_le_bytes());
        }
        (Fmt::U32, SignalValue::Int(v)) => {
            let v = u32::try_from(v.clamp(0, i64::from(u32::MAX))).unwrap_or(0);
            raw.copy_from_slice(&v.to_le_bytes());
        }
        (Fmt::I32, SignalValue::Int(v)) => {
            let v = i32::try_from(v.clamp(i64::from(i32::MIN), i64::from(i32::MAX))).unwrap_or(0);
            raw.copy_from_slice(&v.to_le_bytes());
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a REAL is held as f32 on the wire"
        )]
        (Fmt::F32, SignalValue::Real(v)) => raw.copy_from_slice(&(v as f32).to_bits().to_le_bytes()),
        _ => {}
    }
}

fn clamp_u8(v: i64) -> u8 {
    u8::try_from(v.clamp(0, 255)).unwrap_or(0)
}

/// Which program ports bind to which points of the map, by name: each
/// `(port index, point index)` pairs a port with the point of the same
/// name, compared case-insensitively (ST names are case-insensitive). An
/// input port (`is_input`) binds only to an `in` point and an output port
/// only to an `out` point. Ports with no point are left out, as are points
/// with no port. Where two points carry one name in one direction, the
/// first in map order wins, so the answer is the same on every load.
#[must_use]
pub fn bindings(map: &IoMap, ports: &[(&str, bool)]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (pi, (name, is_input)) in ports.iter().enumerate() {
        let want = if *is_input { Dir::In } else { Dir::Out };
        if let Some(k) = map
            .points
            .iter()
            .position(|p| p.dir == want && p.name.eq_ignore_ascii_case(name))
        {
            out.push((pi, k));
        }
    }
    out
}

#[cfg(test)]
mod tests;
