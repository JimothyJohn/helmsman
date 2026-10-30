//! `logix-tag`: signals read by name from a Logix controller the box only
//! observes. Read-only: every point parses as not writable, so
//! [`crate::IoMap::validate`] refuses an `out` bound to one, and
//! [`Driver::write`] answers [`Error::ReadOnly`] without touching the
//! network. Underneath is `helmsman_proto::logix::TagReader`, which has no
//! way to send anything but read services.
//!
//! A point is `TAG@TYPE`: a Logix tag path (`Program:Main.Temp`,
//! `Recipe[3].Speed`, `Local:1:I.Data`, `Status.5` for one bit of an
//! integer) and the Logix type the tag has on the controller (`bool`,
//! `sint`, `int`, `dint`, `lint`, `usint`, `uint`, `udint`, `ulint`,
//! `real`, `lreal`, `byte`, `word`, `dword`, `lword`; a bit is `@bool`).
//! The type is checked against every reply: a tag that turns out to be a
//! different type is an error, never a reinterpretation. BOOL is a `bool`
//! signal, REAL and LREAL `real`, the rest `int`.
//!
//! ```text
//! device plc logix-tag 192.168.3.20:44818 timeout_ms=500 slot=0
//! in  temp  real plc Program:Main.Temp@real
//! in  count int  plc Count@dint
//! in  fault bool plc Status.5@bool
//! ```
//!
//! Device params: `timeout_ms` (default 1000) and `slot` (route through
//! the backplane to the controller in that slot; leave out to talk to the
//! Ethernet port's own controller). The endpoint's port defaults to 44818.

use std::collections::BTreeMap;
use std::time::Duration;

use helmsman_proto::logix::{self, AtomicType, DataType, TagData, TagPath, TagReader};
use helmsman_proto::session;

use crate::driver::{Driver, Error, PointInfo};
use crate::model::{Ty, Value};

pub const DRIVER: &str = "logix-tag";
/// The encapsulation port, used when the endpoint names none.
pub const DEFAULT_PORT: u16 = 44818;

/// A parsed `TAG@TYPE` address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Point {
    pub path: TagPath,
    /// The tag's type on the controller (for a bit, `Bool`).
    pub ty: AtomicType,
}

impl Point {
    /// # Errors
    /// No `@TYPE`, an unknown type, a bad tag path, or a bit suffix on
    /// anything but `@bool`.
    pub fn parse(addr: &str) -> Result<Self, Error> {
        let bad = |why: &str| Error::Address(format!("{addr:?}: {why}"));
        let (path, ty) = addr
            .rsplit_once('@')
            .ok_or_else(|| bad("want TAG@TYPE, e.g. Program:Main.Temp@real"))?;
        let ty = AtomicType::from_name(ty).ok_or_else(|| bad("unknown Logix type"))?;
        let path = TagPath::parse(path).map_err(|e| bad(&e.to_string()))?;
        if path.bit().is_some() && ty != AtomicType::Bool {
            return Err(bad("a bit (TAG.N) is @bool"));
        }
        Ok(Self { path, ty })
    }

    #[must_use]
    pub const fn signal_ty(&self) -> Ty {
        match self.ty {
            AtomicType::Bool => Ty::Bool,
            AtomicType::Real | AtomicType::Lreal => Ty::Real,
            _ => Ty::Int,
        }
    }

    #[must_use]
    pub const fn info(&self) -> PointInfo {
        PointInfo {
            ty: self.signal_ty(),
            writable: false,
        }
    }

