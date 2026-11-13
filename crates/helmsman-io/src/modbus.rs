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

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{Read, Write};
use std::net::{SocketAddr, SocketAddrV4, TcpStream};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use crate::driver::{Driver, Error, PointInfo};
use crate::model::{Ty, Value};

pub const DRIVER: &str = "modbus-tcp";

// The bytes of Modbus are the codec's (crates/helmsman-modbus), shared with
// the edge node; the names stay here for every user of this module.
pub use helmsman_modbus::{
    COIL_OFF, COIL_ON, ENCAPSULATED_INTERFACE, GATEWAY_PATH_UNAVAILABLE, GATEWAY_TARGET_FAILED,
    ILLEGAL_DATA_ADDRESS, ILLEGAL_DATA_VALUE, ILLEGAL_FUNCTION, MAX_PDU, MAX_READ_BITS, MAX_READ_REGS,
    MAX_WRITE_COILS, MAX_WRITE_REGS, MBAP_LEN, MEI_READ_DEVICE_ID, OBJECT_MAJOR_MINOR_REVISION,
    OBJECT_PRODUCT_CODE, OBJECT_VENDOR_NAME, READ_COILS, READ_DEVICE_ID_BASIC, READ_DISCRETE_INPUTS,
    READ_HOLDING_REGISTERS, READ_INPUT_REGISTERS, SERVER_DEVICE_FAILURE, WRITE_MULTIPLE_COILS,
    WRITE_MULTIPLE_REGISTERS, WRITE_SINGLE_COIL, WRITE_SINGLE_REGISTER,
};

/// Most Read Device Identification replies one identification may take
/// (More Follows, App §6.21 p.44). The three basic objects fit in one
/// reply on every device seen; a server that keeps saying "more" past this
/// is refused, never followed.
pub const MAX_ID_REPLIES: usize = 8;
/// Longest object text kept, in characters.
pub const MAX_ID_TEXT: usize = 64;

/// What a device answers Read Device Identification with: the three basic
/// objects (App §6.21 p.43; all mandatory), as text. The spec says ASCII;
/// a device's bytes are taken lossily, control characters dropped,
/// trimmed and cut at [`MAX_ID_TEXT`] characters, so no name a device
/// sends can break a log line or the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceIdentification {
    /// Object 0x00, `VendorName`.
    pub vendor_name: String,
    /// Object 0x01, `ProductCode`.
    pub product_code: String,
    /// Object 0x02, `MajorMinorRevision`.
    pub revision: String,
    /// The conformity level the device reported (0x01-0x03, 0x81-0x83).
    pub conformity: u8,
}

/// An object's bytes as the text [`DeviceIdentification`] keeps.
#[must_use]
pub fn id_text(bytes: &[u8]) -> String {
    let s: String = String::from_utf8_lossy(bytes)
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    s.trim().chars().take(MAX_ID_TEXT).collect()
}

impl DeviceIdentification {
    fn from_objects(conformity: u8, objects: &[(u8, Vec<u8>)]) -> Result<Self, Error> {
        let get = |id: u8, name: &str| {
            objects
                .iter()
                .find(|(i, _)| *i == id)
                .map(|(_, v)| id_text(v))
                .ok_or_else(|| Error::Protocol(format!("no {name} (object {id:#04x}) in the identification")))
        };
        Ok(Self {
            vendor_name: get(OBJECT_VENDOR_NAME, "VendorName")?,
            product_code: get(OBJECT_PRODUCT_CODE, "ProductCode")?,
            revision: get(OBJECT_MAJOR_MINOR_REVISION, "MajorMinorRevision")?,
            conformity,
        })
    }
}

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

