//! Modbus TCP client: the box as master of Modbus devices (remote I/O blocks,
//! drives, meters).
//!
//! References (see docs/SOURCES.md):
//! - MODBUS Application Protocol Specification V1.1b3 (Modbus Organization,
//!   2012-04-26), cited as "App §x p.y".
//! - MODBUS Messaging on TCP/IP Implementation Guide V1.0b (2006-10-24),
//!   cited as "TCP §x p.y".
//!
//! Addresses are the 0-based PDU addresses of App §4.4: a manual's "40001"
//! is `hr:0`. Point syntax:
//!
//! - `coil:N` (read/write bit), `di:N` (discrete input, read-only bit)
//! - `hr:N` (holding register, read/write), `ir:N` (input register, read-only)
//!   with an optional format: `u16` (default), `i16`, `u32`, `i32`, `f32`.
//!   32-bit formats span N and N+1, high word first; append `sw` (`f32sw`)
//!   for devices that put the low word first.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::driver::{Driver, Error, PointInfo};
use crate::model::{Ty, Value};

pub const DRIVER: &str = "modbus-tcp";

/// App §4.1 p.3: the PDU is at most 253 bytes.
pub const MAX_PDU: usize = 253;
/// TCP §3.1.3 p.5: transaction id, protocol id, length, unit id.
pub const MBAP_LEN: usize = 7;

pub const READ_COILS: u8 = 0x01;
pub const READ_DISCRETE_INPUTS: u8 = 0x02;
pub const READ_HOLDING_REGISTERS: u8 = 0x03;
pub const READ_INPUT_REGISTERS: u8 = 0x04;
pub const WRITE_SINGLE_COIL: u8 = 0x05;
pub const WRITE_SINGLE_REGISTER: u8 = 0x06;
pub const WRITE_MULTIPLE_REGISTERS: u8 = 0x10;

/// App §6.1 p.11, §6.2 p.12: 1 to 2000 bits per read.
pub const MAX_READ_BITS: u16 = 2000;
/// App §6.3 p.15, §6.4 p.16: 1 to 125 registers per read.
pub const MAX_READ_REGS: u16 = 125;
/// App §6.12 p.30: 1 to 123 registers per write.
pub const MAX_WRITE_REGS: u16 = 123;
/// App §6.5 p.17: the only two legal coil values.
pub const COIL_ON: u16 = 0xFF00;
pub const COIL_OFF: u16 = 0x0000;

/// App §7 p.47 exception codes.
pub const ILLEGAL_FUNCTION: u8 = 0x01;
pub const ILLEGAL_DATA_ADDRESS: u8 = 0x02;
pub const ILLEGAL_DATA_VALUE: u8 = 0x03;
pub const SERVER_DEVICE_FAILURE: u8 = 0x04;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Table {
    Coil,
    DiscreteInput,
    Holding,
    Input,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Bit,
    U16,
    I16,
    U32 { swapped: bool },
    I32 { swapped: bool },
    F32 { swapped: bool },
}

impl Format {
    /// Registers the value spans.
    #[must_use]
    pub fn words(self) -> u16 {
        match self {
            Self::Bit | Self::U16 | Self::I16 => 1,
            Self::U32 { .. } | Self::I32 { .. } | Self::F32 { .. } => 2,
        }
    }

    #[must_use]
    pub fn ty(self) -> Ty {
        match self {
            Self::Bit => Ty::Bool,
            Self::F32 { .. } => Ty::Real,
            _ => Ty::Int,
        }
    }

    /// Value from the registers read, in wire order.
    #[must_use]
    pub fn decode(self, regs: &[u16]) -> Value {
        let pair = |swapped: bool| {
            let (hi, lo) = if swapped {
                (regs[1], regs[0])
            } else {
                (regs[0], regs[1])
            };
            (u32::from(hi) << 16) | u32::from(lo)
        };
        match self {
            Self::Bit => Value::Bool(regs[0] != 0),
            Self::U16 => Value::Int(i64::from(regs[0])),
            Self::I16 => Value::Int(i64::from(regs[0].cast_signed())),
            Self::U32 { swapped } => Value::Int(i64::from(pair(swapped))),
            Self::I32 { swapped } => Value::Int(i64::from(pair(swapped).cast_signed())),
            Self::F32 { swapped } => Value::Real(f64::from(f32::from_bits(pair(swapped)))),
        }
    }

