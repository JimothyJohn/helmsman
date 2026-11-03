//! Modbus client: the box as master of Modbus devices (remote I/O blocks,
//! drives, meters). The application layer (points, PDUs, reply checks,
//! [`app`]) is shared; this module is also the TCP transport, and
//! [`crate::rtu`] the serial one.
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

// The bytes of Modbus are the codec's (crates/helmsman-modbus), shared with
// the edge node; the names stay here for every user of this module.
pub use helmsman_modbus::{
    COIL_OFF, COIL_ON, ILLEGAL_DATA_ADDRESS, ILLEGAL_DATA_VALUE, ILLEGAL_FUNCTION, MAX_PDU, MAX_READ_BITS,
    MAX_READ_REGS, MAX_WRITE_COILS, MAX_WRITE_REGS, MBAP_LEN, READ_COILS, READ_DISCRETE_INPUTS,
    READ_HOLDING_REGISTERS, READ_INPUT_REGISTERS, SERVER_DEVICE_FAILURE, WRITE_MULTIPLE_COILS,
    WRITE_MULTIPLE_REGISTERS, WRITE_SINGLE_COIL, WRITE_SINGLE_REGISTER,
};

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

/// Request PDUs, App §6: the codec's, with this crate's [`Error`].
pub mod pdu {
    use super::Error;

    /// FC 0x01-0x04: function, start, quantity.
    ///
    /// # Errors
    /// Quantity outside the spec's range for the function.
    pub fn read(fc: u8, start: u16, qty: u16) -> Result<Vec<u8>, Error> {
        Ok(helmsman_modbus::pdu::read(fc, start, qty)?)
    }

    /// FC 0x05 / 0x06: function, address, value. The reply echoes it.
    #[must_use]
    pub fn write_single(fc: u8, addr: u16, value: u16) -> Vec<u8> {
        helmsman_modbus::pdu::write_single(fc, addr, value)
    }

    /// FC 0x0F: coils from `start`, LSB-first.
    ///
    /// # Errors
    /// Zero or more than 1968 coils, or running past address 65535.
    pub fn write_coils(start: u16, values: &[bool]) -> Result<Vec<u8>, Error> {
        Ok(helmsman_modbus::pdu::write_coils(start, values)?)
    }

    /// FC 0x10: function, start, quantity, byte count, values.
    ///
    /// # Errors
    /// Zero or more than 123 registers, or running past address 65535.
    pub fn write_registers(start: u16, values: &[u16]) -> Result<Vec<u8>, Error> {
        Ok(helmsman_modbus::pdu::write_registers(start, values)?)
    }
}

/// Reply checks, App §6 and §7: the codec's, with this crate's [`Error`].
/// Anything not exactly what the request asks for is a protocol error,
/// never a panic or a guess.
pub mod reply {
    use super::Error;

    /// Strips the function code, turning an exception reply into
    /// [`Error::Exception`].
    ///
    /// # Errors
    /// Empty reply, exception, or a reply to another function.
    pub fn body(fc: u8, pdu: &[u8]) -> Result<&[u8], Error> {
        Ok(helmsman_modbus::reply::body(fc, pdu)?)
    }

    /// FC 0x01/0x02: byte count, then bits LSB-first.
    ///
    /// # Errors
    /// Byte count or length not matching the quantity asked for.
    pub fn bits(fc: u8, qty: u16, pdu: &[u8]) -> Result<Vec<bool>, Error> {
        Ok(helmsman_modbus::reply::bits(fc, qty, pdu)?)
    }

    /// FC 0x03/0x04: byte count, then big-endian registers.
    ///
    /// # Errors
    /// Byte count or length not matching the quantity asked for.
    pub fn registers(fc: u8, qty: u16, pdu: &[u8]) -> Result<Vec<u16>, Error> {
        Ok(helmsman_modbus::reply::registers(fc, qty, pdu)?)
    }

    /// FC 0x05/0x06 echo the request; FC 0x0F and 0x10 echo start and
    /// quantity.
    ///
    /// # Errors
    /// The echo differs from what was sent.
    pub fn echo(request: &[u8], pdu: &[u8]) -> Result<(), Error> {
        Ok(helmsman_modbus::reply::echo(request, pdu)?)
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
        let frame = helmsman_modbus::mbap::frame(tid, unit, pdu)?;
        let s = self.connect()?;
        s.write_all(&frame)?;
        let mut head = [0u8; MBAP_LEN];
        s.read_exact(&mut head)?;
        // A late or foreign reply is never taken for ours.
        let len = helmsman_modbus::mbap::check_head(&head, tid, unit)?;
        let mut body = vec![0u8; len];
        s.read_exact(&mut body)?;
        Ok(body)
    }

    /// # Errors
    /// See [`Self::transact`] and [`reply::bits`].
    pub fn read_bits(&mut self, fc: u8, start: u16, qty: u16) -> Result<Vec<bool>, Error> {
        app::read_bits(self, fc, start, qty)
    }

