//! A device by its profile: `device ID device:NAME ENDPOINT` in a map,
//! signals bound to its channels (`ch:DI-04`), and the box picking the
//! protocol (PROTOCOLS.md step 1, card `device-profiles`).
//!
//! ```text
//! device rio device:moxa-e1213 mac=00-90-e8-7e-af-be
//! in  part_present bool rio ch:DI-04
//! out clamp        bool rio ch:DO-00 safe=false
//! ```
//!
//! `NAME` is the profile `NAME.device` ([`crate::profile`]): the project's
//! own file, else the box's (the box finds it; nothing here reads
//! files). [`ProfileDriver`] wraps the real drivers:
//!
//! - **Which transport.** It opens the profile's transports in the
//!   profile's order and uses the first whose link comes up: a link
//!   error ([`Error::is_link`]) before the first good exchange, or a
//!   transport that will not open in this build, moves it to the next at
//!   once (the E1213 whose one EtherNet/IP connection another controller
//!   owns ends on Modbus TCP). Once one has worked it stays; after a
//!   later link error it starts again from the first, on the next call.
//! - **Points.** `ch:NAME` is the channel `NAME` (case-blind), translated
//!   to the transport's own point. A point error stays a point error.
//! - **The line's params.** `poll_ms` is the box's (how often the link
//!   polls). `DRIVER.KEY=VALUE` sets `KEY` for that one transport,
//!   over the profile's value (`modbus-tcp.unit=2`); `DRIVER.port=N` is
//!   that transport's port on a host endpoint. Anything else is refused:
//!   the rest is the profile's business.
//! - **What is shown.** [`Driver::transport`] names the transport in use,
//!   for a device's details and failure text; the person adding the
//!   device never chooses it.

use std::collections::BTreeMap;

use crate::driver::{Driver, Error, PointInfo};
use crate::model::{Device, Dir, Value};
use crate::profile::{Channel, Profile};

/// How a map names a device by its profile: `device:NAME`.
pub const DRIVER: &str = "device:";
/// How a signal names a channel: `ch:NAME`.
pub const POINT: &str = "ch:";
/// A profile's file name: `NAME.device`.
pub const EXT: &str = ".device";
/// The device line's own param: how often the link polls.
const POLL: &str = "poll_ms";

/// The `NAME` of a `device:NAME` driver name.
#[must_use]
pub fn profile_name(driver: &str) -> Option<&str> {
    driver.strip_prefix(DRIVER)
}

/// The file profile `name` is read from.
#[must_use]
pub fn file_name(name: &str) -> String {
    format!("{name}{EXT}")
}

/// What the one-writer rule compares for a channel: the channel's name,
/// case-blind.
#[must_use]
pub fn output_key(addr: &str) -> String {
    addr.strip_prefix(POINT).unwrap_or(addr).to_ascii_uppercase()
}

/// Opens one transport's driver: the box's opener.
pub type Open = Box<dyn FnMut(&Device) -> Result<Box<dyn Driver + Send>, Error> + Send>;

/// A device reached through whichever of its profile's transports works.
/// See the [module docs](self).
pub struct ProfileDriver {
    profile: Profile,
    /// One device line per transport, in the profile's order.
    lines: Vec<Device>,
    open: Open,
    /// The transport tried now, and its driver once opened.
    index: usize,
    driver: Option<Box<dyn Driver + Send>>,
    /// Whether the transport at `index` has worked since it was opened.
    proven: bool,
}

impl std::fmt::Debug for ProfileDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProfileDriver")
            .field("model", &self.profile.model)
            .field("transport", &self.lines[self.index].driver)
            .field("proven", &self.proven)
            .finish_non_exhaustive()
    }
}

/// `endpoint` with its port set to `port`: a host only, with or without a
/// port of its own.
fn with_port(endpoint: &str, port: &str) -> Result<String, String> {
    if port.parse::<u16>().map_or(true, |p| p == 0) {
        return Err(format!("port {port:?}: 1..=65535"));
    }
    if endpoint.contains('=') {
        return Err(format!(
            "a port needs a host endpoint, not {endpoint:?} (an identity is found on the protocol's own port)"
        ));
    }
    let host = match endpoint.rsplit_once(':') {
        Some((h, p)) if p.bytes().all(|b| b.is_ascii_digit()) && !h.contains(':') => h,
        _ => endpoint,
    };
    Ok(format!("{host}:{port}"))
}