    /// Registers to write, in wire order.
    ///
    /// # Errors
    /// The value is the wrong type or out of the format's range.
    pub fn encode(self, v: Value) -> Result<Vec<u16>, Error> {
        let split = |x: u32, swapped: bool| {
            let (hi, lo) = ((x >> 16) as u16, (x & 0xFFFF) as u16);
            if swapped { vec![lo, hi] } else { vec![hi, lo] }
        };
        let range = |n: i64, what: &str| Error::Point(format!("{n} does not fit {what}"));
        match (self, v) {
            (Self::Bit, Value::Bool(b)) => Ok(vec![u16::from(b)]),
            (Self::U16, Value::Int(n)) => u16::try_from(n).map(|x| vec![x]).map_err(|_| range(n, "u16")),
            (Self::I16, Value::Int(n)) => i16::try_from(n)
                .map(|x| vec![x.cast_unsigned()])
                .map_err(|_| range(n, "i16")),
            (Self::U32 { swapped }, Value::Int(n)) => u32::try_from(n)
                .map(|x| split(x, swapped))
                .map_err(|_| range(n, "u32")),
            (Self::I32 { swapped }, Value::Int(n)) => i32::try_from(n)
                .map(|x| split(x.cast_unsigned(), swapped))
                .map_err(|_| range(n, "i32")),
            #[allow(clippy::cast_possible_truncation)] // f32 is the wire format; rounding is the point.
            (Self::F32 { swapped }, Value::Real(r)) => Ok(split((r as f32).to_bits(), swapped)),
            (f, v) => Err(Error::Type {
                want: f.ty(),
                got: v.ty(),
            }),
        }
    }
}

/// A parsed Modbus point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub table: Table,
    pub addr: u16,
    pub format: Format,
}

impl Point {
    /// Parses `table:addr[:format]`; see the module docs.
    ///
    /// # Errors
    /// Anything else, including a 32-bit format starting at 65535.
    pub fn parse(s: &str) -> Result<Self, Error> {
        let bad = || Error::Address(s.into());
        let mut parts = s.split(':');
        let table = match parts.next() {
            Some("coil") => Table::Coil,
            Some("di") => Table::DiscreteInput,
            Some("hr") => Table::Holding,
            Some("ir") => Table::Input,
            _ => return Err(bad()),
        };
        let addr_text = parts.next().ok_or_else(bad)?;
        if addr_text.is_empty() || !addr_text.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad());
        }
        let addr: u16 = addr_text.parse().map_err(|_| bad())?;
        let format = match (table, parts.next()) {
            (Table::Coil | Table::DiscreteInput, None) => Format::Bit,
            (Table::Coil | Table::DiscreteInput, Some(_)) => return Err(bad()),
            (_, None | Some("u16")) => Format::U16,
            (_, Some("i16")) => Format::I16,
            (_, Some("u32")) => Format::U32 { swapped: false },
            (_, Some("u32sw")) => Format::U32 { swapped: true },
            (_, Some("i32")) => Format::I32 { swapped: false },
            (_, Some("i32sw")) => Format::I32 { swapped: true },
            (_, Some("f32")) => Format::F32 { swapped: false },
            (_, Some("f32sw")) => Format::F32 { swapped: true },
            _ => return Err(bad()),
        };
        if parts.next().is_some() || addr.checked_add(format.words() - 1).is_none() {
            return Err(bad());
        }
        Ok(Self { table, addr, format })
    }

    #[must_use]
    pub fn info(self) -> PointInfo {
        PointInfo {
            ty: self.format.ty(),
            writable: matches!(self.table, Table::Coil | Table::Holding),
        }
    }
}

/// Request PDUs, App §6.
pub mod pdu {
    use super::{Error, MAX_PDU, MAX_READ_BITS, MAX_READ_REGS, MAX_WRITE_REGS, WRITE_MULTIPLE_REGISTERS};

    fn be(v: u16) -> [u8; 2] {
        v.to_be_bytes()
    }