    /// # Errors
    /// See [`Self::transact`] and [`reply::registers`].
    pub fn read_registers(&mut self, fc: u8, start: u16, qty: u16) -> Result<Vec<u16>, Error> {
        app::read_registers(self, fc, start, qty)
    }

    /// # Errors
    /// See [`Self::transact`] and [`reply::echo`].
    pub fn write_coil(&mut self, addr: u16, on: bool) -> Result<(), Error> {
        app::write_coil(self, addr, on)
    }

    /// One register with FC 0x06, more with FC 0x10.
    ///
    /// # Errors
    /// See [`Self::transact`] and [`reply::echo`].
    pub fn write_registers(&mut self, start: u16, values: &[u16]) -> Result<(), Error> {
        app::write_registers(self, start, values)
    }
}

impl Transport for Client {
    fn request(&mut self, pdu: &[u8]) -> Result<Option<Vec<u8>>, Error> {
        self.transact(pdu).map(Some)
    }
}

/// How a request PDU reaches a unit and its reply comes back: TCP here
/// (MBAP, TCP §3.1.3), RTU in [`crate::rtu`] (address + CRC, Serial §2.5.1).
/// The application layer in [`app`] is the same over both.
pub trait Transport {
    /// Sends one request PDU and returns the reply PDU, or `None` when the
    /// request was broadcast and no reply is due (Serial §2.1 p.7).
    ///
    /// # Errors
    /// Link trouble ([`Error::is_link`]), or a request the transport
    /// refuses to send.
    fn request(&mut self, pdu: &[u8]) -> Result<Option<Vec<u8>>, Error>;
}

/// The application layer over any [`Transport`]: build the request PDU,
/// send it, check the reply. A broadcast write succeeds once sent.
pub mod app {
    use super::{COIL_OFF, COIL_ON, Error, Transport, WRITE_SINGLE_COIL, WRITE_SINGLE_REGISTER, pdu, reply};

    fn answer(t: &mut (impl Transport + ?Sized), req: &[u8]) -> Result<Vec<u8>, Error> {
        t.request(req)?
            .ok_or_else(|| Error::Protocol("a read got no reply: broadcast takes writes only".into()))
    }

    /// # Errors
    /// See [`Transport::request`] and [`reply::bits`].
    pub fn read_bits(
        t: &mut (impl Transport + ?Sized),
        fc: u8,
        start: u16,
        qty: u16,
    ) -> Result<Vec<bool>, Error> {
        let req = pdu::read(fc, start, qty)?;
        let r = answer(t, &req)?;
        reply::bits(fc, qty, &r)
    }

    /// # Errors
    /// See [`Transport::request`] and [`reply::registers`].
    pub fn read_registers(
        t: &mut (impl Transport + ?Sized),
        fc: u8,
        start: u16,
        qty: u16,
    ) -> Result<Vec<u16>, Error> {
        let req = pdu::read(fc, start, qty)?;
        let r = answer(t, &req)?;
        reply::registers(fc, qty, &r)
    }

    /// # Errors
    /// See [`Transport::request`] and [`reply::echo`].
    pub fn write_coil(t: &mut (impl Transport + ?Sized), addr: u16, on: bool) -> Result<(), Error> {
        let req = pdu::write_single(WRITE_SINGLE_COIL, addr, if on { COIL_ON } else { COIL_OFF });
        match t.request(&req)? {
            Some(r) => reply::echo(&req, &r),
            None => Ok(()),
        }
    }

    /// One register with FC 0x06, more with FC 0x10.
    ///
    /// # Errors
    /// See [`Transport::request`] and [`reply::echo`].
    pub fn write_registers(
        t: &mut (impl Transport + ?Sized),
        start: u16,
        values: &[u16],
    ) -> Result<(), Error> {
        let req = match values {
            [v] => pdu::write_single(WRITE_SINGLE_REGISTER, start, *v),
            _ => pdu::write_registers(start, values)?,
        };
        match t.request(&req)? {
            Some(r) => reply::echo(&req, &r),
            None => Ok(()),
        }
    }
}

/// [`Driver`] over a [`Transport`]: a TCP [`Client`] by default, an RTU
/// master in [`crate::rtu`]. Reads one point per request for now.
pub struct ModbusDriver<T = Client> {
    pub client: T,
}

impl ModbusDriver<Client> {
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

impl<T: Transport> Driver for ModbusDriver<T> {
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
                    Ok(Value::Bool(app::read_bits(&mut self.client, fc, p.addr, 1)?[0]))
                }
                Table::Holding | Table::Input => {
                    let fc = if p.table == Table::Holding {
                        READ_HOLDING_REGISTERS
                    } else {
                        READ_INPUT_REGISTERS
                    };
                    let regs = app::read_registers(&mut self.client, fc, p.addr, p.format.words())?;
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
                Value::Bool(b) => app::write_coil(&mut self.client, p.addr, b),
                v => Err(Error::Type {
                    want: Ty::Bool,
                    got: v.ty(),
                }),
            },
            Table::Holding => {
                let regs = p.format.encode(value)?;
                app::write_registers(&mut self.client, p.addr, &regs)
            }
        }
    }
}
