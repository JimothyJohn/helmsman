//! `enip-io`: an EtherNet/IP adapter the box masters over one Class 1
//! connection: remote I/O such as the Moxa ioLogik E1213
//! (`docs/devices/moxa-iologik-e1213.md`).
//!
//! The box is the scanner (originator). It opens the connection with
//! `Forward_Open` and from then on produces the O->T assembly (the
//! device's outputs) every RPI and consumes the T->O assembly (its
//! inputs). Reads answer from the latest T->O packet; a write changes the
//! O->T image, which goes out with the next packet.
//!
//! ```text
//! device rio enip-io 192.168.3.60 profile=moxa-e1213 rpi_ms=100
//! device rio enip-io 192.168.3.60 eds=/etc/helmsman/e1213.eds connection=exclusive-owner
//! device rio enip-io 192.168.3.60 config=100 o2t=102:1 t2o=101:51 rpi_ms=100
//! in  part_present bool rio di:0
//! in  count        int  rio in:2:u32
//! out clamp        bool rio do:0 safe=false
//! ```
//!
//! # The connection
//!
//! One of three sources, exactly one per device:
//!
//! - `profile=moxa-e1213`: built in, the E1213's EDS restated
//!   ([`Spec::moxa_e1213`]); `di:N` is 0..=11 and `do:N` 0..=7.
//! - `eds=PATH`: the vendor's EDS, read when the driver opens
//!   (`connection=exclusive-owner` (default) or `input-only` picks which
//!   `ConnectionN`). Sizes, points, real-time formats and the RPI range
//!   come from it.
//! - explicit: `config=N o2t=POINT:BYTES t2o=POINT:BYTES`, with
//!   `run_idle=on|off` for the O->T header (default on).
//!
//! Optional everywhere: `rpi_ms` (default the EDS's, else 100; checked
//! against the EDS's range), `multiplier` (0..=7, default 1: the
//! connection times out after RPI × 2^(multiplier+2) without T->O data),
//! `timeout_ms` (explicit messaging, default 1000), `io_port` (the local
//! UDP port T->O data arrives on, default 2222, where targets send it),
//! `device_io_port` (the device's, default 2222), and `key=off|exact|compatible`
//! (an electronic key from the EDS or profile identity; default off).
//!
//! # Points
//!
//! | point | type | |
//! | --- | --- | --- |
//! | `di:N` | bool | input bit N (byte N/8, bit N%8) |
//! | `do:N` | bool, writable | output bit N; reads what the box commands |
//! | `in:BYTE.BIT` | bool | any input bit |
//! | `out:BYTE.BIT` | bool, writable | any output bit |
//! | `in:OFFSET:FMT` | int / real | `u8 i8 u16 i16 u32 i32` (int), `f32` (real), little-endian as CIP is |
//! | `out:OFFSET:FMT` | int / real, writable | the same in the output assembly |
//!
//! A point past the end of its assembly, a channel the profile doesn't
//! have, a value its format can't hold, or an output point on an
//! input-only connection is a point error.
//!
//! # Errors
//!
//! Link errors (the link goes down, its readers are held): no session, a
//! refused `Forward_Open` (the device's extended status in the message),
//! no T->O data within the connection's timeout, T->O data of another size
//! than the connection's. After one the driver closes the connection
//! (`Forward_Close`, or later over the next session if this one is gone)
//! and opens a new one on the next read or write, sending the outputs it
//! last held from the first packet: exclusive ownership is released before
//! it is taken again, and the outputs come back without a zero glitch.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::driver::{Error, PointInfo};
use crate::model::{Ty, Value};

pub const DRIVER: &str = "enip-io";
/// The encapsulation port, used when the endpoint names none.
pub const DEFAULT_PORT: u16 = 44818;
/// Where targets send T->O data, and take O->T.
pub const IO_PORT: u16 = 2222;

