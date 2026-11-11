//! `profinet-io`: a PROFINET IO device the box masters as its IO
//! controller, RT class 1: remote I/O blocks, drives, a robot's fieldbus
//! interface.
//!
//! The box finds the station by its name, connects with the modules the
//! device line lists, and from then on sends the output data every cycle
//! and takes the input data (`helmsman_profinet::controller`, the state
//! machine; this module gives it a thread, a link and the map's syntax).
//! Reads answer from the latest input frame; a write changes the output
//! image, which goes out with the next frame.
//!
//! ```text
//! device axis profinet-io mdrive if=en0 slots=0:0x1/1:0x1,1:0x30/1:0x1:i128,2:0x31/1:0x1:o128
//! in  status   int  axis in:0:u16
//! in  ready    bool axis in:2.0
//! out speed    int  axis out:4:i32 safe=0
//! ```
//!
//! # The device line
//!
//! The endpoint is the device's **station name** (lowercase letters,
//! digits and hyphens in dot-separated labels). Params:
//!
//! - `slots=` (required): the modules the device is built from, as its
//!   GSDML file names them. Modules are separated by commas; each is
//!   `SLOT:MODULE` followed by one or more `/SUBSLOT:SUBMODULE[:iN][:oN]`,
//!   where `MODULE` and `SUBMODULE` are the ident numbers (decimal or
//!   `0x` hex) and `iN` / `oN` the bytes of input and output data. A
//!   submodule with neither carries no data (an access point).
//! - `if=NAME`: the network interface the device is on (`en0`, `eth0`).
//!   Required for a real device: PROFINET's cyclic data is raw Ethernet,
//!   not IP, so the box must be told which wire.
//! - `ip=A.B.C.D/BITS` and `gateway=A.B.C.D`: the address to give the
//!   station when its own differs (until its next power cycle). Without
//!   `ip` the station must already have one.
//! - `cycle_ms=` 1, 2, 4 .. 512 (default 8): how often each side sends.
//!   `hold=` 1.. (default 3): cycles without a frame before either side
//!   gives up.
//! - `vendor=` and `device=`: the ids the station must report; left out,
//!   any station of that name is taken.
//! - `records=SLOT.SUBSLOT.INDEX:HEX[;...]`: parameter records written
//!   before the device starts.
//! - `startup=legacy|advanced` (default legacy), `vlan=on|off` (default
//!   on: frames carry an 802.1Q priority tag), `frame_id=` (the input
//!   frame id, 0xC000..=0xF7FF; give each device of one box its own).
//!
//! # Points
//!
//! Offsets count bytes in the **input image** (every submodule's input
//! data, one after another in `slots` order) and the **output image**
//! (likewise the outputs).
//!
//! | point | type | |
//! | --- | --- | --- |
//! | `in:BYTE.BIT` | bool | an input bit |
//! | `out:BYTE.BIT` | bool, writable | an output bit; reads what the box commands |
//! | `in:OFFSET:FMT` | int / real | `u8 i8 u16 i16 u32 i32` (int), `f32` (real); big-endian, as PROFINET data is, or little-endian with an `le` suffix (`u16le`) |
//! | `out:OFFSET:FMT` | int / real, writable | the same in the output image |
//!
//! # Errors
//!
//! Until cyclic data flows a read or write is a link error whose text is
//! what the startup is waiting for or why it failed ("no station named
//! ... answers", "... is not built as configured: slot 2 holds module
//! 0x99"); the controller keeps trying. A point whose submodule reports
//! its data bad, a point past its image and a value its format can't
//! hold are point errors.

use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use helmsman_profinet::controller::{Address, Controller, DeviceConfig, Local, Out, State};
use helmsman_profinet::dcp::Ip;
use helmsman_profinet::device::Device as SimDevice;
use helmsman_profinet::eth::Mac;
use helmsman_profinet::rt::{Layout, Module, Record, Submodule};

use crate::driver::{Driver, Error, PointInfo};
use crate::model::{Ty, Value};

pub const DRIVER: &str = "profinet-io";

/// The station name and ids the box announces itself with. The vendor id
/// is not a registered one: a box is a controller, which devices do not
/// check, and it claims no certification.
const LOCAL_NAME: &str = "helmsman";
const LOCAL_VENDOR: u16 = 0xFFFF;
const LOCAL_DEVICE: u16 = 0x0001;

