//! A device profile: what a device is and what its channels are called,
//! with the address of each channel on every protocol the device speaks.
//!
//! The point of it (TODO.md `device-profiles`, Nick 2026-11-11): the
//! person adding a device names it and uses its channels; which protocol
//! carries them, at which address, in which byte order, is the profile's
//! business. One file per device model, written once (by us, or by the
//! agent from the vendor's file) and copied into the project that uses
//! the device.
//!
//! ```text
//! helmsman-device 1
//! vendor Moxa
//! model  ioLogik E1213
//! kind   remote-io
//! # how the box reaches it, best first
//! transport enip-io    profile=moxa-e1213 rpi_ms=100
//! transport modbus-tcp unit=1
//! # channels: direction, name, type, then the point on each transport
//! in  DI-00       bool enip-io=di:0      modbus-tcp=di:0
//! in  DI-04-count int  enip-io=in:18:u32 modbus-tcp=ir:24:u32
//! out DO-00       bool safe=false enip-io=do:0 modbus-tcp=coil:0
//! ```
//!
//! - `transport DRIVER [param=value ...]`: a driver that reaches the
//!   device and its settings, in the order the box should try them.
//!   `cycle_ms=A..B` is the profile's own: how fast and how slow the
//!   device can be asked for data over that transport (the box picks the
//!   rate inside it, [`Transport::limits`]); the rest are the driver's.
//! - `in` / `out NAME TYPE [unit=U] [safe=V] DRIVER=POINT ...`: a channel,
//!   with its point in each transport's own syntax. Every channel names
//!   every transport, so the device looks the same whichever is in use.
//!
//! [`Profile::map`] writes the I/O map lines for one transport: the
//! device line and one signal per channel. That is how a profile is
//! checked, too: [`Profile::parse`] renders each transport and holds the
//! result to the map's own rules (points parse, types match, outputs are
//! writable, one writer per point).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Duration;

use crate::model::{Dir, Ty};
use crate::text;

/// The first line of a profile.
pub const HEADER: &str = "helmsman-device 1";
/// The longest channel name.
pub const MAX_CHANNEL: usize = 32;
/// A transport's own param: `cycle_ms=A..B`, the rate the device can be
/// asked at over it, fastest to slowest, in ms (1..=60000).
pub const CYCLE: &str = "cycle_ms";

/// `A..B` of a `cycle_ms=A..B` value, 1 <= A <= B <= 60000, as durations.
fn parse_limits(v: &str) -> Option<(Duration, Duration)> {
    let (a, b) = v.split_once("..")?;
    let (a, b) = (a.parse::<u64>().ok()?, b.parse::<u64>().ok()?);
    ((1..=60_000).contains(&a) && (a..=60_000).contains(&b))
        .then(|| (Duration::from_millis(a), Duration::from_millis(b)))
}

/// One way to reach the device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transport {
    pub driver: String,
    /// `param=value` words for the device line, as written (the
    /// profile's own, [`CYCLE`], among them).
    pub params: Vec<String>,
}

impl Transport {
    /// The rate limits the transport gives (`cycle_ms=A..B`), if any.
    #[must_use]
    pub fn limits(&self) -> Option<(Duration, Duration)> {
        self.params
            .iter()
            .find_map(|p| p.strip_prefix(CYCLE)?.strip_prefix('='))
            .and_then(parse_limits)
    }

    /// The params that are the driver's: every `KEY=VALUE` but the
    /// profile's own.
    pub fn driver_params(&self) -> impl Iterator<Item = (&str, &str)> {
        self.params
            .iter()
            .filter_map(|p| p.split_once('='))
            .filter(|(k, _)| *k != CYCLE)
    }
}

/// One channel of the device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Channel {
    pub name: String,
    pub dir: Dir,
    pub ty: Ty,
    pub unit: Option<String>,
    /// An output's value when the device is lost, as written.
    pub safe: Option<String>,
    /// Driver name to point.
    pub points: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Profile {
    pub vendor: String,
    pub model: String,
    pub kind: String,
    pub transports: Vec<Transport>,
    pub channels: Vec<Channel>,
}

