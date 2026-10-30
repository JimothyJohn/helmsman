//! The driver contract. Points are addressed by strings in the driver's own
//! syntax (`hr:100:f32` for Modbus), so the same contract works across a WASM
//! sandbox boundary where only bytes and strings cross.

use std::fmt;
use std::io;

use crate::model::{Ty, Value};

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
    /// The driver does not know this device's driver name.
    UnknownDriver(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Address(a) => write!(f, "bad address {a:?}"),
            Self::Type { want, got } => write!(f, "type: point is {want:?}, value is {got:?}"),
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Protocol(m) => write!(f, "protocol: {m}"),
            Self::Exception(c) => write!(f, "device exception {c:#04x}"),
            Self::ReadOnly(a) => write!(f, "{a} is read-only"),
            Self::UnknownDriver(d) => write!(f, "unknown driver {d:?}"),
        }
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

/// Parses an address for a driver by name, without a connection: what
/// [`crate::IoMap::validate`] uses.
///
/// # Errors
/// Unknown driver, or the address does not parse.
pub fn parse_point(driver: &str, addr: &str) -> Result<PointInfo, Error> {
    match driver {
        crate::modbus::DRIVER => crate::modbus::Point::parse(addr).map(crate::modbus::Point::info),
        other => Err(Error::UnknownDriver(other.into())),
    }
}