    /// FC 0x01-0x04: function, start, quantity.
    ///
    /// # Errors
    /// Quantity outside the spec's range for the function.
    pub fn read(fc: u8, start: u16, qty: u16) -> Result<Vec<u8>, Error> {
        let max = if fc <= 0x02 { MAX_READ_BITS } else { MAX_READ_REGS };
        if qty == 0 || qty > max || start.checked_add(qty - 1).is_none() {
            return Err(Error::Protocol(format!(
                "read of {qty} from {start} is out of range"
            )));
        }
        let mut p = vec![fc];
        p.extend(be(start));
        p.extend(be(qty));
        Ok(p)
    }

    /// FC 0x05 / 0x06: function, address, value. The reply echoes it.
    #[must_use]
    pub fn write_single(fc: u8, addr: u16, value: u16) -> Vec<u8> {
        let mut p = vec![fc];
        p.extend(be(addr));
        p.extend(be(value));
        p
    }

    /// FC 0x10: function, start, quantity, byte count, values.
    ///
    /// # Errors
    /// Zero or more than 123 registers, or running past address 65535.
    pub fn write_registers(start: u16, values: &[u16]) -> Result<Vec<u8>, Error> {
        let qty = u16::try_from(values.len()).unwrap_or(u16::MAX);
        if qty == 0 || qty > MAX_WRITE_REGS || start.checked_add(qty - 1).is_none() {
            return Err(Error::Protocol(format!(
                "write of {qty} registers from {start} is out of range"
            )));
        }
        let mut p = vec![WRITE_MULTIPLE_REGISTERS];
        p.extend(be(start));
        p.extend(be(qty));
        p.push(u8::try_from(qty * 2).expect("at most 246"));
        for v in values {
            p.extend(be(*v));
        }
        debug_assert!(p.len() <= MAX_PDU);
        Ok(p)
    }
}

/// Reply checks, App §6 and §7: anything not exactly what the request asks
/// for is a protocol error, never a panic or a guess.
pub mod reply {
    use super::Error;

    /// Strips the function code, turning an exception reply (function code +
    /// 0x80, one exception byte; App §7 p.47) into [`Error::Exception`].
    ///
    /// # Errors
    /// Empty reply, exception, or a reply to another function.
    pub fn body(fc: u8, pdu: &[u8]) -> Result<&[u8], Error> {
        match pdu {
            [] => Err(Error::Protocol("empty reply".into())),
            [f, code] if *f == fc | 0x80 => Err(Error::Exception(*code)),
            [f, ..] if *f == fc | 0x80 => Err(Error::Protocol("malformed exception reply".into())),
            [f, rest @ ..] if *f == fc => Ok(rest),
            [f, ..] => Err(Error::Protocol(format!(
                "reply function {f:#04x} to request {fc:#04x}"
            ))),
        }
    }

    /// FC 0x01/0x02: byte count, then bits LSB-first, last byte zero-padded
    /// (App §6.1 p.11).
    ///
    /// # Errors
    /// Byte count or length not matching the quantity asked for.
    pub fn bits(fc: u8, qty: u16, pdu: &[u8]) -> Result<Vec<bool>, Error> {
        let b = body(fc, pdu)?;
        let want = usize::from(qty).div_ceil(8);
        match b {
            [n, data @ ..] if usize::from(*n) == want && data.len() == want => Ok((0..usize::from(qty))
                .map(|i| data[i / 8] >> (i % 8) & 1 == 1)
                .collect()),
            _ => Err(Error::Protocol(format!("bit reply length for {qty} bits"))),
        }
    }