/// Little-endian numeric formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
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
        Some(match s {
            "u8" => Self::U8,
            "i8" => Self::I8,
            "u16" => Self::U16,
            "i16" => Self::I16,
            "u32" => Self::U32,
            "i32" => Self::I32,
            "f32" => Self::F32,
            _ => return None,
        })
    }

    #[must_use]
    pub const fn width(self) -> usize {
        match self {
            Self::U8 | Self::I8 => 1,
            Self::U16 | Self::I16 => 2,
            Self::U32 | Self::I32 | Self::F32 => 4,
        }
    }

    #[must_use]
    pub const fn ty(self) -> Ty {
        match self {
            Self::F32 => Ty::Real,
            _ => Ty::Int,
        }
    }

    fn decode(self, b: &[u8]) -> Value {
        let mut w = [0u8; 4];
        w[..b.len()].copy_from_slice(b);
        match self {
            Self::U8 => Value::Int(i64::from(b[0])),
            Self::I8 => Value::Int(i64::from(i8::from_le_bytes([b[0]]))),
            Self::U16 => Value::Int(i64::from(u16::from_le_bytes([w[0], w[1]]))),
            Self::I16 => Value::Int(i64::from(i16::from_le_bytes([w[0], w[1]]))),
            Self::U32 => Value::Int(i64::from(u32::from_le_bytes(w))),
            Self::I32 => Value::Int(i64::from(i32::from_le_bytes(w))),
            Self::F32 => Value::Real(f64::from(f32::from_le_bytes(w))),
        }
    }

    fn encode(self, v: Value) -> Result<Vec<u8>, String> {
        let range = |n: i64, lo: i64, hi: i64| {
            if (lo..=hi).contains(&n) {
                Ok(())
            } else {
                Err(format!("{n} does not fit {self:?} ({lo}..={hi})"))
            }
        };
        Ok(match (self, v) {
            (Self::F32, Value::Real(x)) => {
                #[allow(clippy::cast_possible_truncation)]
                let f = x as f32;
                if !x.is_finite() || !f.is_finite() {
                    return Err(format!("{x} does not fit f32"));
                }
                f.to_le_bytes().to_vec()
            }
            (Self::U8, Value::Int(n)) => {
                range(n, 0, 0xFF)?;
                vec![n.to_le_bytes()[0]]
            }
            (Self::I8, Value::Int(n)) => {
                range(n, -128, 127)?;
                vec![n.to_le_bytes()[0]]
            }
            (Self::U16, Value::Int(n)) => {
                range(n, 0, 0xFFFF)?;
                n.to_le_bytes()[..2].to_vec()
            }
            (Self::I16, Value::Int(n)) => {
                range(n, -32768, 32767)?;
                n.to_le_bytes()[..2].to_vec()
            }
            (Self::U32, Value::Int(n)) => {
                range(n, 0, 0xFFFF_FFFF)?;
                n.to_le_bytes()[..4].to_vec()
            }
            (Self::I32, Value::Int(n)) => {
                range(n, i64::from(i32::MIN), i64::from(i32::MAX))?;
                n.to_le_bytes()[..4].to_vec()
            }
            (f, v) => return Err(format!("{f:?} takes {:?}, not {:?}", f.ty(), v.ty())),
        })
    }
}

/// Which assembly a point is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// T->O: the device's inputs.
    In,
    /// O->T: the device's outputs, which the box drives.
    Out,
}

/// What a point reads or writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `di:N` / `do:N`: a channel number.
    Channel(u16),
    Bit {
        byte: u16,
        bit: u8,
    },
    Word {
        offset: u16,
        format: Format,
    },
}

/// A parsed `enip-io` address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub side: Side,
    pub kind: Kind,
}