/// A numeric format and its byte order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    pub kind: Number,
    pub little_endian: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Number {
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    F32,
}

impl Format {
    fn parse(s: &str) -> Option<Self> {
        let (name, little_endian) = s.strip_suffix("le").map_or((s, false), |n| (n, true));
        let kind = match name {
            "u8" => Number::U8,
            "i8" => Number::I8,
            "u16" => Number::U16,
            "i16" => Number::I16,
            "u32" => Number::U32,
            "i32" => Number::I32,
            "f32" => Number::F32,
            _ => return None,
        };
        // A byte has no order to name.
        if little_endian && matches!(kind, Number::U8 | Number::I8) {
            return None;
        }
        Some(Self { kind, little_endian })
    }

    #[must_use]
    pub const fn width(self) -> usize {
        match self.kind {
            Number::U8 | Number::I8 => 1,
            Number::U16 | Number::I16 => 2,
            Number::U32 | Number::I32 | Number::F32 => 4,
        }
    }

    #[must_use]
    pub const fn ty(self) -> Ty {
        match self.kind {
            Number::F32 => Ty::Real,
            _ => Ty::Int,
        }
    }

    /// The value of `b`, which is `width()` bytes.
    fn decode(self, b: &[u8]) -> Value {
        // Work on a little-endian copy, least significant byte first.
        let mut w = [0u8; 4];
        w[..b.len()].copy_from_slice(b);
        if !self.little_endian {
            w[..b.len()].reverse();
        }
        match self.kind {
            Number::U8 => Value::Int(i64::from(w[0])),
            Number::I8 => Value::Int(i64::from(i8::from_le_bytes([w[0]]))),
            Number::U16 => Value::Int(i64::from(u16::from_le_bytes([w[0], w[1]]))),
            Number::I16 => Value::Int(i64::from(i16::from_le_bytes([w[0], w[1]]))),
            Number::U32 => Value::Int(i64::from(u32::from_le_bytes(w))),
            Number::I32 => Value::Int(i64::from(i32::from_le_bytes(w))),
            Number::F32 => Value::Real(f64::from(f32::from_le_bytes(w))),
        }
    }

    fn encode(self, v: Value) -> Result<Vec<u8>, String> {
        let name = self.kind;
        let range = |n: i64, lo: i64, hi: i64| {
            if (lo..=hi).contains(&n) {
                Ok(n.to_le_bytes())
            } else {
                Err(format!("{n} does not fit {name:?} ({lo}..={hi})"))
            }
        };
        let le = match (self.kind, v) {
            (Number::F32, Value::Real(x)) => {
                #[allow(clippy::cast_possible_truncation)]
                let f = x as f32;
                if !x.is_finite() || !f.is_finite() {
                    return Err(format!("{x} does not fit f32"));
                }
                let b = f.to_le_bytes();
                [b[0], b[1], b[2], b[3], 0, 0, 0, 0]
            }
            (Number::U8, Value::Int(n)) => range(n, 0, 0xFF)?,
            (Number::I8, Value::Int(n)) => range(n, -128, 127)?,
            (Number::U16, Value::Int(n)) => range(n, 0, 0xFFFF)?,
            (Number::I16, Value::Int(n)) => range(n, -32768, 32767)?,
            (Number::U32, Value::Int(n)) => range(n, 0, 0xFFFF_FFFF)?,
            (Number::I32, Value::Int(n)) => range(n, i64::from(i32::MIN), i64::from(i32::MAX))?,
            (_, v) => return Err(format!("{name:?} takes {:?}, not {:?}", self.ty(), v.ty())),
        };
        let mut out = le[..self.width()].to_vec();
        if !self.little_endian {
            out.reverse();
        }
        Ok(out)
    }
}

/// Which image a point is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    In,
    Out,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Bit { byte: u16, bit: u8 },
    Word { offset: u16, format: Format },
}

/// A parsed `profinet-io` address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub side: Side,
    pub kind: Kind,
}

