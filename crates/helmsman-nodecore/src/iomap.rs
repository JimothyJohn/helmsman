//! The node's I/O map: the box's map format (`helmsman_io::text`, the
//! `device` and `in`/`out` lines) cut to what a node drives: up to
//! [`MAX_DEVICES`] devices at once, each `modbus-tcp` or `enip-io` (Class 1
//! cyclic I/O), with bool, int and real points (cards node-io-map and
//! node-multi-device). The text is kept as the engineer wrote it (comments
//! and all) beside the program on flash, so the canvas's Devices tab lists
//! the devices and the points by their names, and a program addresses
//! them in the process image by the rule below.
//!
//! ```text
//! device rio   modbus-tcp 192.168.3.60 unit=1
//! device drive enip-io    192.168.33.1 config=1 o2t=112:36 t2o=100:16 rpi_ms=20
//! device press modbus-tcp 192.168.3.71:502
//! in  estop  bool rio   di:0
//! out motor  bool rio   coil:0
//! in  pos    int  drive in:4:i32
//! out accel  int  drive out:0:u32
//! in  level  real press hr:5:f32
//! ```
//!
//! # What a node takes
//!
//! The grammar and the spellings are the box's; a line a node cannot run
//! is refused with its line number and the reason, so a map a box would
//! take but a node cannot run never reaches flash.
//!
//! - **Devices**: one to [`MAX_DEVICES`] `device NAME DRIVER IPV4[:PORT]
//!   [KEY=VALUE ...]` lines; ids unique without regard to case; `box`,
//!   `module` and `screen` are the box's own and a node has none of them.
//!   - `modbus-tcp`: port 502 unless given; `unit=N` (1 to 255, 1 unless
//!     given). Other params (`timeout_ms`, `poll_ms`) are read and ignored.
//!   - `enip-io`: port 44818 (the only one a node opens). The connection is
//!     one of `profile=moxa-e1213` or `config=N o2t=POINT:BYTES
//!     t2o=POINT:BYTES` (`run_idle=on|off`), with `connection=
//!     exclusive-owner|input-only`, `rpi_ms` (at least [`MIN_RPI_MS`]),
//!     `multiplier` (0 to 7) and, with the explicit form, `config_data=HEX`.
//!     `eds=` needs a file, `key=` an identity check, and `io_port=`,
//!     `device_io_port=` and `sweep=` a socket of their own: a node has
//!     none of those, and says so.
//! - **Sim points**: `in|out NAME bool|int|real sim [unit=..]`: a
//!   placeholder with no device and no address (see "Sim points" below).
//!   `sim` is a reserved device id, like `box`: `device sim ...` is
//!   refused. A map may hold sim points and no device at all.
//! - **Points**: `in|out NAME bool|int|real DEVICE ADDRESS [unit=.. wired=..]`;
//!   names are ST identifiers, unique without regard to case. Further
//!   `poll_ms=` and `safe=` options are read and ignored, as they always
//!   were here. The address is the driver's, as on a box:
//!   - Modbus: `in` takes `di:N`, `ir:N[:FMT]` and `hr:N[:FMT]`, `out`
//!     takes `coil:N` and `hr:N[:FMT]`; `FMT` is `u16 i16 u32 u32sw i32
//!     i32sw f32 f32sw` (`u16` unless given). `di` and `coil` are bool,
//!     the registers int (real for `f32`).
//!   - EtherNet/IP: `in` takes `di:N`, `in:BYTE.BIT` and `in:OFFSET:FMT`
//!     (the T->O assembly), `out` takes `do:N`, `out:BYTE.BIT` and
//!     `out:OFFSET:FMT` (the O->T assembly); `FMT` is `u8 i8 u16 i16 u32
//!     i32 f32`, little-endian as CIP is, offsets from the first
//!     application byte (after the run/idle header). A point must lie
//!     inside the assembly and an output needs an exclusive-owner
//!     connection.
//!   - Two points of one direction on one device must not share a bit or
//!     a register.
//!
//! # The process image
//!
//! Each direction (`%I`, `%Q`) is [`IMAGE_BYTES`] bytes: a bool area
//! ([`BOOL_BYTES`]) then a numeric area ([`NUM_BYTES`]). The rule depends
//! on the map text alone:
//!
//! - **Bool**: device number *k* (0 for the first `device` line) owns
//!   [`BOOL_WINDOW_BYTES`] bytes at `4k`, and a bool point numbered *n*
//!   (`di:N`, `coil:N`, `do:N`, or `in:B.b` as `8B+b`) sits at bit `n % 8`
//!   of byte `4k + n / 8`, so *n* is below 32. For the first device that
//!   is `%IX(n/8).(n%8)`, which is what a one-device bool map has always
//!   had. A bool never moves when another device's points change.
//! - **Numeric** (int and real): in the order the lines are written, per
//!   direction, each point takes the next slot of the numeric area: 2
//!   bytes (`%IW`, for `u8 i8 u16 i16`) or 4 bytes (`%ID`, for `u32 i32
//!   f32`) aligned to its size, little-endian, starting at byte
//!   [`NUM_BASE`]. Eight-bit values are widened to 16 (`i8` sign-extended).
//!   Append new numeric points at the end of the map to keep the
//!   addresses of the others.
//!
//! [`Point::at`] gives the place and [`At::st`] the ST address
//! (`%IX4.3`, `%IW32`, `%QD17`).
//!
//! # Sim points
//!
//! A sim point is a value held in the node's memory: it starts at FALSE,
//! 0 or 0.0 at boot, and a program deploy keeps it. A sim `in` is set
//! with `PUT /api/signals/NAME` (the program reads it as a `VAR_INPUT` of
//! that name); a sim `out` is written by the program's `VAR_OUTPUT` of that
//! name and nothing is driven anywhere. It sits in the process image so
//! the program, the signals list and the values stream treat it as any
//! point, but it never moves a device point: sim points are placed after
//! the whole map is read, from the top of each area downwards, in the
//! order the lines are written, and a map in which they would meet a
//! device point is refused.
//!
//! - **Bool**: from the last bool window (the one of device number
//!   [`MAX_DEVICES`] - 1) downwards, 32 to a window, so a map of *d*
//!   devices keeps `MAX_DEVICES - d` windows for them.
//! - **Numeric**: 4-byte slots from the end of the numeric area downwards.
//!   An `int` is an `i32` (a value outside `i32` is refused), a `real` an
//!   `f32`.
//!
//! Adding a sim point therefore moves no device point, and no sim point
//! written before it; one written after it moves up by a slot.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