/// A channel name: 1..=32 of letters, digits, `-` and `_`, starting with
/// a letter.
#[must_use]
pub fn channel_name_ok(name: &str) -> bool {
    (1..=MAX_CHANNEL).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The signal a channel becomes in a map: the device's id, an
/// underscore, and the channel's name in lowercase with `-` as `_`
/// (`rio` and `DI-04-count` give `rio_di_04_count`).
#[must_use]
pub fn signal_name(device: &str, channel: &str) -> String {
    format!("{device}_{}", channel.to_ascii_lowercase().replace('-', "_"))
}

fn ty_name(ty: Ty) -> &'static str {
    match ty {
        Ty::Bool => "bool",
        Ty::Int => "int",
        Ty::Real => "real",
    }
}

impl Profile {
    /// Reads a profile and checks it: every problem, with its line (0 for
    /// one that belongs to the whole file).
    ///
    /// # Errors
    /// Lines that do not read, and whatever the map's own rules refuse
    /// once a transport is rendered.
    pub fn parse(src: &str) -> Result<Self, Vec<(usize, String)>> {
        let mut p = Self::default();
        let mut errs = Vec::new();
        let mut lines = src.lines().enumerate();
        match lines.next() {
            Some((_, first)) if first.trim() == HEADER => {}
            _ => return Err(vec![(1, format!("the first line must be {HEADER:?}"))]),
        }
        for (n, raw) in lines {
            let line = raw.split('#').next().unwrap_or_default();
            let words: Vec<&str> = line.split([' ', '\t']).filter(|w| !w.is_empty()).collect();
            if words.is_empty() {
                continue;
            }
            if let Err(e) = p.declaration(&words) {
                errs.push((n + 1, e));
            }
        }
        if p.model.is_empty() {
            errs.push((0, "no model line".into()));
        }
        if p.transports.is_empty() {
            errs.push((
                0,
                "no transport line: the profile names no way to reach the device".into(),
            ));
        }
        if p.channels.is_empty() {
            errs.push((0, "no channels".into()));
        }
        if errs.is_empty() {
            for (i, t) in p.transports.iter().enumerate() {
                if let Err(why) = p.check(i) {
                    errs.push((0, format!("over {}: {why}", t.driver)));
                }
            }
        }
        if errs.is_empty() { Ok(p) } else { Err(errs) }
    }