fn number(s: &str) -> Option<u16> {
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
                "{addr:?}: want di:N, do:N, in:BYTE.BIT, out:BYTE.BIT, in:OFFSET:FMT or out:OFFSET:FMT"
            ))
        };
        let (head, rest) = addr.split_once(':').ok_or_else(bad)?;
        let side_kind = |side| -> Result<Self, Error> {
            if let Some((byte, bit)) = rest.split_once('.') {
                let byte = number(byte).ok_or_else(bad)?;
                let bit = number(bit).filter(|b| *b < 8).ok_or_else(bad)?;
                return Ok(Self {
                    side,
                    kind: Kind::Bit {
                        byte,
                        bit: u8::try_from(bit).unwrap_or(0),
                    },
                });
            }
            let (offset, format) = rest.split_once(':').ok_or_else(bad)?;
            Ok(Self {
                side,
                kind: Kind::Word {
                    offset: number(offset).ok_or_else(bad)?,
                    format: Format::parse(format).ok_or_else(bad)?,
                },
            })
        };
        match head {
            "di" | "do" => Ok(Self {
                side: if head == "di" { Side::In } else { Side::Out },
                kind: Kind::Channel(number(rest).ok_or_else(bad)?),
            }),
            "in" => side_kind(Side::In),
            "out" => side_kind(Side::Out),
            _ => Err(bad()),
        }
    }

    #[must_use]
    pub const fn ty(&self) -> Ty {
        match self.kind {
            Kind::Channel(_) | Kind::Bit { .. } => Ty::Bool,
            Kind::Word { format, .. } => format.ty(),
        }
    }

    /// Without knowing the device: outputs are writable.
    #[must_use]
    pub const fn info(&self) -> PointInfo {
        PointInfo {
            ty: self.ty(),
            writable: matches!(self.side, Side::Out),
        }
    }

    /// The byte range it occupies, and for a bit, which bit.
    fn bytes(self) -> (usize, usize, Option<u8>) {
        match self.kind {
            Kind::Channel(n) => (usize::from(n / 8), 1, Some(u8::try_from(n % 8).unwrap_or(0))),
            Kind::Bit { byte, bit } => (usize::from(byte), 1, Some(bit)),
            Kind::Word { offset, format } => (usize::from(offset), format.width(), None),
        }
    }

    /// Output bits this point drives, as `BYTE.BIT`: what the one-writer
    /// rule compares, so `do:9`, `out:1.1` and `out:0:u16` collide.
    #[must_use]
    pub fn out_bits(&self) -> Vec<String> {
        if self.side != Side::Out {
            return Vec::new();
        }
        match self.bytes() {
            (byte, _, Some(bit)) => vec![format!("{byte}.{bit}")],
            (start, len, None) => (start..start + len)
                .flat_map(|b| (0..8).map(move |bit| format!("{b}.{bit}")))
                .collect(),
        }
    }

    /// The value at this point in an assembly's data; `None` if the data
    /// is too short for it.
    #[must_use]
    pub fn read_from(&self, data: &[u8]) -> Option<Value> {
        let (start, len, bit) = self.bytes();
        let b = data.get(start..start + len)?;
        Some(match (bit, self.kind) {
            (Some(bit), _) => Value::Bool(b[0] & (1 << bit) != 0),
            (None, Kind::Word { format, .. }) => format.decode(b),
            (None, _) => return None,
        })
    }

    /// Puts `v` at this point in an assembly's data.
    ///
    /// # Errors
    /// `Type` for a value of another type, `Point` for one the format
    /// can't hold or data too short for the point; `data` is unchanged.
    pub fn write_into(&self, data: &mut [u8], v: Value) -> Result<(), Error> {
        if v.ty() != self.ty() {
            return Err(Error::Type {
                want: self.ty(),
                got: v.ty(),
            });
        }
        let (start, len, bit) = self.bytes();
        let n = data.len();
        let short = || Error::Point(format!("past the end of {n} bytes"));
        match (bit, self.kind, v) {
            (Some(bit), _, Value::Bool(on)) => {
                let byte = data.get_mut(start).ok_or_else(short)?;
                if on {
                    *byte |= 1 << bit;
                } else {
                    *byte &= !(1 << bit);
                }
                Ok(())
            }
            (None, Kind::Word { format, .. }, v) => {
                let b = format.encode(v).map_err(Error::Point)?;
                data.get_mut(start..start + len)
                    .ok_or_else(short)?
                    .copy_from_slice(&b);
                Ok(())
            }
            _ => Err(short()),
        }
    }
}

/// Which connection of the device's to open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Connection {
    ExclusiveOwner,
    InputOnly,
}

/// A device's I/O, resolved: the connection to open and its data sizes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    pub connection: Connection,
    pub configuration: u16,
    /// O->T point (the output assembly, or the input-only heartbeat).
    pub o_to_t: u16,
    /// T->O point (the input assembly).
    pub t_to_o: u16,
    pub o_to_t_len: u16,
    pub t_to_o_len: u16,
    /// O->T data carries the 32-bit run/idle header.
    pub o_to_t_run_idle: bool,
    /// T->O data carries one.
    pub t_to_o_run_idle: bool,
    /// RPI range the device accepts (from its EDS), if known.
    pub rpi_min: Option<Duration>,
    pub rpi_max: Option<Duration>,
    pub rpi_default: Option<Duration>,
    /// Channel counts, for `di:N` / `do:N` (a profile knows them).
    pub di_channels: Option<u16>,
    pub do_channels: Option<u16>,
    /// From the EDS or profile, for an electronic key.
    pub identity: Option<helmsman_proto::eds::Identity>,
}