/// A Modbus TCP client for one unit. Clients whose endpoints reach the
/// same address share one connection to it (a [`Line`]): the units behind
/// one gateway (the unit id routes each request, TCP §3.1.3 pp.5-6) cost
/// the gateway one connection, not one each, as the guide recommends
/// (TCP §4.2.1.1 p.10, rule 3).
///
/// One request is in flight on a connection at a time (a gateway's serial
/// line answers one at a time anyway), taken first come first served: a
/// client waits for the requests queued before it, at most one per other
/// client and each bounded by that client's own timeout, then has its
/// whole timeout for its own reply. Replies are matched by transaction
/// id (TCP §4.4.1.2 p.23): a late reply to a request that timed out is
/// skipped, any other foreign one is a protocol error that closes the
/// connection. The connection survives one unit's timeouts and
/// exceptions; it is closed when the stream can no longer be trusted (a
/// write or read error, a reply cut off, a header that does not parse)
/// or when every unit on it has timed out since the last reply came, and
/// the next request from any unit opens it again.
///
/// After any error the client lets go of the connection, so the next
/// request resolves its endpoint again (a `mac=` device that moved is
/// found where it is now).
pub struct Client {
    endpoint: String,
    unit: u8,
    timeout: Duration,
    line: Option<Handle>,
    /// The address the endpoint resolved to at the last connection
    /// (`crate::resolve`: a `mac=` endpoint is looked up again each time).
    resolved: Option<std::net::SocketAddrV4>,
    /// When a `modbus=` endpoint's probe last found nothing, and why: the
    /// probe asks every host in the ARP table, so a missing device is
    /// looked for again only after [`PROBE_COOLDOWN`], not every cycle.
    missed: Option<(Instant, String)>,
    /// `None`: the host's ([`crate::resolve::Resolver::default`]).
    resolver: Option<crate::resolve::Resolver>,
}

/// Modbus/TCP's port, when an endpoint names none.
pub const DEFAULT_PORT: u16 = 502;
/// How long a `modbus=` endpoint that was not found waits before the
/// next probe.
pub const PROBE_COOLDOWN: Duration = Duration::from_secs(10);

impl Client {
    #[must_use]
    pub fn new(endpoint: &str, unit: u8, timeout: Duration) -> Self {
        Self {
            endpoint: endpoint.into(),
            unit,
            timeout,
            line: None,
            resolved: None,
            missed: None,
            resolver: None,
        }
    }

    /// The same client, looking identities up with `r` (its ARP table,
    /// sweep and probe, unit included) instead of the host's: how a test
    /// keeps the probe off the network it runs on.
    #[must_use]
    pub fn with_resolver(mut self, r: crate::resolve::Resolver) -> Self {
        self.resolver = Some(r);
        self
    }

    /// Where the unit was found at the last connection.
    #[must_use]
    pub fn resolved(&self) -> Option<std::net::SocketAddrV4> {
        self.resolved
    }

    /// The shared connection to where the endpoint resolves, resolving it
    /// first when the client has none.
    fn attach(&mut self) -> Result<Arc<Line>, Error> {
        if self.line.is_none() {
            let probed = self.endpoint.starts_with("modbus=");
            if probed
                && let Some((at, why)) = &self.missed
                && at.elapsed() < PROBE_COOLDOWN
            {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    why.clone(),
                )));
            }
            // A `modbus=` endpoint asks the unit this client talks to.
            let host;
            let resolver = if let Some(r) = &self.resolver {
                r
            } else {
                let mut r = crate::resolve::Resolver::default();
                if let Some(p) = r.modbus.as_mut() {
                    p.unit = self.unit;
                }
                host = r;
                &host
            };
            let addr = match crate::resolve::socket_addr(&self.endpoint, DEFAULT_PORT, resolver) {
                Ok(a) => a,
                Err(e) => {
                    if probed {
                        self.missed = Some((Instant::now(), e.to_string()));
                    }
                    return Err(Error::Io(e));
                }
            };
            self.missed = None;
            self.line = Some(Handle::to(addr, self.unit));
        }
        Ok(Arc::clone(&self.line.as_ref().expect("set above").line))
    }

    /// Sends one request PDU and returns the reply PDU.
    ///
    /// # Errors
    /// Network failure or timeout, or a reply whose header does not match the
    /// request.
    pub fn transact(&mut self, pdu: &[u8]) -> Result<Vec<u8>, Error> {
        let r = self.exchange(pdu);
        if r.is_err() {
            self.line = None;
        }
        r
    }

    fn exchange(&mut self, pdu: &[u8]) -> Result<Vec<u8>, Error> {
        let line = self.attach()?;
        let _turn = line.turn();
        let mut conn = lock(&line.conn);
        conn.connect(line.addr, self.timeout)?;
        self.resolved = Some(line.addr);
        conn.exchange(self.unit, pdu, self.timeout, &line.units)
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

    /// Read Device Identification (FC 43/14), basic objects.
    ///
    /// # Errors
    /// See [`app::read_device_identification`].
    pub fn read_device_identification(&mut self) -> Result<DeviceIdentification, Error> {
        app::read_device_identification(self)
    }
}

