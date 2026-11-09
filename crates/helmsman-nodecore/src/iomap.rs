//! The node's I/O map: the box's map format (`helmsman_io::text`, the
//! `device` and `in`/`out` lines) cut to what a node drives: one Modbus
//! TCP block and its discrete points (card node-io-map). The text is kept
//! as the engineer wrote it (comments and all) beside the program on
//! flash, so the canvas's Devices tab lists the block and the points by
//! their names, and a program addresses them as `%IX0.0`.. / `%QX0.0`..
//! in map order of index.
//!
//! ```text
//! device rio modbus-tcp 192.168.3.60:502 unit=1
//! in  estop   bool rio di:0
//! in  run_pb  bool rio di:1
//! out motor   bool rio coil:0
//! ```
//!
//! What a node takes: exactly one `device` line, driver `modbus-tcp`, an
//! IPv4 endpoint (port 502 unless given) and `unit=N` (1 unless given;
//! other `KEY=VALUE`s are kept but mean nothing here); `in NAME bool DEV
//! di:N` and `out NAME bool DEV coil:N` with `N` below [`MAX_POINTS`]
//! (two image bytes a direction); names are ST identifiers, unique
//! without regard to case. Anything else is refused with its line, so a
//! map a box would take but a node cannot run never reaches flash.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

/// Most bytes of map text a node keeps (beside the program in its slot).
pub const MAX_TEXT: usize = 2048;
/// Points a direction: indexes `0..MAX_POINTS`, two bytes of the image.
pub const MAX_POINTS: u8 = 16;
/// The Modbus TCP port when the endpoint names none.
pub const DEFAULT_PORT: u16 = 502;
/// Longest point name (an ST identifier the canvas shows whole).
pub const NAME_MAX: usize = 32;

/// One point of the block, bound to a bit of the image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Point {
    pub name: String,
    /// The discrete input or coil number on the block, and the bit of the
    /// image it lands in (`index / 8` is the byte, `index % 8` the bit).
    pub index: u8,
}