impl Spec {
    /// The Moxa ioLogik E1213, as its EDS (v2.0) declares it; the citations
    /// are in `docs/devices/moxa-iologik-e1213.md`, and a test checks this
    /// against what [`Spec::from_eds`] makes of the EDS.
    #[must_use]
    pub fn moxa_e1213(connection: Connection) -> Self {
        let (o_to_t, o_to_t_len) = match connection {
            Connection::ExclusiveOwner => (102, 1),
            Connection::InputOnly => (111, 0),
        };
        Self {
            connection,
            configuration: 100,
            o_to_t,
            t_to_o: 101,
            o_to_t_len,
            t_to_o_len: 51,
            o_to_t_run_idle: connection == Connection::ExclusiveOwner,
            t_to_o_run_idle: false,
            rpi_min: Some(Duration::from_millis(100)),
            rpi_max: Some(Duration::from_secs(3)),
            rpi_default: Some(Duration::from_millis(100)),
            di_channels: Some(12),
            do_channels: Some(8),
            identity: Some(helmsman_proto::eds::Identity {
                vendor_id: 991,
                device_type: 12,
                product_code: 515,
                major_revision: 4,
                minor_revision: 1,
                vendor_name: Some("Moxa".into()),
                product_name: Some("E1213".into()),
                catalog: Some("ioLogik E1200 Series".into()),
            }),
        }
    }

    /// The connection an EDS declares as `connection`.
    ///
    /// # Errors
    /// The EDS has none, or it isn't Class 1 cyclic point-to-point, needs
    /// configuration data, or uses a real-time format this driver doesn't
    /// speak (anything but modeless, heartbeat and run/idle).
    pub fn from_eds(eds: &helmsman_proto::eds::Eds, connection: Connection) -> Result<Self, String> {
        use helmsman_proto::eds::{AppType, RealTimeFormat};
        let want = match connection {
            Connection::ExclusiveOwner => AppType::ExclusiveOwner,
            Connection::InputOnly => AppType::InputOnly,
        };
        let all = eds.connections().map_err(|e| e.to_string())?;
        let c = all
            .iter()
            .find(|c| c.is(want))
            .ok_or_else(|| format!("the EDS declares no {want:?} connection"))?;
        let name = format!("Connection{} ({:?}, EDS line {})", c.number, c.name, c.line);
        if !c.class1 || !c.cyclic {
            return Err(format!("{name}: not a cyclic Class 1 connection"));
        }
        if !c.o_to_t.point_to_point || !c.t_to_o.point_to_point {
            return Err(format!("{name}: not point-to-point both ways"));
        }
        if c.config_size > 0 {
            return Err(format!(
                "{name}: needs {} bytes of configuration data",
                c.config_size
            ));
        }
        let o_run_idle = match c.o_to_t.format {
            RealTimeFormat::RunIdle => true,
            RealTimeFormat::Modeless | RealTimeFormat::Heartbeat => false,
            RealTimeFormat::Other(v) => return Err(format!("{name}: O->T real-time format {v}")),
        };
        let t_run_idle = match c.t_to_o.format {
            RealTimeFormat::RunIdle => true,
            RealTimeFormat::Modeless => false,
            other => return Err(format!("{name}: T->O real-time format {other:?}")),
        };
        if connection == Connection::InputOnly && c.o_to_t.size != 0 {
            return Err(format!(
                "{name}: input only, but O->T carries {} bytes",
                c.o_to_t.size
            ));
        }
        let lo = c.o_to_t.rpi.min.max(c.t_to_o.rpi.min);
        let hi = match (c.o_to_t.rpi.max, c.t_to_o.rpi.max) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        Ok(Self {
            connection,
            configuration: c.configuration,
            o_to_t: c.o_to_t_point,
            t_to_o: c.t_to_o_point,
            o_to_t_len: c.o_to_t.size,
            t_to_o_len: c.t_to_o.size,
            o_to_t_run_idle: o_run_idle,
            t_to_o_run_idle: t_run_idle,
            rpi_min: lo,
            rpi_max: hi,
            rpi_default: c.o_to_t.rpi.default.or(c.t_to_o.rpi.default),
            di_channels: None,
            do_channels: None,
            identity: eds.identity().ok(),
        })
    }