/// Most requests on one connection whose late replies are remembered to
/// be skipped (they timed out); past it the oldest is forgotten.
pub const MAX_ABANDONED: usize = 16;

/// The one connection the box keeps to an address, shared by every
/// [`Client`] that reaches it. Lives while a client holds it.
pub struct Line {
    addr: SocketAddrV4,
    /// Turns on the connection, first come first served: `(next ticket,
    /// ticket being served)`. A plain mutex is not fair, and a dead unit
    /// polled back to back could take the connection again and again
    /// ahead of a live one waiting for it.
    queue: Mutex<(u64, u64)>,
    turned: Condvar,
    conn: Mutex<Conn>,
    /// The units of the clients holding the line, with how many each:
    /// apart from `conn`, so a client letting go never waits out another's
    /// request.
    units: Mutex<BTreeMap<u8, usize>>,
}

/// A client's turn on a [`Line`]; the next in line goes when it drops.
struct Turn<'a>(&'a Line);

impl Line {
    fn turn(&self) -> Turn<'_> {
        let mut q = lock(&self.queue);
        let ticket = q.0;
        q.0 += 1;
        while q.1 != ticket {
            q = self.turned.wait(q).unwrap_or_else(PoisonError::into_inner);
        }
        Turn(self)
    }
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        lock(&self.0.queue).1 += 1;
        self.0.turned.notify_all();
    }
}

/// Every open [`Line`], by address.
static LINES: Mutex<Vec<Weak<Line>>> = Mutex::new(Vec::new());

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Default)]
struct Conn {
    stream: Option<TcpStream>,
    /// The last transaction id sent; unique on the connection at a time.
    tid: u16,
    /// Requests that timed out on this stream, oldest first: their
    /// replies, if they come, are skipped.
    abandoned: VecDeque<u16>,
    /// Units that timed out since the stream last carried a reply.
    unanswered: BTreeSet<u8>,
    /// The last connect that failed: when, how long it took, and the
    /// error. Another unit asking within that long gets the same answer
    /// instead of waiting out a connect of its own, so a gateway that is
    /// gone costs one connect timeout, not one per unit.
    failed: Option<(Instant, Duration, std::io::ErrorKind, String)>,
}

/// A client's hold on a [`Line`], counted under its unit.
struct Handle {
    line: Arc<Line>,
    unit: u8,
}

impl Handle {
    fn to(addr: SocketAddrV4, unit: u8) -> Self {
        let mut lines = lock(&LINES);
        lines.retain(|w| w.strong_count() > 0);
        let line = lines
            .iter()
            .filter_map(Weak::upgrade)
            .find(|l| l.addr == addr)
            .unwrap_or_else(|| {
                let l = Arc::new(Line {
                    addr,
                    queue: Mutex::new((0, 0)),
                    turned: Condvar::new(),
                    conn: Mutex::default(),
                    units: Mutex::default(),
                });
                lines.push(Arc::downgrade(&l));
                l
            });
        *lock(&line.units).entry(unit).or_default() += 1;
        Self { line, unit }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        let mut units = lock(&self.line.units);
        if let Some(n) = units.get_mut(&self.unit) {
            *n -= 1;
            if *n == 0 {
                units.remove(&self.unit);
            }
        }
    }
}

impl Conn {
    fn connect(&mut self, addr: SocketAddrV4, timeout: Duration) -> Result<(), Error> {
        if self.stream.is_some() {
            return Ok(());
        }
        if let Some((at, took, kind, why)) = &self.failed
            && at.elapsed() < *took
        {
            return Err(Error::Io(std::io::Error::new(*kind, why.clone())));
        }
        let started = Instant::now();
        let s = TcpStream::connect_timeout(&SocketAddr::V4(addr), timeout).and_then(|s| {
            s.set_nodelay(true)?;
            Ok(s)
        });
        match s {
            Ok(s) => {
                self.stream = Some(s);
                self.failed = None;
                self.abandoned.clear();
                self.unanswered.clear();
                Ok(())
            }
            Err(e) => {
                self.failed = Some((Instant::now(), started.elapsed(), e.kind(), e.to_string()));
                Err(Error::Io(e))
            }
        }
    }