fn decimal(s: &str) -> Option<u16> {
    if s.is_empty() || s.len() > 5 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

impl Point {
    /// # Errors
    /// The address is not one of the forms in the module docs.
    pub fn parse(addr: &str) -> Result<Self, Error> {
        let bad = || {
            Error::Address(format!(
                "{addr:?}: want in:BYTE.BIT, out:BYTE.BIT, in:OFFSET:FMT or out:OFFSET:FMT"
            ))
        };
        let (head, rest) = addr.split_once(':').ok_or_else(bad)?;
        let side = match head {
            "in" => Side::In,
            "out" => Side::Out,
            _ => return Err(bad()),
        };
        let kind = if let Some((byte, bit)) = rest.split_once('.') {
            Kind::Bit {
                byte: decimal(byte).ok_or_else(bad)?,
                bit: decimal(bit)
                    .and_then(|b| u8::try_from(b).ok())
                    .filter(|b| *b < 8)
                    .ok_or_else(bad)?,
            }
        } else {
            let (offset, format) = rest.split_once(':').ok_or_else(bad)?;
            Kind::Word {
                offset: decimal(offset).ok_or_else(bad)?,
                format: Format::parse(format).ok_or_else(bad)?,
            }
        };
        Ok(Self { side, kind })
    }

    #[must_use]
    pub const fn ty(&self) -> Ty {
        match self.kind {
            Kind::Bit { .. } => Ty::Bool,
            Kind::Word { format, .. } => format.ty(),
        }
    }

    #[must_use]
    pub const fn info(&self) -> PointInfo {
        PointInfo {
            ty: self.ty(),
            writable: matches!(self.side, Side::Out),
        }
    }

    /// First byte and length in its image.
    const fn span(&self) -> (usize, usize) {
        match self.kind {
            Kind::Bit { byte, .. } => (byte as usize, 1),
            Kind::Word { offset, format } => (offset as usize, format.width()),
        }
    }

    /// Output bits this point drives, as `BYTE.BIT`: what the one-writer
    /// rule compares, so `out:1.1` and `out:0:u16` collide.
    #[must_use]
    pub fn out_bits(&self) -> Vec<String> {
        if self.side != Side::Out {
            return Vec::new();
        }
        match self.kind {
            Kind::Bit { byte, bit } => vec![format!("{byte}.{bit}")],
            Kind::Word { .. } => {
                let (start, len) = self.span();
                (start..start + len)
                    .flat_map(|b| (0..8).map(move |bit| format!("{b}.{bit}")))
                    .collect()
            }
        }
    }

    fn read_from(&self, image: &[u8]) -> Option<Value> {
        let (start, len) = self.span();
        let bytes = image.get(start..start + len)?;
        Some(match self.kind {
            Kind::Bit { bit, .. } => Value::Bool(bytes[0] >> bit & 1 == 1),
            Kind::Word { format, .. } => format.decode(bytes),
        })
    }

    fn write_to(&self, image: &mut [u8], value: Value) -> Result<(), Error> {
        let (start, len) = self.span();
        let Some(bytes) = image.get_mut(start..start + len) else {
            return Err(Error::Point(format!(
                "byte {start} is past the {} output bytes",
                image.len()
            )));
        };
        match (self.kind, value) {
            (Kind::Bit { bit, .. }, Value::Bool(on)) => {
                bytes[0] = (bytes[0] & !(1 << bit)) | (u8::from(on) << bit);
            }
            (Kind::Bit { .. }, v) => {
                return Err(Error::Type {
                    want: Ty::Bool,
                    got: v.ty(),
                });
            }
            (Kind::Word { format, .. }, v) => {
                if v.ty() != format.ty() {
                    return Err(Error::Type {
                        want: format.ty(),
                        got: v.ty(),
                    });
                }
                bytes.copy_from_slice(&format.encode(v).map_err(Error::Point)?);
            }
        }
        Ok(())
    }
}

/// A device line, parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// The interface to reach the device on, when the line names one.
    pub interface: Option<String>,
    pub device: DeviceConfig,
}

fn number(s: &str) -> Option<u32> {
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) if !hex.is_empty() && hex.len() <= 8 => u32::from_str_radix(hex, 16).ok(),
        Some(_) => None,
        None if !s.is_empty() && s.len() <= 10 && s.bytes().all(|b| b.is_ascii_digit()) => s.parse().ok(),
        None => None,
    }
}

fn number16(s: &str) -> Option<u16> {
    number(s).and_then(|n| u16::try_from(n).ok())
}

fn ip(s: &str) -> Option<Ip> {
    s.parse::<std::net::Ipv4Addr>().ok().map(|a| a.octets())
}