/// A node's map, as parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IoMap {
    /// The block's id (the `device` line's name).
    pub device: String,
    pub host: [u8; 4],
    pub port: u16,
    pub unit: u8,
    /// Discrete inputs, in map order.
    pub inputs: Vec<Point>,
    /// Coils, in map order.
    pub outputs: Vec<Point>,
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
            "# The factory map: a Moxa ioLogik E1213 at {}.{}.{}.{}. Change it on the settings page\n# or with PUT /api/io_map (the box's io.map format, one modbus-tcp device, bool points).\ndevice {device} modbus-tcp {}.{}.{}.{}:{DEFAULT_PORT} unit=1\n",
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

    /// Discrete inputs to read each round: up to the highest index named.
    #[must_use]
    pub fn input_count(&self) -> u16 {
        self.inputs
            .iter()
            .map(|p| u16::from(p.index) + 1)
            .max()
            .unwrap_or(0)
    }

    /// Coils to write each round: up to the highest index named.
    #[must_use]
    pub fn coil_count(&self) -> u16 {
        self.outputs
            .iter()
            .map(|p| u16::from(p.index) + 1)
            .max()
            .unwrap_or(0)
    }

    /// The point names in image order (`index` 0 first), `None` where no
    /// point is named; what the signals list shows for the image bits.
    #[must_use]
    pub fn names(points: &[Point]) -> Vec<Option<&str>> {
        let n = points.iter().map(|p| usize::from(p.index) + 1).max().unwrap_or(0);
        let mut out = alloc::vec![None; n];
        for p in points {
            out[usize::from(p.index)] = Some(p.name.as_str());
        }
        out
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

fn bad(line: usize, why: impl Into<String>) -> Bad {
    Bad {
        line,
        why: why.into(),
    }
}

fn is_ident(s: &str) -> bool {
    let mut b = s.bytes();
    b.next().is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        && s.len() <= NAME_MAX
}

fn parse_host(s: &str) -> Option<([u8; 4], u16)> {
    let (host, port) = match s.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().ok().filter(|p| *p != 0)?),
        None => (s, DEFAULT_PORT),
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

fn point_index(token: &str, kind: &str, line: usize) -> Result<u8, Bad> {
    let Some((k, n)) = token.split_once(':') else {
        return Err(bad(
            line,
            alloc::format!("point {token:?}: a node takes {kind}:N"),
        ));
    };
    if k != kind {
        return Err(bad(
            line,
            alloc::format!(
                "point {token:?}: a node takes {kind}:N here (bool points of one Modbus TCP block)"
            ),
        ));
    }
    let i: u16 = n
        .parse()
        .map_err(|_| bad(line, alloc::format!("point {token:?}: N is a number")))?;
    if i >= u16::from(MAX_POINTS) {
        return Err(bad(
            line,
            alloc::format!(
                "point {token:?}: a node holds {kind}:0 to {kind}:{}",
                MAX_POINTS - 1
            ),
        ));
    }
    Ok(u8::try_from(i).unwrap_or(0))
}

/// A parsed `device` line: name, host, port, unit.
type Device = (String, [u8; 4], u16, u8);

fn device_line(t: &[&str], n: usize) -> Result<Device, Bad> {
    let [_, name, driver, endpoint, rest @ ..] = t else {
        return Err(bad(n, "device: want device NAME modbus-tcp HOST[:PORT] [unit=N]"));
    };
    if *driver != "modbus-tcp" {
        return Err(bad(
            n,
            alloc::format!("driver {driver:?}: a node drives a modbus-tcp block"),
        ));
    }
    if !is_ident(name) {
        return Err(bad(
            n,
            alloc::format!("device name {name:?}: letters, digits and '_'"),
        ));
    }
    let (host, port) = parse_host(endpoint).ok_or_else(|| {
        bad(
            n,
            alloc::format!("endpoint {endpoint:?}: an IPv4 address, with :PORT if not 502"),
        )
    })?;
    let mut unit = 1u8;
    for opt in rest {
        match opt.split_once('=') {
            Some(("unit", v)) => {
                unit = v
                    .parse()
                    .ok()
                    .filter(|u| *u != 0)
                    .ok_or_else(|| bad(n, alloc::format!("unit={v}: 1 to 255")))?;
            }
            Some(_) => {}
            None => return Err(bad(n, alloc::format!("{opt:?}: expected KEY=VALUE"))),
        }
    }
    Ok(((*name).to_string(), host, port, unit))
}

/// A parsed `in`/`out` line, checked against the device and the points so
/// far: the point and whether it is an input.
fn point_line(
    t: &[&str],
    n: usize,
    device: Option<&str>,
    inputs: &[Point],
    outputs: &[Point],
) -> Result<(bool, Point), Bad> {
    let [dir, name, ty, dev, point, ..] = t else {
        return Err(bad(n, "want in|out NAME bool DEVICE di:N|coil:N"));
    };
    if *ty != "bool" {
        return Err(bad(
            n,
            alloc::format!("{name}: type {ty:?}; a node's points are bool"),
        ));
    }
    if !is_ident(name) {
        return Err(bad(
            n,
            alloc::format!("{name:?}: a name is letters, digits and '_', at most {NAME_MAX}"),
        ));
    }
    let Some(dname) = device else {
        return Err(bad(n, "the device line comes before its points"));
    };
    if *dev != dname {
        return Err(bad(
            n,
            alloc::format!("{name}: device {dev:?} is not the block ({dname})"),
        ));
    }
    if inputs
        .iter()
        .chain(outputs.iter())
        .any(|p| p.name.eq_ignore_ascii_case(name))
    {
        return Err(bad(n, alloc::format!("{name}: named twice")));
    }
    let input = *dir == "in";
    let (list, kind) = if input { (inputs, "di") } else { (outputs, "coil") };
    let index = point_index(point, kind, n)?;
    if list.iter().any(|p| p.index == index) {
        return Err(bad(n, alloc::format!("{name}: {kind}:{index} is already named")));
    }
    Ok((
        input,
        Point {
            name: (*name).to_string(),
            index,
        },
    ))
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
    let mut device: Option<Device> = None;
    let mut inputs: Vec<Point> = Vec::new();
    let mut outputs: Vec<Point> = Vec::new();
    for (k, raw) in text.lines().enumerate() {
        let n = k + 1;
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let t: Vec<&str> = line.split_whitespace().collect();
        match t.first().copied() {
            Some("device") => {
                if device.is_some() {
                    return Err(bad(n, "a second device: a node drives one Modbus TCP block"));
                }
                device = Some(device_line(&t, n)?);
            }
            Some("in" | "out") => {
                let (input, point) =
                    point_line(&t, n, device.as_ref().map(|d| d.0.as_str()), &inputs, &outputs)?;
                if input {
                    inputs.push(point);
                } else {
                    outputs.push(point);
                }
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
    let Some((name, host, port, unit)) = device else {
        return Err(bad(0, "no device line: a node drives one Modbus TCP block"));
    };
    Ok(IoMap {
        device: name,
        host,
        port,
        unit,
        inputs,
        outputs,
        text: text.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BENCH: &str = "# the press\ndevice rio modbus-tcp 192.168.3.60 unit=1 timeout_ms=300\nin  estop  bool rio di:0\nin  run_pb bool rio di:3   # wired to the green button\nout motor  bool rio coil:0\nout lamp   bool rio coil:2\n";

    #[test]
    fn a_bench_map_parses_to_the_block_and_its_points() {
        let m = parse(BENCH).unwrap();
        assert_eq!(m.device, "rio");
        assert_eq!((m.host, m.port, m.unit), ([192, 168, 3, 60], 502, 1));
        assert_eq!(
            m.inputs
                .iter()
                .map(|p| (p.name.as_str(), p.index))
                .collect::<Vec<_>>(),
            [("estop", 0), ("run_pb", 3)]
        );
        assert_eq!(
            m.outputs
                .iter()
                .map(|p| (p.name.as_str(), p.index))
                .collect::<Vec<_>>(),
            [("motor", 0), ("lamp", 2)]
        );
        assert_eq!((m.input_count(), m.coil_count()), (4, 3));
        assert_eq!(
            IoMap::names(&m.inputs),
            [Some("estop"), None, None, Some("run_pb")]
        );
        assert_eq!(m.text, BENCH, "kept as written");
        let p = parse("device b modbus-tcp 10.0.0.9:1502 unit=7\n").unwrap();
        assert_eq!((p.host, p.port, p.unit), ([10, 0, 0, 9], 1502, 7));
        assert_eq!((p.input_count(), p.coil_count()), (0, 0));
    }

    #[test]
    fn the_factory_map_is_the_e1213_as_the_first_builds_named_it() {
        let m = IoMap::factory([192, 168, 3, 60], "rio");
        assert_eq!((m.input_count(), m.coil_count()), (8, 4));
        assert_eq!(m.inputs[7].name, "di7");
        assert_eq!(m.outputs[3].name, "do3");
        assert_eq!(parse(&m.text).unwrap(), m, "its own text parses back");
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn what_a_node_cannot_run_is_refused_with_its_line() {
        let refused = |text: &str| parse(text).unwrap_err();
        assert_eq!(refused("in a bool rio di:0\n").line, 1);
        assert!(refused("").why.contains("no device"));
        assert!(
            refused("device rio enip 192.168.3.60\n")
                .why
                .contains("modbus-tcp")
        );
        assert!(refused("device rio modbus-tcp box\n").why.contains("IPv4"));
        assert!(
            refused("device rio modbus-tcp 192.168.3.60:0\n")
                .why
                .contains("IPv4")
        );
        assert!(
            refused("device rio modbus-tcp 192.168.3.60 unit=0\n")
                .why
                .contains("unit=0")
        );
        assert!(
            refused("device rio modbus-tcp 192.168.3.60 unit=x\n")
                .why
                .contains("unit=x")
        );
        assert!(
            refused("device rio modbus-tcp 192.168.3.60 loud\n")
                .why
                .contains("KEY=VALUE")
        );
        assert!(
            refused("device a modbus-tcp 1.1.1.1\ndevice b modbus-tcp 1.1.1.2\n")
                .why
                .contains("second device")
        );
        let two = refused("device a modbus-tcp 1.1.1.1\ndevice b modbus-tcp 1.1.1.2\n");
        assert_eq!(two.line, 2);
        let d = "device rio modbus-tcp 192.168.3.60\n";
        assert!(
            refused(&alloc::format!("{d}in level int rio hr:5:i16\n"))
                .why
                .contains("bool")
        );
        assert!(
            refused(&alloc::format!("{d}in x bool other di:0\n"))
                .why
                .contains("not the block")
        );
        assert!(
            refused(&alloc::format!("{d}in x bool rio coil:0\n"))
                .why
                .contains("di:N")
        );
        assert!(
            refused(&alloc::format!("{d}out x bool rio di:0\n"))
                .why
                .contains("coil:N")
        );
        assert!(
            refused(&alloc::format!("{d}in x bool rio di:16\n"))
                .why
                .contains("di:0 to di:15")
        );
        assert!(
            refused(&alloc::format!("{d}in x bool rio di:q\n"))
                .why
                .contains("number")
        );
        assert!(
            refused(&alloc::format!("{d}in x bool rio di:0\nin y bool rio di:0\n"))
                .why
                .contains("already named")
        );
        assert!(
            refused(&alloc::format!("{d}in x bool rio di:0\nout X bool rio coil:0\n"))
                .why
                .contains("named twice")
        );
        assert!(
            refused(&alloc::format!("{d}in 9x bool rio di:0\n"))
                .why
                .contains("letters")
        );
        assert!(
            refused(&alloc::format!("{d}in x bool rio\n"))
                .why
                .contains("want in|out")
        );
        assert!(
            refused(&alloc::format!("{d}screen x\n"))
                .why
                .contains("device, in and out")
        );
        let long = alloc::format!("{d}{}", "# x\n".repeat(600));
        assert!(refused(&long).why.contains("bytes"));
        assert_eq!(
            refused("").to_string(),
            "no device line: a node drives one Modbus TCP block"
        );
        assert_eq!(
            refused("device rio enip 1.1.1.1\n").to_string().split(':').next(),
            Some("line 1")
        );
    }
}