    /// Closes the stream and fails with `e`.
    fn lose<T>(&mut self, e: Error) -> Result<T, Error> {
        self.stream = None;
        Err(e)
    }

    /// One request and its reply, on a connected stream.
    fn exchange(
        &mut self,
        unit: u8,
        pdu: &[u8],
        timeout: Duration,
        units: &Mutex<BTreeMap<u8, usize>>,
    ) -> Result<Vec<u8>, Error> {
        let mut tid = self.tid.wrapping_add(1);
        while self.abandoned.contains(&tid) {
            tid = tid.wrapping_add(1);
        }
        self.tid = tid;
        let frame = helmsman_modbus::mbap::frame(tid, unit, pdu)?;
        let deadline = Instant::now() + timeout;
        let Some(s) = self.stream.as_mut() else {
            return Err(Error::Io(std::io::ErrorKind::NotConnected.into()));
        };
        if let Err(e) = s
            .set_write_timeout(Some(timeout))
            .and_then(|()| s.write_all(&frame))
        {
            return self.lose(Error::Io(e));
        }
        loop {
            let s = self.stream.as_mut().expect("written to above");
            let mut head = [0u8; MBAP_LEN];
            let mut got = 0;
            match read_counted(s, &mut head, deadline, &mut got) {
                Ok(()) => {}
                // Nothing of a reply yet: the unit may still answer, late.
                // The connection stays for the other units unless every
                // unit on it has gone unanswered.
                Err(Error::Io(e)) if got == 0 && e.kind() == std::io::ErrorKind::TimedOut => {
                    let e = Error::Io(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!("unit {unit}: no reply in {} ms", timeout.as_millis()),
                    ));
                    self.unanswered.insert(unit);
                    if lock(units).keys().all(|u| self.unanswered.contains(u)) {
                        return self.lose(e);
                    }
                    if self.abandoned.len() >= MAX_ABANDONED {
                        // Its reply, should it ever come, is then foreign.
                        self.abandoned.pop_front();
                    }
                    self.abandoned.push_back(tid);
                    return Err(e);
                }
                Err(e) => return self.lose(e),
            }
            let got_tid = u16::from_be_bytes([head[0], head[1]]);
            let late = (got_tid != tid)
                .then(|| self.abandoned.iter().position(|t| *t == got_tid))
                .flatten();
            // Protocol id and length must read whatever the reply is for:
            // the stream is out of step otherwise.
            let len = match helmsman_modbus::mbap::check_head(
                &head,
                if late.is_some() { got_tid } else { tid },
                head[6],
            ) {
                Ok(n) => n,
                Err(e) => return self.lose(e.into()),
            };
            let mut body = vec![0u8; len];
            if let Err(e) = read_counted(s, &mut body, deadline, &mut 0) {
                return self.lose(e);
            }
            self.unanswered.clear();
            if let Some(i) = late {
                // A reply to a request given up on: never taken for ours.
                self.abandoned.remove(i);
                continue;
            }
            if head[6] != unit {
                return Err(Error::Protocol(format!("unit {}, expected {unit}", head[6])));
            }
            return Ok(body);
        }
    }
}

impl Transport for Client {
    fn address(&self) -> Option<String> {
        self.resolved.map(|a| a.to_string())
    }

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

    /// Where the unit was found, once connected (a network transport;
    /// see [`crate::Driver::address`]).
    fn address(&self) -> Option<String> {
        None
    }
}

/// The application layer over any [`Transport`]: build the request PDU,
/// send it, check the reply. A broadcast write succeeds once sent.
pub mod app {
    use super::{
        COIL_OFF, COIL_ON, DeviceIdentification, Error, MAX_ID_REPLIES, READ_DEVICE_ID_BASIC, Transport,
        WRITE_SINGLE_COIL, WRITE_SINGLE_REGISTER, pdu, reply,
    };

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