    /// Checks a point against this device: inside its assembly, a channel
    /// it has, writable only on an exclusive owner's outputs.
    ///
    /// # Errors
    /// A point error naming the limit.
    pub fn check(&self, addr: &str, p: &Point) -> Result<PointInfo, Error> {
        if p.side == Side::Out && self.connection == Connection::InputOnly {
            return Err(Error::Point(format!(
                "{addr}: an input-only connection has no outputs"
            )));
        }
        let (len, what) = match p.side {
            Side::In => (self.t_to_o_len, "input"),
            Side::Out => (self.o_to_t_len, "output"),
        };
        if let Kind::Channel(n) = p.kind {
            let limit = match p.side {
                Side::In => self.di_channels,
                Side::Out => self.do_channels,
            };
            if let Some(limit) = limit
                && n >= limit
            {
                return Err(Error::Point(format!(
                    "{addr}: the device has {limit} such channels"
                )));
            }
        }
        let (start, n, _) = p.bytes();
        if start + n > usize::from(len) {
            return Err(Error::Point(format!(
                "{addr}: past the end of the {len}-byte {what} assembly"
            )));
        }
        Ok(PointInfo {
            ty: p.ty(),
            writable: p.side == Side::Out && self.connection == Connection::ExclusiveOwner,
        })
    }
}

/// Device params, parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Params {
    /// The connection, unless it comes from an EDS file read at open.
    pub spec: Option<Spec>,
    pub eds: Option<String>,
    pub connection: Connection,
    pub rpi: Option<Duration>,
    pub multiplier: u8,
    pub timeout: Duration,
    pub io_port: u16,
    pub device_io_port: u16,
    pub key: Key,
}

/// Electronic key in the `Forward_Open`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Off,
    Exact,
    Compatible,
}

const KNOWN: &[&str] = &[
    "profile",
    "eds",
    "connection",
    "config",
    "o2t",
    "t2o",
    "run_idle",
    "rpi_ms",
    "multiplier",
    "timeout_ms",
    "io_port",
    "device_io_port",
    "key",
    "poll_ms",
];

impl Params {
    /// # Errors
    /// An unknown key, a value that doesn't parse, or not exactly one of
    /// `profile`, `eds` and the explicit `config`/`o2t`/`t2o`.
    #[allow(clippy::too_many_lines)] // one param after another
    pub fn parse(params: &BTreeMap<String, String>) -> Result<Self, Error> {
        let bad = |k: &str, why: &str| Error::Address(format!("{k}: {why}"));
        if let Some(k) = params.keys().find(|k| !KNOWN.contains(&k.as_str())) {
            return Err(bad(k, "not an enip-io param"));
        }
        let get = |k: &str| params.get(k).map(String::as_str);
        let int = |k: &str, max: u64| -> Result<Option<u64>, Error> {
            get(k)
                .map(|v| {
                    v.parse::<u64>()
                        .ok()
                        .filter(|n| *n <= max)
                        .ok_or_else(|| bad(k, &format!("want 0..={max}")))
                })
                .transpose()
        };
        let connection = match get("connection") {
            None | Some("exclusive-owner") => Connection::ExclusiveOwner,
            Some("input-only") => Connection::InputOnly,
            Some(_) => return Err(bad("connection", "exclusive-owner or input-only")),
        };
        let explicit = ["config", "o2t", "t2o"]
            .iter()
            .filter(|k| params.contains_key(**k))
            .count();
        let sources = usize::from(params.contains_key("profile"))
            + usize::from(params.contains_key("eds"))
            + usize::from(explicit > 0);
        if sources != 1 {
            return Err(bad(
                "profile",
                "give exactly one of profile=, eds= or config= o2t= t2o=",
            ));
        }
        let spec = if let Some(p) = get("profile") {
            match p {
                "moxa-e1213" => Some(Spec::moxa_e1213(connection)),
                _ => return Err(bad("profile", "known profiles: moxa-e1213")),
            }
        } else if explicit > 0 {
            if explicit != 3 {
                return Err(bad("config", "explicit needs config=, o2t= and t2o="));
            }
            let point = |k: &str| -> Result<(u16, u16), Error> {
                get(k)
                    .and_then(|v| v.split_once(':'))
                    .and_then(|(p, n)| Some((number(p)?, number(n)?)))
                    .ok_or_else(|| bad(k, "want POINT:BYTES"))
            };
            let (o_to_t, o_to_t_len) = point("o2t")?;
            let (t_to_o, t_to_o_len) = point("t2o")?;
            let max = helmsman_proto::eds::MAX_CLASS1_PACKET - 6;
            if o_to_t_len > max || t_to_o_len > max {
                return Err(bad("o2t", &format!("at most {max} bytes a direction")));
            }
            if connection == Connection::InputOnly && o_to_t_len != 0 {
                return Err(bad("o2t", "input-only: the O->T point is a heartbeat, 0 bytes"));
            }
            let run_idle = match get("run_idle") {
                None | Some("on") => true,
                Some("off") => false,
                Some(_) => return Err(bad("run_idle", "on or off")),
            };
            Some(Spec {
                connection,
                configuration: int("config", 0xFFFF)?
                    .and_then(|v| u16::try_from(v).ok())
                    .unwrap_or(0),
                o_to_t,
                t_to_o,
                o_to_t_len,
                t_to_o_len,
                o_to_t_run_idle: run_idle && o_to_t_len > 0,
                t_to_o_run_idle: false,
                rpi_min: None,
                rpi_max: None,
                rpi_default: None,
                di_channels: None,
                do_channels: None,
                identity: None,
            })
        } else {
            None
        };
        if params.contains_key("run_idle") && explicit == 0 {
            return Err(bad("run_idle", "only with explicit config= o2t= t2o="));
        }
        let rpi = int("rpi_ms", 3_600_000)?.map(Duration::from_millis);
        if rpi == Some(Duration::ZERO) {
            return Err(bad("rpi_ms", "want at least 1"));
        }
        let key = match get("key") {
            None | Some("off") => Key::Off,
            Some("exact") => Key::Exact,
            Some("compatible") => Key::Compatible,
            Some(_) => return Err(bad("key", "off, exact or compatible")),
        };
        if key != Key::Off && spec.as_ref().is_some_and(|s| s.identity.is_none()) {
            return Err(bad("key", "needs an identity: profile= or eds="));
        }
        let port = |k: &str, default: u16| -> Result<u16, Error> {
            Ok(int(k, 0xFFFF)?
                .and_then(|v| u16::try_from(v).ok())
                .unwrap_or(default))
        };
        let params = Self {
            spec,
            eds: get("eds").map(str::to_string),
            connection,
            rpi,
            multiplier: int("multiplier", 7)?
                .and_then(|v| u8::try_from(v).ok())
                .unwrap_or(1),
            timeout: Duration::from_millis(int("timeout_ms", 60_000)?.unwrap_or(1000).max(1)),
            io_port: port("io_port", IO_PORT)?,
            device_io_port: port("device_io_port", IO_PORT)?,
            key,
        };
        if let Some(spec) = &params.spec {
            params.rpi_for(spec)?;
        }
        Ok(params)
    }