/// Most bytes of map text a node keeps (beside the program in its 8 KiB
/// slot, which the two share: `program::MAX_SOURCE`). It was 2048 while a
/// node drove one block; a firmware older than this one reads a stored
/// map longer than that as no program (see docs/EDGE.md).
pub const MAX_TEXT: usize = 4096;
/// Devices a node drives at once.
pub const MAX_DEVICES: usize = 16;
/// Points of one device, both directions together.
pub const MAX_POINTS_PER_DEVICE: usize = 32;
/// Points of the whole map, both directions together.
pub const MAX_POINTS: usize = 96;
/// Bool points a direction of one device can number: `0..32`.
pub const BOOL_WINDOW_BITS: usize = 32;
/// Image bytes a device's bool window spans.
pub const BOOL_WINDOW_BYTES: usize = BOOL_WINDOW_BITS / 8;
/// The bool area of a direction: one window a device.
pub const BOOL_BYTES: usize = MAX_DEVICES * BOOL_WINDOW_BYTES;
/// Where the numeric area of a direction starts.
pub const NUM_BASE: usize = BOOL_BYTES;
/// The numeric area of a direction.
pub const NUM_BYTES: usize = 128;
/// One direction of the process image a node moves.
pub const IMAGE_BYTES: usize = NUM_BASE + NUM_BYTES;
/// Most bytes an EtherNet/IP assembly may carry each way.
pub const MAX_ASSEMBLY: u16 = 128;
/// Shortest RPI a node asks for, ms: one scan of the node's.
pub const MIN_RPI_MS: u32 = 10;
/// Longest RPI the box's parser takes, ms.
pub const MAX_RPI_MS: u32 = 3_600_000;
/// The Modbus TCP port when the endpoint names none.
pub const DEFAULT_PORT: u16 = 502;
/// The EtherNet/IP encapsulation port, the only one a node opens.
pub const ENIP_PORT: u16 = 44818;
/// Longest point name (an ST identifier the canvas shows whole).
pub const NAME_MAX: usize = 32;
/// Most bytes of `config_data` (the box's own limit).
pub const MAX_CONFIG_DATA: usize = 480;

/// What a point carries, in the box's names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty {
    Bool,
    Int,
    Real,
}

impl Ty {
    fn word(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Int => "int",
            Self::Real => "real",
        }
    }
}

/// The side of the node's image a point is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    /// `in`: from the device to the program (`%I`).
    In,
    /// `out`: from the program to the device (`%Q`).
    Out,
}

impl Dir {
    const fn index(self) -> usize {
        match self {
            Self::In => 0,
            Self::Out => 1,
        }
    }

    /// `I` or `Q`, the image area's letter.
    #[must_use]
    pub const fn area(self) -> char {
        match self {
            Self::In => 'I',
            Self::Out => 'Q',
        }
    }

    const fn word(self) -> &'static str {
        match self {
            Self::In => "in",
            Self::Out => "out",
        }
    }
}

/// A numeric value's format on the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fmt {
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    F32,
}

impl Fmt {
    /// Bytes the value takes on an EtherNet/IP assembly.
    #[must_use]
    pub const fn width(self) -> u8 {
        match self {
            Self::U8 | Self::I8 => 1,
            Self::U16 | Self::I16 => 2,
            Self::U32 | Self::I32 | Self::F32 => 4,
        }
    }

    /// Bytes the value takes in the process image: 2 or 4.
    #[must_use]
    pub const fn slot(self) -> u8 {
        if self.width() == 4 { 4 } else { 2 }
    }

    #[must_use]
    pub const fn ty(self) -> Ty {
        match self {
            Self::F32 => Ty::Real,
            _ => Ty::Int,
        }
    }
}

/// A Modbus table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Table {
    Coil,
    Discrete,
    Holding,
    Input,
}

impl Table {
    const fn space(self) -> u8 {
        match self {
            Self::Discrete => 0,
            Self::Coil => 1,
            Self::Input => 2,
            Self::Holding => 3,
        }
    }
}

/// Where a point is on its device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Addr {
    /// A coil, discrete input or register at the 0-based PDU address;
    /// `swapped` is the 32-bit formats' `sw` word order.
    Modbus { table: Table, addr: u16, swapped: bool },
    /// A bit of an assembly (`bit` is `Some`), or the start of a numeric
    /// value in it.
    Enip { byte: u16, bit: Option<u8> },
    /// A sim point: no device, no address.
    Sim,
}

/// Where a point sits in its direction of the process image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct At {
    pub byte: u16,
    /// Bit of the byte, for a bool.
    pub bit: Option<u8>,
    /// Bytes a numeric value takes (2 or 4); 1 for a bool.
    pub bytes: u8,
}

impl At {
    /// The ST address: `%IX4.3`, `%IW32`, `%QD17` (word and double-word
    /// numbers count words and double words from the area's start).
    #[must_use]
    pub fn st(&self, dir: Dir) -> String {
        let a = dir.area();
        match (self.bit, self.bytes) {
            (Some(bit), _) => alloc::format!("%{a}X{}.{bit}", self.byte),
            (None, 2) => alloc::format!("%{a}W{}", self.byte / 2),
            (None, _) => alloc::format!("%{a}D{}", self.byte / 4),
        }
    }
}

