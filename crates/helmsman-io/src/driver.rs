//! The driver contract. Points are addressed by strings in the driver's own
//! syntax (`hr:100:f32` for Modbus), so the same contract works across a WASM
//! sandbox boundary where only bytes and strings cross.

use std::fmt;
use std::io;

use crate::model::{Device, Ty, Value};

/// What a point is, once the driver has parsed its address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PointInfo {
    pub ty: Ty,
    pub writable: bool,
}

#[derive(Debug)]
pub enum Error {
    /// The address does not parse in this driver's syntax.
    Address(String),
    /// A value of the wrong type for the point.
    Type { want: Ty, got: Ty },
    /// Network trouble, including timeouts.
    Io(io::Error),
    /// The device answered with something that is not a valid reply.
    Protocol(String),
    /// The device answered with a protocol-level exception (Modbus: the
    /// exception code).
    Exception(u8),
    /// Writing a read-only point.
    ReadOnly(String),
    /// The driver or device refused this one point or its value (a value
    /// its format can't hold, a tag the controller doesn't know, a reply of
    /// another type than declared); the connection itself is fine.
    Point(String),
    /// The driver does not know this device's driver name.
    UnknownDriver(String),
    /// The driver itself failed: a WASM driver ([`crate::wasm`]) trapped
    /// (out of fuel, over its memory cap, a bug) or broke its contract.
    /// Not a link error: the point it was reading or writing goes bad,
    /// the device's other points carry on.
    Driver(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Address(a) => write!(f, "bad address {a:?}"),
            Self::Type { want, got } => write!(f, "type: point is {want:?}, value is {got:?}"),
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Protocol(m) => write!(f, "protocol: {m}"),
            Self::Exception(c) => write!(f, "device exception {c:#04x}"),
            Self::Point(m) => write!(f, "point: {m}"),
            Self::ReadOnly(a) => write!(f, "{a} is read-only"),
            Self::UnknownDriver(d) => write!(f, "unknown driver {d:?}"),
            Self::Driver(m) => write!(f, "driver: {m}"),
        }
    }
}

impl Error {
    /// Whether the error means the link to the device is broken (I/O
    /// trouble, a reply that isn't valid), as opposed to one point being
    /// refused. After a link error nothing the device holds can be trusted.
    #[must_use]
    pub const fn is_link(&self) -> bool {
        matches!(self, Self::Io(_) | Self::Protocol(_))
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// A connection to one device.
pub trait Driver {
    /// Parses an address without touching the device.
    ///
    /// # Errors
    /// The address is not valid in this driver's syntax.
    fn parse(&self, addr: &str) -> Result<PointInfo, Error>;

    /// Reads points, in order. A driver may batch; the caller can't tell.
    ///
    /// # Errors
    /// Any point fails to parse or read; nothing partial is returned.
    fn read(&mut self, addrs: &[&str]) -> Result<Vec<Value>, Error>;

    /// Writes one point.
    ///
    /// # Errors
    /// Bad address, read-only point, wrong value type, or the device refused.
    fn write(&mut self, addr: &str, value: Value) -> Result<(), Error>;
}

/// How a map names a driver compiled to WebAssembly: `wasm:NAME`, NAME
/// being a driver the box has approved. No built-in driver knows these;
/// a box that has the driver parses and opens it through the guest
/// (`helmsman_plc::drivers`).
pub const WASM: &str = "wasm:";

/// The NAME of a `wasm:NAME` driver name.
#[must_use]
pub fn wasm_name(driver: &str) -> Option<&str> {
    driver.strip_prefix(WASM)
}

/// Parses an address for a driver by name, without a connection: what
/// [`crate::IoMap::validate`] uses.
///
/// # Errors
/// Unknown driver, or the address does not parse.
pub fn parse_point(driver: &str, addr: &str) -> Result<PointInfo, Error> {
    match driver {
        crate::modbus::DRIVER | crate::rtu::DRIVER => {
            crate::modbus::Point::parse(addr).map(crate::modbus::Point::info)
        }
        crate::logix::DRIVER => crate::logix::Point::parse(addr).map(|p| p.info()),
        crate::enip::DRIVER => crate::enip::Point::parse(addr).map(|p| p.info()),
        other if wasm_name(other).is_some() => Err(Error::UnknownDriver(format!(
            "{other}: not a WASM driver this box has approved"
        ))),
        other => Err(Error::UnknownDriver(other.into())),
    }
}

/// As [`parse_point`], knowing the device: a driver whose points depend on
/// the device's params (`enip-io`: assembly sizes, channel counts, an
/// input-only connection) checks them here.
///
/// # Errors
/// As [`parse_point`], plus params that don't parse and points the
/// device can't have.
pub fn parse_point_on(device: &Device, addr: &str) -> Result<PointInfo, Error> {
    match device.driver.as_str() {
        crate::enip::DRIVER => crate::enip::parse_on(&device.params, addr),
        driver => parse_point(driver, addr),
    }
}

/// What the one-writer rule compares for an output point: the point
/// itself, or for `enip-io` each output bit it drives (so `do:0` and
/// `out:0.0` are the same writer).
#[must_use]
pub fn output_keys(driver: &str, addr: &str) -> Vec<String> {
    match (driver, crate::enip::Point::parse(addr)) {
        (crate::enip::DRIVER, Ok(p)) => p.out_bits(),
        _ => vec![addr.to_string()],
    }
}

/// Opens a driver for `device` by its driver name. Network drivers connect
/// lazily, on the first read or write, and reconnect after an error; so
/// does a serial port.
///
/// # Errors
/// Unknown driver, or a device param that does not parse.
pub fn open(device: &Device) -> Result<Box<dyn Driver + Send>, Error> {
    open_on(device, None)
}

/// [`open`] on a box: `enip-io` links consume on `io` (the box's one Class 1
/// I/O endpoint, shared with its adapter) unless a device names an
/// `io_port` of its own.
///
/// # Errors
/// As [`open`].
pub fn open_on(
    device: &Device,
    io: Option<&helmsman_proto::endpoint::IoEndpoint>,
) -> Result<Box<dyn Driver + Send>, Error> {
    #[cfg(not(feature = "enip"))]
    let _ = io;
    match device.driver.as_str() {
        crate::modbus::DRIVER => Ok(Box::new(crate::modbus::ModbusDriver::from_params(
            &device.endpoint,
            &device.params,
        )?)),
        crate::rtu::DRIVER => crate::rtu::open(&device.endpoint, &device.params),
        crate::logix::DRIVER => Ok(Box::new(crate::logix::LogixDriver::from_params(
            &device.endpoint,
            &device.params,
        )?)),
        #[cfg(feature = "enip")]
        crate::enip::DRIVER => Ok(Box::new(
            crate::enip::EnipDriver::from_params(&device.endpoint, &device.params)?.on_endpoint(io),
        )),
        #[cfg(not(feature = "enip"))]
        crate::enip::DRIVER => Err(Error::UnknownDriver(format!(
            "{}: this build has no enip-io (helmsman-io's `enip` feature)",
            crate::enip::DRIVER
        ))),
        other => Err(Error::UnknownDriver(other.into())),
    }
}