impl ProfileDriver {
    /// The driver for `device` (a `device:NAME` line) on `profile`, its
    /// transports opened by `open` when first needed.
    ///
    /// # Errors
    /// [`Error::Address`] for a param of the line that is not `poll_ms` or
    /// `DRIVER.KEY` of one of the profile's transports, or a bad port.
    pub fn new(profile: Profile, device: &Device, open: Open) -> Result<Self, Error> {
        let mut lines: Vec<Device> = profile
            .transports
            .iter()
            .map(|t| Device {
                id: device.id.clone(),
                driver: t.driver.clone(),
                endpoint: device.endpoint.clone(),
                params: t
                    .params
                    .iter()
                    .filter_map(|p| p.split_once('='))
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect::<BTreeMap<_, _>>(),
            })
            .collect();
        let bad = |why: String| Error::Address(format!("device {}: {why}", device.id));
        for (k, v) in &device.params {
            if k == POLL {
                continue;
            }
            let Some((driver, key)) = k.rsplit_once('.') else {
                return Err(bad(format!(
                    "{k}: a profile's device takes {POLL} and DRIVER.KEY=VALUE for one transport; \
                     the rest is the profile's"
                )));
            };
            let Some(line) = lines.iter_mut().find(|l| l.driver == driver) else {
                return Err(bad(format!(
                    "{k}: {driver} is not one of the profile's transports ({})",
                    profile
                        .transports
                        .iter()
                        .map(|t| t.driver.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            };
            if key == "port" {
                line.endpoint = with_port(&line.endpoint, v).map_err(|why| bad(format!("{k}: {why}")))?;
            } else if key.is_empty() {
                return Err(bad(format!("{k}: expected {driver}.KEY")));
            } else {
                line.params.insert(key.to_string(), v.clone());
            }
        }
        Ok(Self {
            profile,
            lines,
            open,
            index: 0,
            driver: None,
            proven: false,
        })
    }

    /// The profile it runs on.
    #[must_use]
    pub const fn profile(&self) -> &Profile {
        &self.profile
    }

    /// The device line of each transport, as it is opened.
    #[must_use]
    pub fn lines(&self) -> &[Device] {
        &self.lines
    }

    fn channel(&self, addr: &str) -> Result<&Channel, Error> {
        let name = addr.strip_prefix(POINT).ok_or_else(|| {
            Error::Address(format!(
                "{addr}: a profile's device is read by channel, like {POINT}{}",
                self.profile.channels.first().map_or("DI-00", |c| c.name.as_str())
            ))
        })?;
        self.profile
            .channels
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| {
                Error::Address(format!(
                    "{addr}: the {} {} has no channel {name}",
                    self.profile.vendor, self.profile.model
                ))
            })
    }

    /// The points of `addrs` on the transport at `index`.
    fn points(&self, addrs: &[&str]) -> Result<Vec<String>, Error> {
        let driver = &self.lines[self.index].driver;
        addrs
            .iter()
            .map(|a| {
                let c = self.channel(a)?;
                c.points
                    .get(driver)
                    .cloned()
                    .ok_or_else(|| Error::Address(format!("{a}: no point over {driver}")))
            })
            .collect()
    }

    /// Back to the first transport, nothing open.
    fn restart(&mut self) {
        self.index = 0;
        self.driver = None;
        self.proven = false;
    }

    /// Runs `op` on the transport in use, moving down the profile's list
    /// on a link error until one has worked; see the [module docs](self).
    fn with<T>(
        &mut self,
        addrs: &[&str],
        mut op: impl FnMut(&mut dyn Driver, &[&str]) -> Result<T, Error>,
    ) -> Result<T, Error> {
        loop {
            let points = self.points(addrs)?;
            let last = self.index + 1 == self.lines.len();
            if self.driver.is_none() {
                match (self.open)(&self.lines[self.index]) {
                    Ok(d) => self.driver = Some(d),
                    Err(_) if !last => {
                        self.index += 1;
                        continue;
                    }
                    Err(e) => {
                        self.restart();
                        return Err(e);
                    }
                }
            }
            let refs: Vec<&str> = points.iter().map(String::as_str).collect();
            let driver = self.driver.as_deref_mut().expect("opened above");
            match op(driver, &refs) {
                Ok(v) => {
                    self.proven = true;
                    return Ok(v);
                }
                Err(e) if e.is_link() && !self.proven && !last => {
                    self.driver = None;
                    self.index += 1;
                }
                Err(e) if e.is_link() => {
                    self.restart();
                    return Err(e);
                }
                Err(e) => return Err(e),
            }
        }
    }
}

impl Driver for ProfileDriver {
    fn parse(&self, addr: &str) -> Result<PointInfo, Error> {
        let c = self.channel(addr)?;
        Ok(PointInfo {
            ty: c.ty,
            writable: c.dir == Dir::Out,
        })
    }

    fn read(&mut self, addrs: &[&str]) -> Result<Vec<Value>, Error> {
        self.with(addrs, |d, points| d.read(points))
    }

    fn write(&mut self, addr: &str, value: Value) -> Result<(), Error> {
        if self.channel(addr)?.dir != Dir::Out {
            return Err(Error::ReadOnly(addr.to_string()));
        }
        self.with(&[addr], |d, points| d.write(points[0], value))
    }

    fn address(&self) -> Option<String> {
        self.driver.as_ref().and_then(|d| d.address())
    }

    fn transport(&self) -> Option<String> {
        self.proven.then(|| self.lines[self.index].driver.clone())
    }
}