    /// The signal value in a read reply, checking the controller's type.
    ///
    /// # Errors
    /// A type other than the one declared, a bit past the integer's
    /// width, an integer outside `i64`, or a reply that does not decode.
    pub fn decode(&self, data: &TagData) -> Result<Value, Error> {
        let mismatch = || {
            Error::Point(format!(
                "{} is {} on the controller, the map says {}",
                self.path, data.ty, self.ty
            ))
        };
        let value = data
            .value()
            .map_err(|e| Error::Protocol(format!("{}: {e}", self.path)))?;
        if let Some(bit) = self.path.bit() {
            if !matches!(data.ty, DataType::Atomic(t) if t.is_integer()) {
                return Err(mismatch());
            }
            return value
                .bit(bit)
                .map(Value::Bool)
                .ok_or_else(|| Error::Point(format!("{}: bit past the end of a {}", self.path, data.ty)));
        }
        if data.ty != DataType::Atomic(self.ty) {
            return Err(mismatch());
        }
        Ok(match value {
            logix::Value::Bool(b) => Value::Bool(b),
            logix::Value::Real(x) => Value::Real(f64::from(x)),
            logix::Value::Lreal(x) => Value::Real(x),
            v => {
                let n = v.as_integer().ok_or_else(mismatch)?;
                Value::Int(
                    i64::try_from(n)
                        .map_err(|_| Error::Point(format!("{}: {n} does not fit a signal int", self.path)))?,
                )
            }
        })
    }
}

/// A read-only connection to one controller, opened on first use and
/// dropped after any transport or protocol error.
#[derive(Debug)]
pub struct LogixDriver {
    endpoint: String,
    timeout: Duration,
    slot: Option<u8>,
    reader: Option<TagReader>,
}

fn transport(e: logix::Error) -> Error {
    match e {
        logix::Error::Session(session::Error::Io(io)) => Error::Io(io),
        other => Error::Protocol(other.to_string()),
    }
}

impl LogixDriver {
    /// From device params: `timeout_ms` (default 1000), `slot` (optional).
    ///
    /// # Errors
    /// A param that does not parse.
    pub fn from_params(endpoint: &str, params: &BTreeMap<String, String>) -> Result<Self, Error> {
        let ms = params
            .get("timeout_ms")
            .map_or(Ok(1000), |t| t.parse::<u64>())
            .map_err(|_| Error::Address("timeout_ms".into()))?;
        let slot = params
            .get("slot")
            .map(|s| s.parse::<u8>())
            .transpose()
            .map_err(|_| Error::Address("slot".into()))?;
        let endpoint = if endpoint.contains(':') {
            endpoint.to_string()
        } else {
            format!("{endpoint}:{DEFAULT_PORT}")
        };
        Ok(Self {
            endpoint,
            timeout: Duration::from_millis(ms.max(1)),
            slot,
            reader: None,
        })
    }

    fn reader(&mut self) -> Result<&mut TagReader, Error> {
        if self.reader.is_none() {
            let r = TagReader::connect(self.endpoint.as_str(), self.timeout).map_err(transport)?;
            self.reader = Some(match self.slot {
                Some(s) => r.with_slot(s),
                None => r,
            });
        }
        Ok(self.reader.as_mut().expect("set above"))
    }
}

impl Driver for LogixDriver {
    fn parse(&self, addr: &str) -> Result<PointInfo, Error> {
        Point::parse(addr).map(|p| p.info())
    }

    fn read(&mut self, addrs: &[&str]) -> Result<Vec<Value>, Error> {
        let points: Vec<Point> = addrs.iter().map(|a| Point::parse(a)).collect::<Result<_, _>>()?;
        if points.is_empty() {
            return Ok(Vec::new());
        }
        let items: Vec<(TagPath, u16)> = points.iter().map(|p| (p.path.without_bit(), 1)).collect();
        let replies = match self.reader()?.read_tags(&items) {
            Ok(r) => r,
            Err(e) => {
                self.reader = None;
                return Err(transport(e));
            }
        };
        let mut out = Vec::with_capacity(points.len());
        for (p, reply) in points.iter().zip(replies) {
            match reply {
                Ok(data) => out.push(p.decode(&data)?),
                // The controller refused this tag; the session is fine.
                Err(e @ logix::Error::Cip { .. }) => return Err(Error::Point(format!("{}: {e}", p.path))),
                Err(e) => {
                    self.reader = None;
                    return Err(Error::Protocol(format!("{}: {e}", p.path)));
                }
            }
        }
        Ok(out)
    }

    fn write(&mut self, addr: &str, _value: Value) -> Result<(), Error> {
        Point::parse(addr)?;
        Err(Error::ReadOnly(addr.into()))
    }
}