    fn declaration(&mut self, words: &[&str]) -> Result<(), String> {
        if let Some(w) = words.iter().find(|w| !w.bytes().all(|b| b.is_ascii_graphic())) {
            return Err(format!("{w:?}: only printable ASCII"));
        }
        let text = |rest: &[&str]| rest.join(" ");
        match words {
            ["vendor", rest @ ..] if !rest.is_empty() => self.vendor = text(rest),
            ["model", rest @ ..] if !rest.is_empty() => self.model = text(rest),
            ["kind", kind] => self.kind = (*kind).into(),
            ["transport", driver, params @ ..] => {
                if self.transports.iter().any(|t| t.driver == *driver) {
                    return Err(format!("transport {driver} given twice"));
                }
                if let Some(bad) = params.iter().find(|p| !p.contains('=')) {
                    return Err(format!("{bad:?}: expected KEY=VALUE"));
                }
                if let Some(v) = params
                    .iter()
                    .find_map(|p| p.strip_prefix(CYCLE)?.strip_prefix('='))
                    && parse_limits(v).is_none()
                {
                    return Err(format!("{CYCLE}={v}: expected A..B in ms, 1 <= A <= B <= 60000"));
                }
                self.transports.push(Transport {
                    driver: (*driver).into(),
                    params: params.iter().map(|p| (*p).to_string()).collect(),
                });
            }
            [dir @ ("in" | "out"), name, ty, rest @ ..] => {
                if !channel_name_ok(name) {
                    return Err(format!(
                        "channel {name:?}: 1..={MAX_CHANNEL} letters, digits, - and _, starting with a letter"
                    ));
                }
                if self.channels.iter().any(|c| c.name.eq_ignore_ascii_case(name)) {
                    return Err(format!("channel {name} given twice"));
                }
                let ty = text::ty(ty).ok_or_else(|| format!("type {ty:?}: bool, int or real"))?;
                let mut c = Channel {
                    name: (*name).into(),
                    dir: if *dir == "in" { Dir::In } else { Dir::Out },
                    ty,
                    unit: None,
                    safe: None,
                    points: BTreeMap::new(),
                };
                for word in rest {
                    let (k, v) = word
                        .split_once('=')
                        .filter(|(k, v)| !k.is_empty() && !v.is_empty())
                        .ok_or_else(|| format!("{word:?}: expected unit=, safe= or DRIVER=POINT"))?;
                    let slot = match k {
                        "unit" => &mut c.unit,
                        "safe" => &mut c.safe,
                        driver => {
                            if c.points.insert(driver.into(), v.into()).is_some() {
                                return Err(format!("{driver} given twice"));
                            }
                            continue;
                        }
                    };
                    if slot.replace(v.into()).is_some() {
                        return Err(format!("{k} given twice"));
                    }
                }
                self.channels.push(c);
            }
            _ => {
                return Err(format!(
                    "{:?}: expected vendor, model, kind, transport, in or out",
                    words[0]
                ));
            }
        }
        Ok(())
    }

    /// Holds transport `index` to the map's rules, with a placeholder
    /// device id and endpoint.
    fn check(&self, index: usize) -> Result<(), String> {
        let driver = &self.transports[index].driver;
        for c in &self.channels {
            if !c.points.contains_key(driver) {
                return Err(format!("channel {} has no point", c.name));
            }
            if let Some(other) = c
                .points
                .keys()
                .find(|k| !self.transports.iter().any(|t| &t.driver == *k))
            {
                return Err(format!(
                    "channel {} names {other}, which is not a transport",
                    c.name
                ));
            }
        }
        let map = self.map(index, "dev", "192.0.2.1").ok_or("no such transport")?;
        let file = text::parse(&map).map_err(|errs| {
            errs.iter()
                .map(|(_, e)| e.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        })?;
        file.map.validate().map_err(|errs| {
            errs.iter()
                .map(|e| format!("{e:?}"))
                .collect::<Vec<_>>()
                .join("; ")
        })
    }

    /// The I/O map lines for the device over transport `index`: its
    /// device line, then one signal per channel named by
    /// [`signal_name`]. `None` when there is no such transport.
    #[must_use]
    pub fn map(&self, index: usize, device: &str, endpoint: &str) -> Option<String> {
        let t = self.transports.get(index)?;
        let mut out = format!("device {device} {} {endpoint}", t.driver);
        for (k, v) in t.driver_params() {
            let _ = write!(out, " {k}={v}");
        }
        out.push('\n');
        for c in &self.channels {
            let dir = if c.dir == Dir::In { "in " } else { "out" };
            let point = c.points.get(&t.driver)?;
            let _ = write!(
                out,
                "{dir} {} {} {device} {point}",
                signal_name(device, &c.name),
                ty_name(c.ty)
            );
            if let Some(u) = &c.unit {
                let _ = write!(out, " unit={u}");
            }
            if let Some(s) = &c.safe {
                let _ = write!(out, " safe={s}");
            }
            out.push('\n');
        }
        Some(out)
    }

    /// The channel's point on transport `index`.
    #[must_use]
    pub fn point(&self, index: usize, channel: &str) -> Option<&str> {
        let driver = &self.transports.get(index)?.driver;
        self.channels
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(channel))
            .and_then(|c| c.points.get(driver))
            .map(String::as_str)
    }
}