/// `slots=`: see the module docs.
fn slots(text: &str) -> Result<Vec<Module>, String> {
    let mut modules: Vec<Module> = Vec::new();
    for entry in text.split(',') {
        let bad = |why: &str| format!("slots: {entry:?}: {why}");
        let mut parts = entry.split('/');
        let head = parts.next().unwrap_or_default();
        let (slot, ident) = head
            .split_once(':')
            .ok_or_else(|| bad("want SLOT:MODULE/SUBSLOT:SUBMODULE"))?;
        let slot = number16(slot).ok_or_else(|| bad("the slot is not a number up to 65535"))?;
        let ident = number(ident).ok_or_else(|| bad("the module ident is not a number"))?;
        if modules.iter().any(|m| m.slot == slot) {
            return Err(bad("the slot is listed twice"));
        }
        let mut submodules: Vec<Submodule> = Vec::new();
        for sub in parts {
            let mut fields = sub.split(':');
            let subslot = fields
                .next()
                .and_then(number16)
                .ok_or_else(|| bad("a subslot is not a number up to 65535"))?;
            let ident = fields
                .next()
                .and_then(number)
                .ok_or_else(|| bad("a submodule ident is missing or not a number"))?;
            let (mut input, mut output) = (0, 0);
            for f in fields {
                let len = |n: &str| number16(n).filter(|n| (1..=1440).contains(n));
                match (f.split_at_checked(1), f) {
                    (Some(("i", n)), _) if input == 0 => {
                        input = len(n).ok_or_else(|| bad("iN wants 1..=1440 bytes"))?;
                    }
                    (Some(("o", n)), _) if output == 0 => {
                        output = len(n).ok_or_else(|| bad("oN wants 1..=1440 bytes"))?;
                    }
                    _ => return Err(bad("after the submodule ident come iN and oN, once each")),
                }
            }
            if submodules.iter().any(|s| s.subslot == subslot) {
                return Err(bad("a subslot is listed twice"));
            }
            submodules.push(Submodule {
                subslot,
                ident,
                input,
                output,
                records: Vec::new(),
            });
        }
        if submodules.is_empty() {
            return Err(bad("a module needs at least one /SUBSLOT:SUBMODULE"));
        }
        modules.push(Module {
            api: 0,
            slot,
            ident,
            submodules,
        });
    }
    Ok(modules)
}

/// `records=`: see the module docs.
fn records(text: &str, modules: &mut [Module]) -> Result<(), String> {
    for entry in text.split(';') {
        let bad = |why: &str| format!("records: {entry:?}: {why}");
        let (place, hex) = entry
            .split_once(':')
            .ok_or_else(|| bad("want SLOT.SUBSLOT.INDEX:HEX"))?;
        let mut parts = place.split('.');
        let (Some(slot), Some(subslot), Some(index), None) = (
            parts.next().and_then(number16),
            parts.next().and_then(number16),
            parts.next().and_then(number16),
            parts.next(),
        ) else {
            return Err(bad("want SLOT.SUBSLOT.INDEX:HEX"));
        };
        if hex.is_empty() || hex.len() % 2 != 0 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(bad("the data is not whole bytes of hex"));
        }
        let data = (0..hex.len())
            .step_by(2)
            .filter_map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
            .collect();
        let sub = modules
            .iter_mut()
            .find(|m| m.slot == slot)
            .and_then(|m| m.submodules.iter_mut().find(|s| s.subslot == subslot))
            .ok_or_else(|| bad("no such slot and subslot in slots="))?;
        sub.records.push(Record { index, data });
    }
    Ok(())
}