    /// Read Device Identification, basic objects as a stream (FC 0x2B,
    /// MEI 0x0E, code 01; App §6.21 pp.43-46): the first request from
    /// object 0, each further one from the Next Object Id the server gave,
    /// for as long as it says More Follows. Read-only: nothing on the
    /// device changes. Bounded: at most [`MAX_ID_REPLIES`] replies, each
    /// next object strictly after the one asked for (a server that
    /// restarts at 0 or stands still is refused, not followed), and an
    /// object sent twice is refused.
    ///
    /// # Errors
    /// See [`Transport::request`] and
    /// [`helmsman_modbus::reply::device_id`]; a missing basic object; a
    /// More Follows chain that does not progress or runs too long.
    pub fn read_device_identification(
        t: &mut (impl Transport + ?Sized),
    ) -> Result<DeviceIdentification, Error> {
        let mut object = 0u8;
        let mut found: Vec<(u8, Vec<u8>)> = Vec::new();
        for _ in 0..MAX_ID_REPLIES {
            let req = helmsman_modbus::pdu::read_device_id(READ_DEVICE_ID_BASIC, object);
            let r = answer(t, &req)?;
            let page = helmsman_modbus::reply::device_id(READ_DEVICE_ID_BASIC, &r)?;
            for (id, v) in page.objects {
                if found.iter().any(|(i, _)| *i == id) {
                    return Err(Error::Protocol(format!("object {id:#04x} sent twice")));
                }
                found.push((id, v));
            }
            match page.next {
                None => return DeviceIdentification::from_objects(page.conformity, &found),
                Some(n) if n <= object => {
                    return Err(Error::Protocol(format!(
                        "More Follows from object {n:#04x} after asking from {object:#04x}"
                    )));
                }
                Some(n) => object = n,
            }
        }
        Err(Error::Protocol(format!(
            "More Follows past {MAX_ID_REPLIES} replies"
        )))
    }
}

/// Reads exactly `buf.len()` bytes by `deadline`, however the peer
/// trickles them.
fn read_by(s: &mut TcpStream, buf: &mut [u8], deadline: Instant) -> Result<(), Error> {
    read_counted(s, buf, deadline, &mut 0)
}

/// [`read_by`], counting into `got` the bytes that came (so a caller can
/// tell a reply that never started from one cut off).
fn read_counted(s: &mut TcpStream, buf: &mut [u8], deadline: Instant, got: &mut usize) -> Result<(), Error> {
    *got = 0;
    while *got < buf.len() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "no complete reply in time",
            )));
        }
        s.set_read_timeout(Some(left))?;
        match s.read(&mut buf[*got..]) {
            Ok(0) => {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "closed mid-reply",
                )));
            }
            Ok(n) => *got += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            // A read timeout is WouldBlock on some platforms (macOS), TimedOut
            // on others; the loop's check above says which it was.
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(Error::Io(e)),
        }
    }
    Ok(())
}

/// One connection that must be done by a deadline: the identification
/// probe's transport. Unlike [`Client`], whose timeout is per read, a
/// peer that trickles bytes or keeps saying More Follows cannot stretch
/// it past the deadline.
struct Deadlined {
    stream: TcpStream,
    unit: u8,
    tid: u16,
    deadline: Instant,
}

impl Transport for Deadlined {
    fn request(&mut self, pdu: &[u8]) -> Result<Option<Vec<u8>>, Error> {
        self.tid = self.tid.wrapping_add(1);
        let frame = helmsman_modbus::mbap::frame(self.tid, self.unit, pdu)?;
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "out of time",
            )));
        }
        self.stream.set_write_timeout(Some(left))?;
        self.stream.write_all(&frame)?;
        let mut head = [0u8; MBAP_LEN];
        read_by(&mut self.stream, &mut head, self.deadline)?;
        let len = helmsman_modbus::mbap::check_head(&head, self.tid, self.unit)?;
        let mut body = vec![0u8; len];
        read_by(&mut self.stream, &mut body, self.deadline)?;
        Ok(Some(body))
    }
}

/// Connects to `addr`, asks unit `unit` for its identification
/// ([`app::read_device_identification`]) and hangs up, all within
/// `budget`: connect, every request and every reply count against the
/// one deadline. Read-only.
///
/// # Errors
/// Not reachable or not answering in time, an exception (a device without
/// FC 43/14 answers 0x01, Illegal Function), or a reply that does not read.
pub fn identify(addr: SocketAddr, unit: u8, budget: Duration) -> Result<DeviceIdentification, Error> {
    let deadline = Instant::now() + budget;
    let stream = TcpStream::connect_timeout(&addr, budget.max(Duration::from_millis(1)))?;
    stream.set_nodelay(true)?;
    let mut t = Deadlined {
        stream,
        unit,
        tid: 0,
        deadline,
    };
    app::read_device_identification(&mut t)
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
    fn address(&self) -> Option<String> {
        self.client.address()
    }

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