    /// The RPI to ask for: `rpi_ms`, else the EDS's default, else 100 ms;
    /// inside the EDS's range.
    ///
    /// # Errors
    /// `rpi_ms` outside the range.
    pub fn rpi_for(&self, spec: &Spec) -> Result<Duration, Error> {
        let rpi = self
            .rpi
            .or(spec.rpi_default)
            .unwrap_or(Duration::from_millis(100));
        if spec.rpi_min.is_some_and(|m| rpi < m) || spec.rpi_max.is_some_and(|m| rpi > m) {
            return Err(Error::Address(format!(
                "rpi_ms: {} ms is outside the device's {}..={} ms",
                rpi.as_millis(),
                spec.rpi_min.map_or(0, |d| d.as_millis()),
                spec.rpi_max
                    .map_or_else(|| "any".to_string(), |d| d.as_millis().to_string()),
            )));
        }
        Ok(rpi)
    }
}

/// Parses a point for a device with these params, checking it against the
/// connection when the params say what it is (a profile or explicit
/// sizes; an `eds=` file is read only when the driver opens).
///
/// # Errors
/// A bad address, bad params, or a point the device can't have.
pub fn parse_on(params: &BTreeMap<String, String>, addr: &str) -> Result<PointInfo, Error> {
    let p = Point::parse(addr)?;
    let params = Params::parse(params)?;
    match &params.spec {
        Some(spec) => spec.check(addr, &p),
        None => Ok(PointInfo {
            ty: p.ty(),
            writable: p.side == Side::Out && params.connection == Connection::ExclusiveOwner,
        }),
    }
}

#[cfg(feature = "enip")]
pub use driver::EnipDriver;