/// One point, bound to a place on its device and one in the image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Point {
    pub name: String,
    pub dir: Dir,
    pub ty: Ty,
    /// The numeric format (`None` for a bool).
    pub fmt: Option<Fmt>,
    /// Index into [`IoMap::devices`]; [`SIM_DEVICE`] for a sim point.
    pub device: usize,
    pub addr: Addr,
    pub at: At,
}

/// An EtherNet/IP device's connection (what a node can ask for).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnipLink {
    pub input_only: bool,
    pub configuration: u16,
    /// O->T connection point (the heartbeat's, for input-only).
    pub o_to_t: u16,
    pub t_to_o: u16,
    /// Application bytes each way (header excluded); 0 O->T is a heartbeat.
    pub o_to_t_len: u16,
    pub t_to_o_len: u16,
    /// O->T data carries the 32-bit run/idle header.
    pub o_to_t_run_idle: bool,
    pub rpi_ms: u32,
    pub multiplier: u8,
    pub config_data: Vec<u8>,
    /// `di:N` and `do:N` channel counts, where a profile knows them.
    pub channels: Option<(u16, u16)>,
}

/// How a device is spoken to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    Modbus { unit: u8 },
    Enip(EnipLink),
}

/// One `device` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    pub host: [u8; 4],
    pub port: u16,
    pub link: Link,
}

impl Point {
    /// A sim point: a value in memory, on no device.
    #[must_use]
    pub const fn is_sim(&self) -> bool {
        matches!(self.addr, Addr::Sim)
    }
}

/// A sim point's [`Point::device`]: no device has this index.
pub const SIM_DEVICE: usize = usize::MAX;
/// The device id a sim point stands in for; reserved.
pub const SIM_ID: &str = "sim";

/// A node's map, as parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IoMap {
    /// In `device` line order: the index is the device's number.
    pub devices: Vec<Device>,
    /// In the order the lines are written.
    pub points: Vec<Point>,
    /// The text as written.
    pub text: String,
}

impl IoMap {
    /// The map a node runs with nothing on flash: the E1213's eight
    /// inputs and four outputs at `host`, named `di0`.. and `do0`.. as
    /// the first node builds did.
    #[must_use]
    pub fn factory(host: [u8; 4], device: &str) -> Self {
        let mut text = alloc::format!(
            "# The factory map: a Moxa ioLogik E1213 at {}.{}.{}.{}. Change it on the settings page\n# or with PUT /api/io_map (the box's io.map format: modbus-tcp and enip-io devices).\ndevice {device} modbus-tcp {}.{}.{}.{}:{DEFAULT_PORT} unit=1\n",
            host[0],
            host[1],
            host[2],
            host[3],
            host[0],
            host[1],
            host[2],
            host[3]
        );
        for i in 0..8u8 {
            let _ = fmt::Write::write_fmt(&mut text, format_args!("in  di{i} bool {device} di:{i}\n"));
        }
        for i in 0..4u8 {
            let _ = fmt::Write::write_fmt(&mut text, format_args!("out do{i} bool {device} coil:{i}\n"));
        }
        parse(&text).expect("the factory map parses")
    }

    /// The points of one direction, in the order their image places lie
    /// (what the signals list shows): by byte, then bit. For a one-device
    /// bool map that is the order of the block's own numbering.
    #[must_use]
    pub fn in_image_order(&self, dir: Dir) -> Vec<&Point> {
        let mut v: Vec<&Point> = self.points.iter().filter(|p| p.dir == dir).collect();
        v.sort_by_key(|p| (p.at.byte, p.at.bit.unwrap_or(0)));
        v
    }

    /// Every point's name, direction and ST address: what `GET
    /// /api/io_map` lists beside the text.
    #[must_use]
    pub fn layout(&self) -> Vec<(&str, Dir, String)> {
        [Dir::In, Dir::Out]
            .into_iter()
            .flat_map(|d| self.in_image_order(d))
            .map(|p| (p.name.as_str(), p.dir, p.at.st(p.dir)))
            .collect()
    }
}

/// Why a map was refused: the line (1-based) and what is wrong with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bad {
    pub line: usize,
    pub why: String,
}

impl fmt::Display for Bad {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 {
            write!(f, "{}", self.why)
        } else {
            write!(f, "line {}: {}", self.line, self.why)
        }
    }
}

impl core::error::Error for Bad {}

pub(crate) fn bad(line: usize, why: impl Into<String>) -> Bad {
    Bad {
        line,
        why: why.into(),
    }
}

pub(crate) fn is_ident(s: &str) -> bool {
    let mut b = s.bytes();
    b.next().is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        && s.len() <= NAME_MAX
}

fn parse_host(s: &str) -> Option<([u8; 4], Option<u16>)> {
    let (host, port) = match s.rsplit_once(':') {
        Some((h, p)) => (h, Some(p.parse::<u16>().ok().filter(|p| *p != 0)?)),
        None => (s, None),
    };
    let mut ip = [0u8; 4];
    let mut n = 0;
    for part in host.split('.') {
        if n == 4 {
            return None;
        }
        ip[n] = part.parse().ok()?;
        n += 1;
    }
    (n == 4).then_some((ip, port))
}

/// `KEY=VALUE` words after the endpoint, each key once.
fn params<'a>(rest: &[&'a str], n: usize) -> Result<Vec<(&'a str, &'a str)>, Bad> {
    let mut out: Vec<(&str, &str)> = Vec::new();
    for opt in rest {
        let Some((k, v)) = opt.split_once('=').filter(|(k, _)| !k.is_empty()) else {
            return Err(bad(n, alloc::format!("{opt:?}: expected KEY=VALUE")));
        };
        if out.iter().any(|(have, _)| *have == k) {
            return Err(bad(n, alloc::format!("{k} given twice")));
        }
        out.push((k, v));
    }
    Ok(out)
}

