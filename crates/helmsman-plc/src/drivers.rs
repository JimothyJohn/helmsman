//! The WASM drivers a map loads with, and what a map that names one as
//! `wasm:NAME` needs of them (`wasm-driver-map`): its points parsed by the
//! guest, its link opened on the guest, and a digest so a reload can tell
//! that a driver's bytes changed under an unchanged map line.
//!
//! A driver is a file, `drivers/NAME.wasm` on the box or in the project
//! (`wasm-driver-home`; `crate::driver_dir` finds and checks them when a
//! map loads). Nothing here reads files: a [`Drivers`] set is what was
//! found, and a name that is not in it is refused wherever a map is
//! checked.
//!
//! Without the `wasm` feature (the canvas) no driver loads: a map that
//! names one is refused as naming a driver this build does not have.
//!
//! So is `enip-io` in a helmsman-plc built without its `enip-io` feature
//! (in the default build since the E1213 bench passed; [`not_in_build`]).

use std::collections::BTreeMap;
use std::time::Duration;

use helmsman_io::driver::{parse_point_on, wasm_name};
use helmsman_io::profile::Profile;
use helmsman_io::profile_driver::{Open, ProfileDriver, file_name as profile_file, profile_name};
use helmsman_io::{Device, Driver, Error, IoMap, PointInfo};
use sha2::{Digest, Sha256};

/// The driver files a map loads with: each WASM driver's
/// `helmsman-driver/v1` binary by its name (`wasm:NAME`), and each device
/// profile's text by its file name (`device:NAME` as `NAME.device`,
/// `helmsman_io::profile_driver`), so neither can stand for the other.
pub type Drivers = BTreeMap<String, Vec<u8>>;

/// Most driver files in the box's driver folder.
pub const MAX_DRIVERS: usize = 64;

/// The binary of the driver `device` names by `wasm:NAME`: `None` for a
/// built-in driver, an error for a name that is not in `drivers`.
///
/// # Errors
/// [`Error::UnknownDriver`], naming the driver and the file it would be.
#[must_use]
pub fn guest<'a>(drivers: &'a Drivers, device: &Device) -> Option<Result<&'a [u8], Error>> {
    let name = wasm_name(&device.driver)?;
    Some(drivers.get(name).map(Vec::as_slice).ok_or_else(|| {
        Error::UnknownDriver(format!(
            "{}: no such WASM driver (no file drivers/{}.wasm in the project or on the box)",
            device.driver,
            name.chars()
                .take(64)
                .flat_map(char::escape_debug)
                .collect::<String>()
        ))
    }))
}