    /// FC 0x03/0x04: byte count (2 per register), then big-endian registers
    /// (App §6.3 p.15).
    ///
    /// # Errors
    /// Byte count or length not matching the quantity asked for.
    pub fn registers(fc: u8, qty: u16, pdu: &[u8]) -> Result<Vec<u16>, Error> {
        let b = body(fc, pdu)?;
        let want = usize::from(qty) * 2;
        match b {
            [n, data @ ..] if usize::from(*n) == want && data.len() == want => Ok(data
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_be_bytes(*c))
                .collect()),
            _ => Err(Error::Protocol(format!(
                "register reply length for {qty} registers"
            ))),
        }
    }

    /// FC 0x05/0x06 echo the request; FC 0x10 echoes start and quantity
    /// (App §6.5 p.17, §6.6 p.19, §6.12 p.30).
    ///
    /// # Errors
    /// The echo differs from what was sent.
    pub fn echo(request: &[u8], pdu: &[u8]) -> Result<(), Error> {
        let fc = request[0];
        let b = body(fc, pdu)?;
        let want = if fc == super::WRITE_MULTIPLE_REGISTERS {
            &request[1..5]
        } else {
            &request[1..]
        };
        if b == want {
            Ok(())
        } else {
            Err(Error::Protocol("write reply does not echo the request".into()))
        }
    }
}

/// A Modbus TCP connection to one unit. Every error drops the connection, so
/// the next request starts clean (a late reply can never be taken for the
/// answer to a newer request).
pub struct Client {
    endpoint: String,
    unit: u8,
    timeout: Duration,
    stream: Option<TcpStream>,
    tid: u16,
}

impl Client {
    #[must_use]
    pub fn new(endpoint: &str, unit: u8, timeout: Duration) -> Self {
        Self {
            endpoint: endpoint.into(),
            unit,
            timeout,
            stream: None,
            tid: 0,
        }
    }

    fn connect(&mut self) -> Result<&mut TcpStream, Error> {
        if self.stream.is_none() {
            let mut last = None;
            for addr in self.endpoint.to_socket_addrs()? {
                match TcpStream::connect_timeout(&addr, self.timeout) {
                    Ok(s) => {
                        s.set_read_timeout(Some(self.timeout))?;
                        s.set_write_timeout(Some(self.timeout))?;
                        s.set_nodelay(true)?;
                        self.stream = Some(s);
                        break;
                    }
                    Err(e) => last = Some(e),
                }
            }
            if self.stream.is_none() {
                return Err(Error::Io(
                    last.unwrap_or_else(|| std::io::Error::other("no address")),
                ));
            }
        }
        Ok(self.stream.as_mut().expect("set above"))
    }

    /// Sends one request PDU and returns the reply PDU.
    ///
    /// # Errors
    /// Network failure or timeout, or a reply whose header does not match the
    /// request.
    pub fn transact(&mut self, pdu: &[u8]) -> Result<Vec<u8>, Error> {
        let r = self.exchange(pdu);
        if r.is_err() {
            self.stream = None;
        }
        r
    }

    fn exchange(&mut self, pdu: &[u8]) -> Result<Vec<u8>, Error> {
        self.tid = self.tid.wrapping_add(1);
        let (tid, unit) = (self.tid, self.unit);
        let len = u16::try_from(pdu.len() + 1).map_err(|_| Error::Protocol("request too long".into()))?;
        let mut frame = Vec::with_capacity(MBAP_LEN + pdu.len());
        frame.extend(tid.to_be_bytes());
        frame.extend(0u16.to_be_bytes());
        frame.extend(len.to_be_bytes());
        frame.push(unit);
        frame.extend(pdu);
        let s = self.connect()?;
        s.write_all(&frame)?;
        let mut head = [0u8; MBAP_LEN];
        s.read_exact(&mut head)?;
        let got_tid = u16::from_be_bytes([head[0], head[1]]);
        let proto = u16::from_be_bytes([head[2], head[3]]);
        let len = usize::from(u16::from_be_bytes([head[4], head[5]]));
        if proto != 0 {
            return Err(Error::Protocol(format!("protocol id {proto}")));
        }
        if got_tid != tid {
            return Err(Error::Protocol(format!("transaction {got_tid}, expected {tid}")));
        }
        // Length counts the unit id plus a PDU of 1..=253 bytes.
        if !(2..=MAX_PDU + 1).contains(&len) {
            return Err(Error::Protocol(format!("length field {len}")));
        }
        if head[6] != unit {
            return Err(Error::Protocol(format!("unit {}, expected {unit}", head[6])));
        }
        let mut body = vec![0u8; len - 1];
        s.read_exact(&mut body)?;
        Ok(body)
    }