fn get<'a>(p: &[(&'a str, &'a str)], key: &str) -> Option<&'a str> {
    p.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

fn number(s: &str) -> Option<u16> {
    if s.is_empty() || s.len() > 5 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

fn hex_bytes(s: &str) -> Option<Vec<u8>> {
    if s.is_empty() || !s.len().is_multiple_of(2) || s.len() / 2 > MAX_CONFIG_DATA {
        return None;
    }
    s.as_bytes()
        .chunks(2)
        .map(|c| {
            let d = |b: u8| char::from(b).to_digit(16);
            Some(u8::try_from(d(c[0])? * 16 + d(c[1])?).unwrap_or(0))
        })
        .collect()
}

/// The explicit params of an `enip-io` device, or the E1213's profile.
#[allow(clippy::too_many_lines)] // one param after another, as the box's parser does
fn enip_link(p: &[(&str, &str)], n: usize) -> Result<EnipLink, Bad> {
    let why = |k: &str, w: &str| bad(n, alloc::format!("{k}: {w}"));
    for (k, _) in p {
        match *k {
            "profile" | "connection" | "config" | "config_data" | "o2t" | "t2o" | "run_idle" | "rpi_ms"
            | "multiplier" | "timeout_ms" | "poll_ms" => {}
            "eds" => {
                return Err(why(
                    k,
                    "a node has no files to read an EDS from; give profile=moxa-e1213 or config= o2t= t2o=",
                ));
            }
            "key" => return Err(why(k, "a node does not send an electronic key")),
            "io_port" | "device_io_port" => {
                return Err(why(
                    k,
                    "a node's one I/O socket is UDP 2222, and a device is taken at its own",
                ));
            }
            "sweep" => {
                return Err(why(
                    k,
                    "a node does not search by identity; give the device's IPv4 address",
                ));
            }
            _ => return Err(why(k, "not an enip-io param")),
        }
    }
    let input_only = match get(p, "connection") {
        None | Some("exclusive-owner") => false,
        Some("input-only") => true,
        Some(_) => return Err(why("connection", "exclusive-owner or input-only")),
    };
    let explicit = ["config", "o2t", "t2o"]
        .iter()
        .filter(|k| get(p, k).is_some())
        .count();
    let profile = get(p, "profile");
    if usize::from(profile.is_some()) + usize::from(explicit > 0) != 1 {
        return Err(why(
            "profile",
            "give exactly one of profile=moxa-e1213 or config= o2t= t2o=",
        ));
    }
    let uint = |k: &str, max: u32| -> Result<Option<u32>, Bad> {
        get(p, k)
            .map(|v| {
                v.parse::<u32>()
                    .ok()
                    .filter(|x| *x <= max && v.bytes().all(|b| b.is_ascii_digit()))
                    .ok_or_else(|| why(k, &alloc::format!("want 0 to {max}")))
            })
            .transpose()
    };
    let (configuration, o_to_t, t_to_o, o_to_t_len, t_to_o_len, run_idle, rpi_range, channels);
    if let Some(name) = profile {
        if name != "moxa-e1213" {
            return Err(why("profile", "known profiles: moxa-e1213"));
        }
        if get(p, "config_data").is_some() {
            return Err(why("config_data", "only with explicit config= o2t= t2o="));
        }
        if get(p, "run_idle").is_some() {
            return Err(why("run_idle", "only with explicit config= o2t= t2o="));
        }
        (configuration, t_to_o, t_to_o_len) = (100, 101, 51);
        (o_to_t, o_to_t_len) = if input_only { (111, 0) } else { (102, 1) };
        run_idle = !input_only;
        rpi_range = (100, 3000);
        channels = Some((12, 8));
    } else {
        if explicit != 3 {
            return Err(why("config", "explicit needs config=, o2t= and t2o="));
        }
        let point = |k: &str| -> Result<(u16, u16), Bad> {
            get(p, k)
                .and_then(|v| v.split_once(':'))
                .and_then(|(a, b)| Some((number(a)?, number(b)?)))
                .ok_or_else(|| why(k, "want POINT:BYTES"))
        };
        configuration = uint("config", 0xFFFF)?
            .and_then(|v| u16::try_from(v).ok())
            .unwrap_or(0);
        (o_to_t, o_to_t_len) = point("o2t")?;
        (t_to_o, t_to_o_len) = point("t2o")?;
        if o_to_t_len > MAX_ASSEMBLY || t_to_o_len > MAX_ASSEMBLY {
            return Err(why(
                "o2t",
                &alloc::format!("a node holds at most {MAX_ASSEMBLY} bytes a direction"),
            ));
        }
        if input_only && o_to_t_len != 0 {
            return Err(why("o2t", "input-only: the O->T point is a heartbeat, 0 bytes"));
        }
        run_idle = match get(p, "run_idle") {
            None | Some("on") => o_to_t_len > 0,
            Some("off") => false,
            Some(_) => return Err(why("run_idle", "on or off")),
        };
        rpi_range = (MIN_RPI_MS, MAX_RPI_MS);
        channels = None;
    }
    let rpi_ms = uint("rpi_ms", MAX_RPI_MS)?.unwrap_or(rpi_range.0.max(100));
    if rpi_ms < MIN_RPI_MS || rpi_ms < rpi_range.0 || rpi_ms > rpi_range.1 {
        return Err(why(
            "rpi_ms",
            &alloc::format!(
                "want {} to {} for this device",
                rpi_range.0.max(MIN_RPI_MS),
                rpi_range.1
            ),
        ));
    }
    let multiplier = uint("multiplier", 7)?
        .and_then(|v| u8::try_from(v).ok())
        .unwrap_or(1);
    let config_data = match get(p, "config_data") {
        None => Vec::new(),
        Some(h) => hex_bytes(h).ok_or_else(|| {
            why(
                "config_data",
                &alloc::format!("want 1 to {MAX_CONFIG_DATA} bytes as hex digits, like 00ff00"),
            )
        })?,
    };
    Ok(EnipLink {
        input_only,
        configuration,
        o_to_t,
        t_to_o,
        o_to_t_len,
        t_to_o_len,
        o_to_t_run_idle: run_idle,
        rpi_ms,
        multiplier,
        config_data,
        channels,
    })
}

fn device_line(t: &[&str], n: usize, have: &[Device]) -> Result<Device, Bad> {
    let [_, name, driver, endpoint, rest @ ..] = t else {
        return Err(bad(
            n,
            "device: want device NAME DRIVER HOST[:PORT] [KEY=VALUE ...]",
        ));
    };
    if !is_ident(name) {
        return Err(bad(
            n,
            alloc::format!("device name {name:?}: letters, digits and '_', at most {NAME_MAX}"),
        ));
    }
    if ["box", "module", "screen"].contains(name) {
        return Err(bad(
            n,
            alloc::format!("device name {name:?} is the box's own; a node has no such device"),
        ));
    }
    if name.eq_ignore_ascii_case(SIM_ID) {
        return Err(bad(
            n,
            "device name \"sim\" is reserved for placeholder points (in NAME TYPE sim)",
        ));
    }
    if have.iter().any(|d| d.id.eq_ignore_ascii_case(name)) {
        return Err(bad(n, alloc::format!("device {name}: named twice")));
    }
    if have.len() == MAX_DEVICES {
        return Err(bad(
            n,
            alloc::format!("device {name}: a node drives at most {MAX_DEVICES} devices"),
        ));
    }
    if !matches!(*driver, "modbus-tcp" | "enip-io") {
        return Err(bad(
            n,
            alloc::format!("driver {driver:?}: a node drives modbus-tcp and enip-io devices"),
        ));
    }
    let (host, port) = parse_host(endpoint).ok_or_else(|| {
        bad(
            n,
            alloc::format!("endpoint {endpoint:?}: an IPv4 address, with :PORT if not the driver's own"),
        )
    })?;
    let p = params(rest, n)?;
    let (port, link) = if *driver == "modbus-tcp" {
        let unit = match get(&p, "unit") {
            None => 1,
            Some(v) => v
                .parse::<u8>()
                .ok()
                .filter(|u| *u != 0)
                .ok_or_else(|| bad(n, alloc::format!("unit={v}: 1 to 255")))?,
        };
        (port.unwrap_or(DEFAULT_PORT), Link::Modbus { unit })
    } else {
        let port = port.unwrap_or(ENIP_PORT);
        if port != ENIP_PORT {
            return Err(bad(
                n,
                alloc::format!(
                    "endpoint {endpoint:?}: a node opens EtherNet/IP sessions on port {ENIP_PORT}"
                ),
            ));
        }
        if have
            .iter()
            .any(|d| d.host == host && matches!(d.link, Link::Enip(_)))
        {
            return Err(bad(
                n,
                alloc::format!(
                    "device {name}: another enip-io device is at that address; one connection a device"
                ),
            ));
        }
        (port, Link::Enip(enip_link(&p, n)?))
    };
    Ok(Device {
        id: (*name).to_string(),
        host,
        port,
        link,
    })
}

/// A parsed address before its place in the image is known.
struct Parsed {
    ty: Ty,
    fmt: Option<Fmt>,
    addr: Addr,
    /// For a bool, its number in the device's window.
    number: Option<u16>,
    /// What it occupies on the device, for the overlap check:
    /// `(space, from, to)` with `to` exclusive.
    claim: (u8, u32, u32),
}

const MODBUS_FMTS: &str = "u16 i16 u32 u32sw i32 i32sw f32 f32sw";

fn modbus_point(dir: Dir, ty: Ty, token: &str, n: usize) -> Result<Parsed, Bad> {
    let here = |w: alloc::string::String| bad(n, alloc::format!("point {token:?}: {w}"));
    let mut parts = token.split(':');
    let table = match parts.next() {
        Some("coil") => Table::Coil,
        Some("di") => Table::Discrete,
        Some("hr") => Table::Holding,
        Some("ir") => Table::Input,
        _ => {
            return Err(here(
                "a modbus-tcp point is di:N, coil:N, ir:N[:FMT] or hr:N[:FMT]".into(),
            ));
        }
    };
    let addr = parts
        .next()
        .and_then(number)
        .ok_or_else(|| here("N is a number, 0 to 65535".into()))?;
    let fmt_text = parts.next();
    if parts.next().is_some() {
        return Err(here("too many fields".into()));
    }
    let (fmt, swapped) = match (table, fmt_text) {
        (Table::Coil | Table::Discrete, None) => (None, false),
        (Table::Coil | Table::Discrete, Some(_)) => {
            return Err(here("a coil or discrete input takes no format".into()));
        }
        (_, None | Some("u16")) => (Some(Fmt::U16), false),
        (_, Some("i16")) => (Some(Fmt::I16), false),
        (_, Some("u32")) => (Some(Fmt::U32), false),
        (_, Some("u32sw")) => (Some(Fmt::U32), true),
        (_, Some("i32")) => (Some(Fmt::I32), false),
        (_, Some("i32sw")) => (Some(Fmt::I32), true),
        (_, Some("f32")) => (Some(Fmt::F32), false),
        (_, Some("f32sw")) => (Some(Fmt::F32), true),
        _ => return Err(here(alloc::format!("a format is one of {MODBUS_FMTS}"))),
    };
    let words = fmt.map_or(1, |f| u32::from(f.width() / 2));
    if u32::from(addr) + words > 0x1_0000 {
        return Err(here("a 32-bit format does not fit at the last register".into()));
    }
    let ok_dir = match dir {
        Dir::In => matches!(table, Table::Discrete | Table::Input | Table::Holding),
        Dir::Out => matches!(table, Table::Coil | Table::Holding),
    };
    if !ok_dir {
        return Err(here(match dir {
            Dir::In => "a node reads di, ir and hr into the program".into(),
            Dir::Out => "a node writes coil and hr from the program".into(),
        }));
    }
    let want = fmt.map_or(Ty::Bool, Fmt::ty);
    if want != ty {
        return Err(here(alloc::format!(
            "it is {}, and the signal is {}",
            want.word(),
            ty.word()
        )));
    }
    let number = fmt.is_none().then_some(addr);
    if let Some(i) = number
        && usize::from(i) >= BOOL_WINDOW_BITS
    {
        let kind = if table == Table::Coil { "coil" } else { "di" };
        return Err(here(alloc::format!(
            "a node holds {kind}:0 to {kind}:{}",
            BOOL_WINDOW_BITS - 1
        )));
    }
    Ok(Parsed {
        ty,
        fmt,
        addr: Addr::Modbus { table, addr, swapped },
        number,
        claim: (table.space(), u32::from(addr), u32::from(addr) + words),
    })
}

fn enip_fmt(s: &str) -> Option<Fmt> {
    Some(match s {
        "u8" => Fmt::U8,
        "i8" => Fmt::I8,
        "u16" => Fmt::U16,
        "i16" => Fmt::I16,
        "u32" => Fmt::U32,
        "i32" => Fmt::I32,
        "f32" => Fmt::F32,
        _ => return None,
    })
}

fn enip_point(dir: Dir, ty: Ty, token: &str, link: &EnipLink, n: usize) -> Result<Parsed, Bad> {
    let here = |w: alloc::string::String| bad(n, alloc::format!("point {token:?}: {w}"));
    let form = "an enip-io point is di:N, do:N, in:BYTE.BIT, out:BYTE.BIT, in:OFFSET:FMT or out:OFFSET:FMT";
    let (head, rest) = token.split_once(':').ok_or_else(|| here(form.into()))?;
    let side = match head {
        "di" | "in" => Dir::In,
        "do" | "out" => Dir::Out,
        _ => return Err(here(form.into())),
    };
    if side != dir {
        return Err(here(alloc::format!(
            "{head}: is the device's {} assembly, and the signal is {}",
            if side == Dir::In { "T->O" } else { "O->T" },
            dir.word()
        )));
    }
    let (len, what) = match dir {
        Dir::In => (link.t_to_o_len, "T->O"),
        Dir::Out => (link.o_to_t_len, "O->T"),
    };
    if dir == Dir::Out && link.o_to_t_len == 0 {
        return Err(here("an input-only connection has no outputs".into()));
    }
    let channel = matches!(head, "di" | "do");
    let (byte, bit, fmt) = if channel {
        let c = number(rest).ok_or_else(|| here("N is a number".into()))?;
        (c / 8, Some(u8::try_from(c % 8).unwrap_or(0)), None)
    } else if let Some((b, i)) = rest.split_once('.') {
        let bit = number(i)
            .filter(|b| *b < 8)
            .ok_or_else(|| here("BIT is 0 to 7".into()))?;
        (
            number(b).ok_or_else(|| here("BYTE is a number".into()))?,
            Some(u8::try_from(bit).unwrap_or(0)),
            None,
        )
    } else {
        let (off, f) = rest.split_once(':').ok_or_else(|| here(form.into()))?;
        let fmt = enip_fmt(f).ok_or_else(|| here("a format is one of u8 i8 u16 i16 u32 i32 f32".into()))?;
        (
            number(off).ok_or_else(|| here("OFFSET is a number".into()))?,
            None,
            Some(fmt),
        )
    };
    if let (true, Some((di, do_))) = (channel, link.channels) {
        let limit = if dir == Dir::In { di } else { do_ };
        if number(rest).is_some_and(|c| c >= limit) {
            return Err(here(alloc::format!(
                "this profile has {head}:0 to {head}:{}",
                limit.saturating_sub(1)
            )));
        }
    }
    let width = fmt.map_or(1, |f| u16::from(f.width()));
    if byte + width > len {
        return Err(here(alloc::format!(
            "the {what} assembly is {len} bytes and this ends at byte {}",
            byte + width
        )));
    }
    let derived = fmt.map_or(Ty::Bool, Fmt::ty);
    if derived != ty {
        return Err(here(alloc::format!(
            "it is {}, and the signal is {}",
            derived.word(),
            ty.word()
        )));
    }
    let number_bit = bit.map(|b| u16::from(b) + byte * 8);
    if let Some(i) = number_bit
        && usize::from(i) >= BOOL_WINDOW_BITS
    {
        return Err(here(alloc::format!(
            "a node maps bits 0 to {} of an assembly (bytes 0 to {}); read a later byte as {}:BYTE:u8",
            BOOL_WINDOW_BITS - 1,
            BOOL_WINDOW_BYTES - 1,
            if dir == Dir::In { "in" } else { "out" }
        )));
    }
    let from = u32::from(byte) * 8 + u32::from(bit.unwrap_or(0));
    let to = bit.map_or_else(|| (u32::from(byte) + u32::from(width)) * 8, |_| from + 1);
    Ok(Parsed {
        ty,
        fmt,
        addr: Addr::Enip { byte, bit },
        number: number_bit,
        claim: (0, from, to),
    })
}

/// A point's place on its device: `(direction, device, (space, from, to), name)`.
type Claim = (usize, usize, (u8, u32, u32), String);

/// What a map under construction knows, for the checks a line needs of
/// the lines before it.
struct Taken {
    devices: Vec<Device>,
    points: Vec<Point>,
    /// `(direction, device, space, from, to, name)` of each point.
    claims: Vec<Claim>,
    /// The next free byte of each direction's numeric area.
    cursor: [usize; 2],
}

/// A point line's checks and place: the point, and what it claims on its
/// device (a sim point claims nothing).
type Placed = (Point, Option<(u8, u32, u32)>);

#[allow(clippy::too_many_lines)] // one check after another
fn point_line(t: &[&str], n: usize, taken: &Taken) -> Result<Placed, Bad> {
    let [direction, name, type_word, device_word, rest @ ..] = t else {
        return Err(bad(n, "want in|out NAME bool|int|real DEVICE ADDRESS"));
    };
    let dir = if *direction == "in" { Dir::In } else { Dir::Out };
    let ty = match *type_word {
        "bool" => Ty::Bool,
        "int" => Ty::Int,
        "real" => Ty::Real,
        other => {
            return Err(bad(
                n,
                alloc::format!("{name}: type {other:?}: bool, int or real"),
            ));
        }
    };
    if !is_ident(name) {
        return Err(bad(
            n,
            alloc::format!("{name:?}: a name is letters, digits and '_', at most {NAME_MAX}"),
        ));
    }
    let sim = device_word.eq_ignore_ascii_case(SIM_ID);
    let (address, _options) = match rest {
        [address, options @ ..] if !sim => (*address, options),
        _ if sim => ("", rest),
        _ => return Err(bad(n, "want in|out NAME bool|int|real DEVICE ADDRESS")),
    };
    if sim {
        if let Some(word) = rest.iter().find(|w| !w.contains('=')) {
            return Err(bad(
                n,
                alloc::format!("{name}: sim takes no address ({word:?}); only KEY=VALUE options"),
            ));
        }
        params(rest, n)?;
    } else if taken.devices.is_empty() {
        return Err(bad(n, "the device line comes before its points"));
    }
    if !sim && ["box", "module", "screen"].contains(device_word) {
        return Err(bad(
            n,
            alloc::format!("{name}: {device_word:?} is the box's own device; a node has no such channels"),
        ));
    }
    let device = if sim {
        SIM_DEVICE
    } else {
        let Some(device) = taken.devices.iter().position(|d| d.id == *device_word) else {
            return Err(bad(
                n,
                alloc::format!("{name}: device {device_word:?} is not declared above"),
            ));
        };
        device
    };
    if taken.points.iter().any(|p| p.name.eq_ignore_ascii_case(name)) {
        return Err(bad(n, alloc::format!("{name}: named twice")));
    }
    if taken.points.len() == MAX_POINTS {
        return Err(bad(
            n,
            alloc::format!("{name}: a node holds at most {MAX_POINTS} points"),
        ));
    }
    if sim {
        // Its place in the image is given once the whole map is read.
        let fmt = match ty {
            Ty::Bool => None,
            Ty::Int => Some(Fmt::I32),
            Ty::Real => Some(Fmt::F32),
        };
        let at = At {
            byte: 0,
            bit: (ty == Ty::Bool).then_some(0),
            bytes: if ty == Ty::Bool { 1 } else { 4 },
        };
        return Ok((
            Point {
                name: (*name).to_string(),
                dir,
                ty,
                fmt,
                device,
                addr: Addr::Sim,
                at,
            },
            None,
        ));
    }
    if taken.points.iter().filter(|p| p.device == device).count() == MAX_POINTS_PER_DEVICE {
        return Err(bad(
            n,
            alloc::format!("{name}: a device holds at most {MAX_POINTS_PER_DEVICE} points"),
        ));
    }
    let parsed = match &taken.devices[device].link {
        Link::Modbus { .. } => modbus_point(dir, ty, address, n)?,
        Link::Enip(link) => enip_point(dir, ty, address, link, n)?,
    };
    if let Some((_, _, _, other)) = taken.claims.iter().find(|(d, dv, c, _)| {
        *d == dir.index()
            && *dv == device
            && c.0 == parsed.claim.0
            && c.1 < parsed.claim.2
            && parsed.claim.1 < c.2
    }) {
        return Err(bad(
            n,
            alloc::format!("{name}: {address} is already named (it overlaps {other})"),
        ));
    }
    let at = if let Some(number) = parsed.number {
        At {
            byte: u16::try_from(device * BOOL_WINDOW_BYTES + usize::from(number) / 8).unwrap_or(0),
            bit: Some(u8::try_from(number % 8).unwrap_or(0)),
            bytes: 1,
        }
    } else {
        let slot = usize::from(parsed.fmt.map_or(2, Fmt::slot));
        let start = taken.cursor[dir.index()].next_multiple_of(slot);
        if start + slot > IMAGE_BYTES {
            return Err(bad(
                n,
                alloc::format!(
                    "{name}: the image holds {NUM_BYTES} bytes of numeric {} and they are used up",
                    dir.word()
                ),
            ));
        }
        At {
            byte: u16::try_from(start).unwrap_or(0),
            bit: None,
            bytes: u8::try_from(slot).unwrap_or(2),
        }
    };
    Ok((
        Point {
            name: (*name).to_string(),
            dir,
            ty: parsed.ty,
            fmt: parsed.fmt,
            device,
            addr: parsed.addr,
            at,
        },
        Some(parsed.claim),
    ))
}

/// Gives the sim points their places (see "Sim points" in the module
/// header): after every device point has its own, so none of those moves.
/// `lines` is each sim point's `(index in points, line)`.
fn place_sims(
    points: &mut [Point],
    lines: &[(usize, usize)],
    devices: usize,
    cursor: [usize; 2],
) -> Result<(), Bad> {
    let mut bools = [0usize; 2];
    let mut top = [IMAGE_BYTES; 2];
    for &(i, line) in lines {
        let p = &mut points[i];
        let d = p.dir.index();
        if p.ty == Ty::Bool {
            let k = bools[d];
            bools[d] += 1;
            let window = (MAX_DEVICES - 1)
                .checked_sub(k / BOOL_WINDOW_BITS)
                .filter(|w| *w >= devices);
            let Some(window) = window else {
                return Err(bad(
                    line,
                    alloc::format!(
                        "{}: no bool window left: {devices} device(s) and the sim {} points share {MAX_DEVICES}",
                        p.name,
                        p.dir.word()
                    ),
                ));
            };
            let n = k % BOOL_WINDOW_BITS;
            p.at = At {
                byte: u16::try_from(window * BOOL_WINDOW_BYTES + n / 8).unwrap_or(0),
                bit: Some(u8::try_from(n % 8).unwrap_or(0)),
                bytes: 1,
            };
        } else {
            let start = top[d].saturating_sub(4);
            if top[d] < 4 || start < cursor[d].next_multiple_of(4) {
                return Err(bad(
                    line,
                    alloc::format!(
                        "{}: the image holds {NUM_BYTES} bytes of numeric {} and they are used up",
                        p.name,
                        p.dir.word()
                    ),
                ));
            }
            top[d] = start;
            p.at = At {
                byte: u16::try_from(start).unwrap_or(0),
                bit: None,
                bytes: 4,
            };
        }
    }
    Ok(())
}

/// Parses `text` as a node's map.
///
/// # Errors
/// The first line a node cannot take, and why.
pub fn parse(text: &str) -> Result<IoMap, Bad> {
    if text.len() > MAX_TEXT {
        return Err(bad(
            0,
            alloc::format!("the map is {} bytes; a node keeps {MAX_TEXT}", text.len()),
        ));
    }
    let mut taken = Taken {
        devices: Vec::new(),
        points: Vec::new(),
        claims: Vec::new(),
        cursor: [NUM_BASE; 2],
    };
    let mut sims: Vec<(usize, usize)> = Vec::new();
    for (k, raw) in text.lines().enumerate() {
        let n = k + 1;
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let t: Vec<&str> = line.split_whitespace().collect();
        match t.first().copied() {
            Some("device") => {
                let d = device_line(&t, n, &taken.devices)?;
                taken.devices.push(d);
            }
            Some("in" | "out") => {
                let (p, claim) = point_line(&t, n, &taken)?;
                if let Some(claim) = claim {
                    if p.at.bit.is_none() {
                        taken.cursor[p.dir.index()] = usize::from(p.at.byte) + usize::from(p.at.bytes);
                    }
                    taken
                        .claims
                        .push((p.dir.index(), p.device, claim, p.name.clone()));
                } else {
                    sims.push((taken.points.len(), n));
                }
                taken.points.push(p);
            }
            Some(other) => {
                return Err(bad(
                    n,
                    alloc::format!("{other:?}: a node's map has device, in and out lines"),
                ));
            }
            None => {}
        }
    }
    if taken.devices.is_empty() && taken.points.is_empty() {
        return Err(bad(
            0,
            "no device line: a node drives at least one device (or holds sim points)",
        ));
    }
    place_sims(&mut taken.points, &sims, taken.devices.len(), taken.cursor)?;
    Ok(IoMap {
        devices: taken.devices,
        points: taken.points,
        text: text.to_string(),
    })
}

/// Points a direction the one-block boards move: indexes `0..16`, two
/// image bytes.
pub const ONE_BLOCK_POINTS: u16 = 16;
/// Map text a one-block board keeps beside its program.
pub const ONE_BLOCK_TEXT: usize = 2048;

/// A map cut to what the first nodes drive (crates/helmsman-node, the
/// ESP32 boards): one Modbus TCP block, its discrete inputs and coils
/// below [`ONE_BLOCK_POINTS`]. [`IoMap::one_block`] makes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OneBlock {
    /// The block's id (the `device` line's name).
    pub device: String,
    pub host: [u8; 4],
    pub port: u16,
    pub unit: u8,
    /// The discrete inputs' names in image order (index `k` is bit
    /// `k % 8` of byte `k / 8`), `None` where the map names no point,
    /// up to the highest one named.
    pub inputs: Vec<Option<String>>,
    /// The coils' names, the same way.
    pub outputs: Vec<Option<String>>,
}

impl OneBlock {
    /// Discrete inputs to read each round: up to the highest index named.
    #[must_use]
    pub fn input_count(&self) -> u16 {
        u16::try_from(self.inputs.len()).unwrap_or(ONE_BLOCK_POINTS)
    }

    /// Coils to write each round: up to the highest index named.
    #[must_use]
    pub fn coil_count(&self) -> u16 {
        u16::try_from(self.outputs.len()).unwrap_or(ONE_BLOCK_POINTS)
    }
}

impl IoMap {
    /// This map as a one-block board runs it.
    ///
    /// # Errors
    /// What in the map such a board cannot run: a second device, a device
    /// that is not Modbus TCP, a point that is not a discrete input or a
    /// coil below [`ONE_BLOCK_POINTS`], or more text than
    /// [`ONE_BLOCK_TEXT`].
    pub fn one_block(&self) -> Result<OneBlock, Bad> {
        let no = |why: &str| Bad {
            line: 0,
            why: alloc::format!("{why}: this board drives one Modbus TCP block's discrete inputs and coils"),
        };
        if self.text.len() > ONE_BLOCK_TEXT {
            return Err(no(&alloc::format!(
                "the map is {} bytes and this board keeps {ONE_BLOCK_TEXT}",
                self.text.len()
            )));
        }
        let [device] = self.devices.as_slice() else {
            return Err(no(&alloc::format!("{} devices", self.devices.len())));
        };
        let Link::Modbus { unit } = device.link else {
            return Err(no(&alloc::format!("{} is not a modbus-tcp device", device.id)));
        };
        let mut inputs: Vec<Option<String>> = Vec::new();
        let mut outputs: Vec<Option<String>> = Vec::new();
        for p in &self.points {
            let (list, index) = match (p.dir, p.ty, p.addr) {
                (
                    Dir::In,
                    Ty::Bool,
                    Addr::Modbus {
                        table: Table::Discrete,
                        addr,
                        ..
                    },
                ) if addr < ONE_BLOCK_POINTS => (&mut inputs, usize::from(addr)),
                (
                    Dir::Out,
                    Ty::Bool,
                    Addr::Modbus {
                        table: Table::Coil,
                        addr,
                        ..
                    },
                ) if addr < ONE_BLOCK_POINTS => (&mut outputs, usize::from(addr)),
                _ => return Err(no(&alloc::format!("point {}", p.name))),
            };
            if list.len() <= index {
                list.resize(index + 1, None);
            }
            list[index] = Some(p.name.clone());
        }
        Ok(OneBlock {
            device: device.id.clone(),
            host: device.host,
            port: device.port,
            unit,
            inputs,
            outputs,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests;