impl Config {
    /// Parses a device line's endpoint and params.
    ///
    /// # Errors
    /// A param that does not parse, one this driver does not know, or a
    /// configuration the controller cannot put on the wire.
    pub fn parse(endpoint: &str, params: &BTreeMap<String, String>) -> Result<Self, Error> {
        let bad = |why: String| Error::Address(why);
        let mut modules = slots(
            params
                .get("slots")
                .ok_or_else(|| bad("slots= is missing".into()))?,
        )
        .map_err(bad)?;
        if let Some(text) = params.get("records") {
            records(text, &mut modules).map_err(bad)?;
        }
        let mut device = DeviceConfig::new(endpoint, modules);
        let mut interface = None;
        let mut gateway = [0; 4];
        for (key, value) in params {
            let want = |what: &str| bad(format!("{key}={value}: want {what}"));
            match key.as_str() {
                // `poll_ms` and `timeout_ms` are the link's (how often the
                // box reads the image), not this driver's.
                "slots" | "records" | "poll_ms" | "timeout_ms" => {}
                "if" => {
                    if value.is_empty()
                        || value.len() > 15
                        || !value.bytes().all(|b| b.is_ascii_alphanumeric())
                    {
                        return Err(want("an interface name such as en0 or eth0"));
                    }
                    interface = Some(value.clone());
                }
                "ip" => {
                    let parsed = value.split_once('/').and_then(|(a, bits)| {
                        let bits = decimal(bits).filter(|b| (1..=30).contains(b))?;
                        let mask = (u32::MAX << (32 - u32::from(bits))).to_be_bytes();
                        Some((ip(a)?, mask))
                    });
                    let (ip, mask) = parsed.ok_or_else(|| want("A.B.C.D/BITS, such as 192.168.3.70/24"))?;
                    device.address = Some(Address { ip, mask, gateway });
                }
                "gateway" => gateway = ip(value).ok_or_else(|| want("A.B.C.D"))?,
                "cycle_ms" => {
                    device.reduction_ratio = decimal(value)
                        .filter(|n| (1..=512).contains(n) && n.is_power_of_two())
                        .ok_or_else(|| want("1, 2, 4 .. 512"))?;
                }
                "hold" => {
                    device.data_hold_factor = decimal(value)
                        .filter(|n| *n >= 1)
                        .ok_or_else(|| want("a count of cycles, 1 or more"))?;
                }
                "vendor" => {
                    device.vendor_id = Some(number16(value).ok_or_else(|| want("an id up to 0xFFFF"))?)
                }
                "device" => {
                    device.device_id = Some(number16(value).ok_or_else(|| want("an id up to 0xFFFF"))?)
                }
                "startup" => {
                    device.advanced_startup = match value.as_str() {
                        "legacy" => false,
                        "advanced" => true,
                        _ => return Err(want("legacy or advanced")),
                    };
                }
                "vlan" => {
                    device.vlan = match value.as_str() {
                        "on" => true,
                        "off" => false,
                        _ => return Err(want("on or off")),
                    };
                }
                "frame_id" => {
                    device.input_frame_id =
                        number16(value).ok_or_else(|| want("an id in 0xC000..=0xF7FF"))?;
                }
                _ => return Err(bad(format!("{key}: not a profinet-io param"))),
            }
        }
        // `gateway` may sort after `ip`: apply it once both are read.
        if let Some(a) = &mut device.address {
            a.gateway = gateway;
        } else if gateway != [0; 4] {
            return Err(bad("gateway= without ip=".into()));
        }
        // What the controller itself would refuse, said when the map loads.
        Controller::new(local([0; 6]), device.clone(), 1).map_err(bad)?;
        Ok(Self { interface, device })
    }

    fn layout(&self) -> Layout {
        // `parse` built a controller from these modules, so they lay out.
        Layout::new(&self.device.modules).unwrap_or(Layout {
            places: Vec::new(),
            input_frame_len: 0,
            output_frame_len: 0,
            input_image_len: 0,
            output_image_len: 0,
        })
    }

    /// Checks a point against the images the modules give.
    ///
    /// # Errors
    /// The point does not parse, or lies past its image.
    pub fn check(&self, addr: &str) -> Result<PointInfo, Error> {
        let p = Point::parse(addr)?;
        let layout = self.layout();
        let (len, what) = match p.side {
            Side::In => (layout.input_image_len, "input"),
            Side::Out => (layout.output_image_len, "output"),
        };
        let (start, width) = p.span();
        if start + width > len {
            return Err(Error::Address(format!(
                "{addr}: the device has {len} {what} bytes, this point ends at byte {}",
                start + width
            )));
        }
        Ok(p.info())
    }
}

fn local(mac: Mac) -> Local {
    Local {
        mac,
        station_name: LOCAL_NAME.into(),
        vendor_id: LOCAL_VENDOR,
        device_id: LOCAL_DEVICE,
    }
}

/// [`crate::driver::parse_point_on`] for this driver.
///
/// # Errors
/// As [`Config::parse`] and [`Config::check`].
pub fn parse_on(endpoint: &str, params: &BTreeMap<String, String>, addr: &str) -> Result<PointInfo, Error> {
    Config::parse(endpoint, params)?.check(addr)
}