    /// # Errors
    /// See [`Self::transact`] and [`reply::bits`].
    pub fn read_bits(&mut self, fc: u8, start: u16, qty: u16) -> Result<Vec<bool>, Error> {
        let req = pdu::read(fc, start, qty)?;
        let r = self.transact(&req)?;
        reply::bits(fc, qty, &r)
    }

    /// # Errors
    /// See [`Self::transact`] and [`reply::registers`].
    pub fn read_registers(&mut self, fc: u8, start: u16, qty: u16) -> Result<Vec<u16>, Error> {
        let req = pdu::read(fc, start, qty)?;
        let r = self.transact(&req)?;
        reply::registers(fc, qty, &r)
    }

    /// # Errors
    /// See [`Self::transact`] and [`reply::echo`].
    pub fn write_coil(&mut self, addr: u16, on: bool) -> Result<(), Error> {
        let req = pdu::write_single(WRITE_SINGLE_COIL, addr, if on { COIL_ON } else { COIL_OFF });
        let r = self.transact(&req)?;
        reply::echo(&req, &r)
    }

    /// One register with FC 0x06, more with FC 0x10.
    ///
    /// # Errors
    /// See [`Self::transact`] and [`reply::echo`].
    pub fn write_registers(&mut self, start: u16, values: &[u16]) -> Result<(), Error> {
        let req = match values {
            [v] => pdu::write_single(WRITE_SINGLE_REGISTER, start, *v),
            _ => pdu::write_registers(start, values)?,
        };
        let r = self.transact(&req)?;
        reply::echo(&req, &r)
    }
}

/// [`Driver`] over a [`Client`]. Reads one point per request for now.
pub struct ModbusDriver {
    pub client: Client,
}

impl ModbusDriver {
    /// From device params: `unit` (default 1) and `timeout_ms` (default 1000).
    ///
    /// # Errors
    /// A param that does not parse.
    pub fn from_params(
        endpoint: &str,
        params: &std::collections::BTreeMap<String, String>,
    ) -> Result<Self, Error> {
        let unit = params
            .get("unit")
            .map_or(Ok(1), |u| u.parse::<u8>())
            .map_err(|_| Error::Address("unit".into()))?;
        let ms = params
            .get("timeout_ms")
            .map_or(Ok(1000), |t| t.parse::<u64>())
            .map_err(|_| Error::Address("timeout_ms".into()))?;
        Ok(Self {
            client: Client::new(endpoint, unit, Duration::from_millis(ms.max(1))),
        })
    }
}

impl Driver for ModbusDriver {
    fn parse(&self, addr: &str) -> Result<PointInfo, Error> {
        Point::parse(addr).map(Point::info)
    }

    fn read(&mut self, addrs: &[&str]) -> Result<Vec<Value>, Error> {
        let points: Vec<Point> = addrs.iter().map(|a| Point::parse(a)).collect::<Result<_, _>>()?;
        points
            .into_iter()
            .map(|p| match p.table {
                Table::Coil | Table::DiscreteInput => {
                    let fc = if p.table == Table::Coil {
                        READ_COILS
                    } else {
                        READ_DISCRETE_INPUTS
                    };
                    Ok(Value::Bool(self.client.read_bits(fc, p.addr, 1)?[0]))
                }
                Table::Holding | Table::Input => {
                    let fc = if p.table == Table::Holding {
                        READ_HOLDING_REGISTERS
                    } else {
                        READ_INPUT_REGISTERS
                    };
                    let regs = self.client.read_registers(fc, p.addr, p.format.words())?;
                    Ok(p.format.decode(&regs))
                }
            })
            .collect()
    }

    fn write(&mut self, addr: &str, value: Value) -> Result<(), Error> {
        let p = Point::parse(addr)?;
        match p.table {
            Table::DiscreteInput | Table::Input => Err(Error::ReadOnly(addr.into())),
            Table::Coil => match value {
                Value::Bool(b) => self.client.write_coil(p.addr, b),
                v => Err(Error::Type {
                    want: Ty::Bool,
                    got: v.ty(),
                }),
            },
            Table::Holding => {
                let regs = p.format.encode(value)?;
                self.client.write_registers(p.addr, &regs)
            }
        }
    }
}