/// The profile a `device:NAME` device runs on: `None` for any other
/// device, an error for a profile that is not in `drivers` (or no longer
/// parses).
///
/// # Errors
/// [`Error::UnknownDriver`], naming the file it would be.
#[must_use]
pub fn profile(drivers: &Drivers, device: &Device) -> Option<Result<Profile, Error>> {
    let name = profile_name(&device.driver)?;
    let missing = || {
        Error::UnknownDriver(format!(
            "{}: no profile {} in the project or on the box",
            device.driver,
            profile_file(name)
                .chars()
                .take(64)
                .flat_map(char::escape_debug)
                .collect::<String>()
        ))
    };
    let Some(bytes) = drivers.get(&profile_file(name)) else {
        return Some(Err(missing()));
    };
    // Checked when it was found (`crate::driver_dir`); said again here
    // for a set built some other way.
    let parsed = std::str::from_utf8(bytes)
        .map_err(|_| "not UTF-8 text".to_string())
        .and_then(|t| {
            Profile::parse(t).map_err(|errs| {
                errs.iter()
                    .map(|(n, e)| {
                        if *n == 0 {
                            e.clone()
                        } else {
                            format!("line {n}: {e}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            })
        });
    Some(parsed.map_err(|why| {
        Error::Driver(format!(
            "{}: {} does not read as a profile: {why}",
            device.driver,
            profile_file(name)
        ))
    }))
}

/// The driver file `device` runs on: a `wasm:` device's binary or a
/// `device:` device's profile, when it is in `drivers`.
fn file<'a>(drivers: &'a Drivers, device: &Device) -> Option<&'a [u8]> {
    if let Some(name) = profile_name(&device.driver) {
        return drivers.get(&profile_file(name)).map(Vec::as_slice);
    }
    guest(drivers, device)?.ok()
}

/// SHA-256 of the driver file `device` runs on (a `wasm:` device's
/// binary, a `device:` device's profile) when it is in `drivers`: a
/// changed file is another link.
#[must_use]
pub fn digest(drivers: &Drivers, device: &Device) -> Option<[u8; 32]> {
    file(drivers, device).map(|b| Sha256::digest(b).into())
}

/// Opens a `device:` device on its profile, each of the profile's
/// transports through `base` (the box's opener) unless this build leaves
/// that driver out; `None` for any other device.
///
/// # Errors
/// The profile is not in `drivers`, or the device line's params are not
/// a profile device's.
#[must_use]
pub fn open_profiled(
    drivers: &Drivers,
    device: &Device,
    base: impl Fn(&Device) -> Result<Box<dyn Driver + Send>, Error> + Send + 'static,
    poll: Option<Duration>,
) -> Option<Result<Box<dyn Driver + Send>, Error>> {
    let profile = match profile(drivers, device)? {
        Ok(p) => p,
        Err(e) => return Some(Err(e)),
    };
    let open: Open = Box::new(move |d: &Device| match not_in_build(d) {
        Some(e) => Err(e),
        None => base(d),
    });
    Some(ProfileDriver::new(profile, device, open).map(|mut d| {
        if let Some(poll) = poll {
            d.pace(poll);
        }
        Box::new(d) as Box<dyn Driver + Send>
    }))
}

/// The rate limits of a `device:` device's preferred transport
/// (`cycle_ms=A..B` on the profile's first transport line); `None` for
/// any other device, a profile that is not there, or one with no limits.
#[must_use]
pub fn limits(drivers: &Drivers, device: &Device) -> Option<(Duration, Duration)> {
    profile(drivers, device)?
        .ok()?
        .transports
        .first()
        .and_then(helmsman_io::profile::Transport::limits)
}

/// Opens `device`'s link driver on its guest, for a `wasm:` device;
/// `None` for a built-in driver (the box's opener's).
///
/// # Errors
/// The driver is not in `drivers`, does not load, or its `configure`
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
/// (`helmsman_io::wasm::sandbox`). What a driver file is held to.
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
            "{}: not in this build (helmsman-plc was built without its `enip-io` feature, \
             the EtherNet/IP scanner; use modbus-tcp, or a default build)",
            device.driver
        ))
    })
}

/// Every device of `map` that names a WASM driver not in `drivers`,
/// or a driver this build leaves out ([`not_in_build`]), one
/// line each, whether or not a signal uses it.
#[must_use]
pub fn unknown(map: &IoMap, drivers: &Drivers) -> Vec<String> {
    map.devices
        .iter()
        .filter_map(|d| {
            let profiled = profile(drivers, d).and_then(Result::err);
            match (not_in_build(d), guest(drivers, d), profiled) {
                (Some(e), _, _) | (None, Some(Err(e)), _) | (None, _, Some(e)) => {
                    Some(format!("device {}: {e}", d.id))
                }
                _ => None,
            }
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
    /// As the driver's parse; for a `wasm:` device also a missing
    /// driver, or one that does not load for this device.
    pub fn parse(&mut self, device: &Device, addr: &str) -> Result<PointInfo, Error> {
        if let Some(e) = not_in_build(device) {
            return Err(e);
        }
        if profile_name(&device.driver).is_some() {
            // Parsed on the profile alone: nothing is opened.
            let refuse = |d: &Device| -> Result<Box<dyn Driver + Send>, Error> {
                Err(Error::UnknownDriver(format!(
                    "{}: not opened to check a map",
                    d.driver
                )))
            };
            let d = self
                .loaded
                .entry(device.id.clone())
                .or_insert_with(|| {
                    open_profiled(self.drivers, device, refuse, None)
                        .expect("a device: device")
                        .map_err(|e| e.to_string())
                })
                .as_ref()
                .map_err(|e| Error::UnknownDriver(e.clone()))?;
            return d.parse(addr);
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