/// Something a link received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A raw PROFINET frame.
    Frame(Vec<u8>),
    /// A datagram at the box's context-manager port.
    Udp { ip: Ip, port: u16, data: Vec<u8> },
}

/// The wire under one controller: raw Ethernet frames of `EtherType`
/// 0x8892 on one interface, and UDP at port 34964.
pub trait Link: Send {
    /// The interface's MAC address, the controller's own.
    fn mac(&self) -> Mac;

    /// # Errors
    /// The interface refused the frame.
    fn send_frame(&mut self, frame: &[u8]) -> io::Result<()>;

    /// Sends from the context-manager port.
    ///
    /// # Errors
    /// The socket refused the datagram.
    fn send_udp(&mut self, ip: Ip, port: u16, data: &[u8]) -> io::Result<()>;

    /// Waits up to `timeout` for the next frame or datagram.
    ///
    /// # Errors
    /// The interface or socket failed.
    fn recv(&mut self, timeout: Duration) -> io::Result<Option<Event>>;
}

/// What makes a link for a device line: the box's raw-Ethernet opener, or
/// a test's simulated wire.
pub type Opener = Arc<dyn Fn(&Config) -> io::Result<Box<dyn Link>> + Send + Sync>;

/// A link to a simulated device in the same process: every frame and
/// datagram the controller sends is handed to the device, and what the
/// device sends comes back from `recv`. For tests, the conformance
/// harness and trying a map with no hardware.
pub struct SimLink {
    device: Arc<Mutex<SimDevice>>,
    mac: Mac,
    controller_ip: Ip,
    queue: VecDeque<Event>,
    start: Instant,
    looped: bool,
}

impl SimLink {
    /// A link whose far end is `device`. The device can be reached
    /// through the handle the caller keeps (to set inputs, read outputs
    /// and inject faults).
    #[must_use]
    pub fn new(device: Arc<Mutex<SimDevice>>) -> Self {
        Self {
            device,
            mac: [0x02, 0x48, 0x4C, 0x4D, 0x00, 0x01],
            controller_ip: [192, 168, 3, 10],
            queue: VecDeque::new(),
            start: Instant::now(),
            looped: false,
        }
    }

    /// A link to a new simulated device built exactly as `expected`
    /// describes, with an IP address, whose outputs come back as its
    /// inputs byte for byte (as far as both images reach): a wire from
    /// each output to an input, so a write can be read back.
    #[must_use]
    pub fn looped(expected: &DeviceConfig) -> Self {
        let station = helmsman_profinet::device::station(
            &expected.station_name,
            expected.vendor_id.unwrap_or(0xFFFF),
            expected.device_id.unwrap_or(1),
            expected.address.map_or([192, 168, 3, 70], |a| a.ip),
        );
        let device = SimDevice::new(
            [0x02, 0x48, 0x4C, 0x4D, 0x00, 0x02],
            station,
            expected.modules.clone(),
            1,
        );
        let mut link = Self::new(Arc::new(Mutex::new(device)));
        link.looped = true;
        link
    }

