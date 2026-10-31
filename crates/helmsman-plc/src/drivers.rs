//! The WASM drivers a box has approved, and what a map that names one as
//! `wasm:NAME` needs of them (`wasm-driver-map`): its points parsed by the
//! guest, its link opened on the guest, and a digest so a reload can tell
//! that a driver's bytes changed under an unchanged map line.
//!
//! A driver is a blob in the change set (`store::ChangeSet::drivers`,
//! a `wasm-driver` tree line), proposed and approved like a map change
//! (output-affecting: it reaches a device). Nothing here decides whether
//! one is approved: the change set the box runs is the approved set, and
//! a name that is not in it is refused wherever a map is checked.
//!
//! Without the `wasm` feature (the canvas) no driver loads: a map that
//! names one is refused as naming a driver this build does not have.
//!
//! So is `enip-io` without helmsman-plc's `enip-io` feature, which the box
//! build leaves out until the E1213 bench passes ([`not_in_build`]).

use std::collections::BTreeMap;

use helmsman_io::driver::{parse_point_on, wasm_name};
use helmsman_io::{Device, Driver, Error, IoMap, PointInfo};
use sha2::{Digest, Sha256};

/// Approved WASM drivers by name: each one's `helmsman-driver/v1` binary.
pub type Drivers = BTreeMap<String, Vec<u8>>;

/// Most drivers one change set holds.
pub const MAX_DRIVERS: usize = 64;

/// The binary of the approved driver `device` names by `wasm:NAME`:
/// `None` for a built-in driver, an error for a name the box has not
/// approved.
///
/// # Errors
/// [`Error::UnknownDriver`], naming the driver.
#[must_use]
pub fn guest<'a>(drivers: &'a Drivers, device: &Device) -> Option<Result<&'a [u8], Error>> {
    let name = wasm_name(&device.driver)?;
    Some(drivers.get(name).map(Vec::as_slice).ok_or_else(|| {
        Error::UnknownDriver(format!(
            "{}: not a WASM driver this box has approved",
            device.driver
        ))
    }))
}

/// SHA-256 of the driver binary `device` runs on, for a `wasm:` device
/// whose driver is approved.
#[must_use]
pub fn digest(drivers: &Drivers, device: &Device) -> Option<[u8; 32]> {
    match guest(drivers, device)? {
        Ok(b) => Some(Sha256::digest(b).into()),
        Err(_) => None,
    }
}

/// Opens `device`'s link driver on its guest, for a `wasm:` device;
/// `None` for a built-in driver (the box's opener's).
///
/// # Errors
/// The driver is not approved, does not load, or its `configure`
/// refuses the device's params.
#[must_use]
pub fn open(drivers: &Drivers, device: &Device) -> Option<Result<Box<dyn Driver + Send>, Error>> {
    let bytes = match guest(drivers, device)? {
        Ok(b) => b,
        Err(e) => return Some(Err(e)),
    };
    Some(load(bytes, device).map(|d| Box::new(d) as Box<dyn Driver + Send>))
}

#[cfg(feature = "wasm")]
fn load(bytes: &[u8], device: &Device) -> Result<helmsman_io::wasm::WasmDriver, Error> {
    helmsman_io::wasm::WasmDriver::load(bytes, device, helmsman_io::wasm::Limits::default())
}

#[cfg(not(feature = "wasm"))]
fn load(_: &[u8], device: &Device) -> Result<NoDriver, Error> {
    Err(Error::UnknownDriver(format!(
        "{}: this build has no WASM drivers (helmsman-plc's `wasm` feature)",
        device.driver
    )))
}

/// Never built: what [`load`] would return in a build without WASM.
#[cfg(not(feature = "wasm"))]
enum NoDriver {}

#[cfg(not(feature = "wasm"))]
impl Driver for NoDriver {
    fn parse(&self, _: &str) -> Result<PointInfo, Error> {
        match *self {}
    }
    fn read(&mut self, _: &[&str]) -> Result<Vec<helmsman_io::Value>, Error> {
        match *self {}
    }
    fn write(&mut self, _: &str, _: helmsman_io::Value) -> Result<(), Error> {
        match *self {}
    }
}

/// Whether `bytes` would run as a driver: it exports `helmsman-driver/v1`,
/// imports nothing else, and an instance starts under the default limits
/// (`helmsman_io::wasm::sandbox`). What a driver proposal is held to.
///
/// # Errors
/// Every problem found.
#[cfg(feature = "wasm")]
pub fn check(bytes: &[u8]) -> Result<(), Vec<String>> {
    helmsman_io::wasm::sandbox(bytes, helmsman_io::wasm::Limits::default())
}

/// A built-in driver that `device` names but this build leaves out:
/// `enip-io` (the EtherNet/IP Class 1 scanner) without helmsman-plc's
/// `enip-io` feature. `None` for every other driver.
#[must_use]
pub fn not_in_build(device: &Device) -> Option<Error> {
    (device.driver == helmsman_io::enip::DRIVER && !cfg!(feature = "enip-io")).then(|| {
        Error::UnknownDriver(format!(
            "{}: not in this build (the EtherNet/IP scanner is out of the box until \
             the E1213 bench passes; use modbus-tcp, or build helmsman-plc with `enip-io`)",
            device.driver
        ))
    })
}

/// Every device of `map` that names a WASM driver the box has not
/// approved, or a driver this build leaves out ([`not_in_build`]), one
/// line each, whether or not a signal uses it.
#[must_use]
pub fn unknown(map: &IoMap, drivers: &Drivers) -> Vec<String> {
    map.devices
        .iter()
        .filter_map(|d| match (not_in_build(d), guest(drivers, d)) {
            (Some(e), _) | (None, Some(Err(e))) => Some(format!("device {}: {e}", d.id)),
            _ => None,
        })
        .collect()
}

/// Parses a map's points for its validation ([`IoMap::validate_with`]):
/// built-in drivers as ever, a `wasm:` device's points by its guest,
/// loaded once per device with that device's params.
pub struct Points<'a> {
    drivers: &'a Drivers,
    loaded: BTreeMap<String, Result<Box<dyn Driver + Send>, String>>,
}

impl<'a> Points<'a> {
    #[must_use]
    pub fn new(drivers: &'a Drivers) -> Self {
        Self {
            drivers,
            loaded: BTreeMap::new(),
        }
    }

    /// The point `addr` on `device`.
    ///
    /// # Errors
    /// As the driver's parse; for a `wasm:` device also an unapproved
    /// driver, or one that does not load for this device.
    pub fn parse(&mut self, device: &Device, addr: &str) -> Result<PointInfo, Error> {
        if let Some(e) = not_in_build(device) {
            return Err(e);
        }
        let Some(bytes) = guest(self.drivers, device) else {
            return parse_point_on(device, addr);
        };
        let bytes = bytes?;
        let d = self
            .loaded
            .entry(device.id.clone())
            .or_insert_with(|| {
                load(bytes, device)
                    .map(|d| Box::new(d) as Box<dyn Driver + Send>)
                    .map_err(|e| e.to_string())
            })
            .as_ref()
            .map_err(|e| Error::Driver(e.clone()))?;
        d.parse(addr)
    }
}