#[cfg(feature = "enip")]
mod driver {
    use std::io;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, ToSocketAddrs, UdpSocket};
    use std::time::Duration;

    use helmsman_proto::io::{ElectronicKey, Priority};
    use helmsman_proto::scanner::{self, Closer, IoConfig, IoConnection};
    use helmsman_proto::session::{self, Session};

    use super::{Connection, DEFAULT_PORT, Key, Params, Point, Side, Spec};
    use crate::driver::{Driver, Error, PointInfo};
    use crate::model::Value;

    struct Link {
        session: Session,
        io: IoConnection,
        timeout: Duration,
    }

    /// A Class 1 connection to one adapter, opened on first use and
    /// reopened after a link error.
    pub struct EnipDriver {
        endpoint: String,
        params: Params,
        spec: Option<Spec>,
        /// The O->T image: what the box commands.
        out: Vec<u8>,
        link: Option<Link>,
        /// A connection lost with its session; closed on the next one.
        stale: Option<Closer>,
        /// `Forward_Open`s sent, for tests and status.
        pub opens: u64,
    }

    impl std::fmt::Debug for EnipDriver {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("EnipDriver")
                .field("endpoint", &self.endpoint)
                .field("connected", &self.link.is_some())
                .finish_non_exhaustive()
        }
    }

    fn session_error(e: session::Error) -> Error {
        match e {
            session::Error::Io(io) => Error::Io(io),
            other => Error::Protocol(other.to_string()),
        }
    }

    fn scanner_error(e: scanner::Error) -> Error {
        match e {
            scanner::Error::Session(s) => session_error(s),
            scanner::Error::Io(io) => Error::Io(io),
            scanner::Error::Refused(r) => Error::Io(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                format!("forward open refused: {r}"),
            )),
        }
    }

    fn read_eds(path: &str, connection: Connection) -> Result<Spec, Error> {
        let bad = |why: String| Error::Address(format!("eds={path}: {why}"));
        let len = std::fs::metadata(path).map_err(|e| bad(e.to_string()))?.len();
        if len > helmsman_proto::eds::MAX_TEXT as u64 {
            return Err(bad(format!("{len} bytes, larger than an EDS")));
        }
        let text = std::fs::read(path).map_err(|e| bad(e.to_string()))?;
        let text = String::from_utf8(text).map_err(|_| bad("not UTF-8 text".into()))?;
        let eds = helmsman_proto::eds::Eds::parse(&text).map_err(|e| bad(e.to_string()))?;
        Spec::from_eds(&eds, connection).map_err(bad)
    }

    impl EnipDriver {
        /// From device params (module docs). An `eds=` file is read now.
        ///
        /// # Errors
        /// Params that don't parse, an EDS that doesn't resolve, or an
        /// RPI outside its range.
        pub fn from_params(
            endpoint: &str,
            params: &std::collections::BTreeMap<String, String>,
        ) -> Result<Self, Error> {
            let params = Params::parse(params)?;
            let spec = match (&params.spec, &params.eds) {
                (Some(s), _) => s.clone(),
                (None, Some(path)) => read_eds(path, params.connection)?,
                (None, None) => return Err(Error::Address("profile: no connection given".into())),
            };
            params.rpi_for(&spec)?;
            if params.key != Key::Off && spec.identity.is_none() {
                return Err(Error::Address("key: the EDS has no [Device] identity".into()));
            }
            let endpoint = if endpoint.contains(':') {
                endpoint.to_string()
            } else {
                format!("{endpoint}:{DEFAULT_PORT}")
            };
            Ok(Self {
                endpoint,
                out: vec![0; usize::from(spec.o_to_t_len)],
                spec: Some(spec),
                params,
                link: None,
                stale: None,
                opens: 0,
            })
        }

        fn spec(&self) -> &Spec {
            self.spec.as_ref().expect("set in from_params")
        }

        /// The connection this driver opens.
        #[must_use]
        pub fn connection(&self) -> &Spec {
            self.spec()
        }

        fn config(&self) -> Result<IoConfig, Error> {
            let spec = self.spec();
            let rpi = self.params.rpi_for(spec)?;
            let mut c = IoConfig::exclusive_owner(
                spec.configuration,
                spec.o_to_t,
                spec.t_to_o,
                spec.o_to_t_len,
                spec.t_to_o_len,
                rpi,
            );
            c.o_to_t_run_idle = spec.o_to_t_run_idle;
            c.t_to_o_run_idle = spec.t_to_o_run_idle;
            c.timeout_multiplier = self.params.multiplier;
            c.priority = Priority::Scheduled;
            c.key = match (self.params.key, &spec.identity) {
                (Key::Off, _) | (_, None) => None,
                (k, Some(id)) => Some(ElectronicKey {
                    vendor_id: id.vendor_id,
                    device_type: id.device_type,
                    product_code: id.product_code,
                    major_revision: id.major_revision,
                    minor_revision: id.minor_revision,
                    compatibility: k == Key::Compatible,
                }),
            };
            Ok(c)
        }

        fn connect(&mut self) -> Result<(), Error> {
            let peer: SocketAddr = self
                .endpoint
                .to_socket_addrs()
                .map_err(Error::Io)?
                .find(SocketAddr::is_ipv4)
                .ok_or_else(|| Error::Io(io::Error::new(io::ErrorKind::InvalidInput, "no IPv4 address")))?;
            let IpAddr::V4(ip) = peer.ip() else {
                return Err(Error::Io(io::Error::new(io::ErrorKind::InvalidInput, "IPv6")));
            };
            let mut session = Session::connect(peer, self.params.timeout).map_err(session_error)?;
            if let Some(c) = self.stale.take() {
                // Releases ownership a lost session left behind; "not
                // found" means the device already let it go.
                let _ = c.send(&mut session);
            }
            let port = self.params.io_port;
            let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port)).map_err(|e| {
                // On a box, the box's own adapter holds 2222 unless its
                // --io-port moves it (TODO.md enip-io-port).
                Error::Io(io::Error::new(
                    e.kind(),
                    format!("UDP {port}, where the device sends its inputs: {e}"),
                ))
            })?;
            let config = self.config()?;
            let timeout = config.timeout();
            let target = SocketAddrV4::new(ip, self.params.device_io_port);
            self.opens += 1;
            let io = IoConnection::open_with_output(&mut session, &socket, target, config, &self.out)
                .map_err(scanner_error)?;
            // The first T->O packet, within the connection's timeout.
            if io.wait_for_input(timeout).is_none() {
                let closer = io.closer();
                drop(io);
                let _ = closer.send(&mut session);
                session.close();
                return Err(Error::Io(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "connection opened, but no T->O data within {} ms",
                        timeout.as_millis()
                    ),
                )));
            }
            self.link = Some(Link { session, io, timeout });
            Ok(())
        }

        /// Close the connection after a link error: `Forward_Close` now if
        /// the session still works, else on the next session.
        fn drop_link(&mut self) {
            if let Some(mut link) = self.link.take() {
                let closer = link.io.closer();
                drop(link.io);
                if closer.send(&mut link.session).is_ok() {
                    link.session.close();
                } else {
                    self.stale = Some(closer);
                }
            }
        }

        /// The latest T->O data, from a live connection of the right size.
        fn input(&mut self) -> Result<Vec<u8>, Error> {
            if self.link.is_none() {
                self.connect()?;
            }
            let want = usize::from(self.spec().t_to_o_len);
            let link = self.link.as_ref().expect("connected above");
            let fresh = link
                .io
                .latest_input()
                .filter(|i| i.received.elapsed() < link.timeout);
            let err = match fresh {
                Some(i) if i.data.len() == want => return Ok(i.data),
                Some(i) => Error::Protocol(format!(
                    "T->O data is {} bytes, the connection carries {want}",
                    i.data.len()
                )),
                None => Error::Io(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "no T->O data for {} ms: connection timed out",
                        link.timeout.as_millis()
                    ),
                )),
            };
            self.drop_link();
            Err(err)
        }

        fn point(&self, addr: &str) -> Result<(Point, PointInfo), Error> {
            let p = Point::parse(addr)?;
            let info = self.spec().check(addr, &p)?;
            Ok((p, info))
        }
    }

    impl Drop for EnipDriver {
        fn drop(&mut self) {
            self.drop_link();
        }
    }

    impl Driver for EnipDriver {
        fn parse(&self, addr: &str) -> Result<PointInfo, Error> {
            self.point(addr).map(|(_, i)| i)
        }

        fn read(&mut self, addrs: &[&str]) -> Result<Vec<Value>, Error> {
            let points: Vec<Point> = addrs
                .iter()
                .map(|a| self.point(a).map(|(p, _)| p))
                .collect::<Result<_, _>>()?;
            let input = self.input()?;
            points
                .iter()
                .map(|p| {
                    match p.side {
                        Side::In => p.read_from(&input),
                        Side::Out => p.read_from(&self.out),
                    }
                    .ok_or_else(|| Error::Point("past the end of its assembly".into()))
                })
                .collect()
        }

        fn write(&mut self, addr: &str, value: Value) -> Result<(), Error> {
            let (p, info) = self.point(addr)?;
            if !info.writable {
                return Err(Error::ReadOnly(addr.into()));
            }
            if value.ty() != info.ty {
                return Err(Error::Type {
                    want: info.ty,
                    got: value.ty(),
                });
            }
            let mut out = self.out.clone();
            p.write_into(&mut out, value)?;
            // A live link takes it from the next packet.
            self.input()?;
            let link = self.link.as_ref().expect("live after input()");
            link.io.set_output(&out).map_err(Error::Io)?;
            self.out = out;
            Ok(())
        }
    }
}