    fn now(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_micros()).unwrap_or(u64::MAX)
    }

    fn take(&mut self, outs: Vec<Out>) {
        let ip = lock(&self.device).station().ip;
        for out in outs {
            self.queue.push_back(match out {
                Out::Frame(f) => Event::Frame(f),
                // The device answers from a port of its own choosing.
                Out::Udp { data, .. } => Event::Udp {
                    ip,
                    port: 49200,
                    data,
                },
            });
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Link for SimLink {
    fn mac(&self) -> Mac {
        self.mac
    }

    fn send_frame(&mut self, frame: &[u8]) -> io::Result<()> {
        let mut out = Vec::new();
        lock(&self.device).on_frame(self.now(), frame, &mut out);
        self.take(out);
        Ok(())
    }

    fn send_udp(&mut self, _ip: Ip, port: u16, data: &[u8]) -> io::Result<()> {
        let mut out = Vec::new();
        let (now, ip) = (self.now(), self.controller_ip);
        // The source port a device answers to is the controller's 34964.
        let _ = port;
        lock(&self.device).on_udp(now, ip, helmsman_profinet::rpc::PORT, data, &mut out);
        self.take(out);
        Ok(())
    }

    fn recv(&mut self, timeout: Duration) -> io::Result<Option<Event>> {
        let deadline = Instant::now() + timeout;
        loop {
            let mut out = Vec::new();
            {
                let mut device = lock(&self.device);
                if self.looped {
                    let outputs = device.outputs().to_vec();
                    let n = outputs.len().min(device.inputs_mut().len());
                    device.inputs_mut()[..n].copy_from_slice(&outputs[..n]);
                }
                device.poll(self.now(), &mut out);
            }
            self.take(out);
            if let Some(e) = self.queue.pop_front() {
                return Ok(Some(e));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            std::thread::sleep(left.min(Duration::from_micros(500)));
        }
    }
}

/// What the I/O thread and the driver share.
struct Shared {
    /// The input image, while cyclic data flows.
    inputs: Option<Vec<u8>>,
    /// Per place of the layout: whether its input data is good.
    good: Vec<bool>,
    outputs: Vec<u8>,
    /// What the startup waits for or why it failed.
    status: String,
    address: Option<String>,
    stop: bool,
}

/// One device behind a controller on its own thread, started on first
/// use.
pub struct ProfinetDriver {
    config: Config,
    layout: Layout,
    opener: Opener,
    shared: Arc<Mutex<Shared>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ProfinetDriver {
    /// A driver for a device line, reaching the device through whatever
    /// `opener` makes.
    ///
    /// # Errors
    /// As [`Config::parse`].
    pub fn new(endpoint: &str, params: &BTreeMap<String, String>, opener: Opener) -> Result<Self, Error> {
        let config = Config::parse(endpoint, params)?;
        let layout = config.layout();
        let shared = Arc::new(Mutex::new(Shared {
            inputs: None,
            good: vec![false; layout.places.len()],
            outputs: vec![0; layout.output_image_len],
            status: "not started".into(),
            address: None,
            stop: false,
        }));
        Ok(Self {
            config,
            layout,
            opener,
            shared,
            thread: None,
        })
    }

    /// Starts the controller's thread when it is not running.
    fn start(&mut self) -> Result<(), Error> {
        if self.thread.as_ref().is_some_and(|t| !t.is_finished()) {
            return Ok(());
        }
        let link = (self.opener)(&self.config)?;
        // A seed that differs between boxes and between starts, so two
        // controllers never pick the same AR UUID.
        let seed = u64::from_be_bytes({
            let m = link.mac();
            [m[0], m[1], m[2], m[3], m[4], m[5], 0, 0]
        }) ^ std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| {
                u64::try_from(d.as_nanos() & u128::from(u64::MAX)).unwrap_or(0)
            });
        let controller =
            Controller::new(local(link.mac()), self.config.device.clone(), seed).map_err(Error::Address)?;
        lock(&self.shared).stop = false;
        let shared = Arc::clone(&self.shared);
        let name = format!("profinet-{}", self.config.device.station_name);
        self.thread = Some(
            std::thread::Builder::new()
                .name(name.chars().take(15).collect())
                .spawn(move || run(controller, link, &shared))?,
        );
        Ok(())
    }

    fn down(status: &str) -> Error {
        Error::Io(io::Error::new(io::ErrorKind::NotConnected, status.to_string()))
    }
}

/// The controller's thread: feed it what arrives and the time, send what
/// it produces, and mirror its images into `shared`.
fn run(mut controller: Controller, mut link: Box<dyn Link>, shared: &Mutex<Shared>) {
    let start = Instant::now();
    let now = || u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX);
    let mut out = Vec::new();
    let send = |link: &mut Box<dyn Link>, out: &mut Vec<Out>| -> io::Result<()> {
        for o in out.drain(..) {
            match o {
                Out::Frame(f) => link.send_frame(&f)?,
                Out::Udp { ip, port, data } => link.send_udp(ip, port, &data)?,
            }
        }
        Ok(())
    };
    loop {
        {
            let mut s = lock(shared);
            if s.stop {
                break;
            }
            controller.outputs_mut().copy_from_slice(&s.outputs);
            s.inputs = controller.inputs().map(<[u8]>::to_vec);
            for (i, good) in s.good.iter_mut().enumerate() {
                *good = controller.input_good(i);
            }
            s.status = status(&controller);
            s.address = controller
                .station()
                .map(|st| std::net::Ipv4Addr::from(st.ip).to_string());
        }
        controller.poll(now(), &mut out);
        let wait = controller.next_wake(now()).saturating_sub(now()).min(5_000);
        let received =
            send(&mut link, &mut out).and_then(|()| link.recv(Duration::from_micros(wait.max(100))));
        match received {
            Ok(Some(Event::Frame(f))) => controller.on_frame(now(), &f, &mut out),
            Ok(Some(Event::Udp { ip, port, data })) => controller.on_udp(now(), ip, port, &data, &mut out),
            Ok(None) => {}
            Err(e) => {
                // The interface is gone: say so and end; the driver opens
                // a new link on the next read or write.
                let mut s = lock(shared);
                s.inputs = None;
                s.status = format!("the network interface failed: {e}");
                return;
            }
        }
        if send(&mut link, &mut out).is_err() {
            out.clear();
        }
    }
    controller.release(&mut out);
    let _ = send(&mut link, &mut out);
}

/// The startup's state in words, for a read made before data flows.
fn status(c: &Controller) -> String {
    if let Some(e) = c.error() {
        return e.into();
    }
    match c.state() {
        State::Identifying => "looking for the station by name",
        State::Addressing => "giving the station its IP address",
        State::Connecting => "connecting",
        State::Parameterising => "writing the device's parameters",
        State::WaitingReady => "waiting for the device to report ready",
        State::Running => "waiting for the device's first cyclic data",
        State::Backoff => "waiting to try again",
    }
    .into()
}

impl Driver for ProfinetDriver {
    fn parse(&self, addr: &str) -> Result<PointInfo, Error> {
        self.config.check(addr)
    }

    fn read(&mut self, addrs: &[&str]) -> Result<Vec<Value>, Error> {
        let points: Vec<Point> = addrs.iter().map(|a| Point::parse(a)).collect::<Result<_, _>>()?;
        for a in addrs {
            self.config.check(a)?;
        }
        self.start()?;
        let s = lock(&self.shared);
        let Some(inputs) = &s.inputs else {
            return Err(Self::down(&s.status));
        };
        let mut values = Vec::with_capacity(points.len());
        for (p, addr) in points.iter().zip(addrs) {
            let image = match p.side {
                Side::In => {
                    let (start, len) = p.span();
                    // Every submodule the point's bytes fall in must
                    // report its data good.
                    let bad = self.layout.places.iter().zip(&s.good).find(|(place, good)| {
                        let (from, to) = (
                            place.input_image,
                            place.input_image + usize::from(place.input_len),
                        );
                        !**good && start < to && from < start + len
                    });
                    if let Some((place, _)) = bad {
                        return Err(Error::Point(format!(
                            "{addr}: slot {} subslot {} reports its data bad",
                            place.slot, place.subslot
                        )));
                    }
                    &inputs[..]
                }
                Side::Out => &s.outputs[..],
            };
            values.push(
                p.read_from(image)
                    .ok_or_else(|| Error::Point(format!("{addr}: past the image")))?,
            );
        }
        Ok(values)
    }

    fn write(&mut self, addr: &str, value: Value) -> Result<(), Error> {
        let p = Point::parse(addr)?;
        self.config.check(addr)?;
        if p.side != Side::Out {
            return Err(Error::ReadOnly(addr.into()));
        }
        self.start()?;
        let mut s = lock(&self.shared);
        // The image is kept while the link is down, so the device gets the
        // commanded outputs from its first frame; the write itself is
        // refused until then, as every network driver's is.
        p.write_to(&mut s.outputs, value)?;
        if s.inputs.is_none() {
            return Err(Self::down(&s.status));
        }
        Ok(())
    }

    fn address(&self) -> Option<String> {
        lock(&self.shared).address.clone()
    }
}

impl Drop for ProfinetDriver {
    fn drop(&mut self) {
        lock(&self.shared).stop = true;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The opener a box uses: `if=sim` is a simulated device in the process
/// (the `profinet-sim` feature); anything else needs raw Ethernet, which
/// this build may not have.
#[must_use]
pub fn opener() -> Opener {
    Arc::new(|config| {
        #[cfg(feature = "profinet-sim")]
        if config.interface.as_deref() == Some("sim") {
            return Ok(Box::new(SimLink::looped(&config.device)) as Box<dyn Link>);
        }
        let Some(interface) = &config.interface else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the device line names no interface: add if=NAME (en0, eth0), the network the device is on",
            ));
        };
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("this build cannot send raw Ethernet frames on {interface}"),
        ))
    })
}
